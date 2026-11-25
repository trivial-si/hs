//! Task capture, planning and status workflows.
use crate::application::editing::TaskEdit;
use crate::domain::ids::TaskId;
use rusqlite::{Connection, Transaction};

use crate::{
    Error, Result, Store,
    model::{EntryTask, NewEntry},
    storage::tasks as repo,
    time::Moment,
};

pub use crate::domain::tasks::{Quadrant, Task, TaskInput, TaskStatus};
pub(crate) use repo::load_from;

pub fn get(store: &Store, id: i64) -> Result<Task> {
    load_from(&store.conn, id)
}

pub fn list(store: &Store, status: Option<TaskStatus>) -> Result<Vec<Task>> {
    repo::list_from(&store.conn, status)
}

pub fn matrix(store: &Store) -> Result<Vec<Task>> {
    list(store, Some(TaskStatus::Open))
}

pub fn next(store: &Store) -> Result<Option<Task>> {
    repo::next_from(&store.conn)
}

pub fn add(store: &mut Store, input: TaskInput, now: &Moment) -> Result<Task> {
    store.write(|tx| add_in_transaction(tx, input, now))
}

pub(crate) fn add_in_transaction(
    tx: &Transaction<'_>,
    input: TaskInput,
    now: &Moment,
) -> Result<Task> {
    input.validate()?;
    let id = repo::insert(tx, &input, now.utc)?;
    load_from(tx, id)
}

pub fn edit<F>(store: &mut Store, id: i64, change: F) -> Result<Task>
where
    F: FnOnce(&mut Task) -> Result<()>,
{
    store.write(|tx| apply_edit(tx, id, change))
}

pub(crate) fn edit_task_in_transaction(
    tx: &Transaction<'_>,
    id: TaskId,
    edit: TaskEdit,
) -> Result<Task> {
    apply_edit(tx, id.get(), |task| edit.apply(task))
}

fn apply_edit(
    tx: &Transaction<'_>,
    id: i64,
    change: impl FnOnce(&mut Task) -> Result<()>,
) -> Result<Task> {
    let mut task = load_from(tx, id)?;
    let immutable = (task.id, task.status, task.created_utc, task.completed_utc);
    change(&mut task)?;
    if (task.id, task.status, task.created_utc, task.completed_utc) != immutable {
        return Err(Error::Input(
            "Task ID and history cannot be changed through an edit. Use a task status command."
                .into(),
        ));
    }
    task.validate()?;
    repo::update(tx, &task)?;
    load_from(tx, id)
}

pub(crate) fn template_from(conn: &Connection, id: i64, now: &Moment) -> Result<NewEntry> {
    let task = load_from(conn, id)?;
    if task.status != TaskStatus::Open {
        return Err(Error::Conflict(format!(
            "Task {id} is {}. Reopen it before starting work.",
            task.status.key()
        )));
    }
    let description = if task.description.is_empty() {
        task.title
    } else {
        format!("{} — {}", task.title, task.description)
    };
    Ok(NewEntry {
        activity: task.activity,
        category: task.category,
        description,
        tags: task.tags,
        start: now.local()?,
        end: None,
        clock: None,
        task: Some(EntryTask {
            id,
            quadrant: task.quadrant,
        }),
    })
}

pub(crate) fn set_status_in_transaction(
    conn: &Transaction<'_>,
    id: i64,
    status: TaskStatus,
    now: &Moment,
) -> Result<Task> {
    let mut task = load_from(conn, id)?;
    if !task.change_status(status, now.utc)? {
        return Ok(task);
    }
    repo::update_status(conn, &task)?;
    load_from(conn, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ActivityTemplate;
    use crate::store::{insert_entry, update_entry, write_fixture};
    use chrono::Datelike;

    fn fixed_now() -> Moment {
        Moment::at("2026-03-01T10:00:00Z".parse().unwrap(), chrono_tz::UTC)
    }

    fn task_input(title: &str, quadrant: Option<Quadrant>, due: Option<&str>) -> TaskInput {
        TaskInput {
            title: title.into(),
            activity: ActivityTemplate {
                activity: "Maintenance".into(),
                category: Some("Work".into()),
                description: "Run diagnostics".into(),
                tags: vec!["tag".into(), "tag".into(), "Šifra".into()],
            },
            quadrant,
            due_date: due.map(|s| s.parse().unwrap()),
        }
    }

    #[test]
    fn ranking_uses_priority_then_due_date_then_id_and_skips_nonfocus_quadrants() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("tasks.db")).unwrap();
        let now = fixed_now();
        let unsorted = add(&mut store, task_input("Triage later", None, None), &now).unwrap();
        let delegate = add(
            &mut store,
            task_input("Hand off", Some(Quadrant::Delegate), Some("2020-01-01")),
            &now,
        )
        .unwrap();
        let drop_task = add(
            &mut store,
            task_input("Skip", Some(Quadrant::Drop), None),
            &now,
        )
        .unwrap();
        assert_eq!(next(&store).unwrap(), None);
        let plan = add(
            &mut store,
            task_input("Design", Some(Quadrant::Plan), Some("2020-01-02")),
            &now,
        )
        .unwrap();
        let undated = add(
            &mut store,
            task_input("Fix", Some(Quadrant::Do), None),
            &now,
        )
        .unwrap();
        let first = add(
            &mut store,
            task_input("Fix", Some(Quadrant::Do), Some("2026-03-05")),
            &now,
        )
        .unwrap();
        let second = add(
            &mut store,
            task_input("Fix", Some(Quadrant::Do), Some("2026-03-05")),
            &now,
        )
        .unwrap();
        assert_eq!(next(&store).unwrap().unwrap().id, first.id);
        let ids = matrix(&store)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                first.id,
                second.id,
                undated.id,
                plan.id,
                delegate.id,
                drop_task.id,
                unsorted.id
            ]
        );
        write_fixture(&store.conn, |tx| {
            set_status_in_transaction(tx, first.id, TaskStatus::Done, &now)
        })
        .unwrap();
        assert_eq!(next(&store).unwrap().unwrap().id, second.id);
        assert_eq!(list(&store, Some(TaskStatus::Done)).unwrap().len(), 1);
    }

    #[test]
    fn snapshots_keep_original_metadata_and_task_history_remains_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("tasks.db");
        let mut store = Store::open(&path).unwrap();
        let now = fixed_now();
        let t = add(&mut store, task_input("Calibrate", None, None), &now).unwrap();
        let template = template_from(&store.conn, t.id, &now).unwrap();
        assert_eq!(template.activity, "Maintenance");
        assert_eq!(template.description, "Calibrate — Run diagnostics");
        assert_eq!(template.tags, ["tag", "Šifra"]);
        assert_eq!(
            template.task,
            Some(EntryTask {
                id: t.id,
                quadrant: None
            })
        );
        let prior = write_fixture(&store.conn, |tx| insert_entry(tx, &template)).unwrap();
        edit(&mut store, t.id, |t| {
            t.quadrant = Some(Quadrant::Plan);
            t.title = "Prepare".into();
            t.tags.clear();
            Ok(())
        })
        .unwrap();
        let later = template_from(&store.conn, t.id, &now).unwrap();
        let recent = write_fixture(&store.conn, |tx| insert_entry(tx, &later)).unwrap();
        write_fixture(&store.conn, |tx| update_entry(tx, prior, &template)).unwrap();
        let later_time = Moment::at(now.utc + chrono::Duration::days(1), now.zone);
        let completed = write_fixture(&store.conn, |tx| {
            set_status_in_transaction(tx, t.id, TaskStatus::Done, &now)
        })
        .unwrap();
        assert_eq!(completed.completed_utc, Some(now.utc));
        assert_eq!(
            write_fixture(&store.conn, |tx| set_status_in_transaction(
                tx,
                t.id,
                TaskStatus::Done,
                &later_time
            ))
            .unwrap()
            .completed_utc,
            Some(now.utc)
        );
        assert!(template_from(&store.conn, t.id, &now).is_err());
        let archived = write_fixture(&store.conn, |tx| {
            set_status_in_transaction(tx, t.id, TaskStatus::Archived, &later_time)
        })
        .unwrap();
        assert_eq!(archived.completed_utc, Some(now.utc));
        assert!(
            write_fixture(&store.conn, |tx| set_status_in_transaction(
                tx,
                t.id,
                TaskStatus::Done,
                &later_time
            ))
            .is_err()
        );
        assert!(template_from(&store.conn, t.id, &now).is_err());
        let reopened = write_fixture(&store.conn, |tx| {
            set_status_in_transaction(tx, t.id, TaskStatus::Open, &later_time)
        })
        .unwrap();
        assert_eq!(reopened.completed_utc, None);
        assert_eq!(
            template_from(&store.conn, t.id, &later_time)
                .unwrap()
                .task
                .unwrap()
                .quadrant,
            Some(Quadrant::Plan)
        );

        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.entry(prior).unwrap().as_new(), template);
        assert_eq!(store.entry(recent).unwrap().as_new(), later);
    }

    #[test]
    fn completion_during_a_clock_rollback_cannot_precede_creation() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("tasks.db")).unwrap();
        let created = fixed_now();
        let prior = Moment::at(created.utc - chrono::Duration::days(1), created.zone);
        let task = add(&mut store, task_input("Calibrate", None, None), &created).unwrap();
        let done = write_fixture(&store.conn, |tx| {
            set_status_in_transaction(tx, task.id, TaskStatus::Done, &prior)
        })
        .unwrap();
        assert_eq!(done.completed_utc, Some(done.created_utc));
    }

    #[test]
    fn extended_years_cannot_reorder_planned_tasks_or_leave_partial_writes() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("tasks.db")).unwrap();
        let now = fixed_now();
        let original = add(
            &mut store,
            task_input("Near", Some(Quadrant::Plan), Some("2026-11-25")),
            &now,
        )
        .unwrap();
        for value in ["+10000-01-01", "-0001-01-01"] {
            let mut input = task_input("Invalid", Some(Quadrant::Do), Some(value));
            input.activity.activity = "Uncommitted".into();
            assert!(add(&mut store, input, &now).is_err());
            assert!(
                edit(&mut store, original.id, |t| {
                    t.quadrant = Some(Quadrant::Do);
                    t.due_date = Some(value.parse().unwrap());
                    Ok(())
                })
                .is_err()
            );
        }
        assert_eq!(
            list(&store, None).unwrap().as_slice(),
            std::slice::from_ref(&original)
        );
        assert_eq!(next(&store).unwrap(), Some(original));
        assert!(
            !store
                .activities()
                .unwrap()
                .iter()
                .any(|(name, _)| name == "Uncommitted")
        );
        let earliest = add(
            &mut store,
            task_input("Earliest", Some(Quadrant::Plan), Some("0000-01-01")),
            &now,
        )
        .unwrap();
        add(
            &mut store,
            task_input("Latest", Some(Quadrant::Plan), Some("9999-12-31")),
            &now,
        )
        .unwrap();
        assert_eq!(next(&store).unwrap(), Some(earliest));
    }

    #[test]
    fn old_extended_due_dates_remain_readable_and_rank_chronologically() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("old-dates.db")).unwrap();
        let now = fixed_now();
        let near = add(
            &mut store,
            task_input("Near", Some(Quadrant::Do), Some("2026-03-04")),
            &now,
        )
        .unwrap();
        let far = add(
            &mut store,
            task_input("Far", Some(Quadrant::Do), None),
            &now,
        )
        .unwrap();
        // An early v4 release persisted Chrono's extended-year spelling.
        store
            .conn
            .execute(
                "UPDATE tasks SET due_date='+10000-01-01' WHERE id=?",
                [far.id],
            )
            .unwrap();
        assert_eq!(get(&store, far.id).unwrap().due_date.unwrap().year(), 10000);
        assert_eq!(next(&store).unwrap().unwrap().id, near.id);
        let tasks = matrix(&store).unwrap();
        assert_eq!((tasks[0].id, tasks[1].id), (near.id, far.id));
        edit(&mut store, far.id, |task| {
            task.due_date = None;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn failed_edit_rolls_back_changes_to_task_and_vocabulary() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("tasks.db")).unwrap();
        let now = fixed_now();
        let original = add(
            &mut store,
            task_input("Original", Some(Quadrant::Do), None),
            &now,
        )
        .unwrap();
        store.conn.execute_batch("CREATE TRIGGER reject_task_tag BEFORE INSERT ON task_tags BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
        assert!(
            edit(&mut store, original.id, |t| {
                t.title = "Changed".into();
                t.activity = "New activity".into();
                t.tags = vec!["new tag".into()];
                Ok(())
            })
            .is_err()
        );
        assert_eq!(get(&store, original.id).unwrap(), original);
        assert!(!store.names("tags").unwrap().contains(&"new tag".into()));
        assert!(
            !store
                .activities()
                .unwrap()
                .iter()
                .any(|(name, _)| name == "New activity")
        );
    }
}
