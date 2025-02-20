use std::{ops::Deref, rc::Rc, str::FromStr};

pub use anchor_client::{
    solana_sdk::{
        commitment_config::{CommitmentConfig, CommitmentLevel},
        compute_budget::ComputeBudgetInstruction,
        native_token::LAMPORTS_PER_SOL,
        pubkey::Pubkey,
        signature::{Keypair, Signature, Signer},
        system_instruction, system_program, sysvar,
        transaction::Transaction,
    },
    Client, Program,
};
use console::{style, Style};
use dialoguer::{theme::ColorfulTheme, Confirm};
use mpl_candy_machine_core::{accounts as nft_accounts, instruction as nft_instruction};
use solana_account_decoder::UiAccountEncoding;
use solana_client::{
    rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig},
    rpc_filter::{Memcmp, RpcFilterType},
};

use crate::{
    candy_machine::CANDY_MACHINE_ID,
    common::*,
    parse::parse_sugar_errors,
    setup::{setup_client, sugar_setup},
    utils::*,
};

pub struct WithdrawArgs {
    pub candy_machine: Option<String>,
    pub keypair: Option<String>,
    pub rpc_url: Option<String>,
    pub list: bool,
    pub authority: Option<String>,
}

#[derive(Debug)]
struct WithdrawError {
    candy_machine: String,
    error_message: String,
}

pub fn process_withdraw(args: WithdrawArgs) -> Result<()> {
    // (1) Setting up connection

    println!(
        "{} {}Initializing connection",
        style("[1/2]").bold().dim(),
        COMPUTER_EMOJI
    );

    let pb = spinner_with_style();
    pb.set_message("Connecting...");

    let (program, payer, authority) = setup_withdraw(args.keypair, args.rpc_url, args.authority)?;

    pb.finish_with_message("Connected");

    // if --authority is specified and it does not match the keypair,
    // then we cannot withdraw
    let list = args.list || (payer != authority);

    println!(
        "\n{} {}{} funds",
        style("[2/2]").bold().dim(),
        WITHDRAW_EMOJI,
        if list { "Listing" } else { "Retrieving" }
    );

    // the --list flag takes precedence; even if a candy machine id is passed
    // as an argument, we will list the candy machines (no draining happens)
    let candy_machine = if list { None } else { args.candy_machine };

    // (2) Retrieving data for listing/draining

    match &candy_machine {
        Some(candy_machine) => {
            let candy_machine = Pubkey::from_str(candy_machine)?;

            let pb = spinner_with_style();
            pb.set_message("Draining candy machine...");

            do_withdraw(Rc::new(program), candy_machine, payer)?;

            pb.finish_with_message("Done");
        }
        None => {
            let config = RpcProgramAccountsConfig {
                filters: Some(vec![RpcFilterType::Memcmp(Memcmp::new_base58_encoded(
                    16, // key
                    authority.as_ref(),
                ))]),
                account_config: RpcAccountInfoConfig {
                    encoding: Some(UiAccountEncoding::Base64),
                    data_slice: None,
                    commitment: Some(CommitmentConfig {
                        commitment: CommitmentLevel::Confirmed,
                    }),
                    min_context_slot: None,
                },
                with_context: None,
            };

            let pb = spinner_with_style();
            pb.set_message("Looking up candy machines...");

            let program = Rc::new(program);
            let accounts = program
                .rpc()
                .get_program_accounts_with_config(&program.id(), config)?;

            pb.finish_and_clear();

            let mut total = 0.0f64;

            accounts.iter().for_each(|account| {
                let (_pubkey, account) = account;
                total += account.lamports as f64;
            });

            println!(
                "\nFound {} candy machines, total amount: ◎ {}",
                accounts.len(),
                total / LAMPORTS_PER_SOL as f64
            );

            if !accounts.is_empty() {
                if list {
                    println!("\n{:48} Balance", "Candy Machine ID");
                    println!("{:-<61}", "-");

                    for (pubkey, account) in accounts {
                        println!(
                            "{:48} {:>12.8}",
                            pubkey.to_string(),
                            account.lamports as f64 / LAMPORTS_PER_SOL as f64
                        );
                    }
                } else {
                    let warning = format!(
                        "\n\
                        +-----------------------------------------------------+\n\
                        | {} WARNING: This will drain ALL your Candy Machines |\n\
                        +-----------------------------------------------------+",
                        WARNING_EMOJI
                    );

                    println!("{}\n", style(warning).bold().yellow());

                    let theme = ColorfulTheme {
                        success_prefix: style("✔".to_string()).yellow().force_styling(true),
                        values_style: Style::new().yellow(),
                        ..get_dialoguer_theme()
                    };

                    if !Confirm::with_theme(&theme)
                        .with_prompt("Do you want to continue?")
                        .interact()?
                    {
                        return Err(anyhow!("Withdraw aborted"));
                    }

                    let pb = progress_bar_with_style(accounts.len() as u64);
                    let mut not_drained = 0;
                    let mut error_messages = Vec::new();

                    accounts.iter().for_each(|account| {
                        let (candy_machine, _account) = account;
                        do_withdraw(program.clone(), *candy_machine, payer).unwrap_or_else(|e| {
                            not_drained += 1;
                            error!("Error: {}", e);
                            let error_message = parse_sugar_errors(&e.to_string());
                            error_messages.push(WithdrawError {
                                candy_machine: candy_machine.to_string(),
                                error_message,
                            });
                        });
                        pb.inc(1);
                    });

                    pb.finish();

                    if not_drained > 0 {
                        println!(
                            "{}",
                            style(format!("Could not drain {} candy machine(s)", not_drained))
                                .red()
                                .bold()
                                .dim()
                        );
                        println!("{}", style("Errors:").red().bold().dim());
                        for error in error_messages {
                            println!(
                                "{} {}\n{} {}",
                                style("Candy Machine:").bold().dim(),
                                style(error.candy_machine).bold().red(),
                                style("Error:").bold().dim(),
                                style(error.error_message).bold().red()
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn setup_withdraw(
    keypair: Option<String>,
    rpc_url: Option<String>,
    authority_opt: Option<String>,
) -> Result<(Program<Rc<Keypair>>, Pubkey, Pubkey)> {
    let sugar_config = sugar_setup(keypair, rpc_url)?;
    let client = setup_client(&sugar_config)?;
    let program = client.program(CANDY_MACHINE_ID);
    let payer = program.payer();
    let authority = if let Some(authority_str) = authority_opt {
        Pubkey::from_str(&authority_str)?
    } else {
        payer
    };

    Ok((program, payer, authority))
}

fn do_withdraw<C: Deref<Target = impl Signer> + Clone>(
    program: Rc<Program<C>>,
    candy_machine: Pubkey,
    payer: Pubkey,
) -> Result<()> {
    let compute_units = ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNITS);
    let priority_fee = ComputeBudgetInstruction::set_compute_unit_price(PRIORITY_FEE);
    program
        .request()
        .instruction(compute_units)
        .instruction(priority_fee)
        .accounts(nft_accounts::Withdraw {
            candy_machine,
            authority: payer,
        })
        .args(nft_instruction::Withdraw {})
        .send()?;

    Ok(())
}
