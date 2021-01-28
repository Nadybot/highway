use dashmap::DashSet;
use leaky_bucket_lite::LeakyBucket;
use once_cell::sync::Lazy;
use tokio::time::Duration;

use std::{env::var, sync::Arc};

pub const INVALID_ROOM_MSG: &str = "{\"type\": \"error\", \"message\": \"You attempted to interact with a room that you are not subscribed to\"}";
pub const INVALID_JSON_MSG: &str = "{\"type\": \"error\", \"message\": \"You sent an invalid JSON payload\"}";
pub const INVALID_CMD_MSG: &str =
    "{\"type\": \"error\", \"message\": \"You sent a command that clients may not send\"}";
pub const ROOM_JOIN_MSG: &str = "{\"type\": \"success\", \"message\": \"You joined the room\"}";
pub const ROOM_LEAVE_MSG: &str = "{\"type\": \"success\", \"message\": \"You left the room\"}";
const MIN_PRIV_ROOM_LEN: usize = 32;

static PUBLIC_CHANNELS: Lazy<Arc<DashSet<String>>> = Lazy::new(|| {
    let set = DashSet::with_capacity(10);
    set.insert(String::from("pvp"));
    set.insert(String::from("ooc"));
    set.insert(String::from("pvm"));
    set.insert(String::from("wtb"));
    set.insert(String::from("wts"));
    set.insert(String::from("clan"));
    set.insert(String::from("omni"));
    set.insert(String::from("neutral"));
    set.insert(String::from("rp"));
    set.insert(String::from("chat"));
    Arc::new(set)
});

#[inline(always)]
pub fn public_channels() -> Vec<String> {
    PUBLIC_CHANNELS.iter().map(|i| i.key().clone()).collect()
}

#[inline(always)]
pub fn get_hello_payload() -> String {
    format!(
        "{{\"type\": \"command\", \"cmd\": \"hello\", \"public-rooms\": {:?}}}",
        public_channels()
    )
}

#[inline(always)]
pub fn is_valid_room(room: &str) -> bool {
    PUBLIC_CHANNELS.iter().any(|i| *i == room) || room.len() >= MIN_PRIV_ROOM_LEN
}

#[inline(always)]
pub fn new_room_payload(room: &str) -> String {
    format!(
        "{{\"type\": \"command\", \"cmd\": \"new-public-room\", \"room\": {:?}}}",
        room
    )
}

pub fn get_freq_ratelimiter() -> LeakyBucket {
    let msg_per_sec = var("MSG_PER_SEC")
        .unwrap_or_else(|_| String::from("10"))
        .parse()
        .unwrap();
    LeakyBucket::builder()
        .max(msg_per_sec)
        .tokens(msg_per_sec)
        .refill_interval(Duration::from_secs(1))
        .refill_amount(msg_per_sec)
        .build()
}

pub fn get_size_ratelimiter() -> LeakyBucket {
    let bytes_per_10_seconds = var("BYTES_PER_10_SEC")
        .unwrap_or_else(|_| String::from("5242880"))
        .parse()
        .unwrap();
    LeakyBucket::builder()
        .max(bytes_per_10_seconds)
        .tokens(bytes_per_10_seconds)
        .refill_interval(Duration::from_secs(10))
        .refill_amount(bytes_per_10_seconds)
        .build()
}
