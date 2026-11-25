use super::{
    WireResult,
    command::{NullableQuadrant, Query},
};
use crate::{
    domain::{model::Entry, pomodoro::Session, tasks::Task, time::Moment},
    storage::{
        agent as persistence,
        agent_queries::{self, Filter},
        entries, pomodoro, tasks,
    },
};
use rusqlite::Connection;
use serde_json::{Value, json};

pub(super) fn task(conn: &Connection, t: &Task) -> WireResult<Value> {
    Ok(
        json!({"id":t.id,"revision":persistence::task_revision(conn,t.id)?,"title":t.title,"activity":t.activity,"category":t.category,"description":t.description,"tags":t.tags,"quadrant":t.quadrant.map(|q|q.key()),"due_date":t.due_date.map(|d|d.to_string()),"status":t.status.key(),"created_utc":t.created_utc.to_rfc3339(),"completed_utc":t.completed_utc.map(|t|t.to_rfc3339())}),
    )
}
pub(super) fn entry(conn: &Connection, e: &Entry) -> WireResult<Value> {
    Ok(
        json!({"id":e.id,"revision":persistence::entry_revision(conn,e.id)?,"linked_task_revision":e.task.map(|t|persistence::task_revision(conn,t.id)).transpose()?,"activity":e.activity,"category":e.category,"description":e.description,"tags":e.tags,
        "start_local":e.start.to_string(),"end_local":e.end.map(|t|t.to_string()),"start_utc":e.clock.map(|c|c.start_utc.to_rfc3339()),"end_utc":e.clock.and_then(|c|c.end_utc).map(|t|t.to_rfc3339()),"zone":e.clock.map(|c|c.zone.to_string()),"running":e.running,"task_id":e.task.map(|t|t.id),"quadrant":e.task.and_then(|t|t.quadrant).map(|q|q.key())}),
    )
}
pub(super) fn session(s: &Session, now: &Moment) -> Value {
    json!({"id":s.id,"task_id":s.task_id,"activity":s.activity,"state":s.state().label(),"phase":s.phase().key(),"phase_id":s.phase_id(),"entry_id":s.entry_id(),"deadline_utc":s.deadline().map(|t|t.to_rfc3339()),"remaining_seconds":s.remaining(now),"completed_focus":s.completed_focus(),"interrupted_focus":s.interrupted_focus(),"zone":s.zone().to_string(),"last_observed_utc":s.last_observed().to_rfc3339(),"reconciliation_due":s.deadline().is_some_and(|d|d<=now.utc.max(s.last_observed()))})
}
pub(super) fn context(conn: &Connection, now: &Moment, limit: u32) -> WireResult<Value> {
    let current = entries::current_id(conn)?
        .map(|id| entries::find_entry(conn, id))
        .transpose()?;
    let timer = pomodoro::load(conn, true)?;
    let timer_view = timer
        .as_ref()
        .map(|s| {
            let mut value = session(s, now);
            value["linked_task_revision"] = json!(
                s.task_id
                    .map(|id| persistence::task_revision(conn, id))
                    .transpose()?
            );
            Ok::<_, super::Fault>(value)
        })
        .transpose()?;
    let page = query(
        conn,
        &Query::Tasks {
            after_id: 0,
            limit,
            status: Some("open".into()),
            quadrant: NullableQuadrant::default(),
            search: None,
        },
    )?;
    Ok(
        json!({"database_id":persistence::database_identity(conn)?,"timer_revision":persistence::timer_revision(conn)?,"history_revision":persistence::history_revision(conn)?,"as_of_utc":now.utc.to_rfc3339(),"zone":now.zone.to_string(),"timer_idle":current.is_none()&&timer.is_none(),"current_entry":current.as_ref().map(|e|entry(conn,e)).transpose()?,"pomodoro":timer_view,"open_tasks":page,"state_semantics":"stored; reads do not reconcile expired timers"}),
    )
}
pub(super) fn query(conn: &Connection, input: &Query) -> WireResult<Value> {
    if let Query::Operations { limit } = input {
        let receipts = persistence::recent_receipts(conn, *limit)?;
        let rows:Vec<_>=receipts.into_iter().map(|r|json!({"request_id":r.request_id,"operation":r.operation,"actor":r.actor,"session":r.session,"created_utc":chrono::DateTime::from_timestamp(r.created_utc,0).map(|t|t.to_rfc3339())})).collect();
        return Ok(
            json!({"entity":"operations","items":rows,"limit":limit,"ordering":"most recent first","pagination":"recent window only"}),
        );
    }
    let (entity, filter) = match input {
        Query::Tasks {
            after_id,
            limit,
            status,
            quadrant,
            search,
        } => (
            "tasks",
            Filter {
                after: *after_id,
                limit: *limit,
                status: status.as_deref(),
                quadrant: quadrant.as_filter(),
                search: search.as_deref(),
                task_id: None,
            },
        ),
        Query::Entries {
            after_id,
            limit,
            task_id,
            quadrant,
            search,
        } => (
            "entries",
            Filter {
                after: *after_id,
                limit: *limit,
                status: None,
                quadrant: quadrant.as_filter(),
                search: search.as_deref(),
                task_id: *task_id,
            },
        ),
        Query::Operations { .. } => unreachable!("operation receipts returned above"),
    };
    let mut ids = if matches!(input, Query::Tasks { .. }) {
        agent_queries::task_ids(conn, &filter)?
    } else {
        agent_queries::entry_ids(conn, &filter)?
    };
    let more = ids.len() > filter.limit as usize;
    ids.truncate(filter.limit as usize);
    let next = if more { ids.last().copied() } else { None };
    let rows = ids
        .into_iter()
        .map(|id| {
            if matches!(input, Query::Tasks { .. }) {
                task(conn, &tasks::load_from(conn, id)?)
            } else {
                entry(conn, &entries::find_entry(conn, id)?)
            }
        })
        .collect::<WireResult<Vec<_>>>()?;
    Ok(
        json!({"entity":entity,"items":rows,"next_after_id":next,"limit":filter.limit,"ordering":"id ascending"}),
    )
}
