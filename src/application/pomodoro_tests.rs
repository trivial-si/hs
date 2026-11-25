use super::*;
use crate::parse::parse_entry;
use crate::tracking;
use chrono::Duration;
use rusqlite::params;
use tempfile::TempDir;

fn sandbox() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("hs.db")).unwrap();
    (dir, store)
}

fn at(seconds: i64) -> Moment {
    let origin: DateTime<Utc> = "2026-06-08T09:00:00Z".parse().unwrap();
    Moment::at(origin + Duration::seconds(seconds), chrono_tz::UTC)
}

fn config() -> Config {
    Config {
        focus_seconds: 60,
        break_seconds: 10,
        long_break_seconds: 30,
        rounds: 2,
    }
}

fn work(now: &Moment) -> NewEntry {
    parse_entry("Design@Work, #client #review", now.local().unwrap(), 0).unwrap()
}

#[test]
fn idle_and_paused_pomodoro_views_do_not_need_a_writer_lock() {
    let (dir, mut store) = sandbox();
    let writer = Connection::open(dir.path().join("hs.db")).unwrap();
    for paused in [false, true] {
        if paused {
            start(&mut store, work(&at(0)), config(), &at(0)).unwrap();
            pause(&mut store, &at(15)).unwrap();
        }
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let session = status(&mut store, &at(25)).unwrap();
        assert_eq!(session.map(|s| s.state()), paused.then_some(State::Paused));
        let totals = statistics(&mut store, &at(25)).unwrap();
        assert_eq!(totals.focus_seconds, if paused { 15 } else { 0 });
        writer.execute_batch("ROLLBACK").unwrap();
    }
}

#[test]
fn late_reopen_completes_only_the_started_focus_at_its_deadline() {
    let (dir, mut store) = sandbox();
    let session = start(&mut store, work(&at(0)), config(), &at(0)).unwrap();
    drop(store);
    let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
    let late = 3 * 24 * 3600;
    for offset in [late, late + 60] {
        let saved = status(&mut store, &at(offset)).unwrap().unwrap();
        assert_eq!(
            (saved.state(), saved.phase()),
            (State::Ready, Phase::ShortBreak)
        );
        assert_eq!((saved.completed_focus(), saved.interrupted_focus()), (1, 0));
        assert_eq!(saved.deadline(), None);
        assert_eq!(saved.remaining(&at(offset)), 10);
    }
    let closed = store.entry(session.entry_id().unwrap()).unwrap();
    assert_eq!(closed.clock.unwrap().end_utc, Some(at(60).utc));
    assert!(store.current().unwrap().is_none());
    let totals = statistics(&mut store, &at(late + 60)).unwrap();
    assert_eq!(
        (totals.completed, totals.focus_seconds, totals.break_seconds),
        (1, 60, 0)
    );
    let rest = next(&mut store, &at(late + 60)).unwrap();
    assert_eq!(rest.deadline(), Some(at(late + 70).utc));
    let retry = next(&mut store, &at(late + 61)).unwrap();
    assert_eq!(retry.phase_id(), rest.phase_id());
    assert_eq!(store.entries().unwrap().len(), 1);
}

#[test]
fn three_focus_segments_add_to_sixty_seconds_across_two_pauses() {
    let (_dir, mut store) = sandbox();
    let started = start(&mut store, work(&at(0)), config(), &at(0)).unwrap();
    for (paused_at, resumed_at, remaining) in [(20, 200, 40), (220, 500, 20)] {
        let saved = pause(&mut store, &at(paused_at)).unwrap();
        assert_eq!(saved.remaining(&at(resumed_at)), remaining);
        assert!(store.current().unwrap().is_none());
        let retry = pause(&mut store, &at(paused_at + 1)).unwrap();
        assert_eq!(retry.state(), State::Paused);
        assert!(next(&mut store, &at(paused_at + 2)).is_err());
        let resumed = resume(&mut store, &at(resumed_at)).unwrap();
        assert_eq!(resumed.phase_id(), started.phase_id());
        assert_eq!(resumed.deadline(), Some(at(resumed_at + remaining).utc));
        assert_eq!(
            resume(&mut store, &at(resumed_at + 1)).unwrap().entry_id(),
            resumed.entry_id()
        );
    }
    let totals = statistics(&mut store, &at(800)).unwrap();
    assert_eq!(
        (totals.completed, totals.interrupted, totals.focus_seconds),
        (1, 0, 60)
    );
    let segments: Vec<_> = store
        .entries()
        .unwrap()
        .into_iter()
        .map(|row| {
            let clock = row.clock.unwrap();
            (clock.start_utc, clock.end_utc.unwrap())
        })
        .collect();
    assert_eq!(
        segments,
        [
            (at(0).utc, at(20).utc),
            (at(200).utc, at(220).utc),
            (at(500).utc, at(520).utc)
        ]
    );
}

#[test]
fn paused_break_resumes_without_tracking_work_or_charging_the_pause() {
    let (_dir, mut store) = sandbox();
    let cfg = Config {
        break_seconds: 30,
        ..config()
    };
    start(&mut store, work(&at(0)), cfg, &at(0)).unwrap();
    let rest = next(&mut store, &at(120)).unwrap();
    assert_eq!(rest.phase(), Phase::ShortBreak);
    let suspended = pause(&mut store, &at(130)).unwrap();
    assert_eq!(suspended.remaining(&at(500)), 20);
    assert_eq!(statistics(&mut store, &at(500)).unwrap().break_seconds, 10);
    let going = resume(&mut store, &at(500)).unwrap();
    assert_eq!(going.deadline(), Some(at(520).utc));
    assert_eq!(statistics(&mut store, &at(515)).unwrap().break_seconds, 25);
    let done = status(&mut store, &at(800)).unwrap().unwrap();
    assert_eq!((done.state(), done.phase()), (State::Ready, Phase::Focus));
    let totals = statistics(&mut store, &at(800)).unwrap();
    assert_eq!((totals.focus_seconds, totals.break_seconds), (60, 30));
    assert_eq!(store.entries().unwrap().len(), 1);
    assert!(store.current().unwrap().is_none());
}

#[test]
fn every_second_completed_focus_offers_the_long_break() {
    let (_dir, mut store) = sandbox();
    let cfg = config();
    let mut cursor = 0;
    start(&mut store, work(&at(cursor)), cfg, &at(cursor)).unwrap();
    for round in 1..=4 {
        if round > 1 {
            let focus = next(&mut store, &at(cursor)).unwrap();
            assert_eq!(focus.phase(), Phase::Focus);
        }
        cursor += cfg.focus_seconds;
        let ready = status(&mut store, &at(cursor)).unwrap().unwrap();
        let (phase, duration) = if round % 2 == 0 {
            (Phase::LongBreak, cfg.long_break_seconds)
        } else {
            (Phase::ShortBreak, cfg.break_seconds)
        };
        assert_eq!(
            (ready.state(), ready.phase(), ready.completed_focus()),
            (State::Ready, phase, round)
        );
        let rest = next(&mut store, &at(cursor)).unwrap();
        assert_eq!(rest.remaining(&at(cursor)), duration);
        assert!(store.current().unwrap().is_none());
        cursor += duration;
        assert_eq!(
            status(&mut store, &at(cursor)).unwrap().unwrap().phase(),
            Phase::Focus
        );
    }
    let totals = statistics(&mut store, &at(cursor)).unwrap();
    assert_eq!(
        (totals.completed, totals.focus_seconds, totals.break_seconds),
        (4, 240, 80)
    );
    assert_eq!(store.entries().unwrap().len(), 4);
}

#[test]
fn backward_clock_preserves_elapsed_time_and_cannot_start_the_next_phase_early() {
    let (_dir, mut store) = sandbox();
    start(&mut store, work(&at(0)), config(), &at(0)).unwrap();
    assert_eq!(statistics(&mut store, &at(40)).unwrap().focus_seconds, 40);
    let rolled_back = status(&mut store, &at(20)).unwrap().unwrap();
    assert_eq!(rolled_back.remaining(&at(20)), 20);
    assert_eq!(statistics(&mut store, &at(20)).unwrap().focus_seconds, 40);
    assert_eq!(pause(&mut store, &at(10)).unwrap().remaining(&at(10)), 20);
    let running = resume(&mut store, &at(30)).unwrap();
    assert_eq!(running.deadline(), Some(at(60).utc));
    let second = store.entry(running.entry_id().unwrap()).unwrap();
    assert_eq!(second.clock.unwrap().start_utc, at(40).utc);
    assert_eq!(
        status(&mut store, &at(61)).unwrap().unwrap().state(),
        State::Ready
    );
    let rest = next(&mut store, &at(45)).unwrap();
    assert_eq!(rest.deadline(), Some(at(71).utc));
    assert_eq!(rest.remaining(&at(45)), 10);
    assert_eq!(statistics(&mut store, &at(45)).unwrap().focus_seconds, 60);
}

#[test]
fn autumn_clock_change_completes_after_twenty_five_elapsed_minutes() {
    let (_dir, mut store) = sandbox();
    let zone = chrono_tz::Europe::Zagreb;
    let t0 = Moment::at("2026-10-25T00:50:00Z".parse().unwrap(), zone);
    let t1 = Moment::at(t0.utc + Duration::minutes(25), zone);
    let started = start(&mut store, work(&t0), Config::default(), &t0).unwrap();
    assert_eq!(started.deadline(), Some(t1.utc));
    let previous_second = Moment::at(t1.utc - Duration::seconds(1), zone);
    assert_eq!(
        status(&mut store, &previous_second)
            .unwrap()
            .unwrap()
            .remaining(&previous_second),
        1
    );
    let completed = status(&mut store, &t1).unwrap().unwrap();
    assert_eq!(
        (completed.state(), completed.completed_focus()),
        (State::Ready, 1)
    );
    let entry = store.entry(started.entry_id().unwrap()).unwrap();
    assert!(entry.end.unwrap() < entry.start);
    let clock = entry.clock.unwrap();
    assert_eq!(
        (clock.zone, clock.start_utc, clock.end_utc),
        (zone, t0.utc, Some(t1.utc))
    );
    assert_eq!(statistics(&mut store, &t1).unwrap().focus_seconds, 1_500);
}

#[test]
fn invalid_configuration_or_template_leaves_existing_manual_work_alone() {
    let (_dir, mut store) = sandbox();
    let now = at(0);
    let manual = tracking::start_at(&mut store, work(&now), &now)
        .unwrap()
        .entry;
    for cfg in [
        Config {
            focus_seconds: 0,
            ..config()
        },
        Config {
            break_seconds: -1,
            ..config()
        },
        Config {
            long_break_seconds: i64::MAX,
            ..config()
        },
        Config {
            rounds: 0,
            ..config()
        },
    ] {
        assert!(start(&mut store, work(&now), cfg, &now).is_err());
    }
    let mut backdated = work(&now);
    backdated.start -= Duration::minutes(2);
    let mut bounded = work(&now);
    bounded.end = Some(now.local().unwrap());
    let mut blank = work(&now);
    blank.activity.clear();
    for template in [backdated, bounded, blank] {
        assert!(start(&mut store, template, config(), &now).is_err());
        assert!(status(&mut store, &now).unwrap().is_none());
        assert_eq!(store.current().unwrap().as_ref(), Some(&manual));
    }
    assert_eq!(store.entries().unwrap(), [manual]);
    let totals = statistics(&mut store, &now).unwrap();
    assert_eq!(
        (totals.completed, totals.interrupted, totals.focus_seconds),
        (0, 0, 0)
    );
    for invalid in ["", "0", "0s", "-3m", "five", "9000000000000000000h"] {
        assert!(parse_duration(invalid).is_err(), "{invalid}");
    }
    for (raw, seconds) in [("25", 1500), ("90s", 90), ("2m", 120), ("1h", 3600)] {
        assert_eq!(parse_duration(raw).unwrap(), seconds);
    }
}

#[test]
fn a_deadline_outside_the_session_local_clock_range_cannot_leave_an_unclosable_timer() {
    let (_dir, mut store) = sandbox();
    let tokyo = Moment::at(at(0).utc, chrono_tz::Asia::Tokyo);
    let year_end = Moment::at("9999-12-31T23:59:50Z".parse().unwrap(), chrono_tz::UTC);
    for (now, seconds) in [
        (
            tokyo,
            DateTime::<Utc>::MAX_UTC.timestamp() - tokyo.utc.timestamp(),
        ),
        (year_end, 60),
    ] {
        let cfg = Config {
            focus_seconds: seconds,
            ..config()
        };
        assert!(start(&mut store, work(&now), cfg, &now).is_err());
        assert!(status(&mut store, &now).unwrap().is_none());
        assert!(store.current().unwrap().is_none());
    }
}

#[test]
fn a_manual_handoff_cannot_reclaim_previously_observed_focus_after_clock_rollback() {
    let (_dir, mut store) = sandbox();
    let id = start(&mut store, work(&at(0)), config(), &at(0))
        .unwrap()
        .entry_id()
        .unwrap();
    assert_eq!(statistics(&mut store, &at(40)).unwrap().focus_seconds, 40);
    let manual_at = |seconds| {
        let mut entry = work(&at(seconds));
        entry.start = at(seconds).local().unwrap();
        entry
    };
    assert!(tracking::start_at(&mut store, manual_at(20), &at(20)).is_err());
    assert!(tracking::resume_at(&mut store, Some(id), &at(20)).is_err());
    let still_live = status(&mut store, &at(20)).unwrap().unwrap();
    assert_eq!(
        (still_live.state(), still_live.entry_id()),
        (State::Running, Some(id))
    );
    stop(&mut store, &at(20)).unwrap();
    // A later process sees only completed history, and must enforce the same boundary.
    assert!(tracking::start_at(&mut store, manual_at(0), &at(0)).is_err());
    assert!(tracking::start_at(&mut store, manual_at(39), &at(39)).is_err());
    let resumed = tracking::start_at(&mut store, manual_at(40), &at(40))
        .unwrap()
        .entry;
    assert_eq!(resumed.start, at(40).local().unwrap());
    assert_eq!(
        store.entry(id).unwrap().clock.unwrap().end_utc,
        Some(at(40).utc)
    );
    assert_eq!(statistics(&mut store, &at(50)).unwrap().focus_seconds, 40);
}

#[test]
fn clearing_a_completed_focus_end_cannot_create_an_unowned_running_timer() {
    let (_dir, mut store) = sandbox();
    let session = start(&mut store, work(&at(0)), config(), &at(0)).unwrap();
    stop(&mut store, &at(20)).unwrap();
    let id = session.entry_id().unwrap();
    let saved = store.entry(id).unwrap();
    let attempt = tracking::edit_at(&mut store, id, &at(30), |row| {
        let mut edited = row.as_new();
        edited.end = None;
        Ok(edited)
    });
    assert!(attempt.is_err());
    assert_eq!(store.entry(id).unwrap(), saved);
    assert_eq!(
        statistics(&mut store, &at(3_600)).unwrap().focus_seconds,
        20
    );
}

#[test]
fn an_unresolved_segment_from_an_old_database_does_not_accrue_focus_forever() {
    let (_dir, mut store) = sandbox();
    let session = start(&mut store, work(&at(0)), config(), &at(0)).unwrap();
    stop(&mut store, &at(20)).unwrap();
    let id = session.entry_id().unwrap();
    store
        .conn
        .execute(
            "UPDATE entries SET end_time=NULL, end_utc=NULL WHERE id=?",
            [id],
        )
        .unwrap();
    assert!(!store.entry(id).unwrap().running);
    assert_eq!(statistics(&mut store, &at(3_600)).unwrap().focus_seconds, 0);
    // Recording a known end restores the corrected duration.
    tracking::edit_at(&mut store, id, &at(3_600), |row| {
        let mut edited = row.as_new();
        edited.end = Some(at(15).local().unwrap());
        Ok(edited)
    })
    .unwrap();
    assert_eq!(
        statistics(&mut store, &at(3_600)).unwrap().focus_seconds,
        15
    );
}

#[test]
fn pause_or_resume_at_expiry_commits_completion_without_starting_a_break() {
    for action in [pause, resume] {
        let (_dir, mut store) = sandbox();
        let started = start(&mut store, work(&at(0)), config(), &at(0)).unwrap();
        let result = action(&mut store, &at(60)).unwrap();
        assert_eq!(
            (result.state(), result.phase()),
            (State::Ready, Phase::ShortBreak)
        );
        assert_eq!(result.completed_focus(), 1);
        assert!(store.current().unwrap().is_none());
        assert_eq!(
            store
                .entry(started.entry_id().unwrap())
                .unwrap()
                .clock
                .unwrap()
                .end_utc,
            Some(at(60).utc)
        );
        let state: String = store
            .conn
            .query_row("SELECT state FROM pomodoro_phases", [], |row| row.get(0))
            .unwrap();
        assert_eq!(state, "completed");
        assert_eq!(
            action(&mut store, &at(600)).unwrap().phase_id(),
            result.phase_id()
        );
        assert_eq!(store.entries().unwrap().len(), 1);
    }
}

#[test]
fn incompatible_saved_state_is_rejected_without_fabricating_a_deadline() {
    let (_dir, mut store) = sandbox();
    let live = start(&mut store, work(&at(0)), config(), &at(0)).unwrap();
    let original = store.current().unwrap().unwrap();
    for assignment in [
        "deadline_utc=NULL",
        "current_phase_id=NULL",
        "phase='short_break'",
        "state='paused'",
        "remaining_seconds=0",
    ] {
        let tx = store.conn.transaction().unwrap();
        tx.execute(
            &format!("UPDATE pomodoro_sessions SET {assignment} WHERE id=?"),
            [live.id],
        )
        .unwrap();
        assert!(load(&tx, true).is_err(), "{assignment}");
        tx.rollback().unwrap();
    }
    assert_eq!(store.current().unwrap().unwrap(), original);
    assert_eq!(
        status(&mut store, &at(1))
            .unwrap()
            .unwrap()
            .remaining(&at(1)),
        59
    );
}

#[test]
fn a_deleted_focus_foreign_key_is_recovered_without_stopping_someone_elses_timer() {
    let (_dir, mut store) = sandbox();
    let live = start(&mut store, work(&at(0)), config(), &at(0)).unwrap();
    let tx = store.conn.transaction().unwrap();
    tx.execute("DELETE FROM entries WHERE id=?", [live.entry_id().unwrap()])
        .unwrap();
    let unrelated =
        entries::start_in_transaction(&tx, work(&at(30)), &at(30), StartMode::NewSegment)
            .unwrap()
            .entry;
    tx.commit().unwrap();
    let recovered = status(&mut store, &at(60)).unwrap().unwrap();
    assert_eq!(recovered.state(), State::Stopped);
    assert_eq!(
        (recovered.completed_focus(), recovered.interrupted_focus()),
        (0, 1)
    );
    assert_eq!(store.current().unwrap().unwrap(), unrelated);
    let observed = statistics(&mut store, &at(120)).unwrap();
    assert_eq!((observed.interrupted, observed.focus_seconds), (1, 0));
}

#[test]
fn a_foreign_phase_or_segment_cannot_grant_ownership_of_other_work() {
    let (_dir, mut store) = sandbox();
    let first = start(&mut store, work(&at(0)), config(), &at(0)).unwrap();
    stop(&mut store, &at(5)).unwrap();
    let live = start(&mut store, work(&at(60)), config(), &at(60)).unwrap();
    let tx = store.conn.transaction().unwrap();
    tx.execute(
        "UPDATE pomodoro_sessions SET current_phase_id=? WHERE id=?",
        params![first.phase_id(), live.id],
    )
    .unwrap();
    assert!(load(&tx, true).is_err());
    tx.rollback().unwrap();

    let tx = store.conn.transaction().unwrap();
    let unrelated =
        entries::start_in_transaction(&tx, work(&at(120)), &at(120), StartMode::NewSegment)
            .unwrap()
            .entry;
    tx.execute(
        "UPDATE pomodoro_sessions SET current_entry_id=? WHERE id=?",
        params![unrelated.id, live.id],
    )
    .unwrap();
    tx.commit().unwrap();
    let recovered = status(&mut store, &at(120)).unwrap().unwrap();
    assert_eq!(recovered.state(), State::Stopped);
    assert_eq!(store.current().unwrap().unwrap(), unrelated);
    assert_eq!(recovered.interrupted_focus(), 1);
}

#[test]
fn generated_command_sequences_preserve_ownership_and_measure_only_running_phases() {
    let cfg = Config {
        focus_seconds: 7,
        break_seconds: 3,
        long_break_seconds: 5,
        rounds: 2,
    };
    for initial_seed in 1..=8u32 {
        let (_dir, mut store) = sandbox();
        let mut seed = initial_seed;
        let mut seconds = 0;
        let mut focus_total = 0;
        let mut break_total = 0;
        let mut previous = start(&mut store, work(&at(0)), cfg, &at(0)).unwrap();
        for _ in 0..32 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let elapsed = i64::from((seed >> 8) % 8);
            if previous.state() == State::Running {
                let counted = elapsed.min(previous.remaining(&at(seconds)));
                if previous.phase() == Phase::Focus {
                    focus_total += counted;
                } else {
                    break_total += counted;
                }
            }
            seconds += elapsed;
            let now = at(seconds);
            let observed = status(&mut store, &now).unwrap().unwrap();
            let next_session = match (seed >> 16) % 5 {
                0 if observed.state() == State::Stopped => {
                    start(&mut store, work(&now), cfg, &now).unwrap()
                }
                1 if observed.state() != State::Stopped => pause(&mut store, &now).unwrap(),
                2 if observed.state() != State::Stopped => resume(&mut store, &now).unwrap(),
                3 if matches!(observed.state(), State::Ready | State::Running) => {
                    next(&mut store, &now).unwrap()
                }
                4 => stop(&mut store, &now).unwrap().unwrap(),
                _ => observed,
            };
            let current = store.current().unwrap().map(|row| row.id);
            assert_eq!(current, next_session.entry_id());
            assert_eq!(
                current.is_some(),
                next_session.state() == State::Running && next_session.phase() == Phase::Focus
            );
            let totals = statistics(&mut store, &now).unwrap();
            assert_eq!(
                (totals.focus_seconds, totals.break_seconds),
                (focus_total, break_total),
                "sequence {initial_seed}, time {seconds}"
            );
            previous = next_session;
        }
    }
}

#[test]
fn watched_phase_survives_pause_resume_interruption_and_replacement() {
    let (dir, mut watcher) = sandbox();
    let mut timer = Store::open(&dir.path().join("hs.db")).unwrap();
    assert!(
        watch_status(&mut watcher, None, &at(0))
            .unwrap()
            .watched_phase
            .is_none()
    );
    let first = start(&mut timer, work(&at(0)), config(), &at(0)).unwrap();
    let watched = watch_status(&mut watcher, None, &at(1))
        .unwrap()
        .watched_phase
        .unwrap();
    assert_eq!(watched.session_id, first.id);
    assert_eq!(
        (watched.phase, watched.state, watched.ended_utc),
        (Phase::Focus, PhaseState::Running, None)
    );
    pause(&mut timer, &at(5)).unwrap();
    let paused = watch_status(&mut watcher, Some(watched.id), &at(30))
        .unwrap()
        .watched_phase
        .unwrap();
    assert_eq!(
        (paused.id, paused.state, paused.ended_utc),
        (watched.id, PhaseState::Paused, None)
    );
    resume(&mut timer, &at(40)).unwrap();
    let live = watch_status(&mut watcher, Some(watched.id), &at(70))
        .unwrap()
        .watched_phase
        .unwrap();
    assert_eq!((live.id, live.state), (watched.id, PhaseState::Running));

    stop(&mut timer, &at(71)).unwrap();
    assert!(
        watch_status(&mut watcher, None, &at(71))
            .unwrap()
            .watched_phase
            .is_none()
    );
    let second = start(&mut timer, work(&at(72)), config(), &at(72)).unwrap();
    let later = watch_status(&mut watcher, Some(watched.id), &at(73)).unwrap();
    assert_eq!(later.session.unwrap().id, second.id);
    let interrupted = later.watched_phase.unwrap();
    assert_eq!(
        (interrupted.id, interrupted.session_id),
        (watched.id, first.id)
    );
    assert_eq!(
        (interrupted.state, interrupted.ended_utc),
        (PhaseState::Interrupted, Some(at(71).utc))
    );
    assert!(
        watch_status(&mut watcher, Some(12345), &at(74))
            .unwrap()
            .watched_phase
            .is_none()
    );
}

#[test]
fn late_watch_and_concurrent_next_identify_actual_focus_short_and_long_break_completions() {
    let (dir, mut watcher) = sandbox();
    let mut timer = Store::open(&dir.path().join("hs.db")).unwrap();
    let first = start(&mut timer, work(&at(0)), config(), &at(0)).unwrap();
    // The first observation arrives after the deadline, seeing the upcoming
    // break in the session and the completed focus in phase history.
    let attached = watch_status(&mut watcher, None, &at(75)).unwrap();
    assert_eq!(attached.session.unwrap().phase(), Phase::ShortBreak);
    let focused = attached.watched_phase.unwrap();
    assert_eq!(first.phase_id(), Some(focused.id));
    assert_eq!(
        (focused.phase, focused.state, focused.ended_utc),
        (Phase::Focus, PhaseState::Completed, Some(at(60).utc))
    );

    let already_settled = watch_status(&mut watcher, None, &at(76)).unwrap();
    assert_eq!(already_settled.session.unwrap().state(), State::Ready);
    assert!(already_settled.watched_phase.is_none());
    assert_eq!(
        watch_status(&mut watcher, Some(focused.id), &at(77))
            .unwrap()
            .watched_phase,
        Some(focused)
    );

    let rest = next(&mut timer, &at(80)).unwrap();
    let second = next(&mut timer, &at(100)).unwrap();
    assert_eq!(second.phase(), Phase::Focus);
    let after = watch_status(&mut watcher, rest.phase_id(), &at(101)).unwrap();
    assert_eq!(after.session.unwrap().phase_id(), second.phase_id());
    let short = after.watched_phase.unwrap();
    assert_eq!(
        (short.phase, short.state, short.ended_utc),
        (Phase::ShortBreak, PhaseState::Completed, Some(at(90).utc))
    );

    let long = next(&mut timer, &at(165)).unwrap();
    assert_eq!(long.phase(), Phase::LongBreak);
    stop(&mut timer, &at(200)).unwrap(); // Reconcile completion, then stop the ready session.
    let replacement = start(&mut timer, work(&at(201)), config(), &at(201)).unwrap();
    let saved = watch_status(&mut watcher, long.phase_id(), &at(202)).unwrap();
    assert_eq!(saved.session.unwrap().id, replacement.id);
    let old = saved.watched_phase.unwrap();
    assert_eq!(old.session_id, first.id);
    assert_eq!(
        (old.phase, old.state, old.ended_utc),
        (Phase::LongBreak, PhaseState::Completed, Some(at(195).utc))
    );
}
