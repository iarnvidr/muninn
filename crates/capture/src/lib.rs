//! Bounded metadata parsing and merging, independent of network interception.
//!
//! The service still owns ingest orchestration and the native capture proxy;
//! these parsers can be tested and reused without either of them.

pub mod extract;
pub mod merge;
