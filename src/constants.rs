use leaky_bucket_lite::LeakyBucket;
use tokio::time::Duration;

use crate::config::Ratelimit;

pub const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub const INVALID_ROOM_MSG: &str = "{\"type\": \"error\", \"message\": \"You attempted to interact with an invalid room, either because you are subscribed and trying to subscribe again or because you are not subscribed and unsubscribing or sending a message\"}";
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

pub fn get_ratelimiter(ratelimit: &Ratelimit) -> LeakyBucket {
    LeakyBucket::builder()
        .max(ratelimit.max_tokens)
        .tokens(ratelimit.tokens)
        .refill_interval(Duration::from_millis(ratelimit.refill_millis))
        .refill_amount(ratelimit.refill_amount)
        .build()
}
