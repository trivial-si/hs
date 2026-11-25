//! Hamster workdays begin at a configured local time. Ordinary facts use a
//! civil clock, while a timed fact uses the actual instant in its saved zone.
use chrono::{Datelike, Days, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use chrono_tz::{GapInfo, Tz};

use crate::{
    Error, Result,
    model::Entry,
    time::{Moment, local_at},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Workday {
    start: NaiveTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Today,
    Week,
    Month,
    Dates {
        from: NaiveDate,
        to: Option<NaiveDate>,
    },
    All,
}

/// Half-open local boundaries. Each timed entry projects these into its saved zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkdayRange {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
}

impl Workday {
    pub fn new(start_minutes: u32) -> Result<Self> {
        let start = NaiveTime::from_hms_opt(start_minutes / 60, start_minutes % 60, 0)
            .ok_or_else(|| Error::Input("day start must fall between 00:00 and 23:59".into()))?;
        Ok(Self { start })
    }

    pub fn civil_date(self, local: NaiveDateTime) -> Result<NaiveDate> {
        self.date_on_axis(local, None)
    }

    pub fn date_at(self, instant: &Moment) -> Result<NaiveDate> {
        self.date_on_axis(instant.utc.naive_utc(), Some(instant.zone))
    }

    /// Place the bare clock within the reference's civil workday.
    pub fn civil_clock(self, clock: NaiveTime, reference: NaiveDateTime) -> Result<NaiveDateTime> {
        let date = reference.date();
        // Avoid stepping outside the date range if both clocks are on the same civil date.
        let date = match (reference.time() < self.start, clock < self.start) {
            (true, false) => date.pred_opt(),
            (false, true) => date.succ_opt(),
            _ => Some(date),
        };
        Ok(date.ok_or_else(workday_out_of_range)?.and_time(clock))
    }

    /// Latest actual occurrence of the clock which belongs to the current
    /// workday and has already passed. A repeated hour can straddle the boundary.
    pub fn past_clock(self, clock: NaiveTime, now: &Moment) -> Result<Moment> {
        let day = self.date_at(now)?;
        let mut latest = None;
        // A midnight rollback can put an occurrence stamped with the previous
        // civil date inside this elapsed workday.
        for date in [day.pred_opt(), Some(day), day.succ_opt()]
            .into_iter()
            .flatten()
        {
            let candidates = now.zone.from_local_datetime(&date.and_time(clock));
            for candidate in [candidates.earliest(), candidates.latest()]
                .into_iter()
                .flatten()
            {
                let at = candidate.to_utc();
                if at <= now.utc
                    && latest.is_none_or(|prior| prior < at)
                    && self.date_on_axis(at.naive_utc(), Some(now.zone))? == day
                {
                    latest = Some(at);
                }
            }
        }
        latest.map(|at| Moment::at(at, now.zone)).ok_or_else(|| {
            Error::Input(format!(
                "{clock} has no occurrence at or before now in the current workday in {}.",
                now.zone
            ))
        })
    }

    pub(crate) fn date_on_axis(self, at: NaiveDateTime, zone: Option<Tz>) -> Result<NaiveDate> {
        let day = match zone {
            Some(zone) => local_at(at.and_utc(), zone)?.date(),
            None => at.date(),
        };
        if at < self.start_on_axis(day, zone) {
            return day.pred_opt().ok_or_else(workday_out_of_range);
        }
        // The clock may have fallen back over midnight after tomorrow's first
        // boundary already passed. The elapsed workday does not move backwards.
        match (zone, day.succ_opt()) {
            (Some(zone), Some(next)) if at >= self.start_on_axis(next, Some(zone)) => Ok(next),
            _ => Ok(day),
        }
    }

    pub(crate) fn start_on_axis(self, day: NaiveDate, zone: Option<Tz>) -> NaiveDateTime {
        boundary_on_axis(day.and_time(self.start), zone)
    }

    /// Both dates are inclusive workday labels.
    pub fn between(self, from: NaiveDate, to: NaiveDate) -> Result<WorkdayRange> {
        if from > to {
            return Err(Error::Input("report end date precedes start date".into()));
        }
        let next = to
            .succ_opt()
            .ok_or_else(|| Error::Input("report end date is out of range".into()))?;
        Ok(WorkdayRange {
            start: from.and_time(self.start),
            end: next.and_time(self.start),
        })
    }

    /// Calendar selection follows the invocation clock. A saved observation
    /// cutoff can lie later after clock rollback and bounds still-running entries.
    pub fn range(
        self,
        period: Period,
        entries: &[Entry],
        invocation: &Moment,
        cutoff: &Moment,
    ) -> Result<WorkdayRange> {
        let today = self.date_at(invocation)?;
        let (from, to) = match period {
            Period::Today => (today, today),
            Period::Dates { from, to } => (from, to.unwrap_or(today)),
            Period::Week => {
                let offset = Days::new(u64::from(today.weekday().num_days_from_monday()));
                let monday = today
                    .checked_sub_days(offset)
                    .ok_or_else(workday_out_of_range)?;
                let sunday = monday
                    .checked_add_days(Days::new(6))
                    .ok_or_else(workday_out_of_range)?;
                (monday, sunday)
            }
            Period::Month => {
                let first = today.with_day(1).expect("every month has a first day");
                let last = first
                    .checked_add_months(Months::new(1))
                    .and_then(|next| next.pred_opt())
                    .ok_or_else(workday_out_of_range)?;
                (first, last)
            }
            Period::All => {
                let mut from = today;
                let mut to = today;
                for entry in entries {
                    let (start, end) = if let Some(clock) = entry.clock {
                        let end = clock.end_utc.unwrap_or(if entry.running {
                            cutoff.utc
                        } else {
                            clock.start_utc
                        });
                        (
                            self.date_at(&Moment::at(clock.start_utc, clock.zone))?,
                            self.date_at(&Moment::at(end.max(clock.start_utc), clock.zone))?,
                        )
                    } else {
                        let end = match entry.end {
                            Some(end) => end,
                            None => cutoff.local()?,
                        };
                        (
                            self.civil_date(entry.start)?,
                            self.civil_date(end.max(entry.start))?,
                        )
                    };
                    from = from.min(start);
                    to = to.max(end);
                }
                (from, to)
            }
        };
        self.between(from, to)
    }
}

impl WorkdayRange {
    pub fn new(from: NaiveDate, to: NaiveDate, day_start_minutes: u32) -> Result<Self> {
        Workday::new(day_start_minutes)?.between(from, to)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.start > self.end {
            return Err(Error::Input("report end precedes start".into()));
        }
        Ok(())
    }

    pub(crate) fn workday(self) -> Workday {
        Workday {
            start: self.start.time(),
        }
    }

    pub(crate) fn on_axis(self, zone: Option<Tz>) -> Self {
        Self {
            start: boundary_on_axis(self.start, zone),
            end: boundary_on_axis(self.end, zone),
        }
    }
}

fn workday_out_of_range() -> Error {
    Error::Input("workday out of range".into())
}

fn boundary_on_axis(local: NaiveDateTime, zone: Option<Tz>) -> NaiveDateTime {
    let Some(zone) = zone else { return local };
    // At a fold use the first occurrence; in a gap use the first valid instant.
    zone.from_local_datetime(&local)
        .earliest()
        .or_else(|| GapInfo::new(&local, &zone)?.end)
        .map(|instant| instant.naive_utc())
        .unwrap_or_else(|| {
            // Only a boundary beyond chrono's UTC range has no projection.
            // Saturation keeps intersection ordering at the representable limits.
            if local.year() < 0 {
                NaiveDateTime::MIN
            } else {
                NaiveDateTime::MAX
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(utc: &str, zone: Tz) -> Moment {
        Moment::at(utc.parse().unwrap(), zone)
    }

    fn date(value: &str) -> NaiveDate {
        value.parse().unwrap()
    }

    fn clock(value: &str) -> NaiveTime {
        value.parse().unwrap()
    }

    #[test]
    fn civil_clocks_share_a_workday_without_crossing_date_limits_unnecessarily() {
        let workday = Workday::new(330).unwrap();
        let early = date("2026-01-01").and_time(clock("03:00:00"));
        assert_eq!(workday.civil_date(early).unwrap(), date("2025-12-31"));
        assert_eq!(
            workday.civil_clock(clock("06:00:00"), early).unwrap(),
            date("2025-12-31").and_time(clock("06:00:00"))
        );
        assert!(workday.civil_date(NaiveDateTime::MIN).is_err());
        assert_eq!(
            workday
                .civil_clock(clock("00:01:00"), NaiveDateTime::MIN)
                .unwrap()
                .date(),
            NaiveDate::MIN
        );
        assert!(Workday::new(24 * 60).is_err());
    }

    #[test]
    fn repeated_boundary_distinguishes_elapsed_from_civil_membership() {
        let zone = chrono_tz::Europe::Zagreb;
        let workday = Workday::new(150).unwrap();
        let first = at("2026-10-25T00:10:00Z", zone);
        let second = at("2026-10-25T01:10:00Z", zone);
        assert_eq!(first.local().unwrap(), second.local().unwrap());
        assert_eq!(workday.date_at(&first).unwrap(), date("2026-10-24"));
        assert_eq!(workday.date_at(&second).unwrap(), date("2026-10-25"));
        assert_eq!(
            workday.civil_date(second.local().unwrap()).unwrap(),
            date("2026-10-24")
        );
        assert_eq!(
            workday.past_clock(clock("02:05:00"), &second).unwrap(),
            at("2026-10-25T01:05:00Z", zone)
        );
        assert_eq!(
            workday.past_clock(clock("02:35:00"), &second).unwrap(),
            at("2026-10-25T00:35:00Z", zone)
        );
        assert!(workday.past_clock(clock("03:00:00"), &second).is_err());
    }

    #[test]
    fn skipped_boundary_uses_first_real_instant_but_a_missing_stop_clock_is_rejected() {
        let zone = chrono_tz::Europe::Zagreb;
        let workday = Workday::new(150).unwrap();
        let before = at("2026-03-29T00:59:59Z", zone);
        let after = at("2026-03-29T01:00:00Z", zone);
        assert_eq!(workday.date_at(&before).unwrap(), date("2026-03-28"));
        assert_eq!(workday.date_at(&after).unwrap(), date("2026-03-29"));
        assert!(workday.past_clock(clock("02:45:00"), &after).is_err());
        assert_eq!(
            workday.past_clock(clock("03:00:00"), &after).unwrap(),
            after
        );
    }

    #[test]
    fn a_fold_across_midnight_can_revisit_yesterdays_clock_after_todays_boundary() {
        let zone = chrono_tz::America::St_Johns;
        let workday = Workday::new(0).unwrap();
        let before = at("2008-11-02T02:29:59Z", zone);
        let midnight = at("2008-11-02T02:30:00Z", zone);
        let now = at("2008-11-02T03:05:00Z", zone);
        assert_eq!(
            now.local().unwrap(),
            date("2008-11-01").and_time(clock("23:35:00"))
        );
        assert_eq!(
            workday.civil_date(now.local().unwrap()).unwrap(),
            date("2008-11-01")
        );
        assert_eq!(workday.date_at(&before).unwrap(), date("2008-11-01"));
        assert_eq!(workday.date_at(&midnight).unwrap(), date("2008-11-02"));
        assert_eq!(workday.date_at(&now).unwrap(), date("2008-11-02"));
        assert_eq!(
            workday.past_clock(clock("23:15:00"), &now).unwrap(),
            at("2008-11-02T02:45:00Z", zone)
        );
        assert_eq!(
            workday.past_clock(clock("00:00:00"), &now).unwrap(),
            midnight
        );
        assert!(workday.past_clock(clock("23:40:00"), &now).is_err());
        let today = workday.range(Period::Today, &[], &now, &now).unwrap();
        assert_eq!(today.on_axis(Some(zone)).start, midnight.utc.naive_utc());
    }

    #[test]
    fn period_selection_uses_invocation_workday_for_week_month_and_default_end() {
        let workday = Workday::new(150).unwrap();
        let now = at("2024-03-01T01:00:00Z", chrono_tz::UTC);
        let cutoff = at("2024-03-05T12:00:00Z", chrono_tz::UTC);
        for (period, start, end) in [
            (Period::Today, "2024-02-29", "2024-03-01"),
            (Period::Week, "2024-02-26", "2024-03-04"),
            (Period::Month, "2024-02-01", "2024-03-01"),
            (
                Period::Dates {
                    from: date("2024-02-20"),
                    to: None,
                },
                "2024-02-20",
                "2024-03-01",
            ),
            (Period::All, "2024-02-29", "2024-03-01"),
        ] {
            let range = workday.range(period, &[], &now, &cutoff).unwrap();
            assert_eq!(range.start, date(start).and_time(clock("02:30:00")));
            assert_eq!(range.end, date(end).and_time(clock("02:30:00")));
        }
        assert!(
            workday
                .between(date("2024-03-01"), date("2024-02-29"))
                .is_err()
        );
        assert!(workday.between(NaiveDate::MAX, NaiveDate::MAX).is_err());
    }
}
