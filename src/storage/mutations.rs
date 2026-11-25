//! Small SQLite mutations shared by transactional application workflows.
use crate::{
    Error, Result,
    model::NewEntry,
    time::{Moment, local_at},
};
use chrono::DateTime;
use rusqlite::{Connection, OptionalExtension, Transaction};

pub(crate) fn set_current(tx: &Transaction<'_>, id: i64) -> Result<()> {
    tx.execute(
        "UPDATE current_entry SET entry_id=? WHERE singleton=1",
        [id],
    )?;
    Ok(())
}
pub(crate) fn clear_current(tx: &Transaction<'_>, id: i64) -> Result<()> {
    tx.execute(
        "UPDATE current_entry SET entry_id=NULL WHERE entry_id=?",
        [id],
    )?;
    Ok(())
}
pub(crate) fn delete_entry(tx: &Transaction<'_>, id: i64) -> Result<()> {
    tx.execute("DELETE FROM entries WHERE id=?", [id])?;
    Ok(())
}
pub(crate) fn copy_segment_link(tx: &Transaction<'_>, prior: i64, tail: i64) -> Result<()> {
    tx.execute("INSERT INTO pomodoro_segments(phase_id, entry_id) SELECT phase_id, ?1 FROM pomodoro_segments WHERE entry_id=?2", rusqlite::params![tail, prior])?;
    Ok(())
}

pub(crate) fn latest_completed(conn: &Connection, now: &Moment) -> Result<Option<i64>> {
    let civil: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, end_time FROM entries WHERE end_time IS NOT NULL AND end_utc IS NULL
         ORDER BY end_time DESC, id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let timed: Option<(i64, i64)> = conn.query_row(
        "SELECT id, end_utc FROM entries WHERE end_utc IS NOT NULL ORDER BY end_utc DESC, id DESC LIMIT 1",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    match (civil, timed) {
        (Some((civil_id, local_end)), Some((timed_id, seconds))) => {
            let civil_end = crate::store::parse_datetime(&local_end)?;
            let utc_end = DateTime::from_timestamp(seconds, 0).ok_or_else(|| {
                Error::Input(format!(
                    "Stored UTC end for entry {timed_id} is out of range"
                ))
            })?;
            let timed_end = local_at(utc_end, now.zone)?;
            Ok(Some(if (civil_end, civil_id) > (timed_end, timed_id) {
                civil_id
            } else {
                timed_id
            }))
        }
        (Some((id, _)), None) | (None, Some((id, _))) => Ok(Some(id)),
        (None, None) => Ok(None),
    }
}

pub(crate) fn activate_vocabulary(conn: &Transaction<'_>, entry: &NewEntry) -> Result<()> {
    conn.execute("UPDATE activities SET archived=0 WHERE id=(SELECT id FROM activities WHERE name=? AND category_id=(SELECT id FROM categories WHERE name=?))", rusqlite::params![entry.activity, entry.category.as_deref().unwrap_or("")])?;
    for tag in &entry.tags {
        conn.execute("UPDATE tags SET autocomplete=1 WHERE name=?", [tag])?;
    }
    Ok(())
}

pub(crate) fn set_day_start(tx: &Transaction<'_>, minutes: u32) -> Result<()> {
    tx.execute(
        "UPDATE settings SET value=? WHERE key='day_start'",
        [minutes.to_string()],
    )?;
    Ok(())
}
