use leaky_bucket_lite::LeakyBucket;
use tokio::time::Duration;

use std::env::var;

pub const INVALID_ROOM_MSG: &str = "{\"type\": \"error\", \"message\": \"You attempted to interact with a room that you are not subscribed to\"}";
pub const ROOM_NAME_TOO_SHORT: &str =
    "{\"type\": \"error\", \"message\": \"The room name provided is shorter than 32 characters\"}";
pub const INVALID_JSON_MSG: &str =
    "{\"type\": \"error\", \"message\": \"You sent an invalid JSON payload\"}";
pub const ROOM_JOIN_MSG: &str = "{\"type\": \"success\", \"message\": \"You joined the room\"}";
pub const ROOM_LEAVE_MSG: &str = "{\"type\": \"success\", \"message\": \"You left the room\"}";
const MIN_ROOM_LEN: usize = 32;

#[inline(always)]
pub fn is_valid_room(room: &str) -> bool {
    room.len() >= MIN_ROOM_LEN
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
