//! Execute typed commands under the caller's transaction.
use super::{Fault, WireResult, command::Command, views};
use crate::{
    Error, Result,
    application::{pomodoro, tasks, workflows},
    domain::{
        model::{ActivityTemplate, EntryClock, NewEntry},
        time::{Moment, local_at},
    },
};
use rusqlite::Transaction;
use serde_json::{Value, json};

fn new_entry(
    activity: &ActivityTemplate,
    start: chrono::DateTime<chrono::Utc>,
    end: Option<chrono::DateTime<chrono::Utc>>,
    zone: chrono_tz::Tz,
) -> Result<NewEntry> {
    let mut entry = activity.clone().starting_at(local_at(start, zone)?);
    entry.end = end.map(|t| local_at(t, zone)).transpose()?;
    entry.clock = Some(EntryClock {
        start_utc: start,
        end_utc: end,
        zone,
    });
    Ok(entry)
}
fn change(tx: &Transaction<'_>, result: crate::application::entries::Change) -> WireResult<Value> {
    Ok(json!({"entry":views::entry(tx,&result.entry)?,"adjusted_entry_ids":result.adjusted}))
}

pub(super) fn execute(tx: &Transaction<'_>, command: &Command, now: &Moment) -> WireResult<Value> {
    match command {
        Command::TaskAdd(input) => {
            views::task(tx, &tasks::add_in_transaction(tx, input.clone(), now)?)
        }
        Command::TaskEdit { id, patch } => {
            let old = crate::storage::tasks::load_from(tx, id.get())?;
            views::task(
                tx,
                &tasks::edit_task_in_transaction(tx, *id, patch.resolve(&old))?,
            )
        }
        Command::TaskStatus { id, status } => views::task(
            tx,
            &workflows::change_task_status_in_transaction(tx, *id, *status, now)?,
        ),
        Command::Start(activity) => change(
            tx,
            workflows::start_in_transaction(
                tx,
                new_entry(activity, now.utc, None, now.zone)?,
                now,
            )?,
        ),
        Command::Add {
            activity,
            start,
            end,
            zone,
        } => change(
            tx,
            workflows::add_in_transaction(
                tx,
                new_entry(activity, *start, Some(*end), zone.unwrap_or(now.zone))?,
                now,
            )?,
        ),
        Command::StartTask(id) => change(tx, workflows::start_task_in_transaction(tx, *id, now)?),
        Command::Resume(id) => change(tx, workflows::resume_in_transaction(tx, Some(*id), now)?),
        Command::Stop => Ok(
            json!({"entry":workflows::stop_in_transaction(tx,workflows::StopTime::Now,now)?.as_ref().map(|e|views::entry(tx,e)).transpose()?}),
        ),
        Command::Delete(id) => {
            workflows::delete_in_transaction(tx, *id, now)?;
            Ok(json!({"deleted_entry_id":id.get()}))
        }
        Command::Edit { id, patch } => {
            let result = workflows::edit_with_in_transaction(tx, *id, now, |old| {
                let mut updated = old.as_new();
                if let Some(v) = &patch.activity {
                    updated.activity = v.clone();
                }
                if let Some(v) = &patch.category {
                    updated.category = v.clone();
                }
                if let Some(v) = &patch.description {
                    updated.description = v.clone();
                }
                if let Some(v) = &patch.tags {
                    updated.tags = v.clone();
                }
                if patch.start.is_some() || patch.end.is_some() {
                    let clock=updated.clock.as_mut().ok_or_else(||Error::Input("Legacy civil-time entries must be retimed using hs edit; agent time edits require a saved UTC clock".into()))?;
                    if let Some(start) = patch.start {
                        clock.start_utc = start;
                        updated.start = local_at(start, clock.zone)?;
                    }
                    if let Some(end) = patch.end {
                        clock.end_utc = Some(end);
                        updated.end = Some(local_at(end, clock.zone)?);
                    }
                }
                Ok(updated)
            })?;
            change(tx, result)
        }
        Command::PomoStart { activity, config } => Ok(views::session(
            &pomodoro::start_in_transaction(
                tx,
                new_entry(activity, now.utc, None, now.zone)?,
                *config,
                now,
            )?,
            now,
        )),
        Command::PomoStartTask { id, config } => Ok(views::session(
            &pomodoro::start_task_in_transaction(tx, *id, *config, now)?,
            now,
        )),
        Command::PomoPause => Ok(views::session(
            &pomodoro::pause_in_transaction(tx, now)?,
            now,
        )),
        Command::PomoResume => Ok(views::session(
            &pomodoro::resume_in_transaction(tx, now)?,
            now,
        )),
        Command::PomoNext => Ok(views::session(
            &pomodoro::next_in_transaction(tx, now)?,
            now,
        )),
        Command::PomoStop => Ok(
            json!({"pomodoro":pomodoro::stop_in_transaction(tx,now)?.as_ref().map(|s|views::session(s,now))}),
        ),
        Command::Capabilities
        | Command::Context { .. }
        | Command::Query(_)
        | Command::Receipt { .. }
        | Command::Initialize => Err(Fault::input("Expected a business mutation")),
    }
}
