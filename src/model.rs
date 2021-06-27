use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Message {
    pub room: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "cmd")]
pub enum Command {
    #[serde(rename = "subscribe")]
    Join(JoinOrLeavePayload),
    #[serde(rename = "unsubscribe")]
    Leave(JoinOrLeavePayload),
}

#[derive(Deserialize, Debug, Clone)]
pub struct JoinOrLeavePayload {
    pub room: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Payload {
    #[serde(rename = "message")]
    Message(Message),
    #[serde(rename = "command")]
    Command(Command),
}
