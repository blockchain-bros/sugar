use bundlr_sdk::{tags::Tag, Bundlr, SolanaSigner};
use data_encoding::HEXLOWER;
use glob::glob;
use regex::RegexBuilder;
use ring::digest::{Context, SHA256};
use serde_json;
use std::{
    fs::{self, DirEntry, File, OpenOptions},
    io::{BufReader, Read},
    sync::Arc,
};

use crate::common::*;
use crate::validate::format::Metadata;

pub struct UploadDataArgs<'a> {
    pub bundlr_client: Arc<Bundlr<SolanaSigner>>,
    pub assets_dir: &'a Path,
    pub extension_glob: &'a str,
    pub tags: Vec<Tag>,
    pub data_type: DataType,
}

#[derive(Debug, Clone)]
pub enum DataType {
    Img,
    Metadata,
    Movie,
}

#[derive(Debug, Clone)]
pub struct AssetPair {
    pub name: String,
    pub metadata: String,
    pub metadata_hash: String,
    pub media: String,
    pub media_hash: String,
    pub animation: Option<String>,
    pub animation_hash: Option<String>,
}

impl AssetPair {
    pub fn into_cache_item(self) -> CacheItem {
        CacheItem {
            name: self.name,
            media_hash: self.media_hash,
            media_link: String::new(),
            metadata_hash: self.metadata_hash,
            metadata_link: String::new(),
            on_chain: false,
            animation_hash: self.animation_hash,
            animation_link: self.animation,
        }
    }
}

pub fn get_data_size(assets_dir: &Path, extension: &str) -> Result<u64> {
    let path = assets_dir
        .join(format!("*.{extension}"))
        .to_str()
        .expect("Failed to convert asset directory path from unicode.")
        .to_string();

    let assets = glob(&path)?;

    let mut total_size = 0;

    for asset in assets {
        let asset_path = asset?;
        let size = std::fs::metadata(asset_path)?.len();
        total_size += size;
    }

    Ok(total_size)
}

pub fn count_files(assets_dir: &str) -> Result<Vec<DirEntry>> {
    let files = fs::read_dir(assets_dir)
        .map_err(|_| anyhow!("Failed to read assets directory"))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            !entry
                .file_name()
                .to_str()
                .expect("Failed to convert file name to valid unicode.")
                .starts_with('.')
                && entry
                    .metadata()
                    .expect("Failed to retrieve metadata from file")
                    .is_file()
        });

    Ok(files.collect())
}

pub fn get_asset_pairs(assets_dir: &str) -> Result<HashMap<usize, AssetPair>> {
    // filters out directories and hidden files
    let filtered_files = count_files(assets_dir)?;

    let paths = filtered_files
        .into_iter()
        .map(|entry| {
            let file_name_as_string =
                String::from(entry.path().file_name().unwrap().to_str().unwrap());
            file_name_as_string
        })
        .collect::<Vec<String>>();

    let mut asset_pairs: HashMap<usize, AssetPair> = HashMap::new();

    let paths_ref = &paths;

    let metadata_filenames = paths_ref
        .clone()
        .into_iter()
        .filter(|p| p.to_lowercase().ends_with(".json"))
        .collect::<Vec<String>>();

    for metadata_filename in metadata_filenames {
        let i = metadata_filename.split('.').next().unwrap();

        if i.parse::<usize>().is_err() {
            let error = anyhow!(
                "Couldn't parse filename '{}' to a valid index number.",
                metadata_filename
            );
            error!("{:?}", error);
            return Err(error);
        };

        let img_pattern = format!("^{}\\.((jpg)|(gif)|(png))$", i);

        let img_regex = RegexBuilder::new(&img_pattern)
            .case_insensitive(true)
            .build()
            .expect("Failed to create regex.");

        let img_filenames = paths_ref
            .clone()
            .into_iter()
            .filter(|p| img_regex.is_match(p))
            .collect::<Vec<String>>();

        let img_filename = if img_filenames.is_empty() {
            let error = anyhow!(
                "Couldn't parse image filename at index {} to a valid index number.",
                i.parse::<usize>().unwrap()
            );
            error!("{:?}", error);
            return Err(error);
        } else {
            &img_filenames[0]
        };

        let animation_pattern = format!("^{}\\.((mp4)|(mov)|(webm))$", i);
        let animation_regex = RegexBuilder::new(&animation_pattern)
            .case_insensitive(true)
            .build()
            .expect("Failed to create regex.");
        let animation_filenames = paths_ref
            .clone()
            .into_iter()
            .filter(|p| animation_regex.is_match(p))
            .collect::<Vec<String>>();

        let metadata_filepath = Path::new(assets_dir)
            .join(&metadata_filename)
            .to_str()
            .expect("Failed to convert metadata path from unicode.")
            .to_string();

        let m = File::open(&metadata_filepath)?;
        let metadata: Metadata = serde_json::from_reader(m)?;
        let name = metadata.name.clone();

        let img_filepath = Path::new(assets_dir)
            .join(img_filename)
            .to_str()
            .expect("Failed to convert media path from unicode.")
            .to_string();

        let animation_filename = if !animation_filenames.is_empty() {
            let animation_filepath = Path::new(assets_dir)
                .join(&animation_filenames[0])
                .to_str()
                .expect("Failed to convert media path from unicode.")
                .to_string();

            Some(animation_filepath)
        } else {
            None
        };

        let animation_hash = if let Some(animation_file) = &animation_filename {
            let encoded_filename = encode(animation_file)?;
            Some(encoded_filename)
        } else {
            None
        };

        let asset_pair = AssetPair {
            name,
            metadata: metadata_filepath.clone(),
            metadata_hash: encode(&metadata_filepath)?,
            media: img_filepath.clone(),
            media_hash: encode(&img_filepath)?,
            animation_hash,
            animation: animation_filename,
        };

        asset_pairs.insert(i.parse::<usize>().unwrap(), asset_pair);
    }

    Ok(asset_pairs)
}

fn encode(file: &str) -> Result<String> {
    let input = File::open(file)?;
    let mut reader = BufReader::new(input);
    let mut context = Context::new(&SHA256);
    let mut buffer = [0; 1024];

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        context.update(&buffer[..count]);
    }

    Ok(HEXLOWER.encode(context.finish().as_ref()))
}

pub fn get_updated_metadata(
    metadata_file: &str,
    media_link: &str,
    animation_link: Option<String>,
) -> Result<String> {
    let mut metadata: Metadata = {
        let m = OpenOptions::new().read(true).open(metadata_file)?;
        serde_json::from_reader(&m)?
    };

    metadata.image = media_link.to_string();
    metadata.properties.files[0].uri = media_link.to_string();

    if animation_link.is_some() {
        metadata.animation_url = animation_link.clone();
        metadata.properties.files[1].uri = animation_link.unwrap();
    }

    Ok(serde_json::to_string(&metadata).unwrap())
}
