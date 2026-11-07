//! Shared error type. HTTP adapters map it to a status code.
//! SQLite conversion is enabled by the storage crate, not by pure consumers.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[cfg(feature = "sqlite")]
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn other(message: impl Into<String>) -> Self {
        Error::Other(message.into())
    }
    pub fn bad(message: impl Into<String>) -> Self {
        Error::BadRequest(message.into())
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Error::NotFound(message.into())
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn non_database_error_conversions_preserve_messages() {
        let io: Error = std::io::Error::new(std::io::ErrorKind::NotFound, "cache missing").into();
        assert_eq!(io.to_string(), "io: cache missing");
        let json: Error = serde_json::from_str::<serde_json::Value>("{").unwrap_err().into();
        assert!(matches!(json, Error::Json(_)));
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_feature_preserves_database_conversion() {
        let error: Error = rusqlite::Error::QueryReturnedNoRows.into();
        assert!(matches!(error, Error::Db(rusqlite::Error::QueryReturnedNoRows)));
        assert!(error.to_string().starts_with("database: "));
    }
}
