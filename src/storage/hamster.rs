//! Read a consistent snapshot of a Hamster v9 database into hs's independent schema.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::NaiveDateTime;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, Transaction, params};

use crate::model::NewEntry;
use crate::store::{insert_entry, literal_path};
use crate::{Error, Result, domain::import::ImportSummary};

struct LegacyActivity {
    name: String,
    category_id: Option<i64>,
    archived: bool,
}

struct LegacyTag {
    name: String,
    autocomplete: bool,
}

struct LegacyFact {
    id: i64,
    activity_id: i64,
    start: String,
    end: Option<String>,
    description: Option<String>,
}

pub(crate) struct Snapshot {
    categories: BTreeMap<i64, String>,
    activities: BTreeMap<i64, LegacyActivity>,
    tags: BTreeMap<i64, LegacyTag>,
    fact_tags: BTreeMap<i64, BTreeSet<i64>>,
    facts: Vec<LegacyFact>,
}

fn read_rows<T>(
    source: &Connection,
    table: &str,
    sql: &str,
    mut read: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let result = (|| -> rusqlite::Result<Vec<T>> {
        let mut stmt = source.prepare(sql)?;
        stmt.query_map([], &mut read)?.collect()
    })();
    result.map_err(|error| Error::Input(format!("invalid Hamster {table} table: {error}")))
}

fn valid_name(name: String, kind: &str, id: i64) -> Result<String> {
    if name.trim().is_empty() {
        return Err(Error::Input(format!(
            "Hamster {kind} {id} has an empty name"
        )));
    }
    Ok(name)
}

// Hamster has used both integer and textual values in BOOL columns.
fn flag(value: Option<String>, default: bool, kind: &str, id: i64) -> Result<bool> {
    match value.as_deref().map(str::to_ascii_lowercase).as_deref() {
        None => Ok(default),
        Some("true" | "1") => Ok(true),
        Some("false" | "0") => Ok(false),
        Some(other) => Err(Error::Input(format!(
            "invalid Hamster {kind} flag {other:?} on ID {id}"
        ))),
    }
}

fn insert_legacy_id<T>(map: &mut BTreeMap<i64, T>, id: i64, data: T, table: &str) -> Result<()> {
    if map.insert(id, data).is_some() {
        return Err(Error::Input(format!(
            "duplicate ID {id} in Hamster {table}"
        )));
    }
    Ok(())
}

pub(crate) fn read_snapshot(path: &Path) -> Result<Snapshot> {
    let path = literal_path(path)?;
    let mut source =
        Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|error| {
            Error::Input(format!(
                "cannot open Hamster database {}: {error}",
                path.display()
            ))
        })?;
    source.pragma_update(None, "trusted_schema", false)?;
    let source = source.transaction()?;
    let versions: Vec<i64> = read_rows(&source, "version", "SELECT version FROM version", |r| {
        r.get(0)
    })?;
    if versions != [9] {
        return Err(Error::Input(format!(
            "unsupported Hamster schema {versions:?}; expected version 9"
        )));
    }

    let mut categories = BTreeMap::new();
    for (id, name) in read_rows(
        &source,
        "categories",
        "SELECT id, name FROM categories",
        |r| Ok((r.get(0)?, r.get(1)?)),
    )? {
        insert_legacy_id(
            &mut categories,
            id,
            valid_name(name, "category", id)?,
            "categories",
        )?;
    }
    let mut activities = BTreeMap::new();
    for (id, name, category_id, deleted) in read_rows(
        &source,
        "activities",
        "SELECT id, name, category_id, CAST(deleted AS TEXT) FROM activities",
        |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<i64>>(2)?, r.get(3)?)),
    )? {
        // Hamster uses -1 for the uncategorized activity.
        let category_id = category_id.filter(|&c| c != -1);
        if let Some(c) = category_id
            && !categories.contains_key(&c)
        {
            return Err(Error::Input(format!(
                "Hamster activity {id} references missing category {c}"
            )));
        }
        insert_legacy_id(
            &mut activities,
            id,
            LegacyActivity {
                name: valid_name(name, "activity", id)?,
                category_id,
                archived: flag(deleted, false, "deleted", id)?,
            },
            "activities",
        )?;
    }
    let mut tags = BTreeMap::new();
    for (id, name, autocomplete) in read_rows(
        &source,
        "tags",
        "SELECT id, name, CAST(autocomplete AS TEXT) FROM tags",
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )? {
        insert_legacy_id(
            &mut tags,
            id,
            LegacyTag {
                name: valid_name(name, "tag", id)?,
                autocomplete: flag(autocomplete, true, "autocomplete", id)?,
            },
            "tags",
        )?;
    }
    let facts = read_rows(
        &source,
        "facts",
        "SELECT id, activity_id, start_time, end_time, description FROM facts ORDER BY id",
        |r| {
            Ok(LegacyFact {
                id: r.get(0)?,
                activity_id: r.get(1)?,
                start: r.get(2)?,
                end: r.get(3)?,
                description: r.get(4)?,
            })
        },
    )?;
    let mut fact_ids = BTreeSet::new();
    for fact in &facts {
        if !fact_ids.insert(fact.id) {
            return Err(Error::Input(format!(
                "duplicate ID {} in Hamster facts",
                fact.id
            )));
        }
    }
    let mut fact_tags = BTreeMap::<i64, BTreeSet<i64>>::new();
    for (fact_id, tag_id) in read_rows(
        &source,
        "fact_tags",
        "SELECT fact_id, tag_id FROM fact_tags",
        |r| Ok((r.get(0)?, r.get(1)?)),
    )? {
        if !fact_ids.contains(&fact_id) || !tags.contains_key(&tag_id) {
            return Err(Error::Input(format!(
                "Hamster fact_tags has a dangling link from fact {fact_id} to tag {tag_id}"
            )));
        }
        fact_tags.entry(fact_id).or_default().insert(tag_id);
    }
    Ok(Snapshot {
        categories,
        activities,
        tags,
        facts,
        fact_tags,
    })
}

pub(crate) fn legacy_time(id: i64, raw: &str) -> Result<NaiveDateTime> {
    NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f").map_err(|_| {
        Error::Input(format!(
            "Hamster fact {id} has an invalid local timestamp {raw:?}"
        ))
    })
}

impl Snapshot {
    fn category(&self, activity: &LegacyActivity) -> Option<&str> {
        activity.category_id.map(|id| self.categories[&id].as_str())
    }

    fn import_vocabulary(&self, tx: &Connection, summary: &mut ImportSummary) -> Result<()> {
        for name in self.categories.values() {
            summary.categories += tx.execute(
                "INSERT INTO categories(name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
                [name],
            )?;
        }
        // Hamster may have several legacy IDs for the same name. An active
        // spelling wins when combining its vocabulary into hs for the first time.
        let mut activities = BTreeMap::<(&str, &str), bool>::new();
        for activity in self.activities.values() {
            let key = (
                activity.name.as_str(),
                self.category(activity).unwrap_or(""),
            );
            let archived = activities.entry(key).or_insert(true);
            *archived &= activity.archived;
        }
        for ((name, category), archived) in activities {
            summary.activities += tx.execute(
                "INSERT INTO activities(name, category_id, archived)
                 VALUES (?1, (SELECT id FROM categories WHERE name=?2), ?3)
                 ON CONFLICT(name, category_id) DO NOTHING",
                params![name, category, archived],
            )?;
        }
        let mut tags = BTreeMap::<&str, bool>::new();
        for tag in self.tags.values() {
            *tags.entry(&tag.name).or_default() |= tag.autocomplete;
        }
        for (name, autocomplete) in tags {
            summary.tags += tx.execute(
                "INSERT INTO tags(name, autocomplete) VALUES (?1, ?2) ON CONFLICT(name) DO NOTHING",
                params![name, autocomplete],
            )?;
        }
        Ok(())
    }

    fn entry(&self, fact: &LegacyFact) -> Result<(NewEntry, String)> {
        let activity = self.activities.get(&fact.activity_id).ok_or_else(|| {
            Error::Input(format!(
                "Hamster fact {} references missing activity {}",
                fact.id, fact.activity_id
            ))
        })?;
        let category = self.category(activity);
        let tag_links: Vec<_> = self
            .fact_tags
            .get(&fact.id)
            .into_iter()
            .flatten()
            .map(|&id| (id, self.tags[&id].name.as_str()))
            .collect();
        let fingerprint = crate::digest::sha256_hex(serde_json::to_vec(&(
            fact.id,
            fact.activity_id,
            &activity.name,
            activity.category_id,
            category,
            &fact.description,
            &fact.start,
            &fact.end,
            &tag_links,
        ))?);
        let start = legacy_time(fact.id, &fact.start)?;
        let end = fact
            .end
            .as_deref()
            .map(|raw| legacy_time(fact.id, raw))
            .transpose()?;
        if end.is_some_and(|e| e < start) {
            return Err(Error::Input(format!(
                "Hamster fact {} ends before it starts",
                fact.id
            )));
        }
        let tags = tag_links
            .into_iter()
            .map(|(_, name)| name.to_owned())
            .collect::<BTreeSet<_>>();
        let entry = NewEntry {
            activity: activity.name.clone(),
            category: category.map(str::to_owned),
            description: fact.description.clone().unwrap_or_default(),
            tags: tags.into_iter().collect(),
            start,
            end,
            clock: None,
            task: None,
        };
        Ok((entry, fingerprint))
    }
}

impl Snapshot {
    pub(crate) fn import_into(
        &self,
        tx: &Transaction<'_>,
        source_name: &str,
    ) -> Result<ImportSummary> {
        tx.execute(
            "INSERT INTO import_sources(name) VALUES (?) ON CONFLICT(name) DO NOTHING",
            [source_name],
        )?;
        let source_id: i64 = tx.query_row(
            "SELECT id FROM import_sources WHERE name=?",
            [source_name],
            |r| r.get(0),
        )?;
        let mut summary = ImportSummary::default();
        self.import_vocabulary(tx, &mut summary)?;
        for fact in &self.facts {
            let (entry, fingerprint) = self.entry(fact)?;
            let previous: Option<String> = tx
                .query_row(
                    "SELECT fingerprint FROM import_entries WHERE source_id=? AND legacy_id=?",
                    params![source_id, fact.id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(previous) = previous {
                if previous != fingerprint {
                    return Err(Error::Conflict(format!(
                        "Hamster source {source_name:?}, fact {} changed since its first import; no changes were imported",
                        fact.id
                    )));
                }
                summary.skipped += 1;
                continue;
            }
            let id = insert_entry(tx, &entry)?;
            tx.execute(
            "INSERT INTO import_entries(source_id, legacy_id, entry_id, fingerprint, source_start, source_end)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![source_id, fact.id, id, fingerprint, fact.start, fact.end],
        )?;
            summary.imported += 1;
            summary.unresolved += usize::from(entry.end.is_none());
        }

        Ok(summary)
    }
}
