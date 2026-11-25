use super::*;
use crate::{
    model::EntryTask,
    parse::parse_entry,
    pomodoro::{Config, Phase, State},
    tasks::{Quadrant, TaskInput},
};
use chrono::Duration;
use chrono::{DateTime, Utc};

fn at(seconds: i64) -> Moment {
    let origin: DateTime<Utc> = "2026-03-10T09:00:00Z".parse().unwrap();
    Moment::at(origin + Duration::seconds(seconds), chrono_tz::UTC)
}

fn task(store: &mut Store, quadrant: Option<Quadrant>) -> i64 {
    let input = TaskInput {
        title: "Ship change".into(),
        activity: parse_entry(
            "Design@Work, initial note, #client",
            at(0).local().unwrap(),
            0,
        )
        .unwrap()
        .into(),
        quadrant,
        due_date: None,
    };
    tasks::add(store, input, &at(0)).unwrap().id
}

fn link(id: i64, quadrant: Option<Quadrant>) -> Option<EntryTask> {
    Some(EntryTask { id, quadrant })
}

#[test]
fn manual_starts_distinguish_task_and_priority_and_resume_takes_a_fresh_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let one = task(&mut store, None);
    let two = task(&mut store, None);
    let first = start_task_at(&mut store, one, &at(0)).unwrap().entry;
    let retry = start_task_at(&mut store, one, &at(5)).unwrap().entry;
    assert_eq!(retry.id, first.id);
    let other = start_task_at(&mut store, two, &at(10)).unwrap().entry;
    assert_ne!(other.id, first.id);
    assert_eq!(other.task, link(two, None));
    tasks::edit(&mut store, two, |t| {
        t.quadrant = Some(Quadrant::Plan);
        Ok(())
    })
    .unwrap();
    assert_eq!(store.current().unwrap().unwrap(), other);
    let planned = start_task_at(&mut store, two, &at(20)).unwrap().entry;
    assert_ne!(planned.id, other.id);
    assert_eq!(planned.task, link(two, Some(Quadrant::Plan)));
    tasks::edit(&mut store, two, |t| {
        t.quadrant = Some(Quadrant::Delegate);
        t.title = "Ask reviewer".into();
        t.description = "Check scope".into();
        t.tags = vec!["peer".into()];
        Ok(())
    })
    .unwrap();
    let resumed = resume_at(&mut store, Some(other.id), &at(30))
        .unwrap()
        .entry;
    assert_eq!(resumed.task, link(two, Some(Quadrant::Delegate)));
    assert_eq!(resumed.description, "Ask reviewer — Check scope");
    assert_eq!(resumed.tags, ["peer"]);
    assert_eq!(
        store.entry(planned.id).unwrap().task,
        link(two, Some(Quadrant::Plan))
    );
    let mut unlinked = resumed.as_new();
    unlinked.start = at(40).local().unwrap();
    unlinked.task = None;
    let ordinary = start_at(&mut store, unlinked, &at(40)).unwrap().entry;
    assert_ne!(resumed.id, ordinary.id);
    assert!(ordinary.task.is_none());
}

#[test]
fn a_new_bounded_record_cannot_forge_task_metadata_or_reuse_a_closed_task() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let id = task(&mut store, Some(Quadrant::Plan));
    let mut input = parse_entry("09:00-09:10 Other@Personal", at(900).local().unwrap(), 0).unwrap();
    input.task = link(id, Some(Quadrant::Do));
    let recorded = add_at(&mut store, input.clone(), &at(900)).unwrap().entry;
    assert_eq!(recorded.task, link(id, Some(Quadrant::Plan)));
    assert_eq!(recorded.activity, "Design");
    assert_eq!(recorded.category.as_deref(), Some("Work"));
    assert_eq!((recorded.start, recorded.end), (input.start, input.end));
    complete_task(&mut store, id, &at(900)).unwrap();
    assert!(add_at(&mut store, input, &at(901)).is_err());
    assert_eq!(store.entries().unwrap(), [recorded]);
}

#[test]
fn splitting_completed_task_work_preserves_both_original_quadrant_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let id = task(&mut store, Some(Quadrant::Do));
    start_task_at(&mut store, id, &at(0)).unwrap();
    stop_at(&mut store, at(3_600).local().unwrap(), &at(3_600)).unwrap();
    tasks::edit(&mut store, id, |t| {
        t.quadrant = Some(Quadrant::Drop);
        Ok(())
    })
    .unwrap();
    let gap = parse_entry("09:20-09:35 Call", at(3_600).local().unwrap(), 0).unwrap();
    add_at(&mut store, gap, &at(3_600)).unwrap();
    let records = store.entries().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].task, link(id, Some(Quadrant::Do)));
    assert_eq!(records[1].task, None);
    assert_eq!(records[2].task, link(id, Some(Quadrant::Do)));
    let edited = edit_at(&mut store, records[2].id, &at(3_600), |row| {
        let mut copy = row.as_new();
        copy.task = None;
        copy.description = "Corrected note".into();
        Ok(copy)
    })
    .unwrap()
    .entry;
    assert_eq!(edited.task, link(id, Some(Quadrant::Do)));
}

#[test]
fn completion_and_archive_stop_only_owned_work_and_reopen_starts_no_timer() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let one = task(&mut store, Some(Quadrant::Do));
    let two = task(&mut store, None);
    let first = start_task_at(&mut store, one, &at(0)).unwrap().entry;
    complete_task(&mut store, two, &at(5)).unwrap();
    assert_eq!(store.current().unwrap().unwrap(), first);
    let done = complete_task(&mut store, one, &at(20)).unwrap();
    assert_eq!(
        (done.status, done.completed_utc),
        (TaskStatus::Done, Some(at(20).utc))
    );
    assert_eq!(
        store.entry(first.id).unwrap().end,
        Some(at(20).local().unwrap())
    );
    assert_eq!(
        complete_task(&mut store, one, &at(30))
            .unwrap()
            .completed_utc,
        done.completed_utc
    );
    assert!(start_task_at(&mut store, one, &at(30)).is_err());
    assert!(resume_at(&mut store, Some(first.id), &at(30)).is_err());
    let mut stale = first.as_new();
    stale.start = at(30).local().unwrap();
    assert!(start_at(&mut store, stale.clone(), &at(30)).is_err());
    assert!(pomodoro::start(&mut store, stale, Config::default(), &at(30)).is_err());
    set_task_status_at(&mut store, one, TaskStatus::Open, &at(35)).unwrap();
    assert!(store.current().unwrap().is_none());
    let second = resume_at(&mut store, Some(first.id), &at(40))
        .unwrap()
        .entry;
    set_task_status_at(&mut store, one, TaskStatus::Archived, &at(50)).unwrap();
    assert_eq!(
        store.entry(second.id).unwrap().end,
        Some(at(50).local().unwrap())
    );
    assert!(store.current().unwrap().is_none());
}

#[test]
fn failed_timer_close_rolls_back_task_completion() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let id = task(&mut store, None);
    let started = start_task_at(&mut store, id, &at(0)).unwrap().entry;
    store
        .conn
        .execute_batch(
            "CREATE TRIGGER reject_finish BEFORE UPDATE OF end_time ON entries
        BEGIN SELECT RAISE(ABORT, 'simulated failure'); END;",
        )
        .unwrap();
    assert!(complete_task(&mut store, id, &at(60)).is_err());
    assert_eq!(store.current().unwrap().unwrap(), started);
    let original = tasks::load_from(&store.conn, id).unwrap();
    assert_eq!(
        (original.status, original.completed_utc),
        (TaskStatus::Open, None)
    );
}

#[test]
fn pomodoro_segments_take_new_task_snapshots_and_completed_focus_keeps_task_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let one = task(&mut store, Some(Quadrant::Plan));
    let two = task(&mut store, Some(Quadrant::Plan));
    let cfg = Config {
        focus_seconds: 30,
        ..Config::default()
    };
    let started = pomodoro::start_task(&mut store, one, cfg, &at(0)).unwrap();
    assert_eq!(started.task_id, Some(one));
    assert_eq!(
        pomodoro::start_task(&mut store, one, cfg, &at(1))
            .unwrap()
            .id,
        started.id
    );
    assert!(pomodoro::start_task(&mut store, two, cfg, &at(1)).is_err());
    let original = store.entry(started.entry_id().unwrap()).unwrap();
    assert_eq!(original.task, link(one, Some(Quadrant::Plan)));
    complete_task(&mut store, two, &at(2)).unwrap();
    assert_eq!(store.current().unwrap().unwrap(), original);
    tasks::edit(&mut store, one, |t| {
        t.quadrant = Some(Quadrant::Do);
        t.title = "Urgent review".into();
        t.activity = "Review".into();
        t.description = "Check scope".into();
        t.tags = vec!["new".into()];
        Ok(())
    })
    .unwrap();
    let retried = pomodoro::start_task(&mut store, one, cfg, &at(5)).unwrap();
    assert_eq!(
        (retried.id, retried.entry_id(), retried.deadline()),
        (started.id, started.entry_id(), started.deadline())
    );
    assert_eq!(retried.description, original.description);
    assert_eq!(store.entries().unwrap().len(), 1);
    assert!(
        pomodoro::start_task(
            &mut store,
            one,
            Config {
                focus_seconds: 31,
                ..cfg
            },
            &at(5)
        )
        .is_err()
    );
    assert_eq!(store.current().unwrap().unwrap(), original);
    pomodoro::pause(&mut store, &at(10)).unwrap();
    let resumed = pomodoro::resume(&mut store, &at(20)).unwrap();
    assert_eq!(
        (resumed.task_id, resumed.activity.as_str()),
        (Some(one), "Review")
    );
    let revised = store.entry(resumed.entry_id().unwrap()).unwrap();
    assert_eq!(revised.task, link(one, Some(Quadrant::Do)));
    assert_eq!(revised.description, "Urgent review — Check scope");
    assert_eq!(revised.tags, ["new"]);
    assert_eq!(
        store.entry(original.id).unwrap().task,
        link(one, Some(Quadrant::Plan))
    );
    let ready = pomodoro::status(&mut store, &at(40)).unwrap().unwrap();
    assert_eq!(
        (ready.state(), ready.phase(), ready.completed_focus()),
        (State::Ready, Phase::ShortBreak, 1)
    );
    assert_eq!(
        tasks::load_from(&store.conn, one).unwrap().status,
        TaskStatus::Open
    );
    pomodoro::next(&mut store, &at(41)).unwrap();
    set_task_status_at(&mut store, one, TaskStatus::Archived, &at(42)).unwrap();
    let ended = pomodoro::status(&mut store, &at(43)).unwrap().unwrap();
    assert_eq!(
        (ended.state(), ended.completed_focus()),
        (State::Stopped, 1)
    );
    assert_eq!(store.entries().unwrap().len(), 2);
}

#[test]
fn completing_a_paused_task_stops_its_cycle_without_counting_a_second_interruption() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let id = task(&mut store, Some(Quadrant::Delegate));
    pomodoro::start_task(&mut store, id, Config::default(), &at(0)).unwrap();
    let suspended = pomodoro::pause(&mut store, &at(12)).unwrap();
    assert_eq!(suspended.entry_id(), None);
    complete_task(&mut store, id, &at(40)).unwrap();
    complete_task(&mut store, id, &at(50)).unwrap();
    let session = pomodoro::status(&mut store, &at(60)).unwrap().unwrap();
    assert_eq!(
        (session.state(), session.interrupted_focus()),
        (State::Stopped, 1)
    );
    assert_eq!(
        pomodoro::statistics(&mut store, &at(60))
            .unwrap()
            .focus_seconds,
        12
    );
    assert!(pomodoro::resume(&mut store, &at(60)).is_err());
}

#[test]
fn completing_a_task_after_clock_rollback_uses_the_focus_end_instant() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let id = task(&mut store, None);
    let session = pomodoro::start_task(&mut store, id, Config::default(), &at(5)).unwrap();
    pomodoro::status(&mut store, &at(40)).unwrap();
    let done = complete_task(&mut store, id, &at(20)).unwrap();
    let entry = store.entry(session.entry_id().unwrap()).unwrap();
    assert_eq!(entry.clock.unwrap().end_utc, Some(at(40).utc));
    assert_eq!(done.completed_utc, Some(at(40).utc));
    assert_eq!(
        complete_task(&mut store, id, &at(50))
            .unwrap()
            .completed_utc,
        done.completed_utc
    );
}

#[test]
fn out_of_order_immediate_task_start_retries_reuse_the_committed_timer() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let id = task(&mut store, None);
    let first = start_task_at(&mut store, id, &at(11)).unwrap().entry;
    let stale_now = start_task_at(&mut store, id, &at(10)).unwrap();
    assert_eq!(stale_now.entry, first);
    assert!(stale_now.adjusted.is_empty());
    let mut backdated = first.as_new();
    backdated.start = at(9).local().unwrap();
    assert!(start_at(&mut store, backdated, &at(12)).is_err());
    assert_eq!(store.entries().unwrap(), [first]);
}

#[test]
fn completing_live_focus_commits_task_session_and_work_entry_together() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let id = task(&mut store, Some(Quadrant::Do));
    let active = pomodoro::start_task(&mut store, id, Config::default(), &at(0)).unwrap();
    let entry_id = active.entry_id().unwrap();
    store
        .conn
        .execute_batch(
            "CREATE TRIGGER reject_interruption BEFORE UPDATE OF state ON pomodoro_sessions
        WHEN NEW.state = 'stopped' BEGIN SELECT RAISE(ABORT, 'simulated failure'); END;",
        )
        .unwrap();
    assert!(complete_task(&mut store, id, &at(35)).is_err());
    assert_eq!(
        tasks::load_from(&store.conn, id).unwrap().status,
        TaskStatus::Open
    );
    assert_eq!(store.current().unwrap().unwrap().id, entry_id);
    assert_eq!(
        pomodoro::status(&mut store, &at(35))
            .unwrap()
            .unwrap()
            .state(),
        State::Running
    );

    store
        .conn
        .execute_batch("DROP TRIGGER reject_interruption")
        .unwrap();
    complete_task(&mut store, id, &at(40)).unwrap();
    let finished = store.entry(entry_id).unwrap();
    assert_eq!(finished.clock.unwrap().end_utc, Some(at(40).utc));
    assert!(!finished.running);
    let totals = pomodoro::statistics(&mut store, &at(41)).unwrap();
    assert_eq!(
        (totals.completed, totals.interrupted, totals.focus_seconds),
        (0, 1, 40)
    );
    assert_eq!(
        tasks::load_from(&store.conn, id).unwrap().status,
        TaskStatus::Done
    );
}

#[test]
fn an_outer_failure_rolls_back_timer_switches_task_updates_and_pomodoro_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let human_entry = start_at(
        &mut store,
        parse_entry("Human work", at(0).local().unwrap(), 0).unwrap(),
        &at(0),
    )
    .unwrap()
    .entry;
    let input = TaskInput {
        title: "Test a change".into(),
        activity: parse_entry("Review", at(0).local().unwrap(), 0)
            .unwrap()
            .into(),
        quadrant: Some(Quadrant::Do),
        due_date: None,
    };
    let failed: Result<()> = store.write(|tx| {
        let task = tasks::add_in_transaction(tx, input, &at(1))?;
        let task_id = TaskId::new(task.id)?;
        let manual = start_task_in_transaction(tx, task_id, &at(2))?.entry;
        assert_eq!(manual.clock.unwrap().start_utc, at(2).utc);
        let stopped = stop_in_transaction(tx, StopTime::Now, &at(3))?.unwrap();
        assert_eq!(stopped.clock.unwrap().end_utc, Some(at(3).utc));

        let id = EntryId::new(manual.id)?;
        let resumed = resume_in_transaction(tx, Some(id), &at(4))?.entry;
        assert_eq!(resumed.clock.unwrap().start_utc, at(4).utc);
        edit_with_in_transaction(tx, id, &at(5), |old| {
            let mut edited = old.as_new();
            edited.tags = vec!["a,b".into()];
            Ok(edited)
        })?;
        let edited = find_entry(tx, manual.id)?;
        assert_eq!(edited.tags, ["a,b"]);
        delete_in_transaction(tx, id, &at(5))?;

        let pomo = pomodoro::start_task_in_transaction(tx, task_id, Config::default(), &at(6))?;
        assert_eq!(pomo.task_id, Some(task.id));
        pomodoro::pause_in_transaction(tx, &at(7))?;
        let patch = TaskEdit {
            notes: Some("updated notes".into()),
            ..TaskEdit::default()
        };
        tasks::edit_task_in_transaction(tx, task_id, patch)?;
        let active = pomodoro::resume_in_transaction(tx, &at(8))?;
        let focused = find_entry(tx, active.entry_id().unwrap())?;
        assert_eq!(focused.description, "Test a change — updated notes");
        change_task_status_in_transaction(tx, task_id, TaskStatus::Done, &at(9))?;
        assert!(current_id(tx)?.is_none());

        // The workflow succeeded, but the caller could not persist its receipt.
        Err(Error::Conflict("receipt write failed".into()))
    });
    assert!(matches!(failed, Err(Error::Conflict(message)) if message == "receipt write failed"));
    assert_eq!(
        store.entries().unwrap().as_slice(),
        std::slice::from_ref(&human_entry)
    );
    assert_eq!(store.current().unwrap(), Some(human_entry));
    assert!(tasks::list(&store, None).unwrap().is_empty());
    assert!(pomodoro::load(&store.conn, false).unwrap().is_none());
}

#[test]
fn transaction_task_timers_keep_elapsed_seconds_across_a_fall_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("fold.db")).unwrap();
    let zone = chrono_tz::Europe::Zagreb;
    let start = Moment::at("2026-10-25T00:55:20Z".parse().unwrap(), zone);
    let stop = Moment::at("2026-10-25T01:05:20Z".parse().unwrap(), zone);
    let id = TaskId::new(task(&mut store, None)).unwrap();
    store
        .write(|tx| start_task_in_transaction(tx, id, &start))
        .unwrap();
    let closed = store
        .write(|tx| stop_in_transaction(tx, StopTime::Now, &stop))
        .unwrap()
        .unwrap();
    assert!(closed.end.unwrap() < closed.start);
    let clock = closed.clock.unwrap();
    assert_eq!(
        clock.end_utc.unwrap() - clock.start_utc,
        Duration::minutes(10)
    );
    assert_eq!(clock.zone, zone);
}
