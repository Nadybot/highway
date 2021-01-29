use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

#[derive(Deserialize, Debug, Clone)]
pub struct Message {
    pub room: String,
    pub id: String,
}

#[derive(Clone)]
pub struct InternalMessage {
    pub payload: Payload,
    pub was_relayed: bool,
    pub tungstenite_message: TungsteniteMessage,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "cmd")]
pub enum Command {
    #[serde(rename = "subscribe")]
    Join(JoinOrLeavePayload),
    #[serde(rename = "unsubscribe")]
    Leave(JoinOrLeavePayload),
    #[serde(rename = "new-public-room")]
    NewPublicRoom(JoinOrLeavePayload),
    #[serde(rename = "hello")]
    Hello(HelloPayload),
}

#[derive(Deserialize, Debug, Clone)]
pub struct JoinOrLeavePayload {
    pub room: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct HelloPayload {
    #[serde(rename = "public-rooms")]
    pub public_rooms: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Feedback {
    pub message: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Payload {
    #[serde(rename = "message")]
    Message(Message),
    #[serde(rename = "command")]
    Command(Command),
    #[serde(rename = "success")]
    Success(Feedback),
    #[serde(rename = "error")]
    Error(Feedback),
}
