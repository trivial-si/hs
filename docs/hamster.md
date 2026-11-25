# Coming from Project Hamster

hs is an independent Rust time tracker inspired by [Project Hamster](https://github.com/projecthamster/hamster). It is developed by Trivial.si and contributors, independently of the upstream Hamster project.

Hamster v3.0.3 is the behavioral reference for quick entry, workday boundaries, interval adjustments, and importing schema-v9 SQLite history. This is not a drop-in replacement for the GNOME app, its D-Bus interface, or every Hamster database version.

## Before importing

Back up your Hamster database and keep the original. hs reads a consistent snapshot of that database and imports into its own separate database. Try the import against an explicit destination first:

```sh
hs --db ./hamster-trial.db import hamster /path/to/hamster.db --source-name home --day-start 05:30 --dry-run
hs --db ./hamster-trial.db import hamster /path/to/hamster.db --source-name home --day-start 05:30
hs --db ./hamster-trial.db list --all
```

Replace `05:30` with your actual workday boundary. Compare representative dates and categories before using the new database for daily tracking. Ordinary imported entries preserve local wall-clock timestamps; their reported duration across daylight-saving transitions can differ from elapsed physical time. Pomodoro sessions use elapsed UTC time.

## Import Project Hamster history

Use a Hamster schema-v9 SQLite database as the source. Supply your original workday boundary with `--day-start` if you want to carry it over. The following example previews the import, commits it, then lists the resulting history.

```sh
hs import hamster ~/.local/share/hamster/hamster.db --source-name home --day-start 05:30 --dry-run
hs import hamster ~/.local/share/hamster/hamster.db --source-name home --day-start 05:30
hs list --all
```

The importer reads a consistent snapshot of the source and writes to the separate `hs` database. A dry run may create an empty destination database. It rolls back imported entries, names and settings after validation. On import, a Hamster entry with a missing end appears as `unresolved`. Reports list its ID and omit its duration until you supply an end with `hs edit ID --end TIME`.

Reuse the same `--source-name` for repeat imports from the same history, even if the source file moves. Give each independent history its own name. New source entries are added. Previously imported, unchanged source entries are skipped, preserving local edits and deletions. A change to a previously imported source entry causes the entire import to roll back and identifies the conflicting source ID.


## Credits and provenance

Thank you to the Hamster time tracker developers, including Toms Bauģis, Patryk Zawadzki, and the upstream contributors. See [NOTICE](../NOTICE) for the pinned reference and upstream authors and license.

New contributions that adapt upstream code or tests must preserve applicable notices and identify the source file and revision in the pull request. Import compatibility is scoped to the documented schema and tested behavior; it is not a claim of complete Hamster feature parity.
