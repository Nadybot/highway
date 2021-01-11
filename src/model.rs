use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Payload {
    pub room: String,
    // do not deserialize other keys to save memory
}
