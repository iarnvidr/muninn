//! Media acquisition tools and local-file playback primitives.
//!
//! The service owns jobs and play-while-caching orchestration. The range adapter
//! here reads local complete or growing cache files; it is not an upstream
//! playback fallback.

pub mod range;
pub mod stream_diag;
pub mod ytdlp;
