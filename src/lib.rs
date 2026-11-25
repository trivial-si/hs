//! Local time tracking with a pure domain, transactional application workflows,
//! and concrete SQLite persistence. The CLI is a separate binary in this package.
pub mod application;
mod digest;
pub mod domain;
pub mod error;
pub mod storage;
pub mod time;
pub mod wire;

// Compatibility paths for existing library callers. These re-export the same
// implementations; the canonical module hierarchy defines their ownership.
pub use application::{import, pomodoro, queries as snapshot, tasks, workflows as tracking};
pub use domain::{model, parse, report};
pub use error::{Error, Result};
pub use storage::store;
pub use store::Store;
