//! Bounded keyset pages. Bind every caller-supplied filter as a parameter.
use crate::Result;
use rusqlite::{Connection, functions::FunctionFlags, params};

/// SQLite's built-in lower() only handles ASCII. Use the same Unicode
/// lowercase conversion as the human CLI while filtering before pagination.
pub(crate) fn register_search(conn: &Connection) -> Result<()> {
    conn.create_scalar_function(
        "hs_lower",
        1,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| Ok(ctx.get::<String>(0)?.to_lowercase()),
    )?;
    Ok(())
}

pub(crate) struct Filter<'a> {
    pub after: i64,
    pub limit: u32,
    pub status: Option<&'a str>,
    pub quadrant: Option<Option<&'a str>>,
    pub search: Option<&'a str>,
    pub task_id: Option<i64>,
}
pub(crate) fn task_ids(conn: &Connection, f: &Filter<'_>) -> Result<Vec<i64>> {
    let mut statement=conn.prepare("SELECT t.id FROM tasks t JOIN activities a ON a.id=t.activity_id JOIN categories c ON c.id=a.category_id
        WHERE t.id>?1 AND (?2 IS NULL OR t.status=?2) AND (?3=0 OR t.quadrant IS ?4)
        AND (?5 IS NULL OR instr(hs_lower(t.title||' '||a.name||' '||c.name||' '||t.description),hs_lower(?5))>0
             OR EXISTS(SELECT 1 FROM task_tags tt JOIN tags g ON g.id=tt.tag_id WHERE tt.task_id=t.id AND instr(hs_lower(g.name),hs_lower(?5))>0))
        ORDER BY t.id LIMIT ?6")?;
    Ok(statement
        .query_map(
            params![
                f.after,
                f.status,
                f.quadrant.is_some(),
                f.quadrant.flatten(),
                f.search,
                f.limit + 1
            ],
            |r| r.get(0),
        )?
        .collect::<rusqlite::Result<_>>()?)
}
pub(crate) fn entry_ids(conn: &Connection, f: &Filter<'_>) -> Result<Vec<i64>> {
    // Exposing the equality to SQLite lets the task/id index both select
    // the linked records and emit them in cursor order.
    let task_clause = if f.task_id.is_some() {
        "AND e.task_id=?2"
    } else {
        ""
    };
    let sql = format!(
        "SELECT e.id FROM entries e JOIN activities a ON a.id=e.activity_id
         JOIN categories c ON c.id=a.category_id
         WHERE e.id>?1 {task_clause} AND (?3=0 OR e.quadrant_snapshot IS ?4)
           AND (?5 IS NULL OR instr(hs_lower(a.name||' '||c.name||' '||e.description),hs_lower(?5))>0
                OR EXISTS(SELECT 1 FROM entry_tags et JOIN tags g ON g.id=et.tag_id
                          WHERE et.entry_id=e.id AND instr(hs_lower(g.name),hs_lower(?5))>0))
         ORDER BY e.id LIMIT ?6"
    );
    let mut statement = conn.prepare(&sql)?;
    Ok(statement
        .query_map(
            params![
                f.after,
                f.task_id,
                f.quadrant.is_some(),
                f.quadrant.flatten(),
                f.search,
                f.limit + 1
            ],
            |r| r.get(0),
        )?
        .collect::<rusqlite::Result<_>>()?)
}
