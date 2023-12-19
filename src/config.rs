use std::{
    fmt::{self, Write},
    fs::read_to_string,
    path::Path,
    sync::Arc,
};

#[cfg(target_os = "linux")]
use futures_util::StreamExt;
#[cfg(target_os = "linux")]
use inotify::{Inotify, WatchMask};
use leaky_bucket_lite::LeakyBucket;
use serde::Deserialize;
use serde_json::value::RawValue;
use tracing::{error, info};

use crate::{constants::get_ratelimiter, GlobalStateRef, Room, RoomMetaData};

#[derive(Deserialize, Debug)]
pub struct PublicChannel {
    pub name: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub extra_info: Option<Box<RawValue>>,
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
    #[serde(default)]
    pub admin_password_hash: Option<String>,
    #[serde(default)]
    pub public_channels: Vec<PublicChannel>,
    #[serde(default = "default_behind_proxy")]
    pub behind_proxy: bool,
    #[serde(default)]
    pub metrics_token: Option<String>,
}

impl Config {
    pub fn get_global_freq_ratelimiter(&self) -> LeakyBucket {
        get_ratelimiter(&self.msg_freq_ratelimit)
    }

    pub fn get_global_size_ratelimiter(&self) -> LeakyBucket {
        get_ratelimiter(&self.msg_size_ratelimit)
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct Ratelimit {
    pub max_tokens: u32,
    pub tokens: u32,
    pub refill_amount: u32,
    pub refill_millis: u64,
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
        f.write_str(", \"refill_millis\": ")?;
        self.refill_millis.fmt(f)?;
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
        refill_millis: 1000,
    }
}

const fn default_msg_size() -> Ratelimit {
    Ratelimit {
        max_tokens: 5_242_880,
        tokens: 5_242_880,
        refill_amount: 5_242_880,
        refill_millis: 10000,
    }
}

const fn default_max_message_size() -> usize {
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

#[cfg(target_os = "linux")]
pub async fn reloader(global_state: GlobalStateRef) {
    let file = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("config.json"));

    let file_path = Path::new(&file);

    let Some(mut config_dir_path) = file_path.parent() else {
        error!("config path does not have a parent directory");
        return;
    };

    if config_dir_path.as_os_str().is_empty() {
        config_dir_path = Path::new(".");
    }

    let Ok(inotify) = Inotify::init() else {
        error!("Failed to initialize inotify");
        return;
    };

    if inotify
        .watches()
        .add(config_dir_path, WatchMask::MODIFY)
        .is_err()
    {
        error!("Failed to add inotify watch");
        return;
    };

    let mut buffer = [0; 1024];
    let Ok(mut stream) = inotify.into_event_stream(&mut buffer) else {
        error!("Failed to create inotify event stream");
        return;
    };

    while let Some(Ok(event)) = stream.next().await {
        if let Some(name) = event.name {
            if let Some(expected_name) = file_path.file_name() {
                if name == expected_name {
                    match try_load() {
                        Ok(cfg) => {
                            for room in &cfg.public_channels {
                                if let Some(existing_room) = global_state.rooms.get_mut(&room.name)
                                {
                                    let new_metadata = RoomMetaData {
                                        read_only: room.read_only,
                                        extra_info: room.extra_info.clone(),
                                        msg_freq_ratelimit: room.msg_freq_ratelimit.clone(),
                                        msg_size_ratelimit: room.msg_size_ratelimit.clone(),
                                    };

                                    if **existing_room.metadata.load() != new_metadata {
                                        existing_room.metadata.swap(Arc::new(new_metadata));
                                        existing_room.resend_room_info();
                                    }
                                } else {
                                    let room = Room::new(
                                        room.name.clone(),
                                        room.read_only,
                                        room.extra_info.clone(),
                                        room.msg_freq_ratelimit.clone(),
                                        room.msg_size_ratelimit.clone(),
                                    );

                                    global_state.rooms.insert(room.inner.name.clone(), room);
                                }
                            }

                            info!("Successfully reloaded the configuration");
                        }
                        Err(e) => {
                            error!("Failed to reload config: {e}");
                        }
                    }
                }
            }
        }
    }
}
