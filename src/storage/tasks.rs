//! Task queries and writes. Multi-statement writes share the use case's transaction.
use chrono::{DateTime, Utc};
use rusqlite::{Connection, ToSql, Transaction, params};

use crate::{
    Error, Result,
    domain::tasks::{Quadrant, Task, TaskInput, TaskStatus},
    store::{ensure_activity, ensure_tag},
};

fn timestamp(value: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp(value, 0).ok_or_else(|| {
        Error::Input(format!(
            "Task timestamp {value} is outside the supported range."
        ))
    })
}

// All fields, including tags, come from one SQLite snapshot. Adjacent rows
// are grouped by task, since the final sort keys are task ID then tag name.
fn read_tasks(conn: &Connection, filter: &str, values: &[&dyn ToSql]) -> Result<Vec<Task>> {
    let query = format!(
        "SELECT t.id, t.title, a.name, NULLIF(c.name, ''), t.description,
                t.quadrant, t.due_date, t.status, t.created_utc, t.completed_utc, g.name
         FROM tasks t JOIN activities a ON a.id=t.activity_id JOIN categories c ON c.id=a.category_id
         LEFT JOIN task_tags tg ON tg.task_id=t.id LEFT JOIN tags g ON g.id=tg.tag_id
         {filter}
         ORDER BY CASE t.quadrant WHEN 'do' THEN 0 WHEN 'plan' THEN 1 WHEN 'delegate' THEN 2
                                  WHEN 'drop' THEN 3 ELSE 4 END,
                  t.due_date IS NULL, CAST(t.due_date AS INTEGER), t.due_date, t.id, g.name"
    );
    let mut statement = conn.prepare(&query)?;
    let mut rows = statement.query(values)?;
    let mut result = Vec::<Task>::new();
    while let Some(row) = rows.next()? {
        let id = row.get(0)?;
        if result.last().is_none_or(|task| task.id != id) {
            let due: Option<String> = row.get(6)?;
            let due_date = due
                .as_deref()
                .map(|value| {
                    value.parse().map_err(|_| {
                        Error::Input(format!("Task {id} has an invalid due date {value:?}."))
                    })
                })
                .transpose()?;
            result.push(Task {
                id,
                title: row.get(1)?,
                activity: row.get(2)?,
                category: row.get(3)?,
                description: row.get(4)?,
                tags: Vec::new(),
                quadrant: row
                    .get::<_, Option<String>>(5)?
                    .as_deref()
                    .map(Quadrant::parse)
                    .transpose()?,
                due_date,
                status: TaskStatus::parse(&row.get::<_, String>(7)?)?,
                created_utc: timestamp(row.get(8)?)?,
                completed_utc: row.get::<_, Option<i64>>(9)?.map(timestamp).transpose()?,
            });
        }
        if let Some(tag) = row.get::<_, Option<String>>(10)? {
            result
                .last_mut()
                .expect("task has been added")
                .tags
                .push(tag);
        }
    }
    Ok(result)
}

pub(crate) fn load_from(conn: &Connection, id: i64) -> Result<Task> {
    read_tasks(conn, "WHERE t.id=?", &[&id])?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Input(format!("Task {id} does not exist.")))
}

pub(crate) fn list_from(conn: &Connection, status: Option<TaskStatus>) -> Result<Vec<Task>> {
    match status {
        Some(status) => read_tasks(conn, "WHERE t.status=?", &[&status.key()]),
        None => read_tasks(conn, "", &[]),
    }
}

pub(crate) fn next_from(conn: &Connection) -> Result<Option<Task>> {
    // Limit candidates before joining their tags. Due dates order tasks
    // inside each quadrant and never silently reclassify a task.
    let filter = "WHERE t.id=(
        SELECT id FROM tasks WHERE status='open' AND quadrant IN ('do','plan')
        ORDER BY CASE quadrant WHEN 'do' THEN 0 ELSE 1 END, due_date IS NULL, CAST(due_date AS INTEGER), due_date, id LIMIT 1
    )";
    Ok(read_tasks(conn, filter, &[])?.into_iter().next())
}

fn write_tags(conn: &Transaction<'_>, id: i64, tags: &[String]) -> Result<()> {
    for name in tags {
        let tag_id = ensure_tag(conn, name)?;
        conn.execute(
            "INSERT INTO task_tags(task_id,tag_id) SELECT ?1,?2
             WHERE NOT EXISTS (SELECT 1 FROM task_tags WHERE task_id=?1 AND tag_id=?2)",
            params![id, tag_id],
        )?;
    }
    Ok(())
}

pub(crate) fn insert(
    tx: &Transaction<'_>,
    input: &TaskInput,
    created: DateTime<Utc>,
) -> Result<i64> {
    let entry = &input.activity;
    let activity = ensure_activity(tx, &entry.activity, entry.category.as_deref())?;
    tx.execute(
        "INSERT INTO tasks(title,activity_id,description,quadrant,due_date,created_utc)
         VALUES(?,?,?,?,?,?)",
        params![
            input.title.trim(),
            activity,
            entry.description,
            input.quadrant.map(Quadrant::key),
            input.due_date.map(|d| d.to_string()),
            created.timestamp()
        ],
    )?;
    let id = tx.last_insert_rowid();
    write_tags(tx, id, &entry.tags)?;
    Ok(id)
}

pub(crate) fn update(tx: &Transaction<'_>, task: &Task) -> Result<()> {
    let activity = ensure_activity(tx, &task.activity, task.category.as_deref())?;
    tx.execute(
        "UPDATE tasks SET title=?,activity_id=?,description=?,quadrant=?,due_date=? WHERE id=?",
        params![
            task.title.trim(),
            activity,
            task.description,
            task.quadrant.map(Quadrant::key),
            task.due_date.map(|d| d.to_string()),
            task.id
        ],
    )?;
    tx.execute(
        "DELETE FROM task_tags WHERE task_id=?1 AND tag_id NOT IN
         (SELECT t.id FROM json_each(?2) selected JOIN tags t ON t.name=selected.value)",
        params![task.id, serde_json::to_string(&task.tags)?],
    )?;
    write_tags(tx, task.id, &task.tags)?;
    Ok(())
}

pub(crate) fn update_status(tx: &Transaction<'_>, task: &Task) -> Result<()> {
    tx.execute(
        "UPDATE tasks SET status=?,completed_utc=? WHERE id=?",
        params![
            task.status.key(),
            task.completed_utc.map(|time| time.timestamp()),
            task.id
        ],
    )?;
    Ok(())
}
