//! Versioned machine-readable CLI views. Durations use seconds and instants use RFC 3339.
use std::io::Write;

use chrono::NaiveDate;
pub use hs::application::queries::TaskAction;
use hs::{
    Error, Result,
    application::queries::{Status, Tasks},
    domain::{
        model::{Entry, format_time},
        pomodoro::{Session, State, Statistics},
        report::{self, Report, ReportRange},
        tasks::{Quadrant, Task, TaskStatus},
        time::Moment,
    },
};
use serde_json::{Value, json};

pub enum View<'a> {
    Status(&'a Status),
    Tasks(TaskRead, &'a Tasks, NaiveDate),
    Report {
        data: &'a Report,
        range: &'a ReportRange,
        group_by: &'a str,
        as_of: &'a Moment,
    },
    Pomodoro(Option<&'a Session>, &'a Moment),
    Statistics(&'a Statistics),
}

#[derive(Clone, Copy)]
pub enum TaskRead {
    List,
    Show,
    Next,
    Matrix,
}

fn session_row(session: &Session, now: &Moment) -> Value {
    json!({
        "id": session.id,
        "task_id": session.task_id,
        "entry_id": session.entry_id(),
        "activity": session.activity, "category": session.category,
        "description": session.description, "tags": session.tags,
        "state": session.state().label(), "phase": session.phase().key(),
        "remaining_seconds": session.remaining(now), "deadline_utc": session.deadline().map(|t| t.to_rfc3339()),
        "timezone": session.zone().name(), "completed_focus": session.completed_focus(),
        "interrupted_focus": session.interrupted_focus()
    })
}

pub fn entry_row(entry: &Entry, seconds: Option<i64>) -> Value {
    json!({
        "id": entry.id,
        "activity": entry.activity,
        "category": entry.category,
        "description": entry.description, "tags": entry.tags, "start": format_time(entry.start),
        "end": entry.end.map(format_time), "seconds": seconds, "minutes": seconds.map(|s| s / 60),
        "start_utc": entry.clock.map(|c| c.start_utc.to_rfc3339()),
        "end_utc": entry.clock.and_then(|c| c.end_utc).map(|t| t.to_rfc3339()),
        "timezone": entry.clock.map(|c| c.zone.name()),
        "task_id": entry.task.map(|t| t.id), "quadrant": entry.task.and_then(|t| t.quadrant).map(Quadrant::key),
        "state": if entry.running { "running" } else if entry.end.is_none() { "unresolved" } else { "completed" }
    })
}

fn task_timer(view: &Status, id: i64) -> Option<Value> {
    if let Some(session) = view
        .session
        .as_ref()
        .filter(|s| s.state() != State::Stopped && s.task_id == Some(id))
    {
        return Some(
            json!({"kind":"pomodoro","id":session.id,"entry_id":session.entry_id(),
            "phase":session.phase().key(),"state":session.state().label()}),
        );
    }
    view.current
        .as_ref()
        .filter(|entry| entry.task.is_some_and(|task| task.id == id))
        .map(|entry| json!({"kind":"tracking","entry_id":entry.id,"state":"running"}))
}

fn task_row(task: &Task, view: &Tasks, today: NaiveDate) -> Value {
    json!({
        "id": task.id,
        "title": task.title,
        "activity": task.activity,
        "category": task.category,
        "notes": task.description, "tags": task.tags, "status": task.status.key(),
        "quadrant": task.quadrant.map(Quadrant::key), "due": task.due_date.map(|d| d.to_string()),
        "overdue": task.status == TaskStatus::Open && task.due_date.is_some_and(|d| d < today),
        "created_utc": task.created_utc.to_rfc3339(), "completed_utc": task.completed_utc.map(|t| t.to_rfc3339()),
        "recorded_seconds": view.recorded_seconds.get(&task.id).copied().unwrap_or(0),
        "timer": task_timer(&view.status, task.id)
    })
}

impl View<'_> {
    pub fn write(self, out: &mut impl Write) -> Result<()> {
        let (kind, mut payload) = match self {
            Self::Status(view) => {
                let current = view
                    .current
                    .as_ref()
                    .map(|entry| {
                        report::entry_seconds_at(entry, &view.as_of)
                            .map(|seconds| entry_row(entry, seconds))
                    })
                    .transpose()?;
                let session = view
                    .session
                    .as_ref()
                    .filter(|s| s.state() != State::Stopped);
                (
                    "status",
                    json!({
                        "as_of_utc": view.as_of.utc.to_rfc3339(),
                        "current": current,
                        "pomodoro": session.map(|s| session_row(s, &view.as_of))
                    }),
                )
            }
            Self::Pomodoro(session, as_of) => (
                "pomo.status",
                json!({
                    "as_of_utc": as_of.utc.to_rfc3339(),
                    "session": session.map(|session| session_row(session, as_of))
                }),
            ),
            Self::Statistics(stats) => (
                "pomo.stats",
                json!({
                    "completed_focus": stats.completed,
                    "interrupted_focus": stats.interrupted,
                    "focus_seconds": stats.focus_seconds,
                    "break_seconds": stats.break_seconds
                }),
            ),
            Self::Report {
                data,
                range,
                group_by,
                as_of,
            } => {
                let groups: Vec<_> = data
                    .groups_seconds
                    .iter()
                    .map(|(name, seconds)| json!({"name":name,"seconds":seconds}))
                    .collect();
                (
                    "report",
                    json!({
                        "group_by": group_by,
                        "as_of_utc": as_of.utc.to_rfc3339(),
                        "range": { "start": format_time(range.start), "end": format_time(range.end) },
                        "groups": groups,
                        "total_seconds": data.total_seconds,
                        "total_minutes": data.total_minutes,
                        "unresolved_ids": data.unresolved_ids
                    }),
                )
            }
            Self::Tasks(read, view, today) => {
                let row = |task: &Task| task_row(task, view, today);
                let (kind, mut payload) = match read {
                    TaskRead::List => (
                        "task.list",
                        json!({"tasks":view.tasks.iter().map(row).collect::<Vec<_>>()}),
                    ),
                    TaskRead::Show => ("task.show", json!({"task":view.tasks.first().map(row)})),
                    TaskRead::Next => {
                        let suggested = view.tasks.first();
                        let action = suggested.map_or("triage", |task| {
                            TaskAction::for_task(&view.status, task).key()
                        });
                        (
                            "task.next",
                            json!({ "task": suggested.map(row), "action": action }),
                        )
                    }
                    TaskRead::Matrix => {
                        let priorities = [
                            Some(Quadrant::Do),
                            Some(Quadrant::Plan),
                            Some(Quadrant::Delegate),
                            Some(Quadrant::Drop),
                            None,
                        ];
                        let quadrants: Vec<_> = priorities.into_iter().map(|quadrant| {
                            let tasks: Vec<_> = view.tasks.iter()
                                .filter(|task| task.quadrant == quadrant).map(row).collect();
                            json!({ "key": quadrant.map_or("inbox", Quadrant::key), "tasks": tasks })
                        }).collect();
                        ("matrix", json!({ "quadrants": quadrants }))
                    }
                };
                payload["as_of_utc"] = json!(view.status.as_of.utc.to_rfc3339());
                payload["today"] = json!(today.to_string());
                (kind, payload)
            }
        };
        payload["kind"] = json!(kind);
        payload["schema_version"] = json!(1);
        writeln!(out, "{payload}")?;
        Ok(())
    }
}

pub fn error(kind: &str, code: &str, message: &str) -> Value {
    json!({"schema_version":1,"kind":"error","error":{"kind":kind,"code":code,"message":message}})
}

pub fn report_error(error: &Error, machine: bool) -> std::process::ExitCode {
    if error.is_broken_pipe() {
        return std::process::ExitCode::SUCCESS;
    }
    let class = error.classification();
    if machine {
        eprintln!(
            "{}",
            self::error(class.kind, class.code, &error.to_string())
        );
    } else {
        eprintln!("hs: {error}");
        if class.code == "database_busy" {
            eprintln!("Another command is using this database. Retry after it completes.");
        }
    }
    std::process::ExitCode::from(if matches!(error, Error::Input(_)) {
        2
    } else {
        1
    })
}
