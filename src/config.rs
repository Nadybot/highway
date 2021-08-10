use crate::json::Error;
use log::error;
use serde::Deserialize;

use std::{fs::read_to_string, lazy::SyncLazy, path::Path, process::exit};

#[derive(Deserialize, Debug)]
pub struct PublicChannel {
    pub name: String,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_msg_per_sec")]
    pub msg_per_sec: usize,
    #[serde(default = "default_bytes_per_10_sec")]
    pub bytes_per_10_sec: usize,
    #[serde(default = "default_max_message_size")]
    pub max_message_size: usize,
    #[serde(default = "default_max_frame_size")]
    pub max_frame_size: usize,
    #[serde(default)]
    pub admin_password_hash: Option<String>,
    #[serde(default)]
    pub public_channels: Vec<PublicChannel>,
}

const fn default_port() -> u16 {
    3333
}

const fn default_msg_per_sec() -> usize {
    10
}

const fn default_bytes_per_10_sec() -> usize {
    5_242_880
}

const fn default_max_message_size() -> usize {
    1_048_576
}

const fn default_max_frame_size() -> usize {
    1_048_576
}

pub fn try_load() -> Result<Config, Error> {
    if Path::new("config.json").exists() {
        let mut content = read_to_string("config.json").unwrap();
        crate::json::from_str(&mut content)
    } else {
        let mut content = String::from("{}");
        crate::json::from_str(&mut content)
    }
}

pub static CONFIG: SyncLazy<Config> = SyncLazy::new(|| {
    try_load().unwrap_or_else(|e| {
        error!("Configuration Error: {}", e);
        exit(1)
    })
});
