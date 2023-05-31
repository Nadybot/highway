use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Message<'a> {
    pub room: &'a str,
    #[serde(skip_deserializing)]
    pub user: &'a str,
    #[serde(default, borrow)]
    pub body: Option<&'a RawValue>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct JoinOrLeavePayload<'a> {
    #[serde(borrow)]
    pub room: &'a str,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Payload<'a> {
    #[serde(borrow, rename = "message")]
    Message(Message<'a>),
    #[serde(borrow, rename = "join")]
    Join(JoinOrLeavePayload<'a>),
    #[serde(borrow, rename = "leave")]
    Leave(JoinOrLeavePayload<'a>),
    #[serde(rename = "quit")]
    Quit,
}
