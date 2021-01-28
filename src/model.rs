use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

#[derive(Deserialize, Debug, Clone)]
pub struct Message {
    pub room: String,
    pub id: String,
}

#[derive(Clone)]
pub struct InternalMessage {
    pub message: Message,
    pub tungstenite_message: TungsteniteMessage,
}

#[derive(Deserialize, Debug)]
pub enum CommandType {
    #[serde(rename = "subscribe")]
    Join,
    #[serde(rename = "unsubscribe")]
    Leave,
    #[serde(rename = "new-public-room")]
    NewPublicRoom,
}

#[derive(Deserialize, Debug)]
pub struct CommandPayload {
    pub cmd: CommandType,
    pub room: String,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum Payload {
    #[serde(rename = "message")]
    Message(Message),
    #[serde(rename = "command")]
    Command(CommandPayload),
}
