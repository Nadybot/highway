#[cfg(feature = "simd")]
use serde::Deserialize;
#[cfg(not(feature = "simd"))]
pub use serde_json::{from_reader, from_slice, from_str, to_string, to_vec, Error, Value};
#[cfg(feature = "simd")]
pub use simd_json::{
    from_reader, from_slice, from_str as simd_from_str, to_string, to_vec, Error,
    OwnedValue as Value,
};

#[cfg(feature = "simd")]
pub fn from_str<'a, T>(s: &'a mut str) -> Result<T, Error>
where
    T: Deserialize<'a>,
{
    unsafe { simd_from_str(s) }
}
