//! Distinct identifiers at application command boundaries.
use crate::{Error, Result};
use std::{fmt, num::NonZeroI64, str::FromStr};

macro_rules! identifier {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(NonZeroI64);

        impl $name {
            pub fn new(value: i64) -> Result<Self> {
                NonZeroI64::new(value)
                    .filter(|id| id.get() > 0)
                    .map(Self)
                    .ok_or_else(|| {
                        Error::Input(concat!($kind, " ID must be a positive integer").into())
                    })
            }
            pub fn get(self) -> i64 {
                self.0.get()
            }
        }
        impl TryFrom<i64> for $name {
            type Error = Error;
            fn try_from(value: i64) -> Result<Self> {
                Self::new(value)
            }
        }
        impl FromStr for $name {
            type Err = Error;
            fn from_str(value: &str) -> Result<Self> {
                Self::new(value.parse().map_err(|_| {
                    Error::Input(concat!($kind, " ID must be a positive integer").into())
                })?)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.get().fmt(formatter)
            }
        }
    };
}
identifier!(EntryId, "Entry");
identifier!(TaskId, "Task");
