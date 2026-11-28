# Choose a local time tracker or task manager

[Back to hs](../README.md)

These projects overlap in different ways. Compare the workflow you want to keep and try it with disposable data before moving your history.

| Project | Local storage and process | Planning and focus | Data access and integration |
| --- | --- | --- | --- |
| **hs** | SQLite. CLI commands open the same database when needed. A separate daemon is unnecessary. | Terminal UI with an Eisenhower matrix and built-in Pomodoro sessions. | [JSON CLI and stdio MCP](agents.md), including previews and revision checks for writes. |
| **Project Hamster** | SQLite. The GNOME app uses background D-Bus services. | Desktop activity tracking and reporting, with a GTK interface. | Local database and D-Bus. [Upstream repository and backup instructions](https://github.com/projecthamster/hamster). |
| **Timewarrior** | Local monthly `.data` files. The `timew` CLI runs on demand. | Tagged intervals, terminal charts, and reporting extensions. | JSON interval import and export, plus a [Taskwarrior hook](https://timewarrior.net/docs/taskwarrior/). [Command reference](https://timewarrior.net/reference/timew.1/). |
| **Watson** | Local JSON frames. The CLI runs on demand. | Start and stop project timers with tags and produce reports from the terminal. | JSON and CSV reports and optional remote sync via `watson sync` (such as Crick). [Upstream documentation](https://github.com/jmaupetit/Watson). |
| **Taskwarrior** | Taskwarrior 3 stores tasks in a local SQLite database. Core CLI commands work locally. | Task filters and reports, priorities, due dates, and recurrence. Add time tracking with the Timewarrior hook if needed. | JSON task export, hooks, and optional Taskchampion sync. [Command reference](https://taskwarrior.org/docs/man/task.1/) · [Storage and migration](https://taskwarrior.org/docs/upgrade-3/). |

The built-in matrix and Pomodoro in this comparison are features of hs. Other projects can be paired with separate frontends, timers, or scripts. JSON exports are data exchange formats. Only the hs row describes the write protocol implemented by `hs mcp`.

## Pick by workflow

- Keep **Project Hamster** if its GNOME desktop workflow already fits how you track activities.
- Keep **Timewarrior** for interval editing, report extensions, or an existing Taskwarrior time-tracking hook.
- Keep **Watson** for a compact project-and-tag workflow or an existing `watson sync` setup.
- Keep **Taskwarrior** if recurrence, configurable task reports, or synchronization are central to your task list.
- Try **hs** if you want a single local terminal workflow for the matrix, a focus timer, and time entries.

## Check the migration path

hs reads [Project Hamster schema-v9 history](hamster.md) into a separate SQLite database. Start with a backup and a dry run, then check a few representative dates and categories. Hamster's desktop interface and D-Bus protocol are outside the scope of that importer.

hs does not import Timewarrior intervals, Watson frames, or Taskwarrior tasks directly. Keep the source store and any exports for reference. If you try hs alongside another tracker, start with an empty trial database and compare the new workflow before committing to a migration.
