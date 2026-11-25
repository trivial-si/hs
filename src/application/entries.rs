//! Shared entry operations. Callers own the transaction and timer policy.
use crate::{
    Error, Result,
    model::{Entry, NewEntry},
    storage::mutations::activate_vocabulary,
    store::{
        current_id, find_entry, finish_entry, insert_entry, overlap_candidates, validate_entry,
    },
    tasks,
    time::{Moment, local_at, normalize_entry, resolve_local},
};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{Connection, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartMode {
    ReuseMatching,
    NewSegment,
}

#[derive(Debug)]
pub struct Change {
    pub entry: Entry,
    pub adjusted: Vec<i64>,
}

/// A boundary can be compared on the civil timeline used by ordinary records
/// or on the UTC timeline used by timed focus records.
#[derive(Clone, Copy)]
pub(crate) struct Boundary {
    pub(crate) local: NaiveDateTime,
    pub(crate) utc: Option<DateTime<Utc>>,
}

impl Boundary {
    pub(crate) fn new(
        local: NaiveDateTime,
        utc: Option<DateTime<Utc>>,
        now: &Moment,
    ) -> Result<Self> {
        // A saved focus can be edited in another zone. Compare its instant
        // with ordinary records on the invocation's local wall timeline.
        Ok(Self {
            local: match utc {
                Some(at) => local_at(at, now.zone)?,
                None => local,
            },
            utc: utc.or_else(|| resolve_local(local, now.zone, Some(now.utc)).ok()),
        })
    }

    pub(crate) fn from_now(now: &Moment) -> Result<Self> {
        Ok(Self {
            local: now.local()?,
            utc: Some(now.utc),
        })
    }

    pub(crate) fn start(entry: &NewEntry, now: &Moment) -> Result<Self> {
        Self::new(entry.start, entry.clock.map(|c| c.start_utc), now)
    }

    pub(crate) fn end(entry: &NewEntry, now: &Moment) -> Result<Option<Self>> {
        entry
            .end
            .map(|end| Self::new(end, entry.clock.and_then(|c| c.end_utc), now))
            .transpose()
    }

    pub(crate) fn required_utc(self, now: &Moment) -> Result<DateTime<Utc>> {
        match self.utc {
            Some(utc) => Ok(utc),
            None => resolve_local(self.local, now.zone, Some(now.utc)),
        }
    }

    pub(crate) fn timeline(self, clocked: bool, now: &Moment) -> Result<NaiveDateTime> {
        if clocked {
            Ok(self.required_utc(now)?.naive_utc())
        } else {
            Ok(self.local)
        }
    }
}

/// A library caller can reuse a previous record as a new timer template.
/// Preserve its requested times while loading the open task's current metadata.
pub(crate) fn refresh_linked_template(
    conn: &Connection,
    entry: NewEntry,
    now: &Moment,
) -> Result<NewEntry> {
    let Some(link) = entry.task else {
        return Ok(entry);
    };
    let mut fresh = tasks::template_from(conn, link.id, now)?;
    fresh.start = entry.start;
    fresh.end = entry.end;
    fresh.clock = entry.clock;
    Ok(fresh)
}

pub(crate) fn start_in_transaction(
    conn: &Transaction<'_>,
    mut entry: NewEntry,
    now: &Moment,
    mode: StartMode,
) -> Result<Change> {
    normalize_entry(&mut entry)?;
    validate_entry(&entry)?;
    if entry.end.is_some() {
        return Err(Error::Input(
            "Use hs add to record an entry with an end time".into(),
        ));
    }
    let start = Boundary::start(&entry, now)?;
    let clocked = entry.clock.is_some();
    let current_time = Boundary::from_now(now)?;
    if start.timeline(clocked, now)? > current_time.timeline(clocked, now)? {
        return Err(Error::Input(
            "A running activity cannot start in the future".into(),
        ));
    }
    let mut adjusted = Vec::new();
    if let Some(id) = current_id(conn)? {
        let current = find_entry(conn, id)?;
        let current_start = current
            .clock
            .map_or(current.start, |c| c.start_utc.naive_utc());
        let earlier = start.timeline(current.clock.is_some(), now)? < current_start;
        let mut requested_tags = entry.tags.clone();
        requested_tags.sort();
        requested_tags.dedup();
        if mode == StartMode::ReuseMatching
            && (!earlier || start.utc == Some(now.utc))
            && entry.task == current.task
            && entry.activity == current.activity
            && entry.category == current.category
            && entry.description == current.description
            && requested_tags == current.tags
        {
            return Ok(Change {
                entry: current,
                adjusted,
            });
        }
        if earlier {
            return Err(Error::Conflict(format!(
                "Start precedes running entry {id}. Correct or stop that entry first."
            )));
        }
        finish_entry(conn, id, start.local, start.utc)?;
        adjusted.push(id);
    }
    check_open_interval(conn, start, current_time, true, now)?;
    let id = insert_entry(conn, &entry)?;
    crate::storage::mutations::set_current(conn, id)?;
    activate_vocabulary(conn, &entry)?;
    Ok(Change {
        entry: find_entry(conn, id)?,
        adjusted,
    })
}

/// A timer may claim previously unoccupied time. Bounded edits can explicitly
/// reshape the surrounding history and are handled separately.
pub(crate) fn check_open_interval(
    conn: &Connection,
    start: Boundary,
    end: Boundary,
    include_end: bool,
    now: &Moment,
) -> Result<()> {
    if !include_end
        && start.local >= end.local
        && start.utc.zip(end.utc).is_none_or(|(a, b)| a >= b)
    {
        return Ok(());
    }
    for old in overlap_candidates(
        conn,
        start.local,
        end.local,
        start.utc,
        end.utc,
        include_end,
    )? {
        let Some(old_end) = old.end else {
            continue;
        };
        let clocked = old.clock.is_some();
        let lower = start.timeline(clocked, now)?;
        let upper = end.timeline(clocked, now)?;
        let old_start = old.clock.map_or(old.start, |c| c.start_utc.naive_utc());
        let old_end = old
            .clock
            .and_then(|c| c.end_utc)
            .map_or(old_end, |t| t.naive_utc());
        let touches = old_start < upper || (include_end && old_start == upper);
        let claimed = lower < upper || (include_end && lower == upper);
        if claimed && old_start < old_end && touches && old_end > lower {
            return Err(Error::Conflict(format!(
                "Requested start overlaps completed entry {}. Supply an explicit end time to adjust historical intervals.",
                old.id
            )));
        }
    }
    Ok(())
}
