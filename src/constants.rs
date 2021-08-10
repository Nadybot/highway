use crate::config::CONFIG;

use leaky_bucket_lite::LeakyBucket;
use tokio::time::Duration;

pub const INVALID_ROOM_MSG: &str = "{\"type\": \"error\", \"message\": \"You attempted to interact with a room that you are not subscribed to\"}";
pub const ROOM_NAME_TOO_SHORT: &str =
    "{\"type\": \"error\", \"message\": \"The room name provided is shorter than 32 characters\"}";
pub const ROOM_READ_ONLY: &str = "{\"type\": \"error\", \"message\": \"The room you attempted to send a message in is in read-only mode\"}";
pub const INVALID_JSON_MSG: &str =
    "{\"type\": \"error\", \"message\": \"You sent an invalid JSON payload\"}";
pub const ROOM_JOIN_MSG: &str = "{\"type\": \"success\", \"message\": \"You joined the room\"}";
pub const ROOM_LEAVE_MSG: &str = "{\"type\": \"success\", \"message\": \"You left the room\"}";
const MIN_ROOM_LEN: usize = 32;

pub const fn is_valid_room(room: &str) -> bool {
    room.len() >= MIN_ROOM_LEN
}

pub fn get_freq_ratelimiter() -> LeakyBucket {
    LeakyBucket::builder()
        .max(CONFIG.msg_per_sec)
        .tokens(CONFIG.msg_per_sec)
        .refill_interval(Duration::from_secs(1))
        .refill_amount(CONFIG.msg_per_sec)
        .build()
}

pub fn get_size_ratelimiter() -> LeakyBucket {
    LeakyBucket::builder()
        .max(CONFIG.bytes_per_10_sec)
        .tokens(CONFIG.bytes_per_10_sec)
        .refill_interval(Duration::from_secs(10))
        .refill_amount(CONFIG.bytes_per_10_sec)
        .build()
}
