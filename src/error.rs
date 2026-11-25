#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Input(String),
    #[error("{0}")]
    Conflict(String),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("data error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Stable categories and codes used by machine-readable clients.
pub struct ErrorClass {
    pub kind: &'static str,
    pub code: &'static str,
}

impl Error {
    pub fn classification(&self) -> ErrorClass {
        use rusqlite::{Error::SqliteFailure, ErrorCode};
        let (kind, code) = match self {
            Self::Input(_) => ("input", "invalid_input"),
            Self::Conflict(_) => ("conflict", "conflict"),
            Self::Database(SqliteFailure(error, _))
                if matches!(
                    error.code,
                    ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
                ) =>
            {
                ("database", "database_busy")
            }
            Self::Database(_) => ("database", "database_error"),
            Self::Io(_) => ("io", "io_error"),
            Self::Json(error) if error.is_io() => ("io", "io_error"),
            Self::Json(_) => ("data", "json_error"),
        };
        ErrorClass { kind, code }
    }

    pub fn is_broken_pipe(&self) -> bool {
        use std::io::ErrorKind;
        match self {
            Self::Io(error) => error.kind() == ErrorKind::BrokenPipe,
            Self::Json(error) => error.io_error_kind() == Some(ErrorKind::BrokenPipe),
            _ => false,
        }
    }
}
