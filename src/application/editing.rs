//! Input edits resolved against the latest stored entry inside its transaction.
use crate::{
    Error, Result,
    model::{Entry, NewEntry},
    parse::{parse_end_time, parse_time, standalone_clock},
    time::{Moment, local_at},
    tracking,
};
use chrono::{DateTime, Duration, NaiveDateTime};

#[derive(Debug, Default)]
pub struct EntryEdit {
    pub start: Option<String>,
    pub end: Option<String>,
    pub activity: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub tags: Option<String>,
}

impl EntryEdit {
    pub(crate) fn apply(self, old: &Entry, moment: &Moment, boundary: u32) -> Result<NewEntry> {
        if self.start.is_none()
            && self.end.is_none()
            && self.activity.is_none()
            && self.category.is_none()
            && self.description.is_none()
            && self.tags.is_none()
        {
            return Err(Error::Input(
                "Specify at least one field to edit. See `hs edit --help`.".into(),
            ));
        }
        let now = moment.local()?;
        let mut entry = old.as_new();
        if let Some(value) = &self.start
            && !clocked_edit_time(&mut entry, value, false, moment)?
        {
            entry.start = edit_time(value, old.start, now, boundary)?;
        }
        if let Some(value) = &self.end
            && !clocked_edit_time(&mut entry, value, true, moment)?
        {
            entry.end = Some(if relative_to_now(value) {
                parse_time(value, now, boundary)?
            } else {
                parse_end_time(value, entry.start, boundary)?
            });
        }
        if let Some(value) = self.activity {
            entry.activity = value.trim().to_string();
        }
        if let Some(value) = self.category {
            entry.category = (!value.trim().is_empty()).then(|| value.trim().to_string());
        }
        if let Some(value) = self.description {
            entry.description = value;
        }
        if let Some(value) = self.tags {
            entry.tags = parse_tags(&value);
        }
        Ok(entry)
    }
}

pub fn parse_tags(value: &str) -> Vec<String> {
    let mut tags: Vec<_> = value
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Editable task fields only; IDs and lifecycle history cannot be supplied here.
#[derive(Debug, Default)]
pub struct TaskEdit {
    pub title: Option<String>,
    pub activity: Option<crate::model::ActivityTemplate>,
    /// Change only the reporting name, preserving the task's other metadata.
    pub activity_name: Option<String>,
    /// Absent preserves the category; Some(None) clears it.
    pub category: Option<Option<String>>,
    pub notes: Option<String>,
    pub tags: Option<Vec<String>>,
    pub quadrant: Option<Option<crate::domain::tasks::Quadrant>>,
    pub due: Option<Option<chrono::NaiveDate>>,
}
impl TaskEdit {
    pub(crate) fn apply(self, task: &mut crate::domain::tasks::Task) -> Result<()> {
        if self.title.is_none()
            && self.activity.is_none()
            && self.activity_name.is_none()
            && self.category.is_none()
            && self.notes.is_none()
            && self.tags.is_none()
            && self.quadrant.is_none()
            && self.due.is_none()
        {
            return Err(Error::Input(
                "Specify at least one task field. See `hs task set --help`.".into(),
            ));
        }
        if let Some(title) = self.title {
            task.title = title;
        }
        if let Some(activity) = self.activity {
            task.activity = activity.activity;
            task.category = activity.category;
            if !activity.description.is_empty() {
                task.description = activity.description;
            }
            if !activity.tags.is_empty() {
                task.tags = activity.tags;
            }
        }
        if let Some(name) = self.activity_name {
            task.activity = name.trim().to_string();
        }
        if let Some(category) = self.category {
            task.category = category;
        }
        if let Some(notes) = self.notes {
            task.description = notes;
        }
        if let Some(tags) = self.tags {
            task.tags = tags;
        }
        if let Some(quadrant) = self.quadrant {
            task.quadrant = quadrant;
        }
        if let Some(due) = self.due {
            task.due_date = due;
        }
        Ok(())
    }
}

fn edit_time(
    value: &str,
    reference: NaiveDateTime,
    now: NaiveDateTime,
    boundary: u32,
) -> Result<NaiveDateTime> {
    let reference = if relative_to_now(value) {
        now
    } else {
        reference
    };
    parse_time(value, reference, boundary)
}

pub fn stop_time(
    value: Option<&str>,
    now: NaiveDateTime,
    boundary: u32,
) -> Result<tracking::StopTime> {
    let Some(value) = value.map(str::trim) else {
        return Ok(tracking::StopTime::Now);
    };
    if value.eq_ignore_ascii_case("now") {
        Ok(tracking::StopTime::Now)
    } else if value.starts_with(['-', '+']) {
        let minutes = value.parse::<i64>().map_err(|_| {
            Error::Input("Expected a signed minute offset within the supported range".into())
        })?;
        Ok(tracking::StopTime::OffsetMinutes(minutes))
    } else {
        match standalone_clock(value)? {
            Some(clock) => Ok(tracking::StopTime::Clock(clock)),
            None => Ok(tracking::StopTime::Local(parse_time(value, now, boundary)?)),
        }
    }
}

fn clocked_edit_time(entry: &mut NewEntry, value: &str, end: bool, now: &Moment) -> Result<bool> {
    let value = value.trim();
    let explicit = DateTime::parse_from_rfc3339(value).ok();
    let Some(clock) = entry.clock.as_mut() else {
        if explicit.is_some() {
            return Err(Error::Input("Explicit UTC offsets are supported for Pomodoro entries. Use a local date and time for a legacy entry.".into()));
        }
        return Ok(false);
    };
    let instant = if let Some(instant) = explicit {
        Moment::at(instant.to_utc(), clock.zone).utc
    } else if value.eq_ignore_ascii_case("now") {
        now.utc
    } else if value.starts_with(['-', '+']) {
        value
            .parse::<i64>()
            .ok()
            .and_then(Duration::try_minutes)
            .and_then(|delta| now.utc.checked_add_signed(delta))
            .ok_or_else(|| {
                Error::Input("Expected a signed minute offset within the supported range".into())
            })?
    } else {
        return Ok(false);
    };
    let local = local_at(instant, clock.zone)?;
    if end {
        entry.end = Some(local);
        clock.end_utc = Some(instant);
    } else {
        entry.start = local;
        clock.start_utc = instant;
    }
    Ok(true)
}

fn relative_to_now(value: &str) -> bool {
    let value = value.trim();
    value.eq_ignore_ascii_case("now") || value.starts_with(['-', '+'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::EntryClock, parse::parse_live_entry};
    #[test]
    fn stop_options_keep_now_and_offsets_as_typed_intents() {
        let now = "2026-03-29T03:01:37".parse().unwrap();
        for value in [None, Some("now"), Some("NOW"), Some(" now ")] {
            assert!(matches!(
                stop_time(value, now, 0).unwrap(),
                tracking::StopTime::Now
            ));
        }
        assert!(matches!(
            stop_time(Some("-2"), now, 0).unwrap(),
            tracking::StopTime::OffsetMinutes(-2)
        ));
        assert!(stop_time(Some("-9999999999999999999999"), now, 0).is_err());
        assert!(matches!(
            stop_time(Some(" 0930 "), now, 0).unwrap(),
            tracking::StopTime::Clock(_)
        ));
        assert!(relative_to_now(" NOW "));
        assert!(relative_to_now(" -5 "));
    }

    #[test]
    fn clocked_edits_use_instants_after_travel_and_through_clock_folds() {
        let now = Moment::at("2026-10-25T01:10:37Z".parse().unwrap(), chrono_tz::UTC);
        let zone = chrono_tz::Europe::Zagreb;
        let mut entry = parse_live_entry("Review", now.local().unwrap(), 0).unwrap();
        entry.clock = Some(EntryClock {
            start_utc: now.utc,
            end_utc: None,
            zone,
        });
        entry.start = local_at(now.utc, zone).unwrap();

        assert!(clocked_edit_time(&mut entry, "-20", false, &now).unwrap());
        assert_eq!(
            entry.clock.unwrap().start_utc.to_rfc3339(),
            "2026-10-25T00:50:37+00:00"
        );
        assert_eq!(entry.start.to_string(), "2026-10-25 02:50:37");
        assert!(clocked_edit_time(&mut entry, "now", true, &now).unwrap());
        assert_eq!(entry.end.unwrap().to_string(), "2026-10-25 02:10:37");
        crate::time::normalize_entry(&mut entry).unwrap();
        let clock = entry.clock.unwrap();
        assert_eq!(
            (clock.end_utc.unwrap() - clock.start_utc).num_seconds(),
            20 * 60
        );

        assert!(clocked_edit_time(&mut entry, "2026-10-25T02:05:37+01:00", true, &now).unwrap());
        assert_eq!(
            entry.clock.unwrap().end_utc.unwrap().to_rfc3339(),
            "2026-10-25T01:05:37+00:00"
        );
        assert!(!clocked_edit_time(&mut entry, "02:07", true, &now).unwrap());
        assert!(clocked_edit_time(&mut entry, "+99999999999999999", true, &now).is_err());
    }

    #[test]
    fn legacy_edits_keep_existing_local_time_rules() {
        let now = Moment::at("2026-11-23T10:30:23Z".parse().unwrap(), chrono_tz::UTC);
        let mut entry = parse_live_entry("Review", now.local().unwrap(), 0).unwrap();
        assert!(!clocked_edit_time(&mut entry, "-10", false, &now).unwrap());
        assert!(!clocked_edit_time(&mut entry, "now", true, &now).unwrap());
        assert!(clocked_edit_time(&mut entry, "2026-11-23T10:00:00Z", false, &now).is_err());
    }
}
