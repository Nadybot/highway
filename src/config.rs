use log::error;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Error;

use std::{fs::read_to_string, path::Path, process::exit};

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

#[inline(always)]
fn default_port() -> u16 {
    3333
}

#[inline(always)]
fn default_msg_per_sec() -> usize {
    10
}

#[inline(always)]
fn default_bytes_per_10_sec() -> usize {
    5242880
}

#[inline(always)]
fn default_max_message_size() -> usize {
    1048576
}

#[inline(always)]
fn default_max_frame_size() -> usize {
    1048576
}

pub fn try_load() -> Result<Config, Error> {
    if Path::new("config.json").exists() {
        let content = read_to_string("config.json").unwrap();
        crate::json::from_str(&content)
    } else {
        crate::json::from_str("{}")
    }
}

pub static CONFIG: Lazy<Config> = Lazy::new(|| {
    try_load().unwrap_or_else(|e| {
        error!("Configuration Error: {}", e);
        exit(1)
    })
});
