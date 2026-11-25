//! Compare the caller's live tokens under the same lock as the mutation.
use super::{Fault, WireResult, command::Command};
use crate::{
    Error,
    storage::{agent as persistence, entries, pomodoro as timers},
};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::{Value, json};

fn time_bound(expected: &Value, field: &str) -> WireResult<Option<DateTime<Utc>>> {
    expected[field]
        .as_str()
        .map(|value| {
            let instant = DateTime::parse_from_rfc3339(value).map_err(|_| {
                Fault::input(format!(
                    "{field} must be an RFC3339 timestamp with an offset"
                ))
            })?;
            if instant.timestamp_subsec_nanos() != 0 {
                return Err(Fault::input(format!(
                    "{field} must have whole-second precision"
                )));
            }
            Ok(instant.to_utc())
        })
        .transpose()
}

/// Return the live preconditions that bind a preview to the reviewed side of a deadline.
pub(super) fn check(
    conn: &Connection,
    command: &Command,
    expected: &Value,
    now: &crate::domain::time::Moment,
) -> WireResult<Value> {
    let id = persistence::database_identity(conn)?;
    let timer = persistence::timer_revision(conn)?;
    let fail = |field: &str, actual: Value| {
        Fault::conflict(
            "precondition_failed",
            format!(
                "State changed or wrong target: {field}; refresh context and reconsider the operation"
            ),
            json!({"field":field,"expected":expected[field],"actual":actual}),
        )
    };
    if expected["database_id"] != id {
        return Err(fail("database_id", json!(id)));
    }
    let active = timers::load(conn, true)?;
    let observed = active
        .as_ref()
        .map_or(now.utc, |session| session.observed(now));
    let earliest = time_bound(expected, "not_before_utc")?;
    let latest = time_bound(expected, "before_utc")?;
    if earliest
        .zip(latest)
        .is_some_and(|(start, end)| start >= end)
    {
        return Err(Fault::input("not_before_utc must precede before_utc"));
    }
    for (field, violated) in [
        (
            "not_before_utc",
            earliest.is_some_and(|bound| observed < bound),
        ),
        ("before_utc", latest.is_some_and(|bound| observed >= bound)),
    ] {
        if violated {
            return Err(fail(field, json!(observed.to_rfc3339())));
        }
    }
    if expected.get("timer_revision").is_some_and(|v| v != timer) {
        return Err(fail("timer_revision", json!(timer)));
    }
    if let Some(expected_idle) = expected["timer_idle"].as_bool() {
        let idle = entries::current_id(conn)?.is_none() && active.is_none();
        if idle != expected_idle {
            return Err(fail("timer_idle", json!(idle)));
        }
    }
    if let Some(revision) = expected.get("history_revision") {
        let actual = persistence::history_revision(conn)?;
        if revision != actual {
            return Err(fail("history_revision", json!(actual)));
        }
    }
    let verify_revision = |field: &str, actual: crate::Result<i64>| -> WireResult<()> {
        let actual = actual.map_err(|error| match error {
            Error::Input(_) => fail(field, Value::Null),
            other => Fault::from(other),
        })?;
        if expected[field] != actual {
            return Err(fail(field, json!(actual)));
        }
        Ok(())
    };
    if expected.get("entry_revision").is_some() {
        let id = command
            .entry_id()
            .ok_or_else(|| Fault::input("entry_revision requires a related entry"))?;
        verify_revision(
            "entry_revision",
            persistence::entry_revision(conn, id.get()),
        )?;
    }
    let task_id = match command {
        Command::Resume(id) => entries::find_entry(conn, id.get())?.task.map(|t| t.id),
        Command::PomoResume | Command::PomoNext => active.as_ref().and_then(|s| s.task_id),
        _ => command.task_id().map(|id| id.get()),
    };
    if matches!(
        command,
        Command::Resume(_) | Command::PomoResume | Command::PomoNext
    ) && task_id.is_some()
        && expected.get("task_revision").is_none()
    {
        return Err(Fault::input(
            "preconditions.task_revision is required for this linked task; copy linked_task_revision from the entry or context.pomodoro",
        ));
    }
    if expected.get("task_revision").is_some() {
        let id = task_id.ok_or_else(|| Fault::input("task_revision requires a related task"))?;
        verify_revision("task_revision", persistence::task_revision(conn, id))?;
    }
    // Clock reconciliation is deliberately after token validation: an expired
    // timer shown by a read-only context is still a usable, safe control token.
    let mut conditions = expected.clone();
    conditions["timer_revision"] = json!(timer);
    if let Some(deadline) = active.and_then(|session| session.deadline()) {
        if observed < deadline {
            conditions["before_utc"] = json!(latest.unwrap_or(deadline).min(deadline).to_rfc3339());
        } else {
            // Preview reconciles a private copy. A rollback of the real clock
            // could otherwise make the source phase appear live again.
            conditions["not_before_utc"] =
                json!(earliest.unwrap_or(deadline).max(deadline).to_rfc3339());
        }
    }
    Ok(conditions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Store,
        domain::{model::ActivityTemplate, time::Moment},
        pomodoro,
    };
    use chrono::Duration;

    #[test]
    fn preview_of_an_expired_phase_cannot_apply_on_the_earlier_side_after_a_clock_rollback() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("clock.sqlite")).unwrap();
        let start = Moment::at("2026-03-20T14:00:00Z".parse().unwrap(), chrono_tz::UTC);
        let work = ActivityTemplate {
            activity: "Review".into(),
            category: None,
            description: String::new(),
            tags: vec![],
        };
        let running = pomodoro::start(
            &mut store,
            work.starting_at(start.local().unwrap()),
            pomodoro::Config {
                focus_seconds: 60,
                ..pomodoro::Config::default()
            },
            &start,
        )
        .unwrap();
        let deadline = running.deadline().unwrap();
        let tx = store.conn.transaction().unwrap();
        let initial = json!({"database_id":persistence::database_identity(&tx).unwrap(),"timer_revision":persistence::timer_revision(&tx).unwrap()});
        let late = Moment::at(deadline + Duration::seconds(15), start.zone);
        let reviewed = check(&tx, &Command::PomoNext, &initial, &late).unwrap();
        assert_eq!(reviewed["not_before_utc"], deadline.to_rfc3339());
        assert!(reviewed.get("before_utc").is_none());
        let rollback = Moment::at(deadline - Duration::seconds(1), start.zone);
        let refused = check(&tx, &Command::PomoNext, &reviewed, &rollback).unwrap_err();
        assert_eq!(refused.code, "precondition_failed");
        assert_eq!(refused.details["field"], "not_before_utc");
        let at_deadline = Moment::at(deadline, start.zone);
        check(&tx, &Command::PomoNext, &reviewed, &at_deadline).unwrap();
        let next = pomodoro::next_in_transaction(&tx, &at_deadline).unwrap();
        assert_eq!(next.phase(), pomodoro::Phase::ShortBreak);
    }
}
