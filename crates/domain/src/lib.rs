//! Shared contracts used by storage, policy, capture, media and application roles.
//!
//! This crate has no service runtime, HTTP router or UI assets. SQLite error
//! conversion is opt-in so consumers of the models and policy do not need a
//! database. Native filesystem helpers remain here; this is not a WASM promise.

pub mod error;
pub mod browsing;
pub mod model;
pub mod playlist;
pub mod quality;
pub mod settings;
pub mod util;

pub use error::{Error, Result};

#[cfg(test)]
mod tests {
    #[test]
    fn hex_encodes_bytes_as_lowercase() {
        assert_eq!(super::util::ci_feedback_probe_missing_helper(&[0x00, 0xab, 0xff]), "00abff");
    }
}
