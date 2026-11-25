//! SQLite projection and writes for Pomodoro sessions, phases, and work segments.
use crate::{
    Error, Result,
    domain::pomodoro::{
        Completion, Config, Interruption, Phase, PhaseState, SavedTimer, Session, State,
        Statistics, WatchedPhase,
    },
    model::{ActivityTemplate, NewEntry},
    time::Moment,
};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, Transaction, params};

fn timestamp(value: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp(value, 0)
        .ok_or_else(|| Error::Input("Invalid Pomodoro timestamp".into()))
}
pub(crate) fn load(conn: &Connection, active_only: bool) -> Result<Option<Session>> {
    let filter = if active_only {
        "WHERE s.state != 'stopped'"
    } else {
        ""
    };
    let mut query = conn.prepare(&format!(
        "SELECT s.id, s.activity, s.category, s.description, s.tags, s.focus_seconds,
                s.break_seconds, s.long_break_seconds, s.rounds, s.completed_focus,
                s.interrupted_focus, s.phase, s.state, s.deadline_utc, s.remaining_seconds,
                s.current_entry_id, s.current_phase_id, s.timezone, s.last_observed_utc, s.task_id,
                p.id IS NOT NULL AND p.session_id=s.id AND
                    (s.state NOT IN ('running','paused') OR (s.phase=p.phase AND s.state=p.state)),
                g.entry_id IS NOT NULL
         FROM pomodoro_sessions s
         LEFT JOIN pomodoro_phases p ON p.id=s.current_phase_id
         LEFT JOIN pomodoro_segments g ON g.entry_id=s.current_entry_id AND g.phase_id=p.id
         {filter} ORDER BY s.id DESC LIMIT 1"
    ))?;
    let mut rows = query.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let raw_deadline: Option<i64> = row.get(13)?;
    let phase_id: Option<i64> = row.get(16)?;
    if phase_id.is_some() && !row.get::<_, bool>(20)? {
        return Err(Error::Conflict(
            "The saved Pomodoro phase does not match its session or state".into(),
        ));
    }
    let state = State::parse(&row.get::<_, String>(12)?)?;
    let phase = Phase::parse(&row.get::<_, String>(11)?)?;
    let entry_id = if state == State::Running && phase == Phase::Focus && !row.get::<_, bool>(21)? {
        None
    } else {
        row.get(15)?
    };
    let saved = SavedTimer {
        config: Config {
            focus_seconds: row.get(5)?,
            break_seconds: row.get(6)?,
            long_break_seconds: row.get(7)?,
            rounds: row.get(8)?,
        },
        completed: row.get(9)?,
        interrupted: row.get(10)?,
        phase,
        state,
        deadline: raw_deadline.map(timestamp).transpose()?,
        remaining: row.get(14)?,
        entry_id,
        phase_id,
        zone: row
            .get::<_, String>(17)?
            .parse()
            .map_err(|_| Error::Input("Invalid saved Pomodoro timezone".into()))?,
        last_observed: timestamp(row.get(18)?)?,
    };
    let details = ActivityTemplate {
        activity: row.get(1)?,
        category: row.get(2)?,
        description: row.get(3)?,
        tags: serde_json::from_str(&row.get::<_, String>(4)?)?,
    };
    Session::restore(row.get(0)?, row.get(19)?, details, saved).map(Some)
}

pub(crate) fn watched_phase(conn: &Connection, id: i64) -> Result<Option<WatchedPhase>> {
    let mut statement =
        conn.prepare("SELECT session_id,phase,state,ended_utc FROM pomodoro_phases WHERE id=?")?;
    let mut rows = statement.query([id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let state = PhaseState::parse(&row.get::<_, String>(2)?)?;
    let ended_utc = row.get::<_, Option<i64>>(3)?.map(timestamp).transpose()?;
    if matches!(state, PhaseState::Completed | PhaseState::Interrupted) != ended_utc.is_some() {
        return Err(Error::Conflict(
            "The saved Pomodoro phase state does not match its end time".into(),
        ));
    }
    Ok(Some(WatchedPhase {
        id,
        session_id: row.get(0)?,
        phase: Phase::parse(&row.get::<_, String>(1)?)?,
        state,
        ended_utc,
    }))
}

pub(crate) fn insert_session(
    tx: &Transaction<'_>,
    template: &NewEntry,
    config: Config,
    now: &Moment,
) -> Result<()> {
    tx.execute(
        "INSERT INTO pomodoro_sessions(activity, category, description, tags, focus_seconds, break_seconds,
            long_break_seconds, rounds, phase, state, deadline_utc, remaining_seconds, timezone,
            created_utc, last_observed_utc, task_id) VALUES
            (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'focus', 'ready', NULL, ?5, ?9, ?10, ?10, ?11)",
        params![template.activity,template.category,template.description,serde_json::to_string(&template.tags)?,
            config.focus_seconds,config.break_seconds,config.long_break_seconds,config.rounds,now.zone.name(),
            now.utc.timestamp(),template.task.map(|link| link.id)],
    )?;
    Ok(())
}

pub(crate) fn set_activity(tx: &Transaction<'_>, session: i64, entry: &NewEntry) -> Result<()> {
    tx.execute(
        "UPDATE pomodoro_sessions SET activity=?, category=?, description=?, tags=? WHERE id=?",
        params![
            entry.activity,
            entry.category,
            entry.description,
            serde_json::to_string(&entry.tags)?,
            session
        ],
    )?;
    Ok(())
}

pub(crate) fn insert_phase(
    tx: &Transaction<'_>,
    session: i64,
    phase: Phase,
    at: DateTime<Utc>,
    seconds: i64,
) -> Result<i64> {
    tx.execute("INSERT INTO pomodoro_phases(session_id, phase, state, started_utc, target_seconds, elapsed_seconds)
                VALUES (?, ?, 'running', ?, ?, 0)", params![session,phase.key(),at.timestamp(),seconds])?;
    Ok(tx.last_insert_rowid())
}

pub(crate) fn link_segment(tx: &Transaction<'_>, phase: i64, entry: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO pomodoro_segments(phase_id,entry_id) VALUES (?,?)",
        params![phase, entry],
    )?;
    Ok(())
}

pub(crate) fn observe(tx: &Transaction<'_>, session: i64, at: DateTime<Utc>) -> Result<()> {
    tx.execute(
        "UPDATE pomodoro_sessions SET last_observed_utc=? WHERE id=?",
        params![at.timestamp(), session],
    )?;
    Ok(())
}

pub(crate) fn set_running(
    tx: &Transaction<'_>,
    session: i64,
    phase: i64,
    entry: Option<i64>,
    at: DateTime<Utc>,
    deadline: DateTime<Utc>,
    remaining: i64,
) -> Result<()> {
    tx.execute(
        "UPDATE pomodoro_sessions SET state='running', deadline_utc=?, remaining_seconds=?,
                    current_entry_id=?, current_phase_id=?, last_observed_utc=? WHERE id=?",
        params![
            deadline.timestamp(),
            remaining,
            entry,
            phase,
            at.timestamp(),
            session
        ],
    )?;
    tx.execute(
        "UPDATE pomodoro_phases SET state='running' WHERE id=? AND state='paused'",
        [phase],
    )?;
    Ok(())
}

pub(crate) fn complete(
    tx: &Transaction<'_>,
    session: i64,
    done: Completion,
    next_duration: i64,
) -> Result<()> {
    tx.execute("UPDATE pomodoro_phases SET state='completed', ended_utc=?, elapsed_seconds=target_seconds WHERE id=?",
        params![done.at.timestamp(),done.phase_id])?;
    tx.execute(
        "UPDATE pomodoro_sessions SET state='ready', phase=?, completed_focus=?, deadline_utc=NULL,
                    remaining_seconds=?, current_entry_id=NULL, last_observed_utc=? WHERE id=?",
        params![
            done.next.key(),
            done.completed,
            next_duration,
            done.observed.timestamp(),
            session
        ],
    )?;
    Ok(())
}

pub(crate) fn pause(
    tx: &Transaction<'_>,
    session: &Session,
    at: DateTime<Utc>,
    remaining: i64,
) -> Result<()> {
    tx.execute("UPDATE pomodoro_phases SET state='paused', ended_utc=NULL, elapsed_seconds=max(0,target_seconds-?) WHERE id=?",
        params![remaining,session.phase_id()])?;
    tx.execute(
        "UPDATE pomodoro_sessions SET state='paused', deadline_utc=NULL, remaining_seconds=?,
                    current_entry_id=NULL, last_observed_utc=? WHERE id=?",
        params![remaining, at.timestamp(), session.id],
    )?;
    Ok(())
}

pub(crate) fn stop(tx: &Transaction<'_>, session: i64, change: &Interruption) -> Result<()> {
    if let Some(phase) = change.phase_id {
        tx.execute("UPDATE pomodoro_phases SET state='interrupted', ended_utc=?, elapsed_seconds=max(0,target_seconds-?) WHERE id=?",
            params![change.at.timestamp(),change.remaining,phase])?;
    }
    tx.execute(
        "UPDATE pomodoro_sessions SET state='stopped', deadline_utc=NULL, remaining_seconds=0,
                    current_entry_id=NULL, interrupted_focus=?, last_observed_utc=? WHERE id=?",
        params![change.interrupted_focus, change.at.timestamp(), session],
    )?;
    Ok(())
}

pub(crate) fn statistics(
    tx: &Transaction<'_>,
    as_of: DateTime<Utc>,
    current_focus: Option<i64>,
) -> Result<Statistics> {
    let (completed, interrupted, break_seconds) = tx.query_row(
        "SELECT count(*) FILTER (WHERE phase='focus' AND state='completed'),
                count(*) FILTER (WHERE phase='focus' AND state='interrupted'),
                coalesce(sum(elapsed_seconds) FILTER (WHERE phase!='focus' AND state!='running'),0)
         FROM pomodoro_phases",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let focus_seconds = tx.query_row(
        "SELECT coalesce(sum(max(0,coalesce(e.end_utc,?1)-e.start_utc)),0)
         FROM pomodoro_segments s JOIN entries e ON e.id=s.entry_id
         WHERE e.end_utc IS NOT NULL OR e.id=?2",
        params![as_of.timestamp(), current_focus],
        |row| row.get(0),
    )?;
    Ok(Statistics {
        completed,
        interrupted,
        focus_seconds,
        break_seconds,
    })
}
