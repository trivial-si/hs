CREATE TABLE categories (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO categories(id, name) VALUES (1, '');
CREATE TABLE activities (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    category_id INTEGER NOT NULL REFERENCES categories(id),
    archived INTEGER NOT NULL DEFAULT 0,
    UNIQUE(name, category_id)
);
CREATE TABLE entries (
    id INTEGER PRIMARY KEY,
    activity_id INTEGER NOT NULL REFERENCES activities(id),
    description TEXT NOT NULL DEFAULT '',
    start_time TEXT NOT NULL,
    end_time TEXT,
    CHECK(end_time IS NULL OR end_time >= start_time)
);
CREATE INDEX entries_start ON entries(start_time);
CREATE TABLE tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    autocomplete INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE entry_tags (
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id),
    PRIMARY KEY(entry_id, tag_id)
);
CREATE TABLE current_entry (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    entry_id INTEGER UNIQUE REFERENCES entries(id) ON DELETE SET NULL
);
INSERT INTO current_entry(singleton, entry_id) VALUES (1, NULL);
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT INTO settings VALUES ('day_start', '0'), ('week_start', 'monday');
CREATE TABLE import_sources (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
CREATE TABLE import_entries (
    source_id INTEGER NOT NULL REFERENCES import_sources(id),
    legacy_id INTEGER NOT NULL,
    entry_id INTEGER REFERENCES entries(id) ON DELETE SET NULL,
    fingerprint TEXT NOT NULL,
    source_start TEXT NOT NULL,
    source_end TEXT,
    PRIMARY KEY(source_id, legacy_id)
);
PRAGMA user_version = 1;
PRAGMA application_id = 1752367105;

