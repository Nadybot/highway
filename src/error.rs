use std::{error::Error, fmt};

#[derive(Debug)]
pub enum RequestError {
    InvalidRoom,
    NotWebsocket,
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoom => write!(f, "Invalid room requested"),
            Self::NotWebsocket => write!(f, "Connection is not websocket"),
        }
    }
}

impl Error for RequestError {}
