//! System clock and timezone acquisition at the application boundary.
use crate::{
    Error, Result,
    domain::time::{Moment, resolve_local},
};
use chrono::{NaiveDateTime, Utc};
use chrono_tz::Tz;
use std::{env, ffi::OsStr};

pub fn now() -> Result<Moment> {
    Ok(Moment::at(Utc::now(), system_zone()?))
}

pub(crate) fn from_local(local: NaiveDateTime) -> Result<Moment> {
    let zone = system_zone()?;
    Ok(Moment::at(resolve_local(local, zone, None)?, zone))
}

fn explicit_zone(value: &OsStr) -> Result<Tz> {
    let name = value
        .to_str()
        .ok_or_else(|| Error::Input("TZ must contain an IANA timezone name".into()))?;
    let name = name.strip_prefix(':').unwrap_or(name);
    let name = name.strip_prefix("/usr/share/zoneinfo/").unwrap_or(name);
    match name {
        // POSIX aliases in widespread use for a clock fixed at UTC.
        "" | "UTC0" | "GMT0" => Ok(chrono_tz::UTC),
        _ => name.parse().map_err(|_| {
            Error::Input(format!(
                "Unknown TZ {name:?}. Set an IANA timezone such as Europe/Zagreb."
            ))
        }),
    }
}

fn system_zone() -> Result<Tz> {
    if let Some(value) = env::var_os("TZ")
        && value != "/etc/localtime"
        && value != ":/etc/localtime"
    {
        return explicit_zone(&value);
    }
    let name = iana_time_zone::get_timezone().map_err(|error| {
        Error::Input(format!("Unable to determine local time zone ({error}); set TZ to an IANA zone such as Europe/Zagreb."))
    })?;
    name.parse().map_err(|_| {
        Error::Input(format!(
            "Unknown system time zone {name:?}; set TZ to an IANA zone."
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_unknown_timezone_never_silently_changes_the_clock() {
        assert!(explicit_zone(OsStr::new("Europe/Imaginary")).is_err());
        assert_eq!(
            explicit_zone(OsStr::new(":/usr/share/zoneinfo/Europe/Zagreb")).unwrap(),
            chrono_tz::Europe::Zagreb
        );
        assert_eq!(explicit_zone(OsStr::new("UTC0")).unwrap(), chrono_tz::UTC);
    }
}
