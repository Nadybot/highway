use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;

#[derive(Deserialize, Debug, Clone)]
pub struct Payload {
    pub room: String,
    // do not deserialize other keys to save memory
}

#[derive(Clone)]
pub struct InternalMessage {
    pub room: String,
    pub message: Message,
}
