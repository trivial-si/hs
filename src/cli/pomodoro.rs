use clap::{Args, Subcommand};
use hs::{
    Result, Store,
    application::pomodoro,
    domain::{
        ids::TaskId,
        parse::parse_activity_template,
        pomodoro::{Config, Phase, PhaseState, Session, State, WatchedPhase},
        time::Moment,
    },
};
use std::{
    env,
    io::{self, IsTerminal, Write},
    thread,
    time::Duration,
};

#[derive(Args, Clone, Copy, Default)]
pub struct FollowArgs {
    /// Return immediately instead of watching in an interactive terminal
    #[arg(long)]
    detach: bool,
    /// Mute the completion sound; keep the visual cue
    #[arg(long, visible_alias = "no-sound")]
    no_bell: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Begin focus and watch it in an interactive terminal
    Start {
        #[arg(required_unless_present = "task", conflicts_with = "task")]
        input: Option<String>,
        /// Focus on an open task by its ID
        #[arg(long)]
        task: Option<TaskId>,
        /// Focus duration, e.g. 25m, 90s, 1h (bare numbers are minutes)
        #[arg(long, default_value="25m", value_parser=pomodoro::parse_duration)]
        focus: i64,
        /// Short break duration
        #[arg(long="break", default_value="5m", value_parser=pomodoro::parse_duration)]
        short_break: i64,
        /// Long break duration
        #[arg(long, default_value="15m", value_parser=pomodoro::parse_duration)]
        long_break: i64,
        /// Completed focus periods before each long break
        #[arg(long, default_value="4", value_parser=clap::value_parser!(u32).range(1..))]
        rounds: u32,
        #[command(flatten)]
        follow: FollowArgs,
    },
    /// Show phase, remaining time and completed focus count
    Status,
    /// Pause the current phase and its work entry
    Pause,
    /// Resume the paused phase with its remaining time
    Resume {
        #[command(flatten)]
        follow: FollowArgs,
    },
    /// Start the offered break or next focus period
    Next {
        #[command(flatten)]
        follow: FollowArgs,
    },
    /// End the session, preserving work already recorded
    Stop,
    /// Show a live countdown until the phase ends or pauses; Ctrl-C leaves it running
    Watch {
        /// Mute the completion sound; keep the visual cue
        #[arg(long, visible_alias = "no-sound")]
        no_bell: bool,
    },
    /// Show completed/interrupted focus periods and recorded focus/break totals
    Stats,
}

pub fn run(
    command: Command,
    store: &mut Store,
    now: &Moment,
    machine: bool,
    out: &mut impl Write,
) -> Result<()> {
    let follow = match &command {
        Command::Start { follow, .. } | Command::Resume { follow } | Command::Next { follow } => {
            Some(*follow)
        }
        _ => None,
    };
    let session = match command {
        Command::Start {
            input,
            task,
            focus,
            short_break,
            long_break,
            rounds,
            follow: _,
        } => {
            let config = Config {
                focus_seconds: focus,
                break_seconds: short_break,
                long_break_seconds: long_break,
                rounds,
            };
            if let Some(id) = task {
                Some(pomodoro::start_task(store, id.get(), config, now)?)
            } else {
                let entry = parse_activity_template(
                    input.as_deref().expect("clap requires input or task"),
                )?
                .starting_at(now.local()?);
                Some(pomodoro::start(store, entry, config, now)?)
            }
        }
        Command::Status if machine => {
            let view = hs::snapshot::status(store, now)?;
            return crate::cli::output::View::Pomodoro(view.session.as_ref(), &view.as_of)
                .write(out);
        }
        Command::Status => pomodoro::status(store, now)?,
        Command::Pause => Some(pomodoro::pause(store, now)?),
        Command::Resume { .. } => Some(pomodoro::resume(store, now)?),
        Command::Next { .. } => Some(pomodoro::next(store, now)?),
        Command::Stop => pomodoro::stop(store, now)?,
        Command::Watch { no_bell } => return watch(store, out, no_bell, None),
        Command::Stats => {
            let totals = pomodoro::statistics(store, now)?;
            if machine {
                return crate::cli::output::View::Statistics(&totals).write(out);
            }
            writeln!(out, "Completed focus periods\t{}", totals.completed)?;
            writeln!(out, "Interrupted focus periods\t{}", totals.interrupted)?;
            writeln!(
                out,
                "Recorded focus time\t{}",
                crate::cli::duration_seconds(totals.focus_seconds)
            )?;
            writeln!(
                out,
                "Timed breaks\t{}",
                crate::cli::duration_seconds(totals.break_seconds)
            )?;
            return Ok(());
        }
    };
    show(out, session.as_ref(), now)?;
    if let (Some(follow), Some(session)) = (follow, session.as_ref())
        && session.state() == State::Running
    {
        if !follow.detach && io::stdin().is_terminal() && io::stdout().is_terminal() {
            return watch(store, out, follow.no_bell, session.phase_id());
        }
        writeln!(
            out,
            "Use `hs pomo watch` for a live countdown and completion cues."
        )?;
    }
    Ok(())
}

pub fn show(out: &mut impl Write, session: Option<&Session>, now: &Moment) -> Result<()> {
    let Some(session) = session else {
        writeln!(
            out,
            "No Pomodoro session. Start one with `hs pomo start ACTIVITY`."
        )?;
        return Ok(());
    };
    writeln!(
        out,
        "Pomodoro {} — {}, {}, {} remaining",
        session.id,
        session.phase().label(),
        session.state().label(),
        countdown(session.remaining(now))
    )?;
    writeln!(
        out,
        "{} · {} focus periods completed",
        crate::cli::visible(&session.activity),
        session.completed_focus()
    )?;
    if let Some(id) = session.task_id {
        writeln!(
            out,
            "Task {}: {}",
            id,
            crate::cli::visible(&session.description)
        )?;
    }
    match session.state() {
        State::Ready => writeln!(
            out,
            "Use `hs pomo next` to start {}.",
            session.phase().label()
        )?,
        State::Paused => writeln!(out, "Use `hs pomo resume` to continue.")?,
        _ => {}
    }
    Ok(())
}

fn countdown(seconds: i64) -> String {
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

fn live_countdown(phase: Phase, seconds: i64) -> String {
    // Keep the redraw on one row in narrow terminal panes. Even the longest
    // supported four-digit-year interval fits in 24 columns with these labels.
    let label = match phase {
        Phase::Focus => "Focus",
        Phase::ShortBreak => "Rest",
        Phase::LongBreak => "Long rest",
    };
    format!("{label} {}", countdown(seconds))
}

fn watch(
    store: &mut Store,
    out: &mut impl Write,
    no_bell: bool,
    mut watched: Option<i64>,
) -> Result<()> {
    let interactive = io::stdout().is_terminal();
    let animate = interactive && env::var_os("TERM").is_none_or(|value| value != "dumb");
    let emphasize = animate && env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    if interactive {
        writeln!(
            out,
            "Watching this phase. Ctrl-C returns to the shell; the timer stays saved."
        )?;
        if !no_bell {
            writeln!(out, "Completion sound is on.")?;
        }
        out.flush()?;
    }
    let mut previous_line = None;
    let mut waiting = false;
    loop {
        let now = hs::application::clock::now()?;
        let view = match pomodoro::watch_status(store, watched, &now) {
            Ok(view) => view,
            Err(error) if error.classification().code == "database_busy" => {
                if !waiting {
                    finish_line(out, animate, previous_line.is_some())?;
                    writeln!(out, "Waiting for database access; the timer remains saved.")?;
                    out.flush()?;
                    waiting = true;
                    previous_line = None;
                }
                thread::sleep(Duration::from_millis(250));
                continue;
            }
            Err(error) => {
                finish_line(out, animate, previous_line.is_some())?;
                return Err(error);
            }
        };
        waiting = false;
        let Some(phase) = view.watched_phase else {
            finish_line(out, animate, previous_line.is_some())?;
            show(out, view.session.as_ref(), &now)?;
            out.flush()?;
            return Ok(());
        };
        watched = Some(phase.id);
        let same_session = view
            .session
            .as_ref()
            .is_some_and(|s| s.id == phase.session_id);
        if !same_session {
            finish_line(out, animate, previous_line.is_some())?;
            writeln!(
                out,
                "The watched session was replaced. No completion cue was played."
            )?;
            show(out, view.session.as_ref(), &now)?;
            out.flush()?;
            return Ok(());
        }
        if phase.state == PhaseState::Completed {
            finish_line(out, animate, previous_line.is_some())?;
            completion(out, &phase, view.session.as_ref(), emphasize)?;
            out.flush()?;
            if interactive && !no_bell && !crate::cli::notification::ring() {
                write!(out, "\x07")?;
                out.flush()?;
            }
            return Ok(());
        }
        if phase.state != PhaseState::Running {
            finish_line(out, animate, previous_line.is_some())?;
            show(out, view.session.as_ref(), &now)?;
            out.flush()?;
            return Ok(());
        }
        let Some(session) = view.session.as_ref() else {
            return Ok(());
        };
        let line = live_countdown(phase.phase, session.remaining(&now));
        if previous_line.as_ref() != Some(&line) {
            if animate {
                write!(out, "\r\x1b[2K{line}")?;
            } else {
                writeln!(out, "{line}")?;
            }
            out.flush()?;
            previous_line = Some(line);
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn finish_line(out: &mut impl Write, animate: bool, has_line: bool) -> Result<()> {
    if animate && has_line {
        write!(out, "\r\x1b[2K")?;
    }
    Ok(())
}

fn completion(
    out: &mut impl Write,
    phase: &WatchedPhase,
    latest: Option<&Session>,
    emphasize: bool,
) -> Result<()> {
    let (title, message) = match phase.phase {
        Phase::Focus => ("[FOCUS COMPLETE]", "Your work period is finished."),
        Phase::ShortBreak => ("[BREAK COMPLETE]", "Your short break is finished."),
        Phase::LongBreak => ("[BREAK COMPLETE]", "Your long break is finished."),
    };
    if emphasize {
        writeln!(out, "\x1b[1;7m {title} \x1b[0m")?;
    } else {
        writeln!(out, "{title}")?;
    }
    writeln!(out, "{message}")?;
    if let Some(next) = latest.filter(|s| {
        s.id == phase.session_id && s.phase_id() == Some(phase.id) && s.state() == State::Ready
    }) {
        writeln!(
            out,
            "Pomodoro {} — ready for {}. Use `hs pomo next` when you are ready.",
            next.id,
            next.phase().label()
        )?;
    } else {
        writeln!(
            out,
            "The session has since changed; use `hs pomo status` to see its current state."
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_countdown_fits_narrow_panes_through_the_supported_date_range() {
        let earliest = "0000-01-01T00:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();
        let latest = "9999-12-31T23:59:59Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();
        let longest = (latest - earliest).num_seconds();
        for phase in [Phase::Focus, Phase::ShortBreak, Phase::LongBreak] {
            for seconds in [0, 59, 60, 3599, 3600, longest] {
                let line = live_countdown(phase, seconds);
                assert!(line.is_ascii());
                assert!(line.len() <= 24, "countdown wraps a narrow pane: {line}");
                assert!(line.ends_with(&countdown(seconds)));
            }
        }
        assert_eq!(live_countdown(Phase::Focus, 90), "Focus 01:30");
        assert_eq!(live_countdown(Phase::ShortBreak, 90), "Rest 01:30");
        assert_eq!(live_countdown(Phase::LongBreak, 90), "Long rest 01:30");
    }

    #[test]
    fn focus_and_both_breaks_have_persistent_distinct_completion_messages() {
        for (phase, label, detail) in [
            (Phase::Focus, "[FOCUS COMPLETE]", "work period"),
            (Phase::ShortBreak, "[BREAK COMPLETE]", "short break"),
            (Phase::LongBreak, "[BREAK COMPLETE]", "long break"),
        ] {
            let phase = WatchedPhase {
                id: 1,
                session_id: 2,
                phase,
                state: PhaseState::Completed,
                ended_utc: None,
            };
            let mut text = Vec::new();
            completion(&mut text, &phase, None, false).unwrap();
            let text = String::from_utf8(text).unwrap();
            assert!(text.contains(label));
            assert!(text.contains(detail));
            assert!(!text.contains('\x1b'));
            assert!(!text.contains('\x07'));
        }
    }
}
