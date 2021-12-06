use crate::json::Value;

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Message {
    pub room: String,
    #[serde(skip_deserializing)]
    pub user: String,
    #[serde(default)]
    pub body: Value,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct JoinOrLeavePayload {
    pub room: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Payload {
    #[serde(rename = "message")]
    Message(Message),
    #[serde(rename = "join")]
    Join(JoinOrLeavePayload),
    #[serde(rename = "leave")]
    Leave(JoinOrLeavePayload),
    #[serde(rename = "quit")]
    Quit,
}
