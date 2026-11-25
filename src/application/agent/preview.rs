//! Simulate one checked write on a private SQLite copy and describe its effects.
use super::{Fault, WireResult, command::Command, views};
use crate::{
    domain::time::Moment,
    storage::{agent as persistence, entries, tasks},
};
use rusqlite::Transaction;
use serde_json::{Value, json};

pub(super) struct Simulation {
    before: Value,
    task_before: Option<Value>,
    conditions: Value,
}

impl Simulation {
    pub(super) fn begin(
        tx: &Transaction<'_>,
        command: &Command,
        conditions: Value,
        now: &Moment,
    ) -> WireResult<Self> {
        let before = views::context(tx, now, 10)?;
        let task_before =
            if let Command::TaskEdit { id, .. } | Command::TaskStatus { id, .. } = command {
                Some(views::task(tx, &tasks::load_from(tx, id.get())?)?)
            } else {
                None
            };
        persistence::begin_preview(tx)?;
        Ok(Self {
            before,
            task_before,
            conditions,
        })
    }

    pub(super) fn finish(self, tx: &Transaction<'_>, data: &mut Value) -> WireResult<()> {
        let changes = persistence::preview_entry_changes(tx, 100)?;
        if changes.len() > 100 {
            return Err(Fault::conflict(
                "preview_too_large",
                "Preview changes more than 100 entries; narrow the requested interval",
                json!({"limit":100}),
            ));
        }
        let mut after = Vec::with_capacity(changes.len());
        for change in &changes {
            after.push(entry_at(tx, change.id, change.exists)?);
        }
        persistence::rewind_preview(tx)?;
        let mut effects = Vec::with_capacity(changes.len());
        for (change, after) in changes.iter().zip(after) {
            let before = entry_at(tx, change.id, change.existed)?;
            effects.push(json!({"entry_id":change.id,"before":before,"after":after}));
        }
        data["preview"] = json!(true);
        data["provisional"] = json!(true);
        data["before"] = self.before;
        data["entry_effects"] = json!(effects);
        data["task_before"] = json!(self.task_before);
        data["commit_preconditions"] = self.conditions;
        Ok(())
    }
}

fn entry_at(tx: &Transaction<'_>, id: i64, exists: bool) -> WireResult<Option<Value>> {
    if exists {
        Ok(Some(views::entry(tx, &entries::find_entry(tx, id)?)?))
    } else {
        Ok(None)
    }
}
