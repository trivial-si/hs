-- Keep public entry IDs stable across deletion, process restarts and backups.
-- Store::open runs this inside a transaction with foreign keys disabled so the
-- table rebuild does not cascade deletes into entry_tags or the current timer.
CREATE TABLE entries_v2 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    activity_id INTEGER NOT NULL REFERENCES activities(id),
    description TEXT NOT NULL DEFAULT '',
    start_time TEXT NOT NULL,
    end_time TEXT,
    CHECK(end_time IS NULL OR end_time >= start_time)
);
INSERT INTO entries_v2(id, activity_id, description, start_time, end_time)
SELECT id, activity_id, description, start_time, end_time FROM entries;
DROP TABLE entries;
ALTER TABLE entries_v2 RENAME TO entries;
-- Existing entries indexes and triggers are recreated by Store::open.
PRAGMA user_version = 2;
