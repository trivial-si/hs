-- Revisions are opaque optimistic-concurrency tokens. They count underlying
-- row changes and roll back together with the command that caused them.
CREATE TABLE agent_control (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    database_id TEXT NOT NULL CHECK(length(database_id) = 32),
    timer_revision INTEGER NOT NULL CHECK(typeof(timer_revision) = 'integer' AND timer_revision >= 0),
    history_revision INTEGER NOT NULL CHECK(typeof(history_revision) = 'integer' AND history_revision >= 0)
);
INSERT INTO agent_control VALUES (1, lower(hex(randomblob(16))), 0, 0);

ALTER TABLE tasks ADD COLUMN revision INTEGER NOT NULL DEFAULT 1
    CHECK(typeof(revision) = 'integer' AND revision > 0);
ALTER TABLE entries ADD COLUMN revision INTEGER NOT NULL DEFAULT 1
    CHECK(typeof(revision) = 'integer' AND revision > 0);

CREATE TABLE operation_receipts (
    request_id TEXT PRIMARY KEY NOT NULL,
    fingerprint TEXT NOT NULL,
    operation TEXT NOT NULL,
    response_json TEXT NOT NULL CHECK(json_valid(response_json)),
    actor TEXT,
    session TEXT,
    created_utc INTEGER NOT NULL
);

CREATE TRIGGER tasks_touch AFTER UPDATE OF
    title, activity_id, description, quadrant, due_date, status, created_utc, completed_utc
ON tasks
WHEN (OLD.title, OLD.activity_id, OLD.description, OLD.quadrant, OLD.due_date, OLD.status,
        OLD.created_utc, OLD.completed_utc)
  IS NOT (NEW.title, NEW.activity_id, NEW.description, NEW.quadrant, NEW.due_date, NEW.status,
        NEW.created_utc, NEW.completed_utc)
BEGIN
    UPDATE tasks SET revision = revision + 1 WHERE id = NEW.id;
END;

CREATE TRIGGER task_tags_insert AFTER INSERT ON task_tags
BEGIN
    UPDATE tasks SET revision = revision + 1 WHERE id = NEW.task_id;
END;

CREATE TRIGGER task_tags_delete AFTER DELETE ON task_tags
BEGIN
    UPDATE tasks SET revision = revision + 1 WHERE id = OLD.task_id;
END;

CREATE TRIGGER task_tags_update AFTER UPDATE OF
    task_id, tag_id
ON task_tags
WHEN (OLD.task_id, OLD.tag_id)
  IS NOT (NEW.task_id, NEW.tag_id)
BEGIN
    UPDATE tasks SET revision = revision + 1 WHERE id IN (OLD.task_id, NEW.task_id);
END;

CREATE TRIGGER entries_touch AFTER UPDATE OF
    activity_id, description, start_time, end_time, start_utc, end_utc, timezone, task_id, quadrant_snapshot
ON entries
WHEN (OLD.activity_id, OLD.description, OLD.start_time, OLD.end_time, OLD.start_utc, OLD.end_utc,
        OLD.timezone, OLD.task_id, OLD.quadrant_snapshot)
  IS NOT (NEW.activity_id, NEW.description, NEW.start_time, NEW.end_time, NEW.start_utc,
        NEW.end_utc, NEW.timezone, NEW.task_id, NEW.quadrant_snapshot)
BEGIN
    UPDATE entries SET revision = revision + 1 WHERE id = NEW.id;
END;

CREATE TRIGGER entry_tags_insert AFTER INSERT ON entry_tags
BEGIN
    UPDATE entries SET revision = revision + 1 WHERE id = NEW.entry_id;
END;

CREATE TRIGGER entry_tags_delete AFTER DELETE ON entry_tags
BEGIN
    UPDATE entries SET revision = revision + 1 WHERE id = OLD.entry_id;
END;

CREATE TRIGGER entry_tags_update AFTER UPDATE OF
    entry_id, tag_id
ON entry_tags
WHEN (OLD.entry_id, OLD.tag_id)
  IS NOT (NEW.entry_id, NEW.tag_id)
BEGIN
    UPDATE entries SET revision = revision + 1 WHERE id IN (OLD.entry_id, NEW.entry_id);
END;

-- Entity projections include the names in their shared vocabulary. Flipping
-- an archive/autocomplete flag leaves the projected name untouched.
CREATE TRIGGER revision_tag_rename AFTER UPDATE OF name ON tags
WHEN OLD.name IS NOT NEW.name
BEGIN
    UPDATE tasks SET revision = revision + 1
        WHERE id IN (SELECT task_id FROM task_tags WHERE tag_id = NEW.id);
    UPDATE entries SET revision = revision + 1
        WHERE id IN (SELECT entry_id FROM entry_tags WHERE tag_id = NEW.id);
END;

CREATE TRIGGER revision_activity_edit AFTER UPDATE OF name, category_id ON activities
WHEN (OLD.name, OLD.category_id) IS NOT (NEW.name, NEW.category_id)
BEGIN
    UPDATE tasks SET revision = revision + 1 WHERE activity_id = NEW.id;
    UPDATE entries SET revision = revision + 1 WHERE activity_id = NEW.id;
END;

CREATE TRIGGER revision_category_rename AFTER UPDATE OF name ON categories
WHEN OLD.name IS NOT NEW.name
BEGIN
    UPDATE tasks SET revision = revision + 1
        WHERE activity_id IN (SELECT id FROM activities WHERE category_id = NEW.id);
    UPDATE entries SET revision = revision + 1
        WHERE activity_id IN (SELECT id FROM activities WHERE category_id = NEW.id);
END;

-- An interval edit can trim, split, or remove neighboring entries. Its plan
-- also observes the whole history, independently of the visible timer.
CREATE TRIGGER history_insert AFTER INSERT ON entries
BEGIN
    UPDATE agent_control SET history_revision = history_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER history_delete AFTER DELETE ON entries
BEGIN
    UPDATE agent_control SET history_revision = history_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER history_update AFTER UPDATE OF revision ON entries
WHEN OLD.revision IS NOT NEW.revision
BEGIN
    UPDATE agent_control SET history_revision = history_revision + 1 WHERE singleton = 1;
END;

-- The control token covers the visible timer, including its activity and
-- tags, without coupling it to independent completed history.
CREATE TRIGGER current_entry_insert AFTER INSERT ON current_entry
WHEN NEW.entry_id IS NOT NULL
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER current_entry_delete AFTER DELETE ON current_entry
WHEN OLD.entry_id IS NOT NULL
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER current_entry_update AFTER UPDATE OF entry_id ON current_entry
WHEN OLD.entry_id IS NOT NEW.entry_id
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER current_entry_edit AFTER UPDATE OF revision ON entries
WHEN OLD.revision IS NOT NEW.revision AND EXISTS (SELECT 1 FROM current_entry WHERE entry_id = NEW.id)
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

-- Observation heartbeats only change last_observed_utc. They must not
-- invalidate a plan; a rollover changes the actual phase and state.
CREATE TRIGGER pomo_session_insert AFTER INSERT ON pomodoro_sessions
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER pomo_session_delete AFTER DELETE ON pomodoro_sessions
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER pomo_session_update AFTER UPDATE OF
    activity, category, description, tags, focus_seconds, break_seconds, long_break_seconds, rounds,
        completed_focus, interrupted_focus, phase, state, deadline_utc, remaining_seconds,
        current_entry_id, current_phase_id, timezone, task_id
ON pomodoro_sessions
WHEN (OLD.activity, OLD.category, OLD.description, OLD.tags, OLD.focus_seconds, OLD.break_seconds,
        OLD.long_break_seconds, OLD.rounds, OLD.completed_focus, OLD.interrupted_focus, OLD.phase,
        OLD.state, OLD.deadline_utc, OLD.remaining_seconds, OLD.current_entry_id,
        OLD.current_phase_id, OLD.timezone, OLD.task_id)
  IS NOT (NEW.activity, NEW.category, NEW.description, NEW.tags, NEW.focus_seconds,
        NEW.break_seconds, NEW.long_break_seconds, NEW.rounds, NEW.completed_focus,
        NEW.interrupted_focus, NEW.phase, NEW.state, NEW.deadline_utc, NEW.remaining_seconds,
        NEW.current_entry_id, NEW.current_phase_id, NEW.timezone, NEW.task_id)
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER pomo_phase_insert AFTER INSERT ON pomodoro_phases
WHEN EXISTS (SELECT 1 FROM pomodoro_sessions WHERE id = NEW.session_id AND state != 'stopped')
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER pomo_phase_delete AFTER DELETE ON pomodoro_phases
WHEN EXISTS (SELECT 1 FROM pomodoro_sessions WHERE id = OLD.session_id AND state != 'stopped')
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER pomo_phase_update AFTER UPDATE OF
    session_id, phase, state, started_utc, ended_utc, target_seconds, elapsed_seconds
ON pomodoro_phases
WHEN (OLD.session_id, OLD.phase, OLD.state, OLD.started_utc, OLD.ended_utc, OLD.target_seconds,
        OLD.elapsed_seconds)
  IS NOT (NEW.session_id, NEW.phase, NEW.state, NEW.started_utc, NEW.ended_utc, NEW.target_seconds,
        NEW.elapsed_seconds)
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1
      AND EXISTS (SELECT 1 FROM pomodoro_sessions WHERE id IN (OLD.session_id, NEW.session_id) AND state != 'stopped');
END;

CREATE TRIGGER pomo_segment_insert AFTER INSERT ON pomodoro_segments
WHEN EXISTS (SELECT 1 FROM pomodoro_sessions WHERE current_phase_id = NEW.phase_id AND current_entry_id = NEW.entry_id)
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER pomo_segment_delete AFTER DELETE ON pomodoro_segments
WHEN EXISTS (SELECT 1 FROM pomodoro_sessions WHERE current_phase_id = OLD.phase_id AND current_entry_id = OLD.entry_id)
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER pomo_segment_update AFTER UPDATE OF
    phase_id, entry_id
ON pomodoro_segments
WHEN (OLD.phase_id, OLD.entry_id)
  IS NOT (NEW.phase_id, NEW.entry_id)
BEGIN
    UPDATE agent_control SET timer_revision = timer_revision + 1 WHERE singleton = 1
      AND EXISTS (SELECT 1 FROM pomodoro_sessions
                  WHERE (current_phase_id, current_entry_id) IN
                        ((OLD.phase_id, OLD.entry_id), (NEW.phase_id, NEW.entry_id)));
END;

PRAGMA user_version = 5;
