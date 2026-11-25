CREATE TABLE tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL CHECK(length(trim(title)) > 0),
    activity_id INTEGER NOT NULL REFERENCES activities(id),
    description TEXT NOT NULL DEFAULT '',
    quadrant TEXT CHECK(quadrant IN ('do','plan','delegate','drop')),
    due_date TEXT,
    status TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open','done','archived')),
    created_utc INTEGER NOT NULL,
    completed_utc INTEGER
);
CREATE INDEX tasks_status_due ON tasks(status, quadrant, due_date, id);
CREATE TABLE task_tags (
    task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id),
    PRIMARY KEY(task_id, tag_id)
);

-- Priorities belong to the work entry at the time it is created. Keeping the
-- snapshot independent of the task makes old reports stable after reprioritizing.
ALTER TABLE entries ADD COLUMN task_id INTEGER REFERENCES tasks(id);
ALTER TABLE entries ADD COLUMN quadrant_snapshot TEXT
    CHECK(quadrant_snapshot IS NULL OR (task_id IS NOT NULL AND quadrant_snapshot IN ('do','plan','delegate','drop')));
CREATE INDEX entries_task ON entries(task_id) WHERE task_id IS NOT NULL;
ALTER TABLE pomodoro_sessions ADD COLUMN task_id INTEGER REFERENCES tasks(id);
PRAGMA user_version = 4;
