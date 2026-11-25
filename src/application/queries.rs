//! Coherent read models for command-line views. A reader without a running
//! Pomodoro uses a deferred transaction. A live timer is reconciled under the
//! writer lock before the view is materialized, so no view mixes generations.
use std::collections::BTreeMap;

use rusqlite::{Transaction, TransactionBehavior};

use crate::{
    Error, Result, Store,
    domain::{
        tasks::{Task, TaskStatus},
        workday::{Period, Workday, WorkdayRange},
    },
    model::Entry,
    pomodoro::{self, Session, State},
    report,
    storage::{entries as entry_queries, tasks as task_queries},
    store,
    time::Moment,
};

#[derive(Debug)]
pub struct Status {
    pub current: Option<Entry>,
    pub session: Option<Session>,
    /// Calculation cutoff, clamped to a live session's last observed instant.
    pub as_of: Moment,
}

#[derive(Debug)]
pub struct History {
    pub entries: Vec<Entry>,
    pub day_start: u32,
    pub as_of: Moment,
}

#[derive(Debug)]
pub struct PeriodHistory {
    pub entries: Vec<Entry>,
    pub range: WorkdayRange,
    pub as_of: Moment,
}

#[derive(Debug)]
pub struct Tasks {
    /// Selected planning records in quadrant/deadline/ID order.
    pub tasks: Vec<Task>,
    pub recorded_seconds: BTreeMap<i64, i64>,
    pub status: Status,
}

/// Available continuation for the suggested task, using the same snapshot as its timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAction {
    Start,
    Tracking,
    Watch,
    Resume,
    NextPhase,
    StopOtherPomodoro,
}
impl TaskAction {
    pub fn for_task(view: &Status, task: &Task) -> Self {
        if let Some(session) = &view.session {
            let action = match session.state() {
                State::Running => Some(Self::Watch),
                State::Paused => Some(Self::Resume),
                State::Ready => Some(Self::NextPhase),
                State::Stopped => None,
            };
            if let Some(action) = action {
                return if session.task_id == Some(task.id) {
                    action
                } else {
                    Self::StopOtherPomodoro
                };
            }
        }
        if view
            .current
            .as_ref()
            .is_some_and(|entry| entry.task.is_some_and(|link| link.id == task.id))
        {
            Self::Tracking
        } else {
            Self::Start
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Tracking => "tracking",
            Self::Watch => "pomo_watch",
            Self::Resume => "pomo_resume",
            Self::NextPhase => "pomo_next",
            Self::StopOtherPomodoro => "stop_other_pomodoro",
        }
    }
}

pub(crate) fn read<T>(
    store: &mut Store,
    now: &Moment,
    materialize: impl FnOnce(&Transaction<'_>, Option<Session>, Moment) -> Result<T>,
) -> Result<T> {
    now.local()?;
    let tx = store.conn.transaction()?;
    let session = pomodoro::load(&tx, false)?;
    if session
        .as_ref()
        .is_some_and(|session| session.state() == State::Running)
    {
        // Avoid a read-to-write lock upgrade, which can deadlock two callers.
        tx.rollback()?;
        let tx = store
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        pomodoro::reconcile_in_transaction(&tx, now)?;
        let session = pomodoro::load(&tx, false)?;
        finish(tx, session, now, materialize)
    } else {
        finish(tx, session, now, materialize)
    }
}

fn finish<T>(
    tx: Transaction<'_>,
    session: Option<Session>,
    now: &Moment,
    materialize: impl FnOnce(&Transaction<'_>, Option<Session>, Moment) -> Result<T>,
) -> Result<T> {
    let cutoff = session
        .as_ref()
        .filter(|session| session.state() != State::Stopped)
        .map_or(now.utc, |session| now.utc.max(session.last_observed()));
    let as_of = Moment::at(cutoff, now.zone);
    as_of.local()?;
    let result = materialize(&tx, session, as_of)?;
    tx.commit()?;
    Ok(result)
}

pub fn status(store: &mut Store, now: &Moment) -> Result<Status> {
    read(store, now, |tx, session, as_of| {
        let current = store::current_id(tx)?
            .map(|id| store::find_entry(tx, id))
            .transpose()?;
        Ok(Status {
            current,
            session,
            as_of,
        })
    })
}

pub fn history(store: &mut Store, now: &Moment) -> Result<History> {
    read(store, now, |tx, _, as_of| {
        Ok(History {
            entries: store::load_entries(tx)?,
            day_start: store::day_start_from(tx)?,
            as_of,
        })
    })
}

/// Select workday bounds and records from the same observation. Only All
/// needs the earliest and latest entries to determine its own range.
pub fn history_for_period(
    store: &mut Store,
    period: Period,
    now: &Moment,
) -> Result<PeriodHistory> {
    read(store, now, |tx, _, as_of| {
        let workday = Workday::new(store::day_start_from(tx)?)?;
        if period == Period::All {
            let entries = entry_queries::load_entries(tx)?;
            // The outer workday labels include every entry's origin. Reuse
            // this allocation directly for full-history reports and exports.
            let range = workday.range(period, &entries, now, &as_of)?;
            return Ok(PeriodHistory {
                entries,
                range,
                as_of,
            });
        }
        let range = workday.range(period, &[], now, &as_of)?;
        let candidates = entry_queries::for_history_range(tx, &range)?;
        let mut entries = Vec::new();
        for entry in candidates {
            if report::in_range_at(&entry, &range, &as_of)? {
                entries.push(entry);
            }
        }
        Ok(PeriodHistory {
            entries,
            range,
            as_of,
        })
    })
}

/// All planning records and their work totals, as of a single committed view.
pub fn tasks(store: &mut Store, now: &Moment) -> Result<Tasks> {
    task_list(store, None, now)
}

pub fn task_list(store: &mut Store, status: Option<TaskStatus>, now: &Moment) -> Result<Tasks> {
    read(store, now, |tx, session, as_of| {
        let tasks = task_queries::list_from(tx, status)?;
        let entries = entry_queries::for_task_status(tx, status)?;
        task_view(tx, tasks, entries, session, as_of)
    })
}

/// The requested task and only the recorded work attached to its ID.
pub fn task_detail(store: &mut Store, id: i64, now: &Moment) -> Result<Tasks> {
    read(store, now, |tx, session, as_of| {
        let task = task_queries::load_from(tx, id)?;
        let entries = entry_queries::for_task(tx, id)?;
        task_view(tx, vec![task], entries, session, as_of)
    })
}

/// The first open Do or Plan task in priority/deadline order, when available.
pub fn next_task(store: &mut Store, now: &Moment) -> Result<Tasks> {
    read(store, now, |tx, session, as_of| {
        let task = task_queries::next_from(tx)?;
        let entries = match &task {
            Some(task) => entry_queries::for_task(tx, task.id)?,
            None => Vec::new(),
        };
        task_view(tx, task.into_iter().collect(), entries, session, as_of)
    })
}

fn task_view(
    tx: &Transaction<'_>,
    tasks: Vec<Task>,
    entries: Vec<Entry>,
    session: Option<Session>,
    as_of: Moment,
) -> Result<Tasks> {
    let mut recorded_seconds = BTreeMap::<i64, i64>::new();
    let mut current = None;
    for entry in entries {
        if let Some(task) = entry.task
            && let Some(seconds) = report::entry_seconds_at(&entry, &as_of)?
        {
            let total = recorded_seconds.entry(task.id).or_default();
            *total = total.checked_add(seconds).ok_or_else(|| {
                Error::Input("Recorded task time exceeds the supported range".into())
            })?;
        }
        if entry.running {
            current = Some(entry);
        }
    }
    // Status remains global even when the displayed tasks exclude the active
    // entry. At most one unrelated record is needed for the timer indicator.
    if current.is_none() {
        current = store::current_id(tx)?
            .map(|id| store::find_entry(tx, id))
            .transpose()?;
    }
    Ok(Tasks {
        tasks,
        recorded_seconds,
        status: Status {
            current,
            session,
            as_of,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        parse::parse_live_entry,
        tasks::{self as task_store, TaskInput, TaskStatus},
        tracking,
    };

    fn at(seconds: i64) -> Moment {
        Moment::at(
            "2026-11-23T10:00:00Z"
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap()
                + chrono::Duration::seconds(seconds),
            chrono_tz::UTC,
        )
    }

    #[test]
    fn ordinary_views_remain_readable_during_a_reserved_writer_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hs.db");
        let mut store = Store::open(&path).unwrap();
        tracking::start_at(
            &mut store,
            parse_live_entry("Work", at(0).local().unwrap(), 0).unwrap(),
            &at(0),
        )
        .unwrap();
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(
            status(&mut store, &at(20))
                .unwrap()
                .current
                .unwrap()
                .activity,
            "Work"
        );
        assert_eq!(history(&mut store, &at(20)).unwrap().entries.len(), 1);
        assert!(tasks(&mut store, &at(20)).unwrap().tasks.is_empty());
        writer.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn task_view_freezes_observed_time_and_preserves_its_own_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hs.db");
        let mut store = Store::open(&path).unwrap();
        let task = task_store::add(
            &mut store,
            TaskInput {
                title: "Submit".into(),
                activity: parse_live_entry("Writing", at(0).local().unwrap(), 0)
                    .unwrap()
                    .into(),
                quadrant: Some(task_store::Quadrant::Plan),
                due_date: None,
            },
            &at(0),
        )
        .unwrap();
        pomodoro::start_task(&mut store, task.id, pomodoro::Config::default(), &at(0)).unwrap();
        status(&mut store, &at(40)).unwrap();
        let before = task_detail(&mut store, task.id, &at(20)).unwrap();
        assert_eq!(before.recorded_seconds[&task.id], 40);
        assert_eq!(before.status.as_of, at(40));
        let history = history(&mut store, &at(20)).unwrap();
        assert_eq!(
            report::entry_seconds_at(&history.entries[0], &history.as_of).unwrap(),
            Some(40)
        );

        let mut writer = Store::open(&path).unwrap();
        tracking::complete_task(&mut writer, task.id, &at(50)).unwrap();
        assert_eq!(before.tasks[0].status, TaskStatus::Open);
        assert!(before.status.current.is_some());
        let after = task_list(&mut store, Some(TaskStatus::Done), &at(50)).unwrap();
        assert_eq!(after.tasks[0].status, TaskStatus::Done);
        assert!(after.status.current.is_none());
        assert_eq!(after.recorded_seconds[&task.id], 50);
    }

    #[test]
    fn focused_task_queries_skip_excluded_history_and_preserve_global_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let mut capture = |title: &str, quadrant| {
            task_store::add(
                &mut store,
                TaskInput {
                    title: title.into(),
                    activity: parse_live_entry("Writing", at(0).local().unwrap(), 0)
                        .unwrap()
                        .into(),
                    quadrant: Some(quadrant),
                    due_date: None,
                },
                &at(0),
            )
            .unwrap()
        };
        let next = capture("Send", task_store::Quadrant::Do);
        let later = capture("Draft", task_store::Quadrant::Plan);
        let closed = capture("Done", task_store::Quadrant::Do);
        for (index, task) in [&next, &later, &closed].into_iter().enumerate() {
            let from = at(120 * index as i64);
            let to = at(120 * index as i64 + 60);
            tracking::start_task_at(&mut store, task.id, &from).unwrap();
            tracking::stop_at(&mut store, to.local().unwrap(), &to).unwrap();
        }
        let now = at(600);
        tracking::complete_task(&mut store, closed.id, &now).unwrap();
        let active = tracking::start_at(
            &mut store,
            parse_live_entry("Other work", now.local().unwrap(), 0).unwrap(),
            &now,
        )
        .unwrap()
        .entry;
        // A stored malformed date proves the excluded row wasn't deserialized.
        store
            .conn
            .execute(
                "UPDATE entries SET start_time='' WHERE task_id=?",
                [closed.id],
            )
            .unwrap();
        assert!(tasks(&mut store, &now).is_err());
        let open = task_list(&mut store, Some(TaskStatus::Open), &now).unwrap();
        assert_eq!(open.tasks, [next.clone(), later.clone()]);
        assert_eq!(
            open.recorded_seconds,
            BTreeMap::from([(next.id, 60), (later.id, 60)])
        );
        assert_eq!(open.status.current.as_ref(), Some(&active));
        // Even a malformed task outside the selected status does not block the matrix.
        store
            .conn
            .execute(
                "UPDATE tasks SET due_date='invalid' WHERE id=?",
                [closed.id],
            )
            .unwrap();
        assert!(task_list(&mut store, Some(TaskStatus::Open), &now).is_ok());

        store
            .conn
            .execute(
                "UPDATE entries SET start_time='' WHERE task_id=?",
                [later.id],
            )
            .unwrap();
        assert!(task_list(&mut store, Some(TaskStatus::Open), &now).is_err());
        let detail = task_detail(&mut store, next.id, &now).unwrap();
        assert_eq!(detail.tasks.as_slice(), std::slice::from_ref(&next));
        assert_eq!(detail.recorded_seconds, BTreeMap::from([(next.id, 60)]));
        let suggestion = next_task(&mut store, &now).unwrap();
        assert_eq!(suggestion.tasks, [next]);
        assert_eq!(suggestion.recorded_seconds, detail.recorded_seconds);
        assert_eq!(suggestion.status.current, Some(active));
        assert!(matches!(
            task_detail(&mut store, 999, &now),
            Err(Error::Input(_))
        ));
    }

    #[test]
    fn bounded_history_matches_full_history_at_zone_extremes_gaps_and_folds() {
        use crate::model::{EntryClock, NewEntry};
        use chrono::{DateTime, Duration, Utc};

        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("periods.db")).unwrap();
        store.set_day_start(150).unwrap();
        let transitions: [DateTime<Utc>; 2] = [
            "2026-03-29T01:00:00Z".parse().unwrap(),
            "2026-10-25T01:00:00Z".parse().unwrap(),
        ];
        let template = parse_live_entry("Journal #log", at(0).local().unwrap(), 0).unwrap();
        store::write_fixture(&store.conn, |tx| {
            let unresolved = NewEntry {
                start: "2025-01-01T10:00:00".parse().unwrap(),
                end: None,
                ..template.clone()
            };
            store::insert_entry(tx, &unresolved)?;
            let mut active = template.clone();
            active.start = "2026-10-25T02:25:00".parse().unwrap();
            let id = store::insert_entry(tx, &active)?;
            tx.execute(
                "UPDATE current_entry SET entry_id=? WHERE singleton=1",
                [id],
            )?;
            for transition in transitions {
                for zone in [
                    chrono_tz::Europe::Zagreb,
                    chrono_tz::Pacific::Kiritimati,
                    chrono_tz::America::Adak,
                    chrono_tz::Asia::Kathmandu,
                ] {
                    for minutes in [-1800, -900, -60, -30, -1, 0, 1, 30, 60, 900, 1800] {
                        let start = transition + Duration::minutes(minutes);
                        let end = start + Duration::minutes(if minutes == 0 { 0 } else { 40 });
                        let row = NewEntry {
                            start: Moment::at(start, zone).local()?,
                            end: Some(Moment::at(end, zone).local()?),
                            clock: Some(EntryClock {
                                start_utc: start,
                                end_utc: Some(end),
                                zone,
                            }),
                            ..template.clone()
                        };
                        store::insert_entry(tx, &row)?;
                    }
                }
                // V1 also accepted timestamps written to minute precision.
                let short = transition.format("%Y-%m-%d 02:30").to_string();
                let id = store::insert_entry(tx, &template)?;
                tx.execute(
                    "UPDATE entries SET start_time=?1, end_time=?1 WHERE id=?2",
                    rusqlite::params![short, id],
                )?;
            }
            Ok(())
        })
        .unwrap();
        for change in transitions {
            let now = Moment::at(change + Duration::hours(16), chrono_tz::Europe::Zagreb);
            let all = history(&mut store, &now).unwrap();
            let day = now.local().unwrap().date();
            let workday = Workday::new(all.day_start).unwrap();
            for period in [
                Period::Today,
                Period::Week,
                Period::Month,
                Period::All,
                Period::Dates {
                    from: day.pred_opt().unwrap(),
                    to: day.pred_opt(),
                },
                Period::Dates {
                    from: day.succ_opt().unwrap(),
                    to: day.succ_opt(),
                },
            ] {
                let range = workday
                    .range(period, &all.entries, &now, &all.as_of)
                    .unwrap();
                let expected: Vec<_> = all
                    .entries
                    .iter()
                    .filter(|entry| report::in_range_at(entry, &range, &all.as_of).unwrap())
                    .collect();
                let actual = history_for_period(&mut store, period, &now).unwrap();
                assert_eq!(actual.range, range);
                assert_eq!(
                    actual.entries.iter().collect::<Vec<_>>(),
                    expected,
                    "{period:?} at {change}"
                );
            }
            assert!(
                history_for_period(
                    &mut store,
                    Period::Dates {
                        from: day,
                        to: day.pred_opt()
                    },
                    &now
                )
                .is_err()
            );
        }

        // A malformed far-future row cannot affect the parsing of a bounded
        // report. The all-history adapter still reports the underlying error.
        store::write_fixture(&store.conn, |tx| {
            let id = store::insert_entry(tx, &template)?;
            tx.execute(
                "UPDATE entries SET start_time='unreadable',end_time='unreadable' WHERE id=?",
                [id],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(history_for_period(&mut store, Period::Today, &at(0)).is_ok());
        assert!(history_for_period(&mut store, Period::All, &at(0)).is_err());
    }

    #[test]
    fn overdue_focus_is_capped_before_history_is_materialized() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        pomodoro::start(
            &mut store,
            parse_live_entry("Work", at(0).local().unwrap(), 0).unwrap(),
            pomodoro::Config {
                focus_seconds: 30,
                ..Default::default()
            },
            &at(0),
        )
        .unwrap();
        let view = history(&mut store, &at(900)).unwrap();
        assert_eq!(view.entries[0].clock.unwrap().end_utc, Some(at(30).utc));
        assert!(!view.entries[0].running);
        assert_eq!(
            status(&mut store, &at(900))
                .unwrap()
                .session
                .unwrap()
                .state(),
            State::Ready
        );
    }
}
