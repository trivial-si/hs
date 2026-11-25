//! Guarded timer actions for a human interface that can stay open while other
//! clients change the same database. Expectations and mutations share a lock.
use chrono::{DateTime, Utc};

use crate::{
    Error, Result, Store,
    domain::{
        ids::TaskId,
        pomodoro::{Session, State},
        tasks::{Task, TaskStatus},
        time::Moment,
    },
    model::Entry,
    storage::{entries, pomodoro as persistence, tasks},
};

use super::{pomodoro, queries, workflows};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerExpectation {
    current_entry: Option<i64>,
    session: Option<SessionExpectation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionExpectation {
    id: i64,
    phase_id: Option<i64>,
    state: State,
    deadline: Option<DateTime<Utc>>,
}

impl TimerExpectation {
    pub fn from_status(status: &queries::Status) -> Self {
        Self::from_parts(
            status.current.as_ref().map(|entry| entry.id),
            status.session.as_ref(),
        )
    }

    fn from_parts(current_entry: Option<i64>, session: Option<&Session>) -> Self {
        Self {
            current_entry,
            session: session
                .filter(|session| session.state() != State::Stopped)
                .map(|session| SessionExpectation {
                    id: session.id,
                    phase_id: session.phase_id(),
                    state: session.state(),
                    deadline: session.deadline(),
                }),
        }
    }
}

/// The task being confirmed, including only timers that completing it can stop.
/// An unrelated task's timer does not invalidate this confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatusExpectation {
    task: Task,
    timer: TimerExpectation,
}

impl TaskStatusExpectation {
    pub fn from_task(task: &Task, status: &queries::Status) -> Self {
        Self {
            task: task.clone(),
            timer: task_timer(task.id, status.current.as_ref(), status.session.as_ref()),
        }
    }
}

fn task_timer(id: i64, current: Option<&Entry>, session: Option<&Session>) -> TimerExpectation {
    TimerExpectation::from_parts(
        current
            .filter(|entry| entry.task.is_some_and(|task| task.id == id))
            .map(|entry| entry.id),
        session.filter(|session| session.task_id == Some(id)),
    )
}

/// Complete/archive exactly the task and associated timer that were confirmed.
/// Check before reconciliation or mutation, so a stale confirmation is atomic
/// and cannot close newly started work or overwrite another client's task edit.
pub fn task_status(
    store: &mut Store,
    expected: &TaskStatusExpectation,
    state: TaskStatus,
    now: &Moment,
) -> Result<Task> {
    store.write(|tx| {
        let task = tasks::load_from(tx, expected.task.id)?;
        let session = persistence::load(tx, false)?;
        let current = entries::current_id(tx)?
            .map(|id| entries::find_entry(tx, id))
            .transpose()?;
        if task != expected.task
            || task_timer(task.id, current.as_ref(), session.as_ref()) != expected.timer
        {
            return Err(Error::Conflict(
                "This task or its timer changed in another client. Review it and confirm again."
                    .into(),
            ));
        }
        workflows::change_task_status_in_transaction(tx, TaskId::new(task.id)?, state, now)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerAction {
    Track(TaskId),
    Pomodoro(TaskId),
    Pause,
    Resume,
    Next,
    Stop,
}

/// Apply the action only to the timer the interface displayed or confirmed.
/// Clock observations alone do not invalidate it; identity and phase changes do.
pub fn timer(
    store: &mut Store,
    expected: &TimerExpectation,
    action: TimerAction,
    now: &Moment,
) -> Result<()> {
    store.write(|tx| {
        let session = persistence::load(tx, false)?;
        let current = TimerExpectation::from_parts(entries::current_id(tx)?, session.as_ref());
        if current != *expected {
            return Err(Error::Conflict(
                "The timer changed in another client. Review its current state and try again."
                    .into(),
            ));
        }
        match action {
            TimerAction::Track(task) => {
                workflows::start_task_in_transaction(tx, task, now)?;
            }
            TimerAction::Pomodoro(task) => {
                // This is an explicitly confirmed replacement. Keep stopping
                // the old session and starting the new one in this transaction
                // so an invalid destination rolls back without losing work.
                workflows::stop_in_transaction(tx, workflows::StopTime::Now, now)?;
                pomodoro::start_task_in_transaction(tx, task, Default::default(), now)?;
            }
            TimerAction::Pause => {
                pomodoro::pause_in_transaction(tx, now)?;
            }
            TimerAction::Resume => {
                pomodoro::resume_in_transaction(tx, now)?;
            }
            TimerAction::Next => {
                pomodoro::next_in_transaction(tx, now)?;
            }
            TimerAction::Stop => {
                workflows::stop_in_transaction(tx, workflows::StopTime::Now, now)?;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::tasks,
        domain::{model::ActivityTemplate, tasks::TaskInput},
    };

    fn moment(seconds: i64) -> Moment {
        Moment::at(
            "2026-11-23T10:00:00Z".parse::<DateTime<Utc>>().unwrap()
                + chrono::Duration::seconds(seconds),
            chrono_tz::UTC,
        )
    }

    fn task(store: &mut Store, title: &str) -> TaskId {
        let task = tasks::add(
            store,
            TaskInput {
                title: title.into(),
                activity: ActivityTemplate {
                    activity: title.into(),
                    category: None,
                    description: String::new(),
                    tags: vec![],
                },
                quadrant: None,
                due_date: None,
            },
            &moment(0),
        )
        .unwrap();
        TaskId::new(task.id).unwrap()
    }

    #[test]
    fn confirmed_pomodoro_switch_preserves_work_and_rolls_back_an_invalid_target() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let first = task(&mut store, "First focus");
        let second = task(&mut store, "Second focus");
        pomodoro::start_task(&mut store, first.get(), Default::default(), &moment(0)).unwrap();
        let before = queries::status(&mut store, &moment(10)).unwrap();
        let expected = TimerExpectation::from_status(&before);
        workflows::change_task_status(&mut store, second, TaskStatus::Archived, &moment(10))
            .unwrap();

        assert!(
            timer(
                &mut store,
                &expected,
                TimerAction::Pomodoro(second),
                &moment(20)
            )
            .is_err()
        );
        let after = queries::status(&mut store, &moment(20)).unwrap();
        assert_eq!(TimerExpectation::from_status(&after), expected);
        assert_eq!(
            after.current.as_ref().unwrap().id,
            before.current.as_ref().unwrap().id
        );
        assert_eq!(after.session.as_ref().unwrap().state(), State::Running);

        workflows::change_task_status(&mut store, second, TaskStatus::Open, &moment(20)).unwrap();
        timer(
            &mut store,
            &expected,
            TimerAction::Pomodoro(second),
            &moment(30),
        )
        .unwrap();
        let switched = queries::status(&mut store, &moment(30)).unwrap();
        assert_eq!(
            switched.session.as_ref().unwrap().task_id,
            Some(second.get())
        );
        assert_ne!(
            switched.session.as_ref().unwrap().id,
            before.session.as_ref().unwrap().id
        );
        assert_eq!(switched.current.as_ref().unwrap().activity, "Second focus");
        let history = queries::history(&mut store, &moment(30)).unwrap();
        let prior = history
            .entries
            .iter()
            .find(|entry| entry.id == before.current.as_ref().unwrap().id)
            .unwrap();
        assert_eq!(prior.clock.as_ref().unwrap().end_utc, Some(moment(30).utc));
    }

    #[test]
    fn stale_idle_view_cannot_replace_a_new_timer_from_another_client() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hs.db");
        let mut store = Store::open(&db).unwrap();
        let first = task(&mut store, "First");
        let second = task(&mut store, "Second");
        let expected =
            TimerExpectation::from_status(&queries::status(&mut store, &moment(0)).unwrap());
        let mut other = Store::open(&db).unwrap();
        workflows::start_task(&mut other, first, &moment(1)).unwrap();
        let before = queries::status(&mut other, &moment(1)).unwrap();
        assert!(matches!(
            timer(
                &mut store,
                &expected,
                TimerAction::Track(second),
                &moment(2)
            ),
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            timer(
                &mut store,
                &expected,
                TimerAction::Pomodoro(second),
                &moment(2)
            ),
            Err(Error::Conflict(_))
        ));
        let after = queries::status(&mut store, &moment(2)).unwrap();
        assert_eq!(after.current, before.current);
        assert!(after.session.is_none());
        assert_eq!(
            queries::history(&mut store, &moment(2))
                .unwrap()
                .entries
                .len(),
            1
        );
    }

    #[test]
    fn stale_phase_and_session_confirmations_cannot_stop_their_successors() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hs.db");
        let mut store = Store::open(&db).unwrap();
        let task = task(&mut store, "Review");
        let config = pomodoro::Config {
            focus_seconds: 1,
            break_seconds: 1,
            long_break_seconds: 1,
            rounds: 2,
        };
        pomodoro::start_task(&mut store, task.get(), config, &moment(0)).unwrap();
        pomodoro::next(&mut store, &moment(1)).unwrap();
        let expected =
            TimerExpectation::from_status(&queries::status(&mut store, &moment(1)).unwrap());
        let mut other = Store::open(&db).unwrap();
        pomodoro::next(&mut other, &moment(2)).unwrap();
        pomodoro::next(&mut other, &moment(3)).unwrap();
        let successor =
            TimerExpectation::from_status(&queries::status(&mut other, &moment(3)).unwrap());
        // Both are running breaks with no current work entry in the same session.
        assert_eq!(expected.current_entry, successor.current_entry);
        assert_eq!(
            expected.session.as_ref().unwrap().id,
            successor.session.as_ref().unwrap().id
        );
        assert_ne!(
            expected.session.as_ref().unwrap().phase_id,
            successor.session.as_ref().unwrap().phase_id
        );
        assert!(matches!(
            timer(&mut store, &expected, TimerAction::Stop, &moment(3)),
            Err(Error::Conflict(_))
        ));
        assert_eq!(
            TimerExpectation::from_status(&queries::status(&mut store, &moment(3)).unwrap()),
            successor
        );
        pomodoro::stop(&mut other, &moment(3)).unwrap();
        pomodoro::start_task(&mut other, task.get(), config, &moment(3)).unwrap();
        let replacement =
            TimerExpectation::from_status(&queries::status(&mut other, &moment(3)).unwrap());
        assert!(matches!(
            timer(&mut store, &successor, TimerAction::Stop, &moment(3)),
            Err(Error::Conflict(_))
        ));
        assert_eq!(
            TimerExpectation::from_status(&queries::status(&mut store, &moment(3)).unwrap()),
            replacement
        );
    }

    #[test]
    fn fresh_actions_allow_pause_resume_and_ignore_pure_clock_observations() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let task = task(&mut store, "Review");
        let expected =
            TimerExpectation::from_status(&queries::status(&mut store, &moment(0)).unwrap());
        timer(
            &mut store,
            &expected,
            TimerAction::Pomodoro(task),
            &moment(0),
        )
        .unwrap();
        let expected =
            TimerExpectation::from_status(&queries::status(&mut store, &moment(1)).unwrap());
        queries::status(&mut store, &moment(2)).unwrap();
        timer(&mut store, &expected, TimerAction::Pause, &moment(3)).unwrap();
        let paused = queries::status(&mut store, &moment(4)).unwrap();
        assert_eq!(paused.session.as_ref().unwrap().state(), State::Paused);
        let remaining = paused.session.as_ref().unwrap().remaining(&moment(4));
        timer(
            &mut store,
            &TimerExpectation::from_status(&paused),
            TimerAction::Resume,
            &moment(10),
        )
        .unwrap();
        let resumed = queries::status(&mut store, &moment(10)).unwrap();
        assert_eq!(
            resumed.session.as_ref().unwrap().remaining(&moment(10)),
            remaining
        );
        timer(
            &mut store,
            &TimerExpectation::from_status(&resumed),
            TimerAction::Stop,
            &moment(11),
        )
        .unwrap();
        let stopped = queries::status(&mut store, &moment(11)).unwrap();
        assert!(stopped.current.is_none());
        assert_eq!(stopped.session.as_ref().unwrap().state(), State::Stopped);
    }

    #[test]
    fn stale_task_confirmation_preserves_changed_task_and_its_running_work() {
        for state in [TaskStatus::Done, TaskStatus::Archived] {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("hs.db");
            let mut store = Store::open(&db).unwrap();
            let id = task(&mut store, "Original outcome");
            workflows::start_task(&mut store, id, &moment(1)).unwrap();
            let status = queries::status(&mut store, &moment(2)).unwrap();
            let expected =
                TaskStatusExpectation::from_task(&tasks::get(&store, id.get()).unwrap(), &status);
            let mut other = Store::open(&db).unwrap();
            tasks::edit(&mut other, id.get(), |task| {
                task.title = "Changed outcome requiring more work".into();
                Ok(())
            })
            .unwrap();
            assert!(matches!(
                task_status(&mut store, &expected, state, &moment(3)),
                Err(Error::Conflict(_))
            ));
            let changed = tasks::get(&store, id.get()).unwrap();
            assert_eq!(changed.title, "Changed outcome requiring more work");
            assert_eq!(changed.status, TaskStatus::Open);
            assert_eq!(
                queries::status(&mut store, &moment(3)).unwrap().current,
                status.current
            );

            // Reopening the confirmation acknowledges the new task and can
            // still finish it and its work through the shared workflow.
            let expected = TaskStatusExpectation::from_task(&changed, &status);
            let changed = task_status(&mut store, &expected, state, &moment(4)).unwrap();
            assert_eq!(changed.status, state);
            assert!(
                queries::status(&mut store, &moment(4))
                    .unwrap()
                    .current
                    .is_none()
            );
        }
    }

    #[test]
    fn task_confirmation_rejects_new_associated_timer_but_allows_unrelated_work() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hs.db");
        let mut store = Store::open(&db).unwrap();
        let id = task(&mut store, "Confirm this");
        let unrelated = task(&mut store, "Other work");
        let task = tasks::get(&store, id.get()).unwrap();
        let expected = TaskStatusExpectation::from_task(
            &task,
            &queries::status(&mut store, &moment(1)).unwrap(),
        );
        let mut other = Store::open(&db).unwrap();
        pomodoro::start_task(&mut other, id.get(), Default::default(), &moment(2)).unwrap();
        let running = queries::status(&mut other, &moment(3)).unwrap();
        for state in [TaskStatus::Done, TaskStatus::Archived] {
            assert!(matches!(
                task_status(&mut store, &expected, state, &moment(3)),
                Err(Error::Conflict(_))
            ));
            assert_eq!(tasks::get(&store, id.get()).unwrap(), task);
            assert_eq!(
                TimerExpectation::from_status(&queries::status(&mut store, &moment(3)).unwrap()),
                TimerExpectation::from_status(&running)
            );
        }

        workflows::start_task(&mut other, unrelated, &moment(4)).unwrap();
        let before = queries::status(&mut other, &moment(4)).unwrap();
        let done = task_status(&mut store, &expected, TaskStatus::Done, &moment(5)).unwrap();
        assert_eq!(done.status, TaskStatus::Done);
        assert_eq!(
            queries::status(&mut store, &moment(5)).unwrap().current,
            before.current
        );
    }
}
