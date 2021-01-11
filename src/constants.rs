use crate::json::to_vec;

use lazy_static::lazy_static;
use leaky_bucket::LeakyBucket;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use std::{collections::HashSet, env::var};

pub const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MIN_PRIV_ROOM_LEN: usize = 32;

lazy_static! {
    static ref PUBLIC_CHANNELS: HashSet<String> = {
        let mut set = HashSet::with_capacity(10);
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
        set
    };
    pub static ref CONFIG: WebSocketConfig = {
        WebSocketConfig {
            accept_unmasked_frames: false,
            max_send_queue: None,
            max_message_size: Some(
                var("MAX_MESSAGE_SIZE")
                    .unwrap_or_else(|_| String::from("1048576"))
                    .parse()
                    .unwrap(),
            ),
            max_frame_size: Some(
                var("MAX_FRAME_SIZE")
                    .unwrap_or_else(|_| String::from("1048576"))
                    .parse()
                    .unwrap(),
            ),
        }
    };
    pub static ref PUBLIC_CHANNELS_SERIALIZED: Vec<u8> = to_vec(&*PUBLIC_CHANNELS).unwrap();
}

#[inline(always)]
pub fn is_valid_room(room: &str) -> bool {
    PUBLIC_CHANNELS.iter().any(|i| *i == room) || room.len() >= MIN_PRIV_ROOM_LEN
}

pub fn get_ratelimiter() -> LeakyBucket {
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
        .unwrap()
}
