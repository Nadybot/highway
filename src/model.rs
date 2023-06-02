use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PayloadKind {
    Message,
    Join,
    Leave,
    Quit,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Payload<'a> {
    #[serde(rename = "type")]
    pub kind: PayloadKind,
    #[serde(default, skip_serializing_if = "str::is_empty")]
    pub room: &'a str,
    #[serde(skip_deserializing)]
    pub user: &'a str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<&'a RawValue>,
}

impl<'a> Payload<'a> {
    pub fn is_invalid(&self) -> bool {
        match self.kind {
            PayloadKind::Join | PayloadKind::Leave => self.room.is_empty(),
            PayloadKind::Message => self.room.is_empty() || self.body.is_none(),
            PayloadKind::Quit => false,
        }
    }
}
