//! The timer card is presentation only. Space always describes the controller's
//! contextual action; p describes its separate selected-task action.
use hs::domain::{
    pomodoro::{Config, Phase, State},
    report,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Gauge, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use super::Screen;

struct Card {
    heading: String,
    clock: String,
    context: String,
    detail: String,
    primary: String,
    compact_primary: String,
    selected: String,
    progress: Option<f64>,
    color: Color,
    complete: bool,
}

pub(super) fn height(area: Rect) -> u16 {
    if area.width >= 56 && area.height >= 24 {
        8
    } else if area.height >= 18 {
        6
    } else if area.height >= 12 {
        4
    } else {
        2
    }
}

pub(super) fn render(frame: &mut Frame, area: Rect, screen: &Screen<'_>) {
    let card = Card::new(screen);
    let mut emphasis = Style::default().add_modifier(Modifier::BOLD);
    if screen.color {
        emphasis = emphasis.fg(card.color);
    }
    if card.complete {
        emphasis = emphasis.add_modifier(Modifier::REVERSED);
    }
    if area.height < 6 {
        compact(frame, area, screen, &card, emphasis);
        return;
    }
    let block = Block::default()
        .title(" POMODORO ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(emphasis);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if area.height >= 8 {
        let sections = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);
        let digits = digits(&card.clock);
        let digit_width = UnicodeWidthStr::width(digits[0].as_str()) as u16;
        // Very long configured durations remain a complete, readable ordinary
        // countdown rather than overflowing the card or hiding its controls.
        if digit_width + 30 <= inner.width {
            let columns =
                Layout::horizontal([Constraint::Length(digit_width + 3), Constraint::Min(0)])
                    .split(sections[0]);
            frame.render_widget(
                Paragraph::new(digits.into_iter().map(Line::raw).collect::<Vec<_>>())
                    .style(emphasis),
                columns[0],
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(format!("{}  |  {}", card.heading, card.clock), emphasis),
                    Line::raw(&card.context),
                    Line::raw(&card.detail),
                ]),
                columns[1],
            );
        } else {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(format!("{}  {}", card.clock, card.heading), emphasis),
                    Line::raw(&card.context),
                    Line::raw(&card.detail),
                ]),
                sections[0],
            );
        }
        frame.render_widget(
            Paragraph::new(Line::styled(&card.primary, emphasis)),
            sections[1],
        );
        frame.render_widget(Paragraph::new(card.selected.as_str()), sections[2]);
        if let Some(ratio) = card.progress {
            frame.render_widget(
                Gauge::default()
                    .ratio(ratio)
                    .label(format!("{}% elapsed", (ratio * 100.0).floor() as u8))
                    .gauge_style(emphasis),
                sections[3],
            );
        } else {
            frame.render_widget(
                Paragraph::new(if card.complete {
                    "The next phase starts when you press Space or N."
                } else {
                    "Select a task in the matrix or inbox, then press Space."
                }),
                sections[3],
            );
        }
    } else {
        compact(frame, inner, screen, &card, emphasis);
    }
}

fn compact(frame: &mut Frame, area: Rect, screen: &Screen<'_>, card: &Card, style: Style) {
    let action = if area.width >= 36 {
        &card.primary
    } else {
        &card.compact_primary
    };
    // Put the state after the countdown so a narrow window never clips the
    // remaining time or the primary key. Extra context has lower priority.
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{}  ", card.clock), style),
            Span::styled(&card.heading, style),
        ]),
        Line::styled(action, style),
    ];
    if area.height >= 3 {
        lines.push(Line::raw(&card.context));
    }
    if area.height >= 4 {
        lines.push(Line::raw(if screen.filter.is_empty() {
            card.selected.clone()
        } else {
            format!(
                "Filter: {} [Esc clears]",
                crate::cli::visible(screen.filter)
            )
        }));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

impl Card {
    fn new(screen: &Screen<'_>) -> Self {
        let selected = screen
            .selected
            .and_then(|id| screen.snapshot.tasks.iter().find(|task| task.id == id));
        let session = screen
            .snapshot
            .status
            .session
            .as_ref()
            .filter(|session| session.state() != State::Stopped);
        let sound = if screen.sound { "on" } else { "off" };
        if let Some(session) = session {
            let ready = session.state() == State::Ready;
            let phase = session.phase();
            let remaining = session.remaining(&screen.snapshot.status.as_of);
            let heading = if ready {
                if phase == Phase::Focus {
                    "BREAK COMPLETE"
                } else {
                    "FOCUS COMPLETE"
                }
                .into()
            } else {
                format!(
                    "{} {}",
                    phase.label().to_uppercase(),
                    session.state().label().to_uppercase()
                )
            };
            let action = match session.state() {
                State::Running => "Pause".to_owned(),
                State::Paused => "Resume".to_owned(),
                State::Ready => format!("Start {}", phase.label()),
                State::Stopped => unreachable!(),
            };
            let compact_action = match session.state() {
                State::Ready if phase != Phase::Focus => "Start break",
                State::Ready => "Start focus",
                State::Paused => "Resume",
                _ => "Pause",
            };
            let primary = if ready {
                format!("[Space] {action}   [N] same action   [x] Stop")
            } else {
                format!("[Space] {action}   [x] Stop   [b] Sound {sound}")
            };
            let title = session
                .task_id
                .and_then(|id| screen.snapshot.tasks.iter().find(|task| task.id == id));
            let context = title.map_or_else(
                || format!("Current: {}", crate::cli::visible(&session.activity)),
                |task| format!("Current #{}: {}", task.id, crate::cli::visible(&task.title)),
            );
            let selected = match selected {
                Some(task) if session.task_id != Some(task.id) => format!(
                    "[p] New focus / switch to #{}: {}",
                    task.id,
                    crate::cli::visible(&task.title)
                ),
                Some(_) if session.state() == State::Running => {
                    "[p] This task's Pomodoro is already running.".into()
                }
                Some(_) => format!("[p] {action} for this task"),
                None => "Choose a task in the matrix or inbox to start new focus.".into(),
            };
            let total = phase_seconds(session.config(), phase);
            Self {
                heading,
                clock: countdown(if ready { 0 } else { remaining }),
                context,
                detail: if ready {
                    format!("Next: {} ({})", phase.label(), countdown(remaining))
                } else {
                    format!(
                        "{} focus completed  |  long break every {}",
                        session.completed_focus(),
                        session.config().rounds
                    )
                },
                primary,
                compact_primary: format!("[Space] {compact_action}"),
                selected,
                progress: (!ready)
                    .then(|| ((total - remaining) as f64 / total as f64).clamp(0.0, 1.0)),
                color: if ready || session.state() == State::Paused {
                    Color::Yellow
                } else if phase == Phase::Focus {
                    Color::Cyan
                } else {
                    Color::Green
                },
                complete: ready,
            }
        } else {
            let current = screen.snapshot.status.current.as_ref();
            let context = current.map_or_else(
                || {
                    selected.map_or_else(
                        || "No task selected".into(),
                        |task| {
                            format!(
                                "Selected #{}: {}",
                                task.id,
                                crate::cli::visible(&task.title)
                            )
                        },
                    )
                },
                |entry| {
                    let elapsed = report::entry_seconds_at(entry, &screen.snapshot.status.as_of)
                        .ok()
                        .flatten()
                        .map(countdown)
                        .unwrap_or_else(|| "?".into());
                    format!(
                        "Tracking: {} ({elapsed})",
                        crate::cli::visible(&entry.activity)
                    )
                },
            );
            let primary = match (selected, current) {
                (Some(_), Some(_)) => "[Space / p] Switch to Pomodoro   [x] Stop tracking".into(),
                (Some(_), None) => "[Space / p] Start focus   [s] Track task".into(),
                (None, Some(_)) => "[n] Add/select task for focus   [x] Stop tracking".into(),
                (None, None) => "[n] Add a task, or select one with Tab and j/k".into(),
            };
            Self {
                heading: "READY TO FOCUS".into(),
                clock: countdown(Config::default().focus_seconds),
                context,
                detail: "25m focus / 5m break / 15m long break".into(),
                primary,
                compact_primary: if selected.is_some() {
                    if current.is_some() {
                        "[Space] Switch"
                    } else {
                        "[Space] Start focus"
                    }
                } else if current.is_some() {
                    "[x] Stop tracking"
                } else {
                    "[n] Add a task"
                }
                .into(),
                selected: selected.map_or_else(
                    || "Tab / 1-5: choose a pane; n: add a task".into(),
                    |task| {
                        format!(
                            "[p] New focus for #{}: {}",
                            task.id,
                            crate::cli::visible(&task.title)
                        )
                    },
                ),
                progress: None,
                color: Color::Cyan,
                complete: false,
            }
        }
    }
}

fn phase_seconds(config: Config, phase: Phase) -> i64 {
    match phase {
        Phase::Focus => config.focus_seconds,
        Phase::ShortBreak => config.break_seconds,
        Phase::LongBreak => config.long_break_seconds,
    }
}

fn countdown(seconds: i64) -> String {
    let seconds = seconds.max(0);
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

fn digits(text: &str) -> [String; 3] {
    // Solid half-block strokes form a compact 3-by-5 pixel font. Each glyph
    // keeps the same cell width as before, so controls and tasks retain room.
    let mut rows = [String::new(), String::new(), String::new()];
    for (index, ch) in text.chars().enumerate() {
        let glyph = match ch {
            '0' => ["█▀█", "█ █", "▀▀▀"],
            '1' => ["▄█ ", " █ ", "▀▀▀"],
            '2' => ["▀▀█", "█▀▀", "▀▀▀"],
            '3' => ["▀▀█", "▀▀█", "▀▀▀"],
            '4' => ["█ █", "▀▀█", "  ▀"],
            '5' => ["█▀▀", "▀▀█", "▀▀▀"],
            '6' => ["█▀▀", "█▀█", "▀▀▀"],
            '7' => ["▀▀█", " █ ", " ▀ "],
            '8' => ["█▀█", "█▀█", "▀▀▀"],
            '9' => ["█▀█", "▀▀█", "▀▀▀"],
            ':' => ["▄", "▄", " "],
            _ => [" ", " ", " "],
        };
        for (row, part) in rows.iter_mut().zip(glyph) {
            if index > 0 {
                row.push(' ');
            }
            row.push_str(part);
        }
    }
    rows
}
