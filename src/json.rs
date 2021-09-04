#[cfg(not(feature = "simd"))]
pub use serde_json::{from_reader, from_slice, from_str, to_string, to_vec, Error, Value};
#[cfg(feature = "simd")]
pub use simd_json::{
    from_reader, from_slice, from_str, to_string, to_vec, Error, OwnedValue as Value,
};
