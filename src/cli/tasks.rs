use std::{collections::BTreeMap, io::Write};

mod matrix;

use crate::cli::output::{TaskAction, TaskRead, View};
use chrono::NaiveDate;
use clap::{Args, Subcommand, ValueEnum};
use hs::{
    Error, Result, Store,
    application::{
        editing::{TaskEdit, parse_tags},
        pomodoro, queries as snapshot, tasks, workflows as tracking,
    },
    domain::{
        ids::TaskId,
        model::ActivityTemplate,
        parse::parse_activity_template,
        tasks::{Quadrant, Task, TaskInput, TaskStatus},
        time::Moment,
    },
};

const COLUMNS: &str = "ID\tPRIORITY\tSTATE\tDUE\tRECORDED\tTASK\tTIMER";

#[derive(Clone, Copy, ValueEnum)]
pub enum Priority {
    Do,
    Plan,
    Delegate,
    Drop,
    Inbox,
}

impl Priority {
    fn quadrant(self) -> Option<Quadrant> {
        match self {
            Self::Do => Some(Quadrant::Do),
            Self::Plan => Some(Quadrant::Plan),
            Self::Delegate => Some(Quadrant::Delegate),
            Self::Drop => Some(Quadrant::Drop),
            Self::Inbox => None,
        }
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Capture a finishable outcome; unclassified tasks enter the inbox
    Add {
        title: String,
        /// Optional plain-text label for time reports; initially defaults to the task title
        #[arg(long, conflicts_with = "activity")]
        time_label: Option<String>,
        /// Optional grouping, e.g. Work or Personal; overrides a category in --activity
        #[arg(long)]
        category: Option<String>,
        /// Hamster-style time label and metadata, e.g. 'Writing@Work, #client'
        #[arg(long, conflicts_with = "time_label")]
        activity: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, value_enum, default_value = "inbox")]
        quadrant: Priority,
        /// Calendar deadline YYYY-MM-DD; never changes urgency automatically
        #[arg(long)]
        due: Option<NaiveDate>,
    },
    /// List open tasks, or include done and archived tasks with --all
    List {
        #[arg(long)]
        all: bool,
    },
    /// Show task details, recorded time and current timer
    Show { id: TaskId },
    /// Change task details without interrupting current work
    Set(SetArgs),
    /// Complete a task and stop its own timer, if any
    Done { id: TaskId },
    /// Reopen a task without starting a timer
    Reopen { id: TaskId },
    /// Hide a task while preserving history and stopping its own timer
    Archive { id: TaskId },
    /// Suggest an open Do task, then Plan, ordered by due date and ID
    Next,
}

#[derive(Args)]
pub struct SetArgs {
    id: TaskId,
    #[arg(long)]
    title: Option<String>,
    /// Change only the plain-text label for future time reports; preserves category, notes and tags
    #[arg(long, conflicts_with = "activity")]
    time_label: Option<String>,
    /// Change the grouping without changing the time label; empty text clears it
    #[arg(long)]
    category: Option<String>,
    /// Hamster-style label/category and optional notes/tags; omitted notes and tags are preserved
    #[arg(long, conflicts_with = "time_label")]
    activity: Option<String>,
    /// Empty text clears the notes
    #[arg(long)]
    notes: Option<String>,
    /// Replace tags with a comma-separated list; empty clears them
    #[arg(long)]
    tags: Option<String>,
    #[arg(long, value_enum)]
    quadrant: Option<Priority>,
    /// YYYY-MM-DD, or 'none' to clear the deadline
    #[arg(long)]
    due: Option<String>,
}

pub fn run(
    command: Command,
    store: &mut Store,
    now: &Moment,
    machine: bool,
    out: &mut impl Write,
) -> Result<()> {
    match command {
        Command::Add {
            title,
            time_label,
            category,
            activity: input,
            notes,
            tags: tag_input,
            quadrant,
            due,
        } => {
            let time_label = time_label.as_deref().map(task_time_label).transpose()?;
            let mut entry = if let Some(input) = input {
                parse_activity_template(&input)?
            } else {
                ActivityTemplate {
                    activity: time_label.as_deref().unwrap_or(&title).trim().to_string(),
                    category: None,
                    description: String::new(),
                    tags: vec![],
                }
            };
            if let Some(category) = category {
                entry.category = task_category(&category);
            }
            if let Some(notes) = notes {
                entry.description = notes;
            }
            if let Some(value) = tag_input {
                entry.tags = parse_tags(&value);
            }
            let task = tasks::add(
                store,
                TaskInput {
                    title,
                    activity: entry,
                    quadrant: quadrant.quadrant(),
                    due_date: due,
                },
                now,
            )?;
            writeln!(
                out,
                "Created task {}: {}",
                task.id,
                crate::cli::visible(&task.title)
            )?;
        }
        Command::Set(args) => {
            let activity = args
                .activity
                .as_deref()
                .map(parse_activity_template)
                .transpose()?;
            let due = args
                .due
                .as_deref()
                .map(|value| {
                    if value.eq_ignore_ascii_case("none") {
                        Ok(None)
                    } else {
                        value.parse::<NaiveDate>().map(Some).map_err(|_| {
                            Error::Input("Expected YYYY-MM-DD or 'none' for --due".into())
                        })
                    }
                })
                .transpose()?;
            let patch = TaskEdit {
                title: args.title,
                activity,
                activity_name: args
                    .time_label
                    .as_deref()
                    .map(task_time_label)
                    .transpose()?,
                category: args.category.as_deref().map(task_category),
                notes: args.notes,
                tags: args.tags.as_deref().map(parse_tags),
                quadrant: args.quadrant.map(Priority::quadrant),
                due,
            };
            let task = tracking::edit_task(store, args.id, patch)?;
            writeln!(
                out,
                "Updated task {}: {}",
                task.id,
                crate::cli::visible(&task.title)
            )?;
        }
        Command::Done { id } => change_status(store, id, TaskStatus::Done, now, out)?,
        Command::Archive { id } => change_status(store, id, TaskStatus::Archived, now, out)?,
        Command::Reopen { id } => change_status(store, id, TaskStatus::Open, now, out)?,
        Command::List { all } => {
            let view =
                snapshot::task_list(store, if all { None } else { Some(TaskStatus::Open) }, now)?;
            if machine {
                return View::Tasks(TaskRead::List, &view, now.local()?.date()).write(out);
            }
            let tasks = &view.tasks;
            let metrics = Metrics::from_snapshot(&view);
            if tasks.is_empty() {
                writeln!(out, "No tasks. Capture one with `hs task add TITLE`.")?;
            } else {
                writeln!(out, "{COLUMNS}")?;
                for task in tasks {
                    row(out, task, &metrics, now)?;
                }
            }
        }
        Command::Show { id } => {
            let view = snapshot::task_detail(store, id.get(), now)?;
            if machine {
                return View::Tasks(TaskRead::Show, &view, now.local()?.date()).write(out);
            }
            let task = view
                .tasks
                .iter()
                .find(|task| task.id == id.get())
                .ok_or_else(|| Error::Input(format!("Task {id} does not exist.")))?;
            let metrics = Metrics::from_snapshot(&view);
            writeln!(out, "{COLUMNS}")?;
            row(out, task, &metrics, now)?;
            if task.activity != task.title {
                writeln!(
                    out,
                    "Time label (for reports): {}",
                    crate::cli::visible(&task.activity)
                )?;
            }
            if let Some(category) = &task.category {
                writeln!(out, "Category: {}", crate::cli::visible(category))?;
            }
            if !task.tags.is_empty() {
                writeln!(
                    out,
                    "Tags: {}",
                    task.tags
                        .iter()
                        .map(|value| crate::cli::visible(value))
                        .collect::<Vec<_>>()
                        .join(", ")
                )?;
            }
            if !task.description.is_empty() {
                writeln!(out, "Notes: {}", crate::cli::visible(&task.description))?;
            }
        }
        Command::Next => {
            let view = snapshot::next_task(store, now)?;
            if machine {
                return View::Tasks(TaskRead::Next, &view, now.local()?.date()).write(out);
            }
            if let Some(task) = view.tasks.first() {
                writeln!(
                    out,
                    "Suggested task ({}; then earliest deadline):",
                    task.quadrant.map_or("Inbox", |quadrant| quadrant.label())
                )?;
                writeln!(out, "{COLUMNS}")?;
                row(out, task, &Metrics::from_snapshot(&view), now)?;
                suggest_action(&view.status, task, out)?;
            } else {
                writeln!(
                    out,
                    "No open Do or Plan tasks. Review `hs matrix` to classify your inbox."
                )?;
            }
        }
    }
    Ok(())
}

fn task_category(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn task_time_label(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Error::Input("The time label cannot be empty.".into()));
    }
    Ok(value.to_owned())
}

fn suggest_action(view: &snapshot::Status, task: &Task, out: &mut impl Write) -> Result<()> {
    let task_id = task.id;
    match TaskAction::for_task(view, task) {
        TaskAction::Watch => writeln!(out, "Use `hs pomo watch` to continue this task.")?,
        TaskAction::Resume => writeln!(out, "Use `hs pomo resume` to continue this task.")?,
        TaskAction::NextPhase => writeln!(out, "Use `hs pomo next` to continue this task.")?,
        TaskAction::StopOtherPomodoro => {
            writeln!(
                out,
                "Another Pomodoro is open. Use `hs pomo stop` before starting task {task_id}."
            )?;
            writeln!(
                out,
                "Then use `hs pomo start --task {task_id}` or `hs start --task {task_id}`."
            )?;
        }
        TaskAction::Tracking => writeln!(
            out,
            "This task is already being tracked. Use `hs status` or `hs stop`."
        )?,
        TaskAction::Start => writeln!(
            out,
            "Use `hs pomo start --task {task_id}` or `hs start --task {task_id}`."
        )?,
    }
    Ok(())
}

fn change_status(
    store: &mut Store,
    id: TaskId,
    status: TaskStatus,
    now: &Moment,
    out: &mut impl Write,
) -> Result<()> {
    let task = tracking::change_task_status(store, id, status, now)?;
    writeln!(
        out,
        "Task {} is {}: {}",
        task.id,
        task.status.key(),
        crate::cli::visible(&task.title)
    )?;
    Ok(())
}

struct Metrics<'a> {
    seconds: &'a BTreeMap<i64, i64>,
    timers: BTreeMap<i64, String>,
}

impl<'a> Metrics<'a> {
    fn from_snapshot(view: &'a snapshot::Tasks) -> Self {
        let mut result = Self {
            seconds: &view.recorded_seconds,
            timers: BTreeMap::new(),
        };
        if let Some(entry) = &view.status.current
            && let Some(task) = entry.task
        {
            result.timers.insert(task.id, "tracking".into());
        }
        if let Some(session) = &view.status.session
            && session.state() != pomodoro::State::Stopped
            && let Some(id) = session.task_id
        {
            result.timers.insert(
                id,
                format!("{} {}", session.phase().label(), session.state().label()),
            );
        }
        result
    }
}

fn row(out: &mut impl Write, task: &Task, metrics: &Metrics, now: &Moment) -> Result<()> {
    let today = now.local()?.date();
    let due = task
        .due_date
        .map(|day| {
            format!(
                "{}{}",
                day,
                if task.status == TaskStatus::Open && day < today {
                    " overdue"
                } else {
                    ""
                }
            )
        })
        .unwrap_or_else(|| "—".into());
    writeln!(
        out,
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        task.id,
        task.quadrant.map_or("Inbox", |quadrant| quadrant.label()),
        task.status.key(),
        due,
        crate::cli::duration_seconds(*metrics.seconds.get(&task.id).unwrap_or(&0)),
        crate::cli::visible(&task.title),
        metrics
            .timers
            .get(&task.id)
            .map(String::as_str)
            .unwrap_or("—")
    )?;
    Ok(())
}

pub fn matrix(store: &mut Store, now: &Moment, machine: bool, out: &mut impl Write) -> Result<()> {
    let view = snapshot::task_list(store, Some(TaskStatus::Open), now)?;
    if machine {
        return View::Tasks(TaskRead::Matrix, &view, now.local()?.date()).write(out);
    }
    let metrics = Metrics::from_snapshot(&view);
    matrix::render(
        out,
        &view.tasks,
        &metrics,
        now.local()?.date(),
        crate::cli::terminal::columns(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overdue_calendar_does_not_follow_a_frozen_timer_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("hs.db")).unwrap();
        let start = Moment::at("2026-11-23T23:58:00Z".parse().unwrap(), chrono_tz::UTC);
        let observed = Moment::at("2026-11-24T00:10:00Z".parse().unwrap(), chrono_tz::UTC);
        let rollback = Moment::at("2026-11-23T23:59:00Z".parse().unwrap(), chrono_tz::UTC);
        let task = tasks::add(
            &mut store,
            TaskInput {
                title: "Review".into(),
                activity: parse_activity_template("Writing").unwrap(),
                quadrant: Some(Quadrant::Plan),
                due_date: Some(start.local().unwrap().date()),
            },
            &start,
        )
        .unwrap();
        pomodoro::start_task(&mut store, task.id, Default::default(), &start).unwrap();
        pomodoro::status(&mut store, &observed).unwrap();
        let mut output = Vec::new();
        run(
            Command::List { all: false },
            &mut store,
            &rollback,
            false,
            &mut output,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("2026-11-23\t0h 12m"));
        assert!(!output.contains("overdue"));
    }
}
