use crate::domain::tasks::Quadrant;
use chrono::{DateTime, Datelike, NaiveDateTime, Timelike, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryTask {
    pub id: i64,
    pub quadrant: Option<Quadrant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryClock {
    pub start_utc: DateTime<Utc>,
    pub end_utc: Option<DateTime<Utc>>,
    pub zone: Tz,
}

/// Reusable activity details for a task, independent of any interval or timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityTemplate {
    pub activity: String,
    pub category: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
}

impl ActivityTemplate {
    /// Instantiate details as an ordinary open interval; persistence validates its time.
    pub fn starting_at(self, start: NaiveDateTime) -> NewEntry {
        NewEntry {
            activity: self.activity,
            category: self.category,
            description: self.description,
            tags: self.tags,
            start,
            end: None,
            clock: None,
            task: None,
        }
    }
}

impl From<NewEntry> for ActivityTemplate {
    fn from(entry: NewEntry) -> Self {
        Self {
            activity: entry.activity,
            category: entry.category,
            description: entry.description,
            tags: entry.tags,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEntry {
    pub activity: String,
    pub category: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
    pub start: NaiveDateTime,
    pub end: Option<NaiveDateTime>,
    pub clock: Option<EntryClock>,
    pub task: Option<EntryTask>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: i64,
    pub activity: String,
    pub category: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
    pub start: NaiveDateTime,
    pub end: Option<NaiveDateTime>,
    pub clock: Option<EntryClock>,
    pub task: Option<EntryTask>,
    pub running: bool,
}

impl Entry {
    pub fn as_new(&self) -> NewEntry {
        NewEntry {
            activity: self.activity.clone(),
            category: self.category.clone(),
            description: self.description.clone(),
            tags: self.tags.clone(),
            start: self.start,
            end: self.end,
            clock: self.clock,
            task: self.task,
        }
    }
}

pub fn minute(time: NaiveDateTime) -> NaiveDateTime {
    time.with_second(0).unwrap().with_nanosecond(0).unwrap()
}

/// New civil timestamps use four-digit ISO years so SQLite's timestamp index
/// and range constraints have the same ordering as calendar arithmetic.
pub fn validate_civil_time(value: NaiveDateTime) -> crate::Result<()> {
    if !(0..=9999).contains(&value.year()) {
        return Err(crate::Error::Input(
            "Stored dates must use four-digit years (0000 through 9999).".into(),
        ));
    }
    Ok(())
}

pub fn format_time(time: NaiveDateTime) -> String {
    time.format("%Y-%m-%d %H:%M:%S%.f").to_string()
}
