use std::mem::transmute;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::constants;

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PayloadKind {
    Error,
    Join,
    Leave,
    Message,
    Quit,
    Success,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Payload<'a> {
    #[serde(rename = "type")]
    pub kind: PayloadKind,
    #[serde(default, skip_serializing_if = "str::is_empty")]
    pub room: &'a str,
    #[serde(skip_deserializing, skip_serializing_if = "str::is_empty")]
    pub user: &'a str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<&'a RawValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<&'a RawValue>,
}

impl<'a> Payload<'a> {
    pub fn is_invalid(&self) -> bool {
        match self.kind {
            PayloadKind::Join | PayloadKind::Leave => self.room.is_empty(),
            PayloadKind::Message => self.room.is_empty() || self.body.is_none(),
            PayloadKind::Quit => false,
            _ => true, // Other kinds are not allowed to be sent by the client
        }
    }

    pub fn reply_room_join(&self) -> Self {
        Self {
            kind: PayloadKind::Success,
            room: self.room,
            user: "",
            id: self.id,
            body: Some(unsafe { transmute(constants::ROOM_JOIN_MSG) }),
        }
    }

    pub fn reply_room_leave(&self) -> Self {
        Self {
            kind: PayloadKind::Success,
            room: self.room,
            user: "",
            id: self.id,
            body: Some(unsafe { transmute(constants::ROOM_LEAVE_MSG) }),
        }
    }

    pub fn reply_invalid_msg(&self) -> Self {
        Self {
            kind: PayloadKind::Error,
            room: self.room,
            user: "",
            id: self.id,
            body: Some(unsafe { transmute(constants::INVALID_MSG) }),
        }
    }

    pub fn reply_invalid_room(&self) -> Self {
        Self {
            kind: PayloadKind::Error,
            room: self.room,
            user: "",
            id: self.id,
            body: Some(unsafe { transmute(constants::INVALID_ROOM_MSG) }),
        }
    }

    pub fn reply_room_name_too_short(&self) -> Self {
        Self {
            kind: PayloadKind::Error,
            room: self.room,
            user: "",
            id: self.id,
            body: Some(unsafe { transmute(constants::ROOM_NAME_TOO_SHORT) }),
        }
    }

    pub fn reply_room_read_only(&self) -> Self {
        Self {
            kind: PayloadKind::Error,
            room: self.room,
            user: "",
            id: self.id,
            body: Some(unsafe { transmute(constants::ROOM_READ_ONLY) }),
        }
    }
}
