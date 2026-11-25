use crate::application::editing::{EntryEdit, TaskEdit};
pub use crate::application::entries::Change;
use crate::application::entries::{
    Boundary, StartMode, check_open_interval, refresh_linked_template,
    start_in_transaction as start_current,
};
use crate::domain::ids::{EntryId, TaskId};
use crate::storage::mutations::{activate_vocabulary, latest_completed};
use chrono::{NaiveDateTime, NaiveTime};
use rusqlite::{Connection, Transaction};

use crate::{
    Error, Result, Store,
    model::{Entry, EntryClock, NewEntry},
    pomodoro,
    store::{
        current_id, find_entry, finish_entry, insert_entry, overlap_candidates, update_entry,
        validate_entry,
    },
    tasks::{self, Task, TaskStatus},
    time::{Moment, local_at, normalize_entry},
};

pub fn start(store: &mut Store, entry: NewEntry, now: NaiveDateTime) -> Result<Change> {
    start_at(store, entry, &Moment::from_local(now)?)
}

pub fn start_at(store: &mut Store, entry: NewEntry, now: &Moment) -> Result<Change> {
    store.write(|tx| start_in_transaction(tx, entry, now))
}

pub(crate) fn start_in_transaction(
    tx: &Transaction<'_>,
    entry: NewEntry,
    now: &Moment,
) -> Result<Change> {
    pomodoro::reconcile_in_transaction(tx, now)?;
    let entry = refresh_linked_template(tx, entry, now)?;
    start_manual(tx, entry, now)
}

/// Read the task and write its initial quadrant snapshot under the same lock.
pub fn start_task_at(store: &mut Store, task_id: i64, now: &Moment) -> Result<Change> {
    store.write(|tx| start_task_with_clock(tx, task_id, None, now))
}

fn clock_at(now: &Moment) -> EntryClock {
    EntryClock {
        start_utc: now.utc,
        end_utc: None,
        zone: now.zone,
    }
}

pub(crate) fn start_task_in_transaction(
    tx: &Transaction<'_>,
    task_id: TaskId,
    now: &Moment,
) -> Result<Change> {
    start_task_with_clock(tx, task_id.get(), Some(clock_at(now)), now)
}

fn start_task_with_clock(
    tx: &Transaction<'_>,
    task_id: i64,
    clock: Option<EntryClock>,
    now: &Moment,
) -> Result<Change> {
    pomodoro::reconcile_in_transaction(tx, now)?;
    let mut entry = tasks::template_from(tx, task_id, now)?;
    entry.clock = clock;
    start_manual(tx, entry, now)
}

fn start_manual(conn: &Transaction<'_>, mut entry: NewEntry, now: &Moment) -> Result<Change> {
    normalize_entry(&mut entry)?;
    let boundary = Boundary::start(&entry, now)?;
    let interrupted = interrupt_manual(conn, now, boundary)?;
    let mut result = start_current(conn, entry, now, StartMode::ReuseMatching)?;
    result.adjusted.extend(interrupted);
    Ok(result)
}

/// Stop Pomodoro ownership first, even when the requested activity text is
/// unchanged. Its tracking entry and the manual mutation share a transaction.
fn interrupt_manual(conn: &Transaction<'_>, now: &Moment, at: Boundary) -> Result<Option<i64>> {
    let before = current_id(conn)?;
    let utc = match (at.utc, before) {
        (None, Some(id)) if find_entry(conn, id)?.clock.is_some() => Some(at.required_utc(now)?),
        _ => at.utc,
    };
    // At "now", close at the last observed focus instant. The open-interval
    // check below rolls the switch back if the new timer would overlap it.
    let explicit = utc.filter(|instant| *instant != now.utc);
    pomodoro::interrupt_in_transaction(conn, now, explicit)?;
    Ok(match (before, current_id(conn)?) {
        (Some(id), None) => Some(id),
        _ => None,
    })
}

pub fn add(store: &mut Store, entry: NewEntry, now: NaiveDateTime) -> Result<Change> {
    add_at(store, entry, &Moment::from_local(now)?)
}

pub fn add_at(store: &mut Store, entry: NewEntry, now: &Moment) -> Result<Change> {
    store.write(|tx| add_in_transaction(tx, entry, now))
}

pub(crate) fn add_in_transaction(
    tx: &Transaction<'_>,
    entry: NewEntry,
    now: &Moment,
) -> Result<Change> {
    pomodoro::reconcile_in_transaction(tx, now)?;
    let mut entry = refresh_linked_template(tx, entry, now)?;
    if entry.end.is_none() {
        return start_manual(tx, entry, now);
    }
    normalize_entry(&mut entry)?;
    validate_entry(&entry)?;
    check_future_end(&entry, now)?;
    let adjusted = adjust_overlaps(tx, &entry, None, now)?;
    let id = insert_entry(tx, &entry)?;
    activate_vocabulary(tx, &entry)?;
    Ok(Change {
        entry: find_entry(tx, id)?,
        adjusted,
    })
}

/// Parse quick entry time against settings read under this command's transaction.
pub fn start_input(store: &mut Store, input: &str, now: &Moment) -> Result<Change> {
    store.write(|tx| {
        let boundary = crate::store::day_start_from(tx)?;
        let entry = crate::parse::parse_live_entry(input, now.local()?, boundary)?;
        start_in_transaction(tx, entry, now)
    })
}

pub fn add_input(store: &mut Store, input: &str, now: &Moment) -> Result<Change> {
    store.write(|tx| {
        let boundary = crate::store::day_start_from(tx)?;
        let entry = crate::parse::parse_live_entry(input, now.local()?, boundary)?;
        add_in_transaction(tx, entry, now)
    })
}

pub fn start_task(store: &mut Store, id: TaskId, now: &Moment) -> Result<Change> {
    start_task_at(store, id.get(), now)
}
pub fn resume_entry(store: &mut Store, id: Option<EntryId>, now: &Moment) -> Result<Change> {
    resume_at(store, id.map(EntryId::get), now)
}
pub fn delete_entry(store: &mut Store, id: EntryId, now: &Moment) -> Result<()> {
    delete_at(store, id.get(), now)
}
pub fn change_task_status(
    store: &mut Store,
    id: TaskId,
    status: TaskStatus,
    now: &Moment,
) -> Result<Task> {
    set_task_status_at(store, id.get(), status, now)
}
pub fn edit_task(store: &mut Store, id: TaskId, edit: TaskEdit) -> Result<Task> {
    tasks::edit(store, id.get(), |task| edit.apply(task))
}
pub fn edit_entry(store: &mut Store, id: EntryId, edit: EntryEdit, now: &Moment) -> Result<Change> {
    store.write(|tx| edit_entry_in_transaction(tx, id, edit, now))
}

pub(crate) fn edit_entry_in_transaction(
    tx: &Transaction<'_>,
    id: EntryId,
    edit: EntryEdit,
    now: &Moment,
) -> Result<Change> {
    apply_entry_edit(tx, id.get(), now, |entry, boundary| {
        edit.apply(entry, now, boundary)
    })
}

pub(crate) fn edit_with_in_transaction(
    tx: &Transaction<'_>,
    id: EntryId,
    now: &Moment,
    change: impl FnOnce(&Entry) -> Result<NewEntry>,
) -> Result<Change> {
    apply_entry_edit(tx, id.get(), now, |entry, _| change(entry))
}

fn check_future_end(entry: &NewEntry, now: &Moment) -> Result<()> {
    let clocked = entry.clock.is_some();
    if let Some(end) = Boundary::end(entry, now)?
        && end.timeline(clocked, now)? > Boundary::from_now(now)?.timeline(clocked, now)?
    {
        return Err(Error::Input(
            "A recorded interval must end on or before now".into(),
        ));
    }
    Ok(())
}

pub fn stop(store: &mut Store, at: NaiveDateTime) -> Result<Option<Entry>> {
    stop_at(store, at, &Moment::from_local(at)?)
}

pub fn stop_at(store: &mut Store, at: NaiveDateTime, now: &Moment) -> Result<Option<Entry>> {
    stop_with(store, StopTime::Local(at), now)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopTime {
    Now,
    Local(NaiveDateTime),
    Clock(NaiveTime),
    OffsetMinutes(i64),
}

impl StopTime {
    fn resolve(self, conn: &Connection, clocked: bool, now: &Moment) -> Result<Boundary> {
        let minutes = match self {
            Self::Now => return Boundary::from_now(now),
            Self::Local(at) => return Boundary::new(at, None, now),
            Self::Clock(clock) => {
                let workday =
                    crate::domain::workday::Workday::new(crate::store::day_start_from(conn)?)?;
                if clocked {
                    return Boundary::from_now(&workday.past_clock(clock, now)?);
                }
                let local = workday.civil_clock(clock, now.local()?)?;
                return Boundary::new(local, None, now);
            }
            Self::OffsetMinutes(minutes) => minutes,
        };
        let out_of_range = || Error::Input("Stop time is outside the supported range".into());
        let delta = chrono::Duration::try_minutes(minutes).ok_or_else(out_of_range)?;
        if clocked {
            let utc = now.utc.checked_add_signed(delta).ok_or_else(out_of_range)?;
            return Boundary::from_now(&Moment::at(utc, now.zone));
        }
        let local = crate::model::minute(now.local()?)
            .checked_add_signed(delta)
            .ok_or_else(out_of_range)?;
        Boundary::new(local, None, now)
    }
}

/// Resolve offsets after identifying the timer under the same write lock.
pub fn stop_with(store: &mut Store, at: StopTime, now: &Moment) -> Result<Option<Entry>> {
    store.write(|tx| stop_in_transaction(tx, at, now))
}

pub(crate) fn stop_in_transaction(
    tx: &Transaction<'_>,
    at: StopTime,
    now: &Moment,
) -> Result<Option<Entry>> {
    pomodoro::reconcile_in_transaction(tx, now)?;
    let id = current_id(tx)?;
    let active = id.map(|id| find_entry(tx, id)).transpose()?;
    let clocked = active.as_ref().is_some_and(|entry| entry.clock.is_some());
    let at = at.resolve(tx, clocked, now)?;
    let end = at.timeline(clocked, now)?;
    if end > Boundary::from_now(now)?.timeline(clocked, now)? {
        return Err(Error::Input("Stop time must be on or before now".into()));
    }
    // A stop at the current clock reading must honor already observed focus
    // time if the system clock went backwards. A backdated stop is explicit.
    let explicit = at.utc.filter(|instant| *instant != now.utc);
    pomodoro::interrupt_in_transaction(tx, now, explicit)?;
    let still_running = current_id(tx)?;
    if let Some(entry) = active.filter(|entry| Some(entry.id) == still_running) {
        if end < entry.clock.map_or(entry.start, |c| c.start_utc.naive_utc()) {
            return Err(Error::Input(
                "Stop time precedes the running activity. Use hs edit to correct its start time."
                    .into(),
            ));
        }
        finish_entry(tx, entry.id, at.local, at.utc)?;
    }
    id.map(|id| find_entry(tx, id)).transpose()
}

pub fn resume(store: &mut Store, id: Option<i64>, now: NaiveDateTime) -> Result<Change> {
    resume_at(store, id, &Moment::from_local(now)?)
}

pub fn resume_at(store: &mut Store, id: Option<i64>, now: &Moment) -> Result<Change> {
    store.write(|tx| resume_with_clock(tx, id, None, now))
}

pub(crate) fn resume_in_transaction(
    tx: &Transaction<'_>,
    id: Option<EntryId>,
    now: &Moment,
) -> Result<Change> {
    resume_with_clock(tx, id.map(EntryId::get), Some(clock_at(now)), now)
}

fn resume_with_clock(
    tx: &Transaction<'_>,
    id: Option<i64>,
    clock: Option<EntryClock>,
    now: &Moment,
) -> Result<Change> {
    pomodoro::reconcile_in_transaction(tx, now)?;
    let interrupted = interrupt_manual(tx, now, Boundary::from_now(now)?)?;
    let id = match id {
        Some(id) => id,
        None => latest_completed(tx, now)?.ok_or_else(|| {
            Error::Input("There is no completed entry to resume. Use hs start ACTIVITY.".into())
        })?,
    };
    let prior = find_entry(tx, id)?;
    let mut new = match prior.task {
        Some(link) => tasks::template_from(tx, link.id, now)?,
        None => prior.as_new(),
    };
    new.start = now.local()?;
    new.end = None;
    new.clock = clock;
    let mut result = start_current(tx, new, now, StartMode::ReuseMatching)?;
    result.adjusted.extend(interrupted);
    Ok(result)
}

pub fn edit<F>(store: &mut Store, id: i64, now: NaiveDateTime, change: F) -> Result<Change>
where
    F: FnOnce(&Entry) -> Result<NewEntry>,
{
    edit_at(store, id, &Moment::from_local(now)?, change)
}

/// Apply a caller's edits to the latest row while holding the write transaction.
pub fn edit_at<F>(store: &mut Store, id: i64, now: &Moment, change: F) -> Result<Change>
where
    F: FnOnce(&Entry) -> Result<NewEntry>,
{
    store.write(|tx| apply_entry_edit(tx, id, now, |entry, _| change(entry)))
}

fn apply_entry_edit<F>(tx: &Transaction<'_>, id: i64, now: &Moment, change: F) -> Result<Change>
where
    F: FnOnce(&Entry, u32) -> Result<NewEntry>,
{
    pomodoro::reconcile_in_transaction(tx, now)?;
    pomodoro::entry_changed_in_transaction(tx, id, now)?;
    let original = find_entry(tx, id)?;
    let boundary = crate::store::day_start_from(tx)?;
    let mut replacement = change(&original, boundary)?;
    if original.end.is_some() && replacement.end.is_none() {
        return Err(Error::Input(
            "A completed entry needs an end time. Use hs resume to start another timer.".into(),
        ));
    }
    // Correcting a recorded interval preserves its historical task and quadrant.
    replacement.task = original.task;
    if replacement.clock.is_none() {
        replacement.clock = original.clock;
    }
    normalize_entry(&mut replacement)?;
    validate_entry(&replacement)?;
    check_future_end(&replacement, now)?;
    let start = Boundary::start(&replacement, now)?;
    let clocked = replacement.clock.is_some();
    if original.running
        && start.timeline(clocked, now)? > Boundary::from_now(now)?.timeline(clocked, now)?
    {
        return Err(Error::Input(
            "A running activity cannot start in the future".into(),
        ));
    }
    let times_changed = replacement.start != original.start
        || replacement.end != original.end
        || replacement.clock != original.clock;
    let adjusted = if original.running && replacement.end.is_none() {
        let previous = Boundary::start(&original.as_new(), now)?;
        check_open_interval(tx, start, previous, false, now)?;
        Vec::new()
    } else if times_changed {
        adjust_overlaps(tx, &replacement, Some(id), now)?
    } else {
        Vec::new()
    };
    update_entry(tx, id, &replacement)?;
    if original.running && replacement.end.is_some() {
        crate::storage::mutations::clear_current(tx, id)?;
    }
    activate_vocabulary(tx, &replacement)?;
    Ok(Change {
        entry: find_entry(tx, id)?,
        adjusted,
    })
}

pub fn delete(store: &mut Store, id: i64) -> Result<()> {
    delete_at(store, id, &Moment::system()?)
}

pub fn delete_at(store: &mut Store, id: i64, now: &Moment) -> Result<()> {
    store.write(|tx| delete_in_transaction(tx, EntryId::new(id)?, now))
}

pub(crate) fn delete_in_transaction(tx: &Transaction<'_>, id: EntryId, now: &Moment) -> Result<()> {
    pomodoro::reconcile_in_transaction(tx, now)?;
    pomodoro::entry_changed_in_transaction(tx, id.get(), now)?;
    find_entry(tx, id.get())?;
    crate::storage::mutations::delete_entry(tx, id.get())
}

pub fn complete_task(store: &mut Store, task_id: i64, now: &Moment) -> Result<Task> {
    set_task_status_at(store, task_id, TaskStatus::Done, now)
}

pub fn set_task_status_at(
    store: &mut Store,
    task_id: i64,
    status: TaskStatus,
    now: &Moment,
) -> Result<Task> {
    store.write(|tx| change_task_status_in_transaction(tx, TaskId::new(task_id)?, status, now))
}

pub(crate) fn change_task_status_in_transaction(
    tx: &Transaction<'_>,
    task_id: TaskId,
    status: TaskStatus,
    now: &Moment,
) -> Result<Task> {
    pomodoro::reconcile_in_transaction(tx, now)?;
    let task_id = task_id.get();
    let mut changed_at = *now;
    if status != TaskStatus::Open {
        changed_at.utc = pomodoro::interrupt_task_in_transaction(tx, task_id, now)?;
        if let Some(id) = current_id(tx)? {
            let current = find_entry(tx, id)?;
            if current.task.is_some_and(|link| link.id == task_id) {
                finish_entry(tx, id, now.local()?, Some(now.utc))?;
            }
        }
    }
    tasks::set_status_in_transaction(tx, task_id, status, &changed_at)
}

fn move_start(entry: &mut NewEntry, boundary: Boundary, now: &Moment) -> Result<()> {
    if let Some(clock) = &mut entry.clock {
        clock.start_utc = boundary.required_utc(now)?;
        entry.start = local_at(clock.start_utc, clock.zone)?;
    } else {
        entry.start = boundary.local;
    }
    Ok(())
}

fn adjust_overlaps(
    conn: &Transaction<'_>,
    new: &NewEntry,
    exclude: Option<i64>,
    now: &Moment,
) -> Result<Vec<i64>> {
    let Some(end) = Boundary::end(new, now)? else {
        return Ok(Vec::new());
    };
    let start = Boundary::start(new, now)?;
    let mut adjusted = Vec::new();
    for old in overlap_candidates(conn, start.local, end.local, start.utc, end.utc, false)? {
        if Some(old.id) == exclude {
            continue;
        }
        let clocked = old.clock.is_some();
        let lower = start.timeline(clocked, now)?;
        let upper = end.timeline(clocked, now)?;
        let left = old.clock.map_or(old.start, |c| c.start_utc.naive_utc());
        let right = match (old.end, old.running) {
            (Some(t), _) => old
                .clock
                .and_then(|c| c.end_utc)
                .map_or(t, |t| t.naive_utc()),
            (None, true) => Boundary::from_now(now)?.timeline(clocked, now)?,
            (None, false) => continue,
        };
        if lower >= upper || left >= upper || right <= lower {
            continue;
        }
        if left < lower {
            pomodoro::entry_changed_in_transaction(conn, old.id, now)?;
            if !old.running && right > upper {
                let mut tail = old.as_new();
                move_start(&mut tail, end, now)?;
                let tail_id = insert_entry(conn, &tail)?;
                // Both retained pieces still belong to the same focus phase.
                // The inserted interruption is an ordinary, unrelated entry.
                crate::storage::mutations::copy_segment_link(conn, old.id, tail_id)?;
            }
            finish_entry(conn, old.id, start.local, start.utc)?;
        } else if old.running || right > upper {
            pomodoro::entry_changed_in_transaction(conn, old.id, now)?;
            // The hook can close an owned running entry. Keep that closed end
            // when moving its start; interrupted focus must never restart here.
            let mut trimmed = find_entry(conn, old.id)?.as_new();
            move_start(&mut trimmed, end, now)?;
            update_entry(conn, old.id, &trimmed)?;
        } else {
            // Hamster retains existing intervals fully enclosed by the new interval.
            continue;
        }
        adjusted.push(old.id);
    }
    Ok(adjusted)
}

#[cfg(test)]
#[path = "task_tracking_tests.rs"]
mod task_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_entry;
    use crate::pomodoro::{Config, State};
    use chrono_tz::Tz;
    fn now() -> NaiveDateTime {
        crate::store::parse_datetime("2026-11-23 12:00:00").unwrap()
    }
    #[test]
    fn split_retains_tags_and_switch_is_persistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hs.db");
        let mut store = Store::open(&path).unwrap();
        add(
            &mut store,
            parse_entry("09:00-11:00 Development@Work, #code", now(), 0).unwrap(),
            now(),
        )
        .unwrap();
        add(
            &mut store,
            parse_entry("10:00-10:15 Meeting@Work", now(), 0).unwrap(),
            now(),
        )
        .unwrap();
        let records = store.entries().unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].tags, vec!["code"]);
        assert_eq!(records[2].tags, vec!["code"]);
        start(&mut store, parse_entry("Lunch", now(), 0).unwrap(), now()).unwrap();
        drop(store);
        let mut reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.current().unwrap().unwrap().activity, "Lunch");
        assert!(stop(&mut reopened, now()).unwrap().is_some());
        assert!(stop(&mut reopened, now()).unwrap().is_none());
    }

    #[test]
    fn invalid_edit_rolls_back_and_unresolved_history_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let unresolved = parse_entry("08:00 Old", now(), 0).unwrap();
        let tx = store.conn.transaction().unwrap();
        let old_id = insert_entry(&tx, &unresolved).unwrap();
        tx.commit().unwrap();
        let change = start(&mut store, parse_entry("Work", now(), 0).unwrap(), now()).unwrap();
        let before = store.entries().unwrap();
        let result = edit(&mut store, change.entry.id, now(), |old| {
            let mut replacement = old.as_new();
            replacement.end = Some(old.start - chrono::Duration::minutes(5));
            Ok(replacement)
        });
        assert!(result.is_err());
        assert_eq!(before, store.entries().unwrap());
        assert!(store.entry(old_id).unwrap().end.is_none());
        assert!(!store.entry(old_id).unwrap().running);
    }

    #[test]
    fn resume_prefers_latest_end_then_latest_id_for_overlapping_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        for text in [
            "10:00-10:30 Call",
            "09:00-11:00 Writing",
            "08:00-11:00 Review",
        ] {
            add(&mut store, parse_entry(text, now(), 0).unwrap(), now()).unwrap();
        }
        let resumed = resume(&mut store, None, now()).unwrap();
        assert_eq!(resumed.entry.activity, "Review");
        assert_eq!(resumed.entry.start, now());
        assert!(resumed.entry.running);
    }

    #[test]
    fn editing_a_running_start_checks_only_the_newly_claimed_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let paid = add(
            &mut store,
            parse_entry("10:00-10:30 Paid call", now(), 0).unwrap(),
            now(),
        )
        .unwrap()
        .entry;
        let open = start(
            &mut store,
            parse_entry("11:00 Open", now(), 0).unwrap(),
            now(),
        )
        .unwrap()
        .entry;
        let imported = parse_entry("11:15-11:45 Imported", now(), 0).unwrap();
        let tx = store.conn.transaction().unwrap();
        let imported_id = insert_entry(&tx, &imported).unwrap();
        tx.commit().unwrap();
        let before = store.entries().unwrap();
        let result = edit(&mut store, open.id, now(), |_| {
            parse_entry("09:00 Open", now(), 0)
        });
        assert!(matches!(result, Err(Error::Conflict(_))));
        assert_eq!(store.entries().unwrap(), before);

        let result = edit(&mut store, open.id, now(), |_| {
            parse_entry("10:30 Open", now(), 0)
        })
        .unwrap();
        assert!(result.entry.running);
        assert!(result.adjusted.is_empty());
        assert_eq!(store.entry(imported_id).unwrap().as_new(), imported);
        let bounded = edit(&mut store, open.id, now(), |_| {
            parse_entry("10:15-11:45 Open", now(), 0)
        })
        .unwrap();
        assert_eq!(bounded.adjusted, [paid.id]);
        assert!(!bounded.entry.running);
    }

    fn moment(utc: &str, zone: Tz) -> Moment {
        Moment::at(utc.parse().unwrap(), zone)
    }

    fn focus(store: &mut Store, now: &Moment) -> i64 {
        pomodoro::start(
            store,
            parse_entry("Design@Work, #client", now.local().unwrap(), 0).unwrap(),
            Config {
                focus_seconds: 7_200,
                ..Config::default()
            },
            now,
        )
        .unwrap()
        .entry_id()
        .unwrap()
    }

    #[test]
    fn manual_start_with_identical_details_ends_pomodoro_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let begun = moment("2026-03-10T09:00:00Z", chrono_tz::UTC);
        let switched = moment("2026-03-10T09:05:12Z", chrono_tz::UTC);
        let template = parse_entry("Design@Work, #client", begun.local().unwrap(), 0).unwrap();
        let ordinary = start_at(&mut store, template.clone(), &begun)
            .unwrap()
            .entry;
        let id = focus(&mut store, &begun);
        assert_ne!(ordinary.id, id);
        assert_eq!(
            store.entry(ordinary.id).unwrap().end,
            Some(begun.local().unwrap())
        );
        let mut manual = template;
        manual.start = switched.local().unwrap();
        let change = start_at(&mut store, manual, &switched).unwrap();
        assert_ne!(change.entry.id, id);
        assert!(change.entry.running);
        assert!(change.entry.clock.is_none());
        assert_eq!(change.adjusted, [id]);
        assert_eq!(
            store.entry(id).unwrap().clock.unwrap().end_utc,
            Some(switched.utc)
        );
        let session = pomodoro::status(&mut store, &switched).unwrap().unwrap();
        assert_eq!(session.state(), State::Stopped);
        assert_eq!(session.interrupted_focus(), 1);
    }

    #[test]
    fn failed_edit_rolls_back_pomodoro_interruption_and_valid_edit_closes_at_now() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let begun = moment("2026-03-10T09:00:00Z", chrono_tz::UTC);
        let changed = moment("2026-03-10T09:04:13Z", chrono_tz::UTC);
        let id = focus(&mut store, &begun);
        assert!(
            edit_at(&mut store, id, &changed, |_| Err(Error::Input(
                "invalid edit".into()
            )))
            .is_err()
        );
        let session = pomodoro::status(&mut store, &changed).unwrap().unwrap();
        assert_eq!(
            (
                session.state(),
                session.interrupted_focus(),
                session.entry_id()
            ),
            (State::Running, 0, Some(id))
        );
        assert!(store.entry(id).unwrap().running);
        let result = edit_at(&mut store, id, &changed, |row| {
            assert!(!row.running);
            let mut corrected = row.as_new();
            corrected.description = "Corrected detail".into();
            Ok(corrected)
        })
        .unwrap();
        assert!(!result.entry.running);
        assert_eq!(result.entry.description, "Corrected detail");
        assert_eq!(result.entry.clock.unwrap().end_utc, Some(changed.utc));
        assert_eq!(
            pomodoro::status(&mut store, &changed)
                .unwrap()
                .unwrap()
                .state(),
            State::Stopped
        );
    }

    #[test]
    fn bounded_add_only_interrupts_if_the_owned_entry_changes_and_uses_its_zone() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let begun = moment("2026-01-10T09:00:00Z", chrono_tz::UTC);
        let away = moment("2026-01-10T10:00:00Z", chrono_tz::Europe::Zagreb);
        let id = focus(&mut store, &begun);
        let before = parse_entry("09:00-10:00 Morning", away.local().unwrap(), 0).unwrap();
        add_at(&mut store, before, &away).unwrap();
        assert_eq!(store.current().unwrap().unwrap().id, id);
        let inside = parse_entry("10:30-10:45 Coffee", away.local().unwrap(), 0).unwrap();
        assert_eq!(add_at(&mut store, inside, &away).unwrap().adjusted, [id]);
        let interrupted = store.entry(id).unwrap();
        let expected = moment("2026-01-10T09:30:00Z", chrono_tz::UTC);
        assert_eq!(interrupted.end, Some(expected.local().unwrap()));
        assert_eq!(interrupted.clock.unwrap().end_utc, Some(expected.utc));
        assert!(store.current().unwrap().is_none());
        assert_eq!(
            pomodoro::status(&mut store, &away)
                .unwrap()
                .unwrap()
                .interrupted_focus(),
            1
        );
    }

    #[test]
    fn manual_stop_during_pause_is_committed_with_no_current_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let begun = moment("2026-03-10T09:00:00Z", chrono_tz::UTC);
        let paused = moment("2026-03-10T09:02:36Z", chrono_tz::UTC);
        let stopped = moment("2026-03-10T09:03:00Z", chrono_tz::UTC);
        let id = focus(&mut store, &begun);
        pomodoro::pause(&mut store, &paused).unwrap();
        assert!(
            stop_at(&mut store, stopped.local().unwrap(), &stopped)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.entry(id).unwrap().clock.unwrap().end_utc,
            Some(paused.utc)
        );
        assert_eq!(
            pomodoro::status(&mut store, &stopped)
                .unwrap()
                .unwrap()
                .state(),
            State::Stopped
        );
    }

    #[test]
    fn bare_stop_clocks_choose_the_past_occurrence_on_the_current_focus_workday() {
        let zone = chrono_tz::Europe::Zagreb;
        // The current 02:10 CET is later than the workday's 02:30 CEST start.
        let now = moment("2026-10-25T01:10:50Z", zone);
        for (requested, expected_utc) in [
            ("02:40", "2026-10-25T00:40:00Z"),
            ("02:05", "2026-10-25T01:05:00Z"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
            store.set_day_start(150).unwrap();
            let id = focus(&mut store, &moment("2026-10-25T00:30:00Z", zone));
            let clock = |value| StopTime::Clock(NaiveTime::parse_from_str(value, "%H:%M").unwrap());
            assert!(stop_with(&mut store, clock("01:59"), &now).is_err());
            assert_eq!(store.current().unwrap().unwrap().id, id);
            let closed = stop_with(&mut store, clock(requested), &now)
                .unwrap()
                .unwrap();
            assert_eq!(
                closed.clock.unwrap().end_utc,
                Some(moment(expected_utc, zone).utc)
            );
        }
    }

    #[test]
    fn relative_stop_counts_elapsed_minutes_through_both_zagreb_clock_changes() {
        let zone = chrono_tz::Europe::Zagreb;
        // Spring jumps from 01:59 to 03:00. Autumn repeats the 02:00 hour.
        for (now_utc, minutes) in [
            ("2026-03-29T01:01:37Z", -2),
            ("2026-10-25T01:01:37Z", -2),
            ("2026-10-25T01:15:37Z", -60),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
            let now = moment(now_utc, zone);
            let begun = Moment::at(now.utc - chrono::Duration::minutes(75), zone);
            let id = focus(&mut store, &begun);
            for invalid in [1, i64::MAX, i64::MIN] {
                assert!(stop_with(&mut store, StopTime::OffsetMinutes(invalid), &now).is_err());
                assert_eq!(store.current().unwrap().unwrap().id, id);
            }
            let closed = stop_with(&mut store, StopTime::OffsetMinutes(minutes), &now)
                .unwrap()
                .unwrap();
            let expected = Moment::at(now.utc + chrono::Duration::minutes(minutes), zone);
            assert_eq!(closed.clock.unwrap().end_utc, Some(expected.utc));
            assert_eq!(closed.end, Some(expected.local().unwrap()));
            assert_eq!(
                pomodoro::statistics(&mut store, &now)
                    .unwrap()
                    .focus_seconds,
                (expected.utc - begun.utc).num_seconds()
            );
        }
    }

    #[test]
    fn relative_stop_preserves_civil_minute_rules_and_now_preserves_seconds() {
        for request in [StopTime::OffsetMinutes(-2), StopTime::Now] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
            let now = moment("2026-03-29T01:01:37Z", chrono_tz::Europe::Zagreb);
            let entry = parse_entry("01:00 Work", now.local().unwrap(), 0).unwrap();
            start_at(&mut store, entry, &now).unwrap();
            let closed = stop_with(&mut store, request, &now).unwrap().unwrap();
            let expected = if request == StopTime::Now {
                now.local().unwrap()
            } else {
                crate::store::parse_datetime("2026-03-29 02:59:00").unwrap()
            };
            assert_eq!(closed.end, Some(expected));
            assert!(closed.clock.is_none());
        }
    }

    #[test]
    fn ordinary_stop_now_honors_observed_focus_but_an_explicit_stop_can_be_backdated() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let at = |seconds| {
            let origin = moment("2026-03-10T09:00:00Z", chrono_tz::UTC);
            Moment::at(origin.utc + chrono::Duration::seconds(seconds), origin.zone)
        };
        focus(&mut store, &at(0));
        pomodoro::status(&mut store, &at(40)).unwrap();
        let rolled_back = at(-10);
        let closed = stop_at(&mut store, rolled_back.local().unwrap(), &rolled_back)
            .unwrap()
            .unwrap();
        assert_eq!(closed.clock.unwrap().end_utc, Some(at(40).utc));
        assert_eq!(
            pomodoro::statistics(&mut store, &rolled_back)
                .unwrap()
                .focus_seconds,
            40
        );

        focus(&mut store, &at(60));
        pomodoro::status(&mut store, &at(100)).unwrap();
        let corrected = stop_at(&mut store, at(75).local().unwrap(), &at(110))
            .unwrap()
            .unwrap();
        assert_eq!(corrected.clock.unwrap().end_utc, Some(at(75).utc));
        assert_eq!(
            pomodoro::statistics(&mut store, &at(110))
                .unwrap()
                .focus_seconds,
            55
        );
    }

    #[test]
    fn late_manual_resume_closes_expired_focus_at_deadline_then_starts_unclocked_work() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let begun = moment("2026-03-10T09:00:00Z", chrono_tz::Europe::Zagreb);
        let later = moment("2026-03-10T12:15:12Z", chrono_tz::America::New_York);
        let id = focus(&mut store, &begun);
        let change = resume_at(&mut store, None, &later).unwrap();
        assert_ne!(change.entry.id, id);
        assert_eq!(change.entry.start, later.local().unwrap());
        assert!(change.entry.clock.is_none());
        assert!(change.entry.running);
        let original = store.entry(id).unwrap().clock.unwrap();
        assert_eq!(
            original.end_utc,
            Some(begun.utc + chrono::Duration::hours(2))
        );
        let session = pomodoro::status(&mut store, &later).unwrap().unwrap();
        assert_eq!(
            (
                session.state(),
                session.completed_focus(),
                session.interrupted_focus()
            ),
            (State::Stopped, 1, 0)
        );
    }

    #[test]
    fn manual_stop_and_split_across_fold_keep_utc_and_original_zone() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let zone = chrono_tz::Europe::Zagreb;
        let before_fold = moment("2026-10-25T00:50:00Z", zone);
        let after_fold = moment("2026-10-25T01:10:00Z", zone);
        let id = focus(&mut store, &before_fold);
        let ended = stop_at(&mut store, after_fold.local().unwrap(), &after_fold)
            .unwrap()
            .unwrap();
        assert_eq!(ended.id, id);
        assert!(ended.end.unwrap() < ended.start);
        assert_eq!(ended.clock.unwrap().end_utc, Some(after_fold.utc));
        let later = moment("2026-10-25T01:20:00Z", chrono_tz::UTC);
        let insert = parse_entry("01:01-01:05 Water", later.local().unwrap(), 0).unwrap();
        let change = add_at(&mut store, insert, &later).unwrap();
        assert_eq!(change.adjusted, [id]);
        let first = store.entry(id).unwrap();
        let second = store
            .entries()
            .unwrap()
            .into_iter()
            .find(|e| e.id != id && e.clock.is_some())
            .unwrap();
        let split_start = moment("2026-10-25T01:01:00Z", zone);
        let split_end = moment("2026-10-25T01:05:00Z", zone);
        let left = first.clock.unwrap();
        let right = second.clock.unwrap();
        assert_eq!(
            (left.start_utc, left.end_utc, left.zone),
            (before_fold.utc, Some(split_start.utc), zone)
        );
        assert_eq!(
            (right.start_utc, right.end_utc, right.zone),
            (split_end.utc, Some(after_fold.utc), zone)
        );
        assert_eq!(first.end, Some(split_start.local().unwrap()));
        assert_eq!(second.start, split_end.local().unwrap());
        let linked: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM pomodoro_segments WHERE phase_id=
             (SELECT phase_id FROM pomodoro_segments WHERE entry_id=?1) AND entry_id IN (?1,?2)",
                rusqlite::params![first.id, second.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(linked, 2);
        assert_eq!(
            pomodoro::statistics(&mut store, &later)
                .unwrap()
                .focus_seconds,
            16 * 60
        );
    }

    #[test]
    fn editing_saved_focus_from_another_zone_preserves_unrelated_wall_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let tokyo = chrono_tz::Asia::Tokyo;
        let begun = moment("2026-03-10T08:00:00Z", tokyo);
        let stopped = moment("2026-03-10T08:25:00Z", tokyo);
        let later = moment("2026-03-10T19:00:00Z", chrono_tz::UTC);
        let id = focus(&mut store, &begun);
        stop_at(&mut store, stopped.local().unwrap(), &stopped).unwrap();
        let evening = parse_entry("17:00-18:00 Dinner", later.local().unwrap(), 0).unwrap();
        let dinner = add_at(&mut store, evening, &later).unwrap().entry;
        // The saved 17:00 Tokyo entry was 08:00 in the current zone. Changing
        // its original wall endpoints must leave today's 17:00 dinner intact.
        let change = edit_at(&mut store, id, &later, |row| {
            let mut entry = row.as_new();
            entry.start += chrono::Duration::minutes(5);
            entry.end = Some(stopped.local().unwrap() - chrono::Duration::minutes(5));
            Ok(entry)
        })
        .unwrap();
        assert!(change.adjusted.is_empty());
        assert_eq!(store.entry(dinner.id).unwrap(), dinner);
        assert_eq!(store.entries().unwrap().len(), 2);
    }
}
