//! SQLite stores and their unchanged schema migrations.
//!
//! Opening storage does not start a scheduler, HTTP service or GUI. Database
//! files, cache layout and serialized settings remain compatible with the app.

pub mod browsing;
pub mod content;
pub mod migrations;
pub mod videodb;
