//! Civil Hamster dates carry no zone. Live timers keep the instant and IANA
//! zone together so elapsed time remains meaningful across DST and travel.
use crate::{Error, Result};
use chrono::{DateTime, LocalResult, NaiveDateTime, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    pub utc: DateTime<Utc>,
    pub zone: Tz,
}

impl Moment {
    pub fn at(utc: DateTime<Utc>, zone: Tz) -> Self {
        Self {
            utc: utc.with_nanosecond(0).expect("zero nanoseconds is valid"),
            zone,
        }
    }

    pub fn local(self) -> Result<NaiveDateTime> {
        local_at(self.utc, self.zone)
    }
}

pub fn local_at(utc: DateTime<Utc>, zone: Tz) -> Result<NaiveDateTime> {
    let instant = utc.naive_utc();
    let offset = zone.offset_from_utc_datetime(&instant).fix();
    instant.checked_add_offset(offset).ok_or_else(|| {
        Error::Input(format!(
            "UTC instant {utc} is outside the representable local-time range in {zone}"
        ))
    })
}

/// Resolve a civil edit in its entry's original zone. A known UTC endpoint
/// can select the same offset when a civil time occurs twice during a fold.
pub fn resolve_local(
    local: NaiveDateTime,
    zone: Tz,
    reference: Option<DateTime<Utc>>,
) -> Result<DateTime<Utc>> {
    match zone.from_local_datetime(&local) {
        LocalResult::Single(value) => Ok(value.to_utc()),
        LocalResult::None if chrono_tz::GapInfo::new(&local, &zone).is_some() => Err(Error::Input(
            format!("{local} does not exist in {zone} because the clock moves forward."),
        )),
        LocalResult::None => Err(Error::Input(format!(
            "{local} in {zone} falls outside the representable UTC range."
        ))),
        LocalResult::Ambiguous(first, second) => {
            if let Some(known) = reference {
                let offset = known.with_timezone(&zone).offset().fix();
                for candidate in [first, second] {
                    if candidate.offset().fix() == offset {
                        return Ok(candidate.to_utc());
                    }
                }
            }
            Err(Error::Input(format!(
                "{local} occurs twice in {zone}; specify an unambiguous time or an explicit UTC instant."
            )))
        }
    }
}

/// Synchronize clock endpoints after a caller edits the corresponding civil
/// fields. Imported wall-clock entries keep their original interpretation.
pub fn normalize_entry(entry: &mut crate::model::NewEntry) -> Result<()> {
    entry.clock = normalized_clock(entry)?;
    Ok(())
}

pub(crate) fn normalized_clock(
    entry: &crate::model::NewEntry,
) -> Result<Option<crate::model::EntryClock>> {
    let Some(mut clock) = entry.clock else {
        return Ok(None);
    };
    if entry.start.nanosecond() != 0 || entry.end.is_some_and(|end| end.nanosecond() != 0) {
        return Err(Error::Input(
            "Timed entries use whole-second precision.".into(),
        ));
    }
    clock.start_utc = resolve_local(entry.start, clock.zone, Some(clock.start_utc))?;
    clock.end_utc = entry
        .end
        .map(|end| resolve_local(end, clock.zone, clock.end_utc.or(Some(clock.start_utc))))
        .transpose()?;
    if clock.end_utc.is_some_and(|end| end < clock.start_utc) {
        return Err(Error::Input(
            "End time must be on or after the start instant.".into(),
        ));
    }
    Ok(Some(clock))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extreme_utc_values_return_errors_for_unrepresentable_local_dates() {
        let east = chrono_tz::Asia::Tokyo;
        let west = chrono_tz::America::New_York;
        assert!(local_at(DateTime::<Utc>::MAX_UTC, east).is_err());
        assert!(Moment::at(DateTime::<Utc>::MIN_UTC, west).local().is_err());
        let err = resolve_local(NaiveDateTime::MAX, west, None).unwrap_err();
        assert!(
            err.to_string()
                .contains("outside the representable UTC range")
        );
        assert_eq!(
            local_at(DateTime::<Utc>::MAX_UTC, chrono_tz::UTC).unwrap(),
            NaiveDateTime::MAX
        );
    }

    #[test]
    fn folds_retain_reference_offset_and_gaps_are_rejected() {
        let zone = chrono_tz::Europe::Zagreb;
        let local: NaiveDateTime = "2026-10-25T02:12:00".parse().unwrap();
        assert!(resolve_local(local, zone, None).is_err());
        let first = "2026-10-25T00:12:00Z".parse::<DateTime<Utc>>().unwrap();
        let second = "2026-10-25T01:12:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(resolve_local(local, zone, Some(first)).unwrap(), first);
        assert_eq!(resolve_local(local, zone, Some(second)).unwrap(), second);
        let gap = "2026-03-29T02:15:00".parse().unwrap();
        assert!(resolve_local(gap, zone, Some(first)).is_err());
    }
}
