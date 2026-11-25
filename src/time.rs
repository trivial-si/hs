//! Compatibility exports. New code uses domain::time and application::clock.
pub use crate::domain::time::*;

impl Moment {
    /// Compatibility adapter for obtaining the system clock.
    pub fn system() -> crate::Result<Self> {
        crate::application::clock::now()
    }
    /// Compatibility adapter interpreting a civil time in the system zone.
    pub fn from_local(local: chrono::NaiveDateTime) -> crate::Result<Self> {
        crate::application::clock::from_local(local)
    }
}
