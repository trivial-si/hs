# User guide

[Back to hs](../README.md)

## Tasks and tracked time

You can plan tasks first or start tracking immediately. Both workflows produce time entries in the same reports.

| Concept | What it means | Example |
| --- | --- | --- |
| **Task** | An outcome you can prioritize and mark done. It can have many time entries. | `Send proposal` |
| **Time label** (activity) | The name used to group recorded work in reports. It defaults to the task's name; choosing a separate label is optional. | `Writing` across several tasks |
| **Category** | One optional group for the work. | `Work` |
| **Tags** | Optional labels you can combine and filter by. | `client`, `follow-up` |
| **Time entry** | One recorded stretch of work, with a start and end. | 09:00–09:25 writing the proposal |
| **Pomodoro** | A way to run the timer with focus periods and breaks. Focus creates time entries. | 25 minutes writing, then a 5-minute break |

For the simplest setup, give a task a name and start its timer. Add a category, tags, or a separate time label when you need them for reports. “Activity” remains the name used in Hamster, exports, and the agent API.

## Build and run

Building from source requires Rust 1.88 or newer.

From the directory containing `Cargo.toml`, use a Rust toolchain and a C compiler for the bundled SQLite library.

```sh
cargo build --release --locked
./target/release/hs --help
install -Dm755 target/release/hs "$HOME/.local/bin/hs"
```

The examples below assume `~/.local/bin` is on your `PATH`. The default database is `$XDG_DATA_HOME/hs/hs.db` when `XDG_DATA_HOME` is an absolute path, and `~/.local/share/hs/hs.db` otherwise. Use the global option `--db PATH` for a separate database, for example `hs --db ./trial.db status`. You can read your records from another terminal while a timer is running. SQLite serializes changes, including closing one activity and starting the next.

Database paths are literal filenames. For example, `--db ':memory:'` saves a persistent file named `:memory:`, and `--db 'file:trial.db'` saves a file with that exact name. If an older version left an empty `file:` placeholder, open its previous database using the plain path, such as `--db trial.db`. To initialize the literal empty file, use `--db './file:trial.db'`.

## Choose a workflow

**Plan a task first — recommended for the matrix and TUI.** Capture the outcome, then select it in the TUI and press Space for a Pomodoro or `s` for ordinary tracking. Mark the task done when you have sent the proposal; stopping its timer only ends the current stretch of work.

```sh
hs task add 'Send proposal' --quadrant plan
hs tui
```

**Track unplanned work immediately.** Start with a time label, optionally followed by a category and tags. This creates a time entry without adding a task to the matrix.

```sh
hs start 'Writing@Work, #client'
hs stop
hs list --today
```

## Track a workday

```sh
hs start 'Development@Work, fix issue #123, #backend'
hs status
hs start 'Design review@Work'
hs stop
hs list --today
hs resume
```

Starting a different activity ends the current one at the same instant. Repeating the current activity with identical details keeps its existing start time. Stopping an ordinary activity after less than a minute keeps the entry and reports zero whole minutes. `resume` copies the most recent completed entry’s details into a new timer. Give it an entry ID to choose a specific activity.

Entry IDs increase as records are created and stay reserved after deletion. Opening an existing database with the regular tracking CLI upgrades it to schema v5 in a transaction. The agent API uses the explicit `initialize` operation to create or upgrade a database. The upgrade preserves records, running timers, Pomodoro sessions, and previously reserved IDs from schema v2 onward. For schema v1, the highest surviving ID sets the initial allocation point.

Quote the entire quick entry. Use `--` before an entry that begins with a minus sign.

| Command | What it records |
| --- | --- |
| `hs start -- '-5 Support@Work'` | Start support five minutes ago. |
| `hs add -- '-30 -10 Review@Work'` | Record the twenty minutes between thirty and ten minutes ago. |
| `hs add '2025-01-15 09:00-10:30 Planning@Work'` | Record ninety minutes on a specific date. |
| `hs add '2025-01-15 23:45 30 Deploy, #release'` | Record thirty minutes across midnight. |
| `hs stop --at -2` | Stop the running activity two minutes ago. |

An entry can include `activity@category, description, #tag one #tag two`. A single-word hashtag in the description also becomes a tag. Numeric references such as `#123` stay in the text. Clocks accept `09:30`, `9.30`, `9,30`, and `0930`. A bare clock refers to the current workday. A clock-only end that falls before the start rolls into the next day. Signed minute offsets are relative to the current time. Recorded intervals must end on or before now, including when a positive offset is used.

New local timestamps and task due dates use four-digit years from `0000` through `9999`. Set `TZ` to an IANA timezone such as `Europe/Zagreb` to override the system timezone. An unrecognized override produces an error.

Find an entry ID with `hs list`. This example corrects entry 42 and reviews the result.

```sh
hs list --all --search planning
hs edit 42 --start '09:15' --end '10:00' --tags 'planning,review'
hs list --all --search planning
```

Clock-only edits use the entry’s workday. You can also edit `--activity`, `--category` and `--description`. Pass `--category ''` or `--tags ''` to clear those fields. `hs delete 42` removes the entry. `hs activities`, `hs categories`, and `hs tags` list names available for reuse.

For a backdated start that intersects a completed entry, use `hs add` with an explicit end. Bounded additions and time edits can trim or split intersecting entries. Existing entries fully covered by the new interval are kept, so overlapping time can contribute to both entries’ totals. The command displays IDs of adjusted entries.

## Use the interactive terminal interface

Run `hs tui` (or `hs ui`) for a keyboard-driven matrix and inbox. It uses the same database as the ordinary commands and updates when another terminal changes a task or timer. A prominent Pomodoro panel above the matrix shows a large countdown, phase, active task, progress, and the controls available now. Use `hs tui --no-sound` to start muted, or press `b` to toggle sound.

Select a task and press **Space** to start focus. Space then pauses or resumes the current Pomodoro. When a phase finishes, the panel shows **00:00**, the completed phase, and the offered next phase: press Space to start that break or focus. Nothing starts automatically. Selecting a different task leaves the current session's Space control unchanged; press **p** to start or switch to the selected task instead. A switch asks for confirmation and names both the current session and the new task. Completion banners and sounds work while the interface stays open.

| Key | Action |
| --- | --- |
| Arrow keys | Move between panes spatially: Up / Down between matrix rows and the inbox, Left / Right between quadrants in a row. Up from the inbox returns through the last visited column (left initially). Arrows stop at the outer edges. |
| `Tab` / `Shift-Tab`, `h` / `l`, or `1`–`5` | Select a quadrant or the inbox. Numbers mean Do, Plan, Drop, Delegate, Inbox. |
| `j` / `k` | Select the next / previous task; Page Up / Down moves ten tasks, Home / End selects the first / last task. |
| `Enter` | Open task details. Page Up / Down scrolls long dialogs. |
| `/` | Search task names, time labels, categories, and tags; `Esc` clears the applied filter. |
| `n` | Add a task in the selected quadrant or inbox. |
| `e` / `u` / `m` | Rename, edit the due date, or move the task to another quadrant. |
| `d` / `a` | Complete or archive the task after confirmation. Its own timer also stops. |
| `s` / `p` | Track the selected task or start its default 25/5/15-minute Pomodoro. `p` resumes or advances an existing Pomodoro for that task. |
| `Space` | Start focus on the selected task when idle; otherwise pause/resume the current Pomodoro or start its offered break/focus. |
| `N` | Start the current Pomodoro's offered next phase (alias for Space when ready). |
| `x` | Stop the displayed timer after confirmation. |
| `?` / `F1` | Show all keys. |
| `q` / `Ctrl-C` | Leave the interface; any timer stays saved and running. |

Editors support typing or paste, Backspace by visible character, and `Ctrl-U` to clear. `Enter` saves; `Esc` cancels. A due date accepts `YYYY-MM-DD` or `none`. Switching to another timer asks for confirmation, and a timer changed by another client makes a stale confirmation fail instead of stopping the replacement. Completing or archiving also checks that the task and its associated timer still match the confirmation. If they changed, review them and confirm again. Undocumented Ctrl/Alt combinations do not activate ordinary shortcuts.

Wide terminals show all four quadrants with the inbox below; small terminals show the selected pane. `NO_COLOR` keeps selection and completion emphasis without colors. The interface requires an interactive input/output terminal with cursor support. Use `hs matrix` for ordinary printed output or `hs --json matrix` for structured output.

## Prioritize tasks with the Eisenhower matrix

Use a task for an outcome you can finish, such as “Send proposal.” You can give its recorded work a reusable time label, such as `Writing`, and add a category and tags for reporting. These commands create a task and show it in the matrix.

```sh
hs task add 'Send proposal' --time-label Writing --category Work --tags client --quadrant plan --due 2026-12-01
hs matrix
```

| Quadrant | Important | Urgent |
| --- | --- | --- |
| `do` | Yes | Yes |
| `plan` | Yes | No |
| `delegate` | No | Yes |
| `drop` | No | No |

`hs matrix` draws the open tasks in an ASCII coordinate diagram: importance increases upward, and urgency increases to the right. Plan is quadrant II (upper left), Do is I (upper right), Drop is III (lower left), and Delegate is IV (lower right). The Inbox sits below the diagram.

Each task shows its `#ID` and full wrapped title, with a due date, recorded time, or active timer when present. Accented text, CJK characters, and emoji retain their display width. Terminals narrower than 60 columns show a small quadrant key followed by full-width task sections. When output is redirected, `COLUMNS=120 hs matrix` selects the width; the default is 80, capped at 160. `hs --json matrix` keeps the same structured output at every width.

Omit the quadrant to capture a task in the Inbox. Use `--quadrant inbox` on a later edit to return it there. Due dates help order open tasks and identify overdue work. You decide when its urgency changes. Delegate and Drop are planning labels. Archiving remains a separate action.

Use the ID returned by `hs task add` in later commands. This example uses task ID 7.

```sh
hs task show 7
hs start --task 7
hs stop
hs task set 7 --quadrant do
hs pomo start --task 7
hs task done 7
hs report --week --group-by quadrant
```

`hs task list` shows open tasks with their recorded time. Add `--all` to include completed and archived work. `hs task next` suggests an open Do task first, then a Plan task, with earlier due dates first within each quadrant.

By default, creating a task copies its name into the time label. Renaming the task later changes only its name: the saved time label and existing time entries stay as they were. Set a new label explicitly when you want future work to use it.

```sh
hs task set 7 --time-label Writing --category Work --tags client
```

`--time-label` is plain text: `@` and commas stay part of the label. Changing it preserves the category, notes, and tags. Use `--category`, `--notes`, or `--tags` to change those separately; an empty string clears the corresponding optional field.

The existing `--activity` option still accepts Hamster-style details, such as `--activity 'Writing@Work, check figures, #client'`. It replaces the time label and category. Embedded notes and tags replace those fields when present; omitting them preserves existing notes and tags. Explicit `--category`, `--notes`, and `--tags` override embedded values. Choose either `--time-label` or `--activity` in a command. Task activity details and Pomodoro starts accept no time prefix.

A paused Pomodoro offers `resume`, a finished phase offers `next`, and an active phase offers `watch`. If another Pomodoro is open, the suggestion explains how to stop it before switching tasks.

Completing or archiving a task closes its active timer or Pomodoro session in the same transaction. The recorded work remains available in reports. `hs task reopen 7` returns the task to your open work. Use `hs task archive 7` to file it away.

The matrix shows each open task’s current quadrant. Each work entry saves the task ID and quadrant at the time its segment starts. In the example, the first entry counts toward Plan and the later focus entry counts toward Do. Changing a task’s title, notes or priority lets the current timer continue with its original snapshot. A resumed timer or new focus segment picks up the task’s current quadrant. Reports include unlinked and untriaged work under **Unclassified**. JSON and TSV exports include `task_id` and the saved `quadrant`.

## Use a Pomodoro timer

Start a focus period with the same activity, category and tag syntax as ordinary tracking. The defaults are 25 minutes of focus, a 5-minute short break, and a 15-minute long break after every four completed focus periods.

```sh
hs pomo start 'Write proposal@Work, #client'
# When focus finishes, start the offered break:
hs pomo next
```

When a phase ends, `hs pomo next` starts the offered break or focus period. Time spent waiting to start the next phase stays out of the totals. A focus period creates a work entry visible in `hs list` and your regular reports. For example, a focus period started at 09:00 ends at its 09:25 deadline even if you next open `hs` at 11:00.

| Command | Effect |
| --- | --- |
| `hs pomo pause` | Pause the countdown and close the current focus entry. |
| `hs pomo resume` | Continue with the remaining time. Resuming focus opens a fresh work entry. |
| `hs pomo next` | Start the next phase once it is ready. |
| `hs pomo stop` | End the session and keep the work already recorded. |
| `hs pomo stats` | Show completed and interrupted focus counts, recorded focus time, and timed breaks across sessions. |

Paused time is excluded from focus and break totals. Breaks appear in Pomodoro statistics only. Regular `hs start`, `hs resume`, and `hs stop` end an open Pomodoro session. Editing or deleting the current Pomodoro work entry interrupts its session.

For a different rhythm, pass durations and a round count when starting a session.

```sh
hs pomo start 'Study' --focus 45m --break 10m --long-break 20m --rounds 3
```

Bare durations mean minutes. The suffixes `s`, `m`, and `h` select seconds, minutes, and hours. `hs status` also shows an open Pomodoro.

In an interactive terminal, `hs pomo start`, `hs pomo next`, and `hs pomo resume` keep a live countdown open. Finishing work shows **[FOCUS COMPLETE]**; finishing either kind of break shows **[BREAK COMPLETE]**. Both play a short two-note chime. The banner stays visible, and the next phase starts only when you run `hs pomo next`.

Use `--no-bell` (or `--no-sound`) on these commands for a silent countdown with the same visual cue. Use `--detach` to return to the shell immediately, then `hs pomo watch` to attach again. `watch` also accepts `--no-bell`. Ctrl-C closes the watcher and leaves the timer saved. **Keep a watcher open to receive completion cues**; there is no background notification service. Pausing, stopping, or replacing a watched session closes its watcher without ringing. Attaching to an already-ready phase does not repeat its alert. Each attached watcher alerts independently.

Sound uses an installed system audio player (`paplay`, `pw-play`, `aplay`, or `play` on Linux; `afplay` on macOS; PowerShell on Windows), falling back to the terminal bell if playback fails. Audible output depends on the system or terminal's sound settings. `TERM=dumb` uses plain lines, and `NO_COLOR` disables banner styling. Redirected commands return immediately; an explicitly requested `watch` can still print progress to a pipe, without sound or terminal escapes. JSON and MCP output remain silent and do not attach watchers.

Deadlines are stored in SQLite; tracking, Pomodoro, and report commands apply an elapsed deadline when they run. This also works across a restart. A pause or resume arriving after completion shows the next ready phase without starting it.

## Workdays, reports and exports

```sh
hs config day-start
hs config day-start 05:30
hs report --week --group-by category
hs report --from 2025-01-13 --to 2025-01-19 --group-by day
hs export --week --format json > week.json
hs export --month --format tsv > month.tsv
```

A new database uses midnight as its workday boundary. With a 05:30 boundary, Monday’s workday runs until 05:30 Tuesday. Weeks begin on Monday at the configured time. `list`, `report`, and `export` default to today’s workday and accept `--week`, `--month`, `--all`, or a date range. Both dates supplied to `--from` and `--to` are inclusive.

The list shows each entry’s full duration in the `ENTRY DURATION` column. Reports count the portion of each interval inside the selected period. A running timer contributes through the instant the report is taken. Grouping by day splits overnight entries at the workday boundary. Grouping by tag allocates an entry’s duration to each of its tags and counts the entry once in the grand total. Exports keep the original timestamps and include clipped duration in seconds and whole minutes. Pomodoro work entries also carry UTC endpoints and the session’s saved IANA timezone. JSON includes the report range and calculation time.

Pomodoro focus uses whole seconds of elapsed UTC time. Reports add the seconds across focus segments before converting to minutes and split workdays in the session’s saved timezone. This keeps elapsed time accurate when clocks move forward or back. For an edit in a repeated local hour, you can give an explicit instant such as `hs edit 42 --end '2026-10-25T02:05:00+01:00'`.

If the system clock moves backward, an open Pomodoro retains elapsed time already observed by `hs`. Calendar selections such as `--today` and overdue task labels still use the local date and time when the command runs.

Ordinary and imported entries keep their local wall-clock interpretation. Quick-entry clock times use minute precision. Imported timestamps retain their seconds and fractional seconds, and ordinary and imported reports count completed wall-clock minutes. Across a daylight-saving transition their displayed duration can differ from elapsed physical time.

### Read command results as JSON

Use the global `--json` flag with `status`, `report`, `matrix`, `task list`, `task show`, `task next`, `pomo status`, or `pomo stats`.

```sh
hs --json status
hs --json task list
hs --json matrix
hs --json report --week --group-by quadrant
hs --json pomo status
hs --json pomo stats
```

Successful commands write one JSON object to stdout. Scripts can inspect `schema_version` (currently `1`) and `kind` to identify the view. Durations use seconds and UTC instants use RFC 3339. For example, `hs --json task next` returns the suggested task or `null`, along with its action. Errors produce a nonzero exit status and a JSON object on stderr with `kind` set to `error`. Use `hs export --format json` for individual work entries and their clipped durations.

## Backups and shell completion

```sh
hs backup ./hs-backup.db
hs --db ./hs-backup.db report --all
```

Choose a new backup path. The backup command takes a consistent SQLite snapshot and checks its integrity. You can open a backup with `--db` to review it or continue tracking in a separate copy.

`hs completions bash`, `hs completions zsh`, and `hs completions fish` generate scripts from the running executable’s command definitions. For example, use `source <(hs completions bash)` for the current Bash session. After changing CLI options, regenerate the shipped copies from the source checkout.

```sh
cargo run --locked --quiet -- completions bash > completions/hs.bash
cargo run --locked --quiet -- completions zsh > completions/_hs
cargo run --locked --quiet -- completions fish > completions/hs.fish
```
