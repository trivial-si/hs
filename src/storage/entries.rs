//! Indexed entry reads used by status, history, task totals and overlap checks.
use chrono::{DateTime, Datelike, NaiveDateTime, Utc};
use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;

use crate::{
    Error, Result,
    domain::{
        tasks::{Quadrant, TaskStatus},
        workday::WorkdayRange,
    },
    model::{Entry, EntryClock, EntryTask, format_time},
};

pub(crate) fn parse_datetime(value: &str) -> Result<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M"))
        .map_err(|_| Error::Input(format!("Invalid stored date/time: {value}")))
}

pub(crate) fn load_entries(conn: &Connection) -> Result<Vec<Entry>> {
    load_selected(conn, "", &[])
}

/// Conservative candidates, followed by the report's exact saved-zone check.
/// Four-digit civil years sort as stored text; old extended-year report bounds
/// need a calendar comparison instead. Minute prefixes also cover old v1 rows
/// that omitted seconds at an exact workday boundary.
pub(crate) fn for_history_range(conn: &Connection, range: &WorkdayRange) -> Result<Vec<Entry>> {
    if [range.start, range.end]
        .iter()
        .any(|t| !(0..=9999).contains(&t.year()))
    {
        return load_entries(conn);
    }
    let lower = range.start.format("%Y-%m-%d %H:%M").to_string();
    let upper = range.end.format("%Y-%m-%d %H:%M").to_string();
    // Every chrono UTC offset has magnitude <24h. A boundary inside a
    // DST gap maps to the transition, between its two offset projections.
    let utc_lower = range.start.and_utc().timestamp() - 86_400;
    let utc_upper = range.end.and_utc().timestamp() + 86_400;
    load_selected(
        conn,
        "WHERE e.id IN (
             SELECT id FROM entries WHERE start_utc IS NULL AND start_time < ?2
                AND (start_time >= ?1 OR end_time IS NULL OR end_time > ?1
                     OR end_time GLOB '+*' OR end_time GLOB '-*')
             UNION ALL
             SELECT id FROM entries WHERE start_utc IS NOT NULL AND start_utc < ?4
                AND (start_utc >= ?3 OR end_utc IS NULL OR end_utc > ?3)
         )",
        &[&lower, &upper, &utc_lower, &utc_upper],
    )
}

/// Load only entries that can contribute to one task's recorded time.
pub(crate) fn for_task(conn: &Connection, id: i64) -> Result<Vec<Entry>> {
    load_selected(conn, "WHERE e.task_id=?", &[&id])
}

/// Use the task/entry index to exclude unlinked history and other statuses.
pub(crate) fn for_task_status(conn: &Connection, status: Option<TaskStatus>) -> Result<Vec<Entry>> {
    match status {
        Some(status) => load_selected(
            conn,
            "WHERE e.task_id IN (SELECT id FROM tasks WHERE status=?)",
            &[&status.key()],
        ),
        // Looking up each task ID through entries_task also avoids scanning
        // every unrelated entry_tags row in a large imported history.
        None => load_selected(conn, "WHERE e.task_id IN (SELECT id FROM tasks)", &[]),
    }
}

pub(crate) fn overlap_candidates(
    conn: &Connection,
    start: NaiveDateTime,
    end: NaiveDateTime,
    utc_start: Option<DateTime<Utc>>,
    utc_end: Option<DateTime<Utc>>,
    include_end: bool,
) -> Result<Vec<Entry>> {
    let before_end = if include_end { "<=" } else { "<" };
    let local_start = format_time(start);
    let local_end = format_time(end);
    if let (Some(first), Some(last)) = (utc_start, utc_end) {
        load_selected(conn,
            &format!("WHERE (e.start_utc IS NOT NULL AND e.start_utc {before_end} ?4 AND (e.end_utc IS NULL OR e.end_utc > ?3))
             OR (e.start_utc IS NULL AND e.start_time {before_end} ?2 AND (e.end_time IS NULL OR e.end_time > ?1))"),
            &[&local_start, &local_end, &first.timestamp(), &last.timestamp()],
        )
    } else {
        load_selected(
            conn,
            &format!(
                "WHERE e.start_time {before_end} ?2 AND (e.end_time IS NULL OR e.end_time > ?1)"
            ),
            &[&local_start, &local_end],
        )
    }
}

fn utc_instant(seconds: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp(seconds, 0)
        .ok_or_else(|| Error::Input(format!("Stored UTC timestamp {seconds} is out of range")))
}

fn load_selected(
    conn: &Connection,
    filter: &str,
    parameters: &[&dyn rusqlite::ToSql],
) -> Result<Vec<Entry>> {
    let mut tag_map: HashMap<i64, Vec<String>> = HashMap::new();
    let mut statement = conn.prepare(&format!("SELECT e.id, t.name FROM entries e JOIN entry_tags et ON et.entry_id=e.id JOIN tags t ON t.id=et.tag_id {filter} ORDER BY t.name"))?;
    for row in statement.query_map(parameters, |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })? {
        let (id, name) = row?;
        tag_map.entry(id).or_default().push(name);
    }
    let mut statement = conn.prepare(&format!(
        "SELECT e.id, a.name, NULLIF(c.name, ''), e.description, e.start_time, e.end_time, EXISTS(SELECT 1 FROM current_entry WHERE entry_id=e.id), e.start_utc, e.end_utc, e.timezone, e.task_id, e.quadrant_snapshot FROM entries e JOIN activities a ON a.id=e.activity_id JOIN categories c ON c.id=a.category_id {filter} ORDER BY e.start_time, e.id"
    ))?;
    let mut rows = statement.query(parameters)?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next()? {
        let id = row.get(0)?;
        let end: Option<String> = row.get(5)?;
        let clock = match row.get::<_, Option<i64>>(7)? {
            Some(start) => {
                let name: String = row.get(9)?;
                let zone = name.parse::<Tz>().map_err(|_| {
                    Error::Input(format!("Entry {id} has unknown timezone {name:?}"))
                })?;
                Some(EntryClock {
                    start_utc: utc_instant(start)?,
                    end_utc: row.get::<_, Option<i64>>(8)?.map(utc_instant).transpose()?,
                    zone,
                })
            }
            None => None,
        };
        let task = match row.get::<_, Option<i64>>(10)? {
            Some(id) => Some(EntryTask {
                id,
                quadrant: row
                    .get::<_, Option<String>>(11)?
                    .as_deref()
                    .map(Quadrant::parse)
                    .transpose()?,
            }),
            None => None,
        };
        entries.push(Entry {
            clock,
            task,
            id,
            activity: row.get(1)?,
            category: row.get(2)?,
            description: row.get(3)?,
            start: parse_datetime(&row.get::<_, String>(4)?)?,
            end: end.as_deref().map(parse_datetime).transpose()?,
            running: row.get(6)?,
            tags: tag_map.remove(&id).unwrap_or_default(),
        });
    }
    Ok(entries)
}

pub(crate) fn find_entry(conn: &Connection, id: i64) -> Result<Entry> {
    load_selected(conn, "WHERE e.id=?", &[&id])?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Input(format!("Entry {id} does not exist")))
}

pub(crate) fn current_id(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT entry_id FROM current_entry WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}
