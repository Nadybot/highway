use std::{
    fmt::{self, Write},
    fs::read_to_string,
    path::Path,
    process::exit,
    sync::LazyLock,
};

use serde::Deserialize;
use tracing::error;

#[derive(Deserialize, Debug)]
pub struct PublicChannel {
    pub name: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub msg_freq_ratelimit: Option<Ratelimit>,
    #[serde(default)]
    pub msg_size_ratelimit: Option<Ratelimit>,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_connections_per_ip")]
    pub connections_per_ip: usize,
    #[serde(default = "default_msg_freq")]
    pub msg_freq_ratelimit: Ratelimit,
    #[serde(default = "default_msg_size")]
    pub msg_size_ratelimit: Ratelimit,
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

#[derive(Deserialize, Debug, Clone)]
pub struct Ratelimit {
    pub max_tokens: u32,
    pub tokens: u32,
    pub refill_amount: u32,
    pub refill_secs: u64,
}

// Hacky workaround to save on the JSON serialization
impl fmt::Display for Ratelimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("{\"max_tokens\": ")?;
        self.max_tokens.fmt(f)?;
        f.write_str(", \"tokens\": ")?;
        self.tokens.fmt(f)?;
        f.write_str(", \"refill_amount\": ")?;
        self.refill_amount.fmt(f)?;
        f.write_str(", \"refill_secs\": ")?;
        self.refill_secs.fmt(f)?;
        f.write_char('}')?;

        Ok(())
    }
}

const fn default_port() -> u16 {
    3333
}

const fn default_connections_per_ip() -> usize {
    50
}

const fn default_msg_freq() -> Ratelimit {
    Ratelimit {
        max_tokens: 10,
        tokens: 10,
        refill_amount: 10,
        refill_secs: 1,
    }
}

const fn default_msg_size() -> Ratelimit {
    Ratelimit {
        max_tokens: 5_242_880,
        tokens: 5_242_880,
        refill_amount: 5_242_880,
        refill_secs: 10,
    }
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

pub fn try_load() -> Result<Config, serde_json::Error> {
    let file = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("config.json"));
    let path = Path::new(&file);

    if path.exists() {
        let content = read_to_string(path).unwrap();
        serde_json::from_str(&content)
    } else {
        serde_json::from_str("{}")
    }
}

pub static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    try_load().unwrap_or_else(|e| {
        error!("Configuration Error: {}", e);
        exit(1)
    })
});
