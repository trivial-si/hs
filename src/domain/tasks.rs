//! Planning data and task invariants, independent of persistence.
use chrono::{DateTime, Datelike, NaiveDate, Utc};

use crate::{Error, Result, model::ActivityTemplate};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Quadrant {
    Do,
    Plan,
    Delegate,
    Drop,
}

impl Quadrant {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "do" => Ok(Self::Do),
            "plan" => Ok(Self::Plan),
            "delegate" => Ok(Self::Delegate),
            "drop" => Ok(Self::Drop),
            _ => Err(Error::Input(format!(
                "Unknown quadrant {value:?}. Use do, plan, delegate, or drop."
            ))),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Do => "do",
            Self::Plan => "plan",
            Self::Delegate => "delegate",
            Self::Drop => "drop",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Do => "Do",
            Self::Plan => "Plan",
            Self::Delegate => "Delegate",
            Self::Drop => "Drop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Open,
    Done,
    Archived,
}

impl TaskStatus {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "open" => Ok(Self::Open),
            "done" => Ok(Self::Done),
            "archived" => Ok(Self::Archived),
            _ => Err(Error::Input(format!(
                "Unknown task status {value:?}. Use open, done, or archived."
            ))),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub activity: String,
    pub category: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
    pub quadrant: Option<Quadrant>,
    pub due_date: Option<NaiveDate>,
    pub status: TaskStatus,
    pub created_utc: DateTime<Utc>,
    pub completed_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct TaskInput {
    pub title: String,
    pub activity: ActivityTemplate,
    pub quadrant: Option<Quadrant>,
    pub due_date: Option<NaiveDate>,
}

fn validate(title: &str, activity: &str, tags: &[String], due: Option<NaiveDate>) -> Result<()> {
    // ISO civil dates with four-digit years sort chronologically as SQLite
    // text. Chrono also accepts signed, extended years, which would not.
    if due.is_some_and(|date| !(0..=9999).contains(&date.year())) {
        return Err(Error::Input(
            "Use a due date with a four-digit year (YYYY-MM-DD).".into(),
        ));
    }
    if title.trim().is_empty() {
        return Err(Error::Input("A task title is required.".into()));
    }
    if activity.trim().is_empty() {
        return Err(Error::Input("An activity name is required.".into()));
    }
    if tags.iter().any(|tag| tag.trim().is_empty()) {
        return Err(Error::Input("Tags must have a name.".into()));
    }
    Ok(())
}

impl TaskInput {
    pub(crate) fn validate(&self) -> Result<()> {
        validate(
            &self.title,
            &self.activity.activity,
            &self.activity.tags,
            self.due_date,
        )
    }
}

impl Task {
    pub(crate) fn validate(&self) -> Result<()> {
        validate(&self.title, &self.activity, &self.tags, self.due_date)
    }

    pub(crate) fn change_status(&mut self, status: TaskStatus, now: DateTime<Utc>) -> Result<bool> {
        if self.status == status {
            return Ok(false);
        }
        if self.status == TaskStatus::Archived && status == TaskStatus::Done {
            return Err(Error::Conflict(format!(
                "Task {} is archived. Reopen it before marking it done.",
                self.id
            )));
        }
        self.completed_utc = match status {
            TaskStatus::Done => Some(now.max(self.created_utc)),
            TaskStatus::Open => None,
            TaskStatus::Archived => self.completed_utc,
        };
        self.status = status;
        Ok(true)
    }
}
