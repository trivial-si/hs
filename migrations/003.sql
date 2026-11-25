-- Keep local civil fields for legacy records. A live timer can cross a DST
-- fold with an earlier displayed end time and a later end instant.
CREATE TABLE entries_v3 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    activity_id INTEGER NOT NULL REFERENCES activities(id),
    description TEXT NOT NULL DEFAULT '',
    start_time TEXT NOT NULL,
    end_time TEXT,
    start_utc INTEGER,
    end_utc INTEGER,
    timezone TEXT,
    CHECK (
        (start_utc IS NULL AND end_utc IS NULL AND timezone IS NULL
            AND (end_time IS NULL OR end_time >= start_time))
        OR (start_utc IS NOT NULL AND timezone IS NOT NULL
            AND ((end_time IS NULL AND end_utc IS NULL)
                 OR (end_time IS NOT NULL AND end_utc IS NOT NULL AND end_utc >= start_utc)))
    )
);
INSERT INTO entries_v3(id, activity_id, description, start_time, end_time)
SELECT id, activity_id, description, start_time, end_time FROM entries;
-- An empty copy can still have a positive high-water mark if the user
-- deleted the latest entry or all entries from a previously used v2 store.
UPDATE sqlite_sequence SET seq=max(seq, COALESCE(
    (SELECT seq FROM sqlite_sequence WHERE name='entries'), 0
)) WHERE name='entries_v3';
DROP TABLE entries;
ALTER TABLE entries_v3 RENAME TO entries;
-- Store::open installs the instant index after restoring custom indexes.

CREATE TABLE pomodoro_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    activity TEXT NOT NULL,
    category TEXT,
    description TEXT NOT NULL DEFAULT '',
    tags TEXT NOT NULL DEFAULT '[]',
    focus_seconds INTEGER NOT NULL CHECK(focus_seconds > 0),
    break_seconds INTEGER NOT NULL CHECK(break_seconds > 0),
    long_break_seconds INTEGER NOT NULL CHECK(long_break_seconds > 0),
    rounds INTEGER NOT NULL CHECK(rounds > 0),
    completed_focus INTEGER NOT NULL DEFAULT 0 CHECK(completed_focus >= 0),
    interrupted_focus INTEGER NOT NULL DEFAULT 0 CHECK(interrupted_focus >= 0),
    phase TEXT NOT NULL CHECK(phase IN ('focus','short_break','long_break')),
    state TEXT NOT NULL CHECK(state IN ('running','paused','ready','stopped')),
    deadline_utc INTEGER,
    remaining_seconds INTEGER NOT NULL CHECK(remaining_seconds >= 0),
    current_entry_id INTEGER REFERENCES entries(id) ON DELETE SET NULL,
    current_phase_id INTEGER REFERENCES pomodoro_phases(id) ON DELETE SET NULL,
    timezone TEXT NOT NULL,
    created_utc INTEGER NOT NULL,
    last_observed_utc INTEGER NOT NULL
);
CREATE UNIQUE INDEX one_active_pomodoro ON pomodoro_sessions((1)) WHERE state != 'stopped';

CREATE TABLE pomodoro_phases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL REFERENCES pomodoro_sessions(id) ON DELETE CASCADE,
    phase TEXT NOT NULL CHECK(phase IN ('focus','short_break','long_break')),
    state TEXT NOT NULL CHECK(state IN ('running','paused','completed','interrupted')),
    started_utc INTEGER NOT NULL,
    ended_utc INTEGER,
    target_seconds INTEGER NOT NULL CHECK(target_seconds > 0),
    elapsed_seconds INTEGER NOT NULL DEFAULT 0 CHECK(elapsed_seconds >= 0)
);
CREATE INDEX pomodoro_session_phases ON pomodoro_phases(session_id);
CREATE TABLE pomodoro_segments (
    id INTEGER PRIMARY KEY,
    phase_id INTEGER NOT NULL REFERENCES pomodoro_phases(id) ON DELETE CASCADE,
    entry_id INTEGER UNIQUE REFERENCES entries(id) ON DELETE SET NULL
);
CREATE INDEX pomodoro_phase_segments ON pomodoro_segments(phase_id);
PRAGMA user_version = 3;
