mod mcp;
mod notification;
pub(crate) mod output;
pub(crate) mod pomodoro;
pub(crate) mod tasks;
mod terminal;
mod tui;

use std::{
    env,
    io::{self, Read, Write},
    path::PathBuf,
};

use chrono::{NaiveDate, NaiveTime, Timelike};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use hs::{
    Error, Result, Store,
    application::{
        editing::{EntryEdit, stop_time},
        import::import_hamster,
        queries as snapshot, workflows as tracking,
    },
    domain::{
        ids::{EntryId, TaskId},
        model::Entry,
        pomodoro::State,
        report::{self, GroupBy},
        time::Moment,
    },
};

#[derive(Parser)]
#[command(
    name = "hs",
    version,
    about = "Track activities and review your time locally",
    after_help = "Examples:\n  hs start 'Development@Work, #backend'\n  hs start -- '-5 Meeting@Work'\n  hs add '09:00-10:30 Planning@Work'\n  hs report --week --group-by category"
)]
struct Cli {
    /// Use a separate hs database
    #[arg(long, global = true, value_name = "PATH")]
    db: Option<PathBuf>,
    /// Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Execute one versioned JSON request from stdin; emit one JSON response
    Api,
    /// Print agent operations and their JSON Schemas without opening a database
    Capabilities,
    /// Inspect stored state as JSON without advancing timers or migrating data
    Context,
    /// Serve local agent tools over MCP stdio (protocol 2025-11-25)
    Mcp,
    /// Capture and prioritize tasks
    Task {
        #[command(subcommand)]
        command: crate::cli::tasks::Command,
    },
    /// Show open tasks by importance and urgency, followed by the inbox
    Matrix,
    /// Open the interactive matrix, inbox, task editor and timer controls
    #[command(visible_alias = "ui")]
    Tui {
        /// Keep completion banners without playing sounds
        #[arg(long, visible_alias = "no-bell")]
        no_sound: bool,
    },
    /// Run persistent focus and break timers
    Pomo {
        #[command(subcommand)]
        command: crate::cli::pomodoro::Command,
    },
    /// Start an activity, closing the current one at the same time
    Start {
        /// Hamster-style entry, quoted as one argument
        #[arg(required_unless_present = "task", conflicts_with = "task")]
        input: Option<String>,
        /// Track an open task by its ID
        #[arg(long)]
        task: Option<TaskId>,
    },
    /// Record a past interval or start an activity with no end time
    Add { input: String },
    /// Show the current activity
    Status,
    /// Stop the current activity
    Stop {
        /// Stop at a clock time, dated time, or signed minute offset
        #[arg(long, value_name = "TIME", allow_hyphen_values = true)]
        at: Option<String>,
    },
    /// Start a new entry using a previous entry's details
    Resume {
        /// Entry to copy; defaults to the latest completed entry
        id: Option<EntryId>,
    },
    /// Correct an entry by its ID
    Edit(EditArgs),
    /// Delete an entry by its ID
    Delete { id: EntryId },
    /// List entries, including unfinished imported records
    List {
        #[command(flatten)]
        period: PeriodArgs,
        /// Search activity, category, description, and tags
        #[arg(long)]
        search: Option<String>,
    },
    /// Show totals for a day, week, month, or date range
    Report {
        #[command(flatten)]
        period: PeriodArgs,
        #[arg(long, value_enum, default_value = "activity")]
        group_by: Group,
    },
    /// Export matching entries and clipped durations
    Export {
        #[command(flatten)]
        period: PeriodArgs,
        #[arg(long, value_enum, default_value = "tsv")]
        format: ExportFormat,
    },
    /// Import history from another time tracker
    Import {
        #[command(subcommand)]
        source: ImportCommand,
    },
    /// View or change workday settings
    Config {
        #[command(subcommand)]
        setting: Setting,
    },
    /// Create a consistent SQLite backup at a new path
    Backup { path: PathBuf },
    /// List activities available for reuse
    Activities,
    /// List known categories
    Categories,
    /// List tags available for reuse
    Tags,
    /// Print a shell completion script
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Args, Default)]
struct PeriodArgs {
    /// Current workday (the default)
    #[arg(long, conflicts_with_all=["week", "month", "all", "from", "to"])]
    today: bool,
    /// Current Monday-based week
    #[arg(long, conflicts_with_all=["month", "all", "from", "to"])]
    week: bool,
    /// Current calendar month
    #[arg(long, conflicts_with_all=["all", "from", "to"])]
    month: bool,
    /// All recorded history
    #[arg(long, conflicts_with_all=["from", "to"])]
    all: bool,
    /// First workday, YYYY-MM-DD
    #[arg(long)]
    from: Option<NaiveDate>,
    /// Last workday inclusive, YYYY-MM-DD; defaults to today
    #[arg(long, requires = "from")]
    to: Option<NaiveDate>,
}

#[derive(Args)]
struct EditArgs {
    id: EntryId,
    #[arg(long, allow_hyphen_values = true)]
    start: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    end: Option<String>,
    #[arg(long)]
    activity: Option<String>,
    /// Use an empty string to remove the category
    #[arg(long)]
    category: Option<String>,
    #[arg(long)]
    description: Option<String>,
    /// Replace tags with a comma-separated list; empty clears tags
    #[arg(long)]
    tags: Option<String>,
}

#[derive(Subcommand)]
enum ImportCommand {
    /// Import a Hamster schema-v9 database
    Hamster {
        path: PathBuf,
        /// Stable identity for this source; reuse after moving its file
        #[arg(long, default_value = "hamster")]
        source_name: String,
        /// Validate and preview without committing imported records
        #[arg(long)]
        dry_run: bool,
        /// Original Hamster workday boundary, HH:MM
        #[arg(long, value_parser=parse_boundary)]
        day_start: Option<u32>,
    },
}

#[derive(Subcommand)]
enum Setting {
    /// Print or set the start of a workday (default 00:00)
    DayStart {
        #[arg(value_parser=parse_boundary)]
        time: Option<u32>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Group {
    Activity,
    Category,
    Tag,
    Day,
    Quadrant,
}
#[derive(Clone, Copy, ValueEnum)]
enum ExportFormat {
    Tsv,
    Json,
}
#[derive(Clone, Copy, ValueEnum)]
enum Shell {
    Bash,
    Zsh,
    Fish,
}

pub fn run() -> std::process::ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if error.use_stderr()
                && env::args_os()
                    .skip(1)
                    .take_while(|a| a != "--")
                    .any(|a| a == "--json")
            {
                eprintln!(
                    "{}",
                    crate::cli::output::error("input", "invalid_arguments", &error.to_string())
                );
                return std::process::ExitCode::from(2);
            }
            error.exit()
        }
    };
    if matches!(
        cli.command,
        Command::Api | Command::Context | Command::Capabilities
    ) {
        return match execute_agent(cli) {
            Ok(code) => std::process::ExitCode::from(code),
            Err(error) => crate::cli::output::report_error(&error, true),
        };
    }
    let machine = cli.json;
    match execute(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => crate::cli::output::report_error(&error, machine),
    }
}

fn write_agent_response(response: &serde_json::Value) -> Result<u8> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    serde_json::to_writer(&mut out, response)?;
    writeln!(out)?;
    out.flush()?;
    Ok(
        if response.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
            match response["error"]["code"].as_str() {
                Some("invalid_input" | "invalid_json") => 2,
                _ => 1,
            }
        } else {
            0
        },
    )
}

fn execute_agent(cli: Cli) -> Result<u8> {
    if matches!(cli.command, Command::Capabilities) {
        return write_agent_response(&hs::application::agent::capabilities());
    }
    let request = if matches!(cli.command, Command::Context) {
        serde_json::json!({"schema_version":1,"operation":"context"})
    } else {
        let mut bytes = Vec::new();
        if let Err(error) = io::stdin()
            .lock()
            .take(hs::application::agent::MAX_REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
        {
            return write_agent_response(&hs::application::agent::error_response(
                "io_error",
                &error.to_string(),
            ));
        }
        if bytes.len() > hs::application::agent::MAX_REQUEST_BYTES {
            return write_agent_response(&hs::application::agent::error_response(
                "invalid_input",
                "Request exceeds 1 MiB",
            ));
        }
        match hs::wire::parse(&bytes) {
            Ok(value) => value,
            Err(error) => {
                return write_agent_response(&hs::application::agent::error_response(
                    "invalid_json",
                    &error.to_string(),
                ));
            }
        }
    };
    let path = cli.db.or_else(|| default_database().ok());
    let response = match path {
        Some(path) => hs::application::agent::handle(&path, request),
        None => hs::application::agent::handle_without_database(request),
    };
    write_agent_response(&response)
}

fn execute(cli: Cli) -> Result<()> {
    use crate::cli::{pomodoro::Command as Pomo, tasks::Command as Task};
    let machine = cli.json;
    if matches!(cli.command, Command::Mcp) {
        let path = cli.db.or_else(|| default_database().ok());
        return mcp::run(path.as_deref());
    }
    if machine
        && !matches!(
            &cli.command,
            Command::Status
                | Command::Matrix
                | Command::Report { .. }
                | Command::Task {
                    command: Task::List { .. } | Task::Show { .. } | Task::Next
                }
                | Command::Pomo {
                    command: Pomo::Status | Pomo::Stats
                }
        )
    {
        return Err(Error::Input("--json supports status, report, matrix, task list/show/next and pomo status/stats. Use export --format json for entry exports.".into()));
    }
    if let Command::Tui { no_sound } = cli.command {
        tui::validate()?;
        let path = cli.db.map(Ok).unwrap_or_else(default_database)?;
        let mut store = Store::open(&path)?;
        return tui::run(&mut store, no_sound);
    }
    let mut out = io::BufWriter::new(io::stdout().lock());
    if let Command::Completions { shell } = cli.command {
        out.write_all(&completions(shell))?;
        out.flush()?;
        return Ok(());
    }
    let path = cli.db.map(Ok).unwrap_or_else(default_database)?;
    let moment = hs::application::clock::now()?;
    let mut store = Store::open(&path)?;
    match cli.command {
        Command::Api
        | Command::Capabilities
        | Command::Context
        | Command::Mcp
        | Command::Tui { .. } => unreachable!(),
        Command::Task { command } => {
            crate::cli::tasks::run(command, &mut store, &moment, machine, &mut out)?
        }
        Command::Matrix => crate::cli::tasks::matrix(&mut store, &moment, machine, &mut out)?,
        Command::Pomo { command } => {
            crate::cli::pomodoro::run(command, &mut store, &moment, machine, &mut out)?
        }
        Command::Start { input, task } => show_change(
            &mut out,
            "Started",
            if let Some(id) = task {
                tracking::start_task(&mut store, id, &moment)?
            } else {
                tracking::start_input(
                    &mut store,
                    input.as_deref().expect("clap requires input or task"),
                    &moment,
                )?
            },
        )?,
        Command::Add { input } => show_change(
            &mut out,
            "Recorded",
            tracking::add_input(&mut store, &input, &moment)?,
        )?,
        Command::Status => {
            let view = snapshot::status(&mut store, &moment)?;
            if machine {
                crate::cli::output::View::Status(&view).write(&mut out)?;
            } else {
                match view.current {
                    Some(entry) => show_entry(&mut out, &entry, &view.as_of)?,
                    None => writeln!(out, "No activity is running.")?,
                }
                if let Some(session) = view.session
                    && session.state() != State::Stopped
                {
                    crate::cli::pomodoro::show(&mut out, Some(&session), &view.as_of)?;
                }
            }
        }
        Command::Stop { at } => {
            let at = stop_time(at.as_deref(), moment.local()?, store.day_start()?)?;
            match tracking::stop_with(&mut store, at, &moment)? {
                Some(entry) => {
                    write!(out, "Stopped ")?;
                    show_entry(&mut out, &entry, &moment)?;
                }
                None => writeln!(
                    out,
                    "No activity is running. Any open Pomodoro session has been stopped."
                )?,
            }
        }
        Command::Resume { id } => show_change(
            &mut out,
            "Started",
            tracking::resume_entry(&mut store, id, &moment)?,
        )?,
        Command::Edit(args) => {
            let patch = EntryEdit {
                start: args.start,
                end: args.end,
                activity: args.activity,
                category: args.category,
                description: args.description,
                tags: args.tags,
            };
            let change = tracking::edit_entry(&mut store, args.id, patch, &moment)?;
            show_change(&mut out, "Updated", change)?;
        }
        Command::Delete { id } => {
            tracking::delete_entry(&mut store, id, &moment)?;
            writeln!(out, "Deleted entry {id}.")?;
        }
        Command::List { period, search } => {
            let view = snapshot::history_for_period(&mut store, period.period(), &moment)?;
            let entries = view.entries;
            let moment = view.as_of;
            let search = search.map(|value| value.to_lowercase());
            writeln!(out, "ID\tSTART\tEND\tENTRY DURATION\tACTIVITY\tDETAILS")?;
            for entry in &entries {
                if search
                    .as_ref()
                    .is_some_and(|needle| !matches_search(entry, needle))
                {
                    continue;
                }
                show_entry(&mut out, entry, &moment)?;
            }
        }
        Command::Report { period, group_by } => {
            let view = snapshot::history_for_period(&mut store, period.period(), &moment)?;
            let entries = view.entries;
            let range = view.range;
            let moment = view.as_of;
            let group = match group_by {
                Group::Activity => GroupBy::Activity,
                Group::Category => GroupBy::Category,
                Group::Tag => GroupBy::Tag,
                Group::Day => GroupBy::Day,
                Group::Quadrant => GroupBy::Quadrant,
            };
            let result = report::summarize_at(&entries, &range, group, &moment)?;
            if machine {
                let key = group_by
                    .to_possible_value()
                    .expect("group is a fixed CLI value");
                crate::cli::output::View::Report {
                    data: &result,
                    range: &range,
                    group_by: key.get_name(),
                    as_of: &moment,
                }
                .write(&mut out)?;
            } else {
                writeln!(out, "{} to {} (end exclusive)", range.start, range.end)?;
                for (name, seconds) in result.groups_seconds {
                    writeln!(out, "{}\t{}", duration_seconds(seconds), visible(&name))?;
                }
                writeln!(out, "Total\t{}", duration_seconds(result.total_seconds))?;
                if !result.unresolved_ids.is_empty() {
                    writeln!(
                        out,
                        "Unfinished imported entries excluded from totals: {}",
                        result
                            .unresolved_ids
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )?;
                }
                if matches!(group_by, Group::Tag) {
                    writeln!(
                        out,
                        "Entries with several tags contribute to each tag. Total counts each entry once."
                    )?;
                }
            }
        }
        Command::Export { period, format } => {
            let view = snapshot::history_for_period(&mut store, period.period(), &moment)?;
            let entries = view.entries;
            let range = view.range;
            let moment = view.as_of;
            let now = moment.local()?;
            match format {
                ExportFormat::Tsv => {
                    write!(out, "{}", report::export_tsv_at(&entries, &range, &moment)?)?
                }
                ExportFormat::Json => {
                    let mut rows = Vec::new();
                    for entry in &entries {
                        let totals = report::summarize_at(
                            std::slice::from_ref(entry),
                            &range,
                            GroupBy::Activity,
                            &moment,
                        )?;
                        let seconds =
                            (entry.end.is_some() || entry.running).then_some(totals.total_seconds);
                        rows.push(crate::cli::output::entry_row(entry, seconds));
                    }
                    serde_json::to_writer_pretty(
                        &mut out,
                        &serde_json::json!({"schema_version":1,"range":{"start":hs::model::format_time(range.start),"end":hs::model::format_time(range.end)},"as_of":hs::model::format_time(now),"entries":rows}),
                    )?;
                    writeln!(out)?;
                }
            }
        }
        Command::Import {
            source:
                ImportCommand::Hamster {
                    path,
                    source_name,
                    dry_run,
                    day_start,
                },
        } => {
            let result = import_hamster(&mut store, &path, &source_name, day_start, dry_run)?;
            writeln!(
                out,
                "{}: {result}",
                if dry_run { "Preview" } else { "Imported" }
            )?;
            if result.unresolved > 0 {
                writeln!(
                    out,
                    "{}use `hs list --all` and `hs edit ID --end TIME` to finish imported open entries.",
                    if dry_run {
                        "After importing, "
                    } else {
                        "To review them, "
                    }
                )?;
            }
        }
        Command::Config {
            setting: Setting::DayStart { time },
        } => {
            if let Some(time) = time {
                store.set_day_start(time)?;
            }
            let value = store.day_start()?;
            writeln!(out, "{:02}:{:02}", value / 60, value % 60)?;
        }
        Command::Backup { path } => {
            snapshot::status(&mut store, &moment)?;
            store.backup(&path)?;
            writeln!(out, "Backup saved to {}", path.display())?;
        }
        Command::Activities => {
            for (activity, category) in store.activities()? {
                writeln!(
                    out,
                    "{}{}",
                    visible(&activity),
                    category
                        .map(|value| format!("@{}", visible(&value)))
                        .unwrap_or_default()
                )?;
            }
        }
        Command::Categories | Command::Tags => {
            let kind = if matches!(cli.command, Command::Categories) {
                "categories"
            } else {
                "tags"
            };
            for name in store.names(kind)? {
                writeln!(out, "{}", visible(&name))?;
            }
        }
        Command::Completions { .. } => unreachable!(),
    }
    out.flush()?;
    Ok(())
}

fn parse_boundary(value: &str) -> std::result::Result<u32, String> {
    NaiveTime::parse_from_str(value, "%H:%M")
        .map(|time| time.num_seconds_from_midnight() / 60)
        .map_err(|_| "Expected a workday boundary between 00:00 and 23:59".to_string())
}

fn default_database() -> Result<PathBuf> {
    if let Some(data) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        let data = PathBuf::from(data);
        if data.is_absolute() {
            return Ok(data.join("hs/hs.db"));
        }
    }
    env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/share/hs/hs.db"))
        .ok_or_else(|| Error::Input("Set XDG_DATA_HOME or HOME, or supply --db PATH".into()))
}

impl PeriodArgs {
    fn period(&self) -> hs::domain::workday::Period {
        use hs::domain::workday::Period;
        if self.all {
            Period::All
        } else if self.week {
            Period::Week
        } else if self.month {
            Period::Month
        } else if let Some(from) = self.from {
            Period::Dates { from, to: self.to }
        } else {
            Period::Today
        }
    }
    #[cfg(test)]
    fn range(
        &self,
        entries: &[Entry],
        moment: &Moment,
        cutoff: &Moment,
        boundary: u32,
    ) -> Result<hs::report::ReportRange> {
        hs::domain::workday::Workday::new(boundary)?.range(self.period(), entries, moment, cutoff)
    }
}

fn show_change(out: &mut impl Write, verb: &str, change: tracking::Change) -> Result<()> {
    writeln!(
        out,
        "{verb} entry {}: {}{}{}",
        change.entry.id,
        visible(&change.entry.activity),
        change
            .entry
            .category
            .map(|value| format!("@{}", visible(&value)))
            .unwrap_or_default(),
        change
            .entry
            .task
            .map(|task| format!(" [task {}]", task.id))
            .unwrap_or_default()
    )?;
    if !change.adjusted.is_empty() {
        writeln!(
            out,
            "Adjusted existing entries: {}",
            change
                .adjusted
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )?;
    }
    Ok(())
}

fn show_entry(out: &mut impl Write, entry: &Entry, now: &Moment) -> Result<()> {
    let elapsed = report::entry_seconds_at(entry, now)?
        .map(duration_seconds)
        .unwrap_or_else(|| "?".into());
    let start = entry
        .clock
        .map(|clock| clock.start_utc.with_timezone(&clock.zone).to_rfc3339())
        .unwrap_or_else(|| entry.start.format("%Y-%m-%d %H:%M").to_string());
    let end = match entry.end {
        Some(end) => entry
            .clock
            .and_then(|clock| {
                clock
                    .end_utc
                    .map(|utc| utc.with_timezone(&clock.zone).to_rfc3339())
            })
            .unwrap_or_else(|| end.format("%Y-%m-%d %H:%M").to_string()),
        None if entry.running => "running".into(),
        None => "unresolved".into(),
    };
    let category = entry
        .category
        .as_ref()
        .map(|value| format!("@{}", visible(value)))
        .unwrap_or_default();
    let tags = entry
        .tags
        .iter()
        .map(|tag| format!("#{}", visible(tag)))
        .collect::<Vec<_>>()
        .join(" ");
    let task = entry
        .task
        .map(|task| {
            format!(
                " [task {}, {}]",
                task.id,
                task.quadrant
                    .map_or("unclassified", |quadrant| quadrant.key())
            )
        })
        .unwrap_or_default();
    writeln!(
        out,
        "{}\t{}\t{}\t{}\t{}{}\t{} {}{}",
        entry.id,
        start,
        end,
        elapsed,
        visible(&entry.activity),
        category,
        visible(&entry.description),
        tags,
        task
    )?;
    Ok(())
}

pub(crate) fn duration_seconds(seconds: i64) -> String {
    let base = format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60);
    if seconds % 60 == 0 {
        base
    } else {
        format!("{base} {:02}s", seconds % 60)
    }
}

pub(crate) fn visible(text: &str) -> String {
    text.chars()
        .flat_map(|character| {
            if character.is_control() {
                character.escape_default().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

fn matches_search(entry: &Entry, needle: &str) -> bool {
    [
        &entry.activity,
        entry.category.as_deref().unwrap_or(""),
        &entry.description,
    ]
    .iter()
    .any(|text| text.to_lowercase().contains(needle))
        || entry
            .tags
            .iter()
            .any(|tag| tag.to_lowercase().contains(needle))
}

fn completions(shell: Shell) -> Vec<u8> {
    let shell = match shell {
        Shell::Bash => clap_complete::Shell::Bash,
        Shell::Zsh => clap_complete::Shell::Zsh,
        Shell::Fish => clap_complete::Shell::Fish,
    };
    // Generate from the same command schema as parsing. Buffering also lets our
    // normal I/O error handling own stdout failures from a closed shell pipe.
    let mut output = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "hs", &mut output);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use hs::{model::EntryClock, time::local_at};

    #[test]
    fn all_history_contains_work_after_a_repeated_workday_boundary() {
        let zone = chrono_tz::Europe::Zagreb;
        let now = Moment::at("2026-10-25T01:20:00Z".parse().unwrap(), zone);
        let start = "2026-10-25T01:10:00Z".parse().unwrap();
        let entry = Entry {
            id: 1,
            activity: "Review".into(),
            category: None,
            description: String::new(),
            tags: vec![],
            start: local_at(start, zone).unwrap(),
            end: None,
            clock: Some(EntryClock {
                start_utc: start,
                end_utc: None,
                zone,
            }),
            task: None,
            running: true,
        };
        let range = PeriodArgs {
            all: true,
            ..Default::default()
        }
        .range(std::slice::from_ref(&entry), &now, &now, 150)
        .unwrap();
        assert_eq!(range.start.date().to_string(), "2026-10-25");
        assert_eq!(range.end.date().to_string(), "2026-10-26");
        assert!(report::in_range_at(&entry, &range, &now).unwrap());
        assert_eq!(
            report::summarize_at(&[entry], &range, GroupBy::Activity, &now)
                .unwrap()
                .total_seconds,
            600
        );
    }

    #[test]
    fn calendar_period_uses_invocation_date_while_all_history_uses_elapsed_cutoff() {
        let now = Moment::at("2026-11-23T23:59:00Z".parse().unwrap(), chrono_tz::UTC);
        let cutoff = Moment::at("2026-11-24T00:10:00Z".parse().unwrap(), chrono_tz::UTC);
        let start = "2026-11-23T23:58:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();
        let entry = Entry {
            id: 1,
            activity: "Review".into(),
            category: None,
            description: String::new(),
            tags: vec![],
            start: start.naive_utc(),
            end: None,
            clock: Some(EntryClock {
                start_utc: start,
                end_utc: None,
                zone: chrono_tz::UTC,
            }),
            task: None,
            running: true,
        };
        let today = PeriodArgs::default()
            .range(std::slice::from_ref(&entry), &now, &cutoff, 0)
            .unwrap();
        assert_eq!(today.start.date().to_string(), "2026-11-23");
        assert_eq!(
            report::summarize_at(std::slice::from_ref(&entry), &today, GroupBy::Day, &cutoff)
                .unwrap()
                .total_seconds,
            120
        );
        let all = PeriodArgs {
            all: true,
            ..Default::default()
        }
        .range(&[entry], &now, &cutoff, 0)
        .unwrap();
        assert_eq!(all.end.date().to_string(), "2026-11-25");
    }
}
