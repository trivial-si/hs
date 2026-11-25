//! Persistent Pomodoro phases. All timer and work-entry transitions share a transaction.
use crate::domain::ids::TaskId;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, Transaction};

use crate::{
    Error, Result, Store,
    application::entries::{self, StartMode},
    domain::pomodoro::{Tick, deadline},
    model::{EntryClock, NewEntry},
    storage::pomodoro as persistence,
    store::{current_id, find_entry, finish_entry},
    tasks,
    time::{Moment, local_at},
};

pub use crate::domain::pomodoro::{
    Config, Phase, PhaseState, Session, State, Statistics, WatchedPhase,
};
pub(crate) use crate::storage::pomodoro::load;

fn required(conn: &Connection) -> Result<Session> {
    load(conn, true)?.ok_or_else(|| {
        Error::Conflict("There is no active Pomodoro. Use `hs pomo start ACTIVITY`.".into())
    })
}

fn start_segment(
    conn: &Transaction<'_>,
    session: &Session,
    phase_id: i64,
    now: &Moment,
) -> Result<i64> {
    let now = Moment::at(now.utc, session.zone());
    let mut entry = match session.task_id {
        Some(task_id) => {
            let entry = tasks::template_from(conn, task_id, &now)?;
            // The session display follows the current segment. Existing entries
            // keep their own text, tags and priority snapshot.
            persistence::set_activity(conn, session.id, &entry)?;
            entry
        }
        None => NewEntry {
            activity: session.activity.clone(),
            category: session.category.clone(),
            description: session.description.clone(),
            tags: session.tags.clone(),
            start: now.local()?,
            end: None,
            clock: None,
            task: None,
        },
    };
    entry.clock = Some(EntryClock {
        start_utc: now.utc,
        end_utc: None,
        zone: session.zone(),
    });
    let id = entries::start_in_transaction(conn, entry, &now, StartMode::NewSegment)?
        .entry
        .id;
    persistence::link_segment(conn, phase_id, id)?;
    Ok(id)
}

fn close_segment(conn: &Transaction<'_>, session: &Session, at: DateTime<Utc>) -> Result<()> {
    if let Some(id) = session.entry_id()
        && current_id(conn)? == Some(id)
    {
        let entry = find_entry(conn, id)?;
        let clock = entry.clock.ok_or_else(|| {
            Error::Conflict("The Pomodoro work entry lost its clock information".into())
        })?;
        if at < clock.start_utc {
            return Err(Error::Input(
                "The requested time precedes the current focus segment".into(),
            ));
        }
        finish_entry(conn, id, local_at(at, clock.zone)?, Some(at))?;
    }
    Ok(())
}

pub fn reconcile(store: &mut Store, now: &Moment) -> Result<()> {
    store.write(|tx| reconcile_in_transaction(tx, now))
}

pub(crate) fn reconcile_in_transaction(conn: &Transaction<'_>, now: &Moment) -> Result<()> {
    let Some(session) = load(conn, true)? else {
        return Ok(());
    };
    let owner = if session.state() == State::Running && session.phase() == Phase::Focus {
        current_id(conn)?
    } else {
        None
    };
    match session.tick(now, owner)? {
        Tick::Idle => {}
        Tick::InterruptLostFocus => {
            interrupt_session(conn, &session, now, None)?;
        }
        Tick::Observe(observed) => {
            if observed != session.last_observed() {
                persistence::observe(conn, session.id, observed)?;
            }
        }
        Tick::Complete(done) => {
            close_segment(conn, &session, done.at)?;
            persistence::complete(conn, session.id, done, session.config().duration(done.next))?;
        }
    }
    Ok(())
}

/// Interrupt an open session without ever closing an unrelated running entry.
pub(crate) fn interrupt_in_transaction(
    conn: &Transaction<'_>,
    now: &Moment,
    at_utc: Option<DateTime<Utc>>,
) -> Result<()> {
    if let Some(session) = load(conn, true)? {
        interrupt_session(conn, &session, now, at_utc)?;
    }
    Ok(())
}

fn interrupt_session(
    conn: &Transaction<'_>,
    session: &Session,
    now: &Moment,
    at_utc: Option<DateTime<Utc>>,
) -> Result<DateTime<Utc>> {
    let interruption = session.interruption(now, at_utc)?;
    close_segment(conn, session, interruption.at)?;
    persistence::stop(conn, session.id, &interruption)?;
    Ok(interruption.at)
}

pub(crate) fn entry_changed_in_transaction(
    conn: &Transaction<'_>,
    id: i64,
    now: &Moment,
) -> Result<()> {
    if let Some(session) = load(conn, true)?.filter(|s| s.entry_id() == Some(id)) {
        interrupt_session(conn, &session, now, None)?;
    }
    Ok(())
}

pub(crate) fn interrupt_task_in_transaction(
    conn: &Transaction<'_>,
    task_id: i64,
    now: &Moment,
) -> Result<DateTime<Utc>> {
    if let Some(session) = load(conn, true)?.filter(|s| s.task_id == Some(task_id)) {
        // A paused focus, a break, or a ready session has no running work row.
        return interrupt_session(conn, &session, now, None);
    }
    Ok(now.utc)
}

pub fn status(store: &mut Store, now: &Moment) -> Result<Option<Session>> {
    crate::snapshot::read(store, now, |_, session, _| Ok(session))
}

#[derive(Debug)]
pub struct WatchStatus {
    pub session: Option<Session>,
    pub watched_phase: Option<WatchedPhase>,
}

/// Attach to a stored running phase, or poll a phase already selected by its caller.
/// The selected phase and the latest session are returned in one reconciled snapshot.
pub fn watch_status(
    store: &mut Store,
    watched_phase_id: Option<i64>,
    now: &Moment,
) -> Result<WatchStatus> {
    let id = match watched_phase_id {
        Some(id) => id,
        None => {
            now.local()?;
            // Pin the identity observed at attachment, before a status read
            // can reconcile an overdue phase into Ready. A later attachment
            // to that already-ready session has no running phase to announce.
            let session = load(&store.conn, false)?;
            let Some(id) = session
                .as_ref()
                .filter(|s| s.state() == State::Running)
                .and_then(Session::phase_id)
            else {
                return Ok(WatchStatus {
                    session,
                    watched_phase: None,
                });
            };
            id
        }
    };
    crate::snapshot::read(store, now, |tx, session, _| {
        Ok(WatchStatus {
            session,
            watched_phase: persistence::watched_phase(tx, id)?,
        })
    })
}

pub fn start(
    store: &mut Store,
    template: NewEntry,
    config: Config,
    now: &Moment,
) -> Result<Session> {
    store.write(|tx| start_in_transaction(tx, template, config, now))
}

pub(crate) fn start_in_transaction(
    tx: &Transaction<'_>,
    template: NewEntry,
    config: Config,
    now: &Moment,
) -> Result<Session> {
    start_with(tx, config, now, |conn| {
        entries::refresh_linked_template(conn, template, now)
    })
}

pub fn start_task(
    store: &mut Store,
    task_id: i64,
    config: Config,
    now: &Moment,
) -> Result<Session> {
    store.write(|tx| start_task_in_transaction(tx, TaskId::new(task_id)?, config, now))
}

pub(crate) fn start_task_in_transaction(
    tx: &Transaction<'_>,
    task_id: TaskId,
    config: Config,
    now: &Moment,
) -> Result<Session> {
    start_with(tx, config, now, |conn| {
        tasks::template_from(conn, task_id.get(), now)
    })
}

fn start_with(
    tx: &Transaction<'_>,
    config: Config,
    now: &Moment,
    get_template: impl FnOnce(&Connection) -> Result<NewEntry>,
) -> Result<Session> {
    config.validate(now)?;
    reconcile_in_transaction(tx, now)?;
    let mut template = get_template(tx)?;
    let local_now = now.local()?;
    if template.start != local_now && template.start != crate::model::minute(local_now) {
        return Err(Error::Input(
            "Pomodoro focus begins now. Use `hs add` for backdated work.".into(),
        ));
    }
    if template.end.is_some() {
        return Err(Error::Input(
            "A Pomodoro starts now and runs for its configured focus duration".into(),
        ));
    }
    template.tags.sort();
    template.tags.dedup();
    let task_id = template.task.map(|link| link.id);
    if let Some(existing) = load(tx, true)? {
        let same_target = match (existing.task_id, task_id) {
            (Some(current), Some(requested)) => current == requested,
            (None, None) => {
                existing.activity == template.activity
                    && existing.category == template.category
                    && existing.description == template.description
                    && existing.tags == template.tags
            }
            _ => false,
        };
        if existing.state() == State::Running
            && existing.phase() == Phase::Focus
            && same_target
            && existing.config() == config
        {
            return Ok(existing);
        }
        return Err(Error::Conflict(
            "A Pomodoro session is already open. Use `hs pomo pause`, `resume`, `next`, or `stop`."
                .into(),
        ));
    }
    persistence::insert_session(tx, &template, config, now)?;
    begin_phase(tx, now)?;
    required(tx)
}

fn begin_phase(conn: &Transaction<'_>, now: &Moment) -> Result<()> {
    let session = required(conn)?;
    let now = &Moment::at(session.observed(now), session.zone());
    let seconds = session.duration();
    let deadline = deadline(now, seconds)?;
    let phase_id = persistence::insert_phase(conn, session.id, session.phase(), now.utc, seconds)?;
    let entry_id = if session.phase() == Phase::Focus {
        Some(start_segment(conn, &session, phase_id, now)?)
    } else {
        None
    };
    persistence::set_running(
        conn, session.id, phase_id, entry_id, now.utc, deadline, seconds,
    )
}

pub fn next(store: &mut Store, now: &Moment) -> Result<Session> {
    store.write(|tx| next_in_transaction(tx, now))
}

pub(crate) fn next_in_transaction(tx: &Transaction<'_>, now: &Moment) -> Result<Session> {
    reconcile_in_transaction(tx, now)?;
    match required(tx)?.state() {
        State::Ready => begin_phase(tx, now)?,
        State::Running => {} // Retrying next cannot skip an active phase.
        _ => {
            return Err(Error::Conflict(
                "Use `hs pomo resume` to continue a paused phase".into(),
            ));
        }
    }
    required(tx)
}

pub fn pause(store: &mut Store, now: &Moment) -> Result<Session> {
    store.write(|tx| pause_in_transaction(tx, now))
}

pub(crate) fn pause_in_transaction(tx: &Transaction<'_>, now: &Moment) -> Result<Session> {
    reconcile_in_transaction(tx, now)?;
    let session = required(tx)?;
    if session.state() == State::Running {
        let at = session.observed(now);
        let remaining = session.remaining(now);
        close_segment(tx, &session, at)?;
        persistence::pause(tx, &session, at, remaining)?;
    }
    // An expiry racing this command is a successful transition to Ready.
    // Return it so the caller can offer `next` without rolling completion back.
    required(tx)
}

pub fn resume(store: &mut Store, now: &Moment) -> Result<Session> {
    store.write(|tx| resume_in_transaction(tx, now))
}

pub(crate) fn resume_in_transaction(tx: &Transaction<'_>, now: &Moment) -> Result<Session> {
    reconcile_in_transaction(tx, now)?;
    let session = required(tx)?;
    if let Some((phase_id, remaining)) = session.paused() {
        let at = session.observed(now);
        let moment = Moment::at(at, session.zone());
        let entry_id = if session.phase() == Phase::Focus {
            Some(start_segment(tx, &session, phase_id, &moment)?)
        } else {
            None
        };
        let end = deadline(&moment, remaining)?;
        persistence::set_running(tx, session.id, phase_id, entry_id, at, end, remaining)?;
    }
    required(tx)
}

pub fn stop(store: &mut Store, now: &Moment) -> Result<Option<Session>> {
    store.write(|tx| stop_in_transaction(tx, now))
}

pub(crate) fn stop_in_transaction(tx: &Transaction<'_>, now: &Moment) -> Result<Option<Session>> {
    reconcile_in_transaction(tx, now)?;
    interrupt_in_transaction(tx, now, None)?;
    load(tx, false)
}

pub fn statistics(store: &mut Store, now: &Moment) -> Result<Statistics> {
    crate::snapshot::read(store, now, statistics_from)
}

fn statistics_from(
    tx: &Transaction<'_>,
    session: Option<Session>,
    as_of: Moment,
) -> Result<Statistics> {
    let owner = session.as_ref().and_then(Session::entry_id);
    let mut totals = persistence::statistics(tx, as_of.utc, owner)?;
    if let Some(session) = session
        && session.state() == State::Running
        && session.phase() != Phase::Focus
    {
        totals.break_seconds = totals
            .break_seconds
            .checked_add(session.duration() - session.remaining(&as_of))
            .ok_or_else(|| {
                Error::Input("Recorded break time exceeds the supported range".into())
            })?;
    }
    Ok(totals)
}

/// Accept bare minutes or explicit seconds/minutes/hours, e.g. 25, 90s, 5m, 1h.
pub fn parse_duration(value: &str) -> std::result::Result<i64, String> {
    let (digits, multiplier) = match value.as_bytes().last() {
        Some(b's') => (&value[..value.len() - 1], 1),
        Some(b'm') => (&value[..value.len() - 1], 60),
        Some(b'h') => (&value[..value.len() - 1], 3600),
        _ => (value, 60),
    };
    digits
        .parse::<i64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .filter(|n| *n > 0)
        .ok_or_else(|| "Use a positive duration such as 25m, 90s, or 1h".into())
}

#[cfg(test)]
#[path = "pomodoro_tests.rs"]
mod tests;
