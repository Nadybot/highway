use crate::json::Error;
use serde::Deserialize;
use tracing::error;

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
    #[serde(default = "default_connections_per_ip")]
    pub connections_per_ip: usize,
    #[serde(default = "default_msg_per_sec")]
    pub msg_per_sec: u32,
    #[serde(default = "default_bytes_per_10_sec")]
    pub bytes_per_10_sec: u32,
    #[serde(default = "default_max_message_size")]
    pub max_message_size: usize,
    #[serde(default = "default_max_frame_size")]
    pub max_frame_size: usize,
    #[serde(default)]
    pub admin_password_hash: Option<String>,
    #[serde(default)]
    pub public_channels: Vec<PublicChannel>,
    #[serde(default = "default_behind_proxy")]
    pub behind_proxy: bool,
    #[serde(default)]
    pub metrics_token: Option<String>,
}

const fn default_port() -> u16 {
    3333
}

const fn default_connections_per_ip() -> usize {
    50
}

const fn default_msg_per_sec() -> u32 {
    10
}

const fn default_bytes_per_10_sec() -> u32 {
    5_242_880
}

const fn default_max_message_size() -> usize {
    1_048_576
}

const fn default_max_frame_size() -> usize {
    1_048_576
}

const fn default_behind_proxy() -> bool {
    false
}

pub fn try_load() -> Result<Config, Error> {
    let file = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("config.json"));
    let path = Path::new(&file);

    if path.exists() {
        let mut content = read_to_string(path).unwrap();
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
