//! Terminal presentation only: all task and timer changes stay in the controller.
mod pomodoro;

use chrono::NaiveDate;
use hs::{
    application::queries::Tasks,
    domain::{pomodoro::State, tasks::Task},
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Pane order follows keyboard numbers, not physical matrix position.
const TITLES: [&str; 5] = ["I DO", "II PLAN", "III DROP", "IV DELEGATE", "INBOX"];
const MEANINGS: [&str; 5] = [
    "important / urgent",
    "important / not urgent",
    "not important / not urgent",
    "not important / urgent",
    "unclassified tasks",
];

pub(super) struct Screen<'a> {
    pub snapshot: &'a Tasks,
    pub panes: [Vec<&'a Task>; 5],
    pub active: usize,
    pub selected: Option<i64>,
    pub filter: &'a str,
    pub message: &'a str,
    pub modal: Modal<'a>,
    pub modal_scroll: u16,
    pub today: NaiveDate,
    pub sound: bool,
    pub color: bool,
}

#[derive(Clone, Copy, Default)]
pub(super) enum Modal<'a> {
    #[default]
    None,
    Help,
    Details(&'a Task),
    Input {
        title: &'a str,
        value: &'a str,
        hint: &'a str,
    },
    Move {
        title: &'a str,
    },
    Confirm {
        title: &'a str,
        message: &'a str,
    },
}

/// Return the effective modal offset so the controller cannot accumulate
/// invisible overscroll beyond the last page, including after a resize.
pub(super) fn render(frame: &mut Frame, screen: &Screen<'_>) -> u16 {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return screen.modal_scroll;
    }
    if area.width < 20 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("hs tui\nEnlarge this terminal.\nCtrl-C: quit\nTimer stays saved.")
                .wrap(Wrap { trim: true }),
            area,
        );
        return screen.modal_scroll;
    }
    let context_height = u16::from(area.height >= 18);
    let footer_height = if area.height < 12 { 2 } else { 3 };
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(pomodoro::height(area)),
            Constraint::Length(context_height),
            Constraint::Min(3),
            Constraint::Length(footer_height),
        ])
        .split(area);
    header(frame, areas[0], screen);
    pomodoro::render(frame, areas[1], screen);
    context(frame, areas[2], screen);
    if areas[3].width >= 70 && areas[3].height >= 11 {
        matrix(frame, areas[3], screen);
    } else {
        pane(frame, areas[3], screen, screen.active.min(4));
    }
    footer(frame, areas[4], screen);
    popup(frame, area, screen)
}

fn accent(color: bool) -> Style {
    if color {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

fn subdued(color: bool) -> Style {
    if color {
        Style::default().fg(Color::Gray)
    } else {
        Style::default()
    }
}

fn header(frame: &mut Frame, area: Rect, screen: &Screen<'_>) {
    if frame.area().height < 18 && !screen.filter.is_empty() {
        frame.render_widget(
            Paragraph::new(format!(
                "Filter: {} [Esc clears]",
                crate::cli::visible(screen.filter)
            ))
            .style(accent(screen.color)),
            area,
        );
        return;
    }
    let title = Line::from(vec![
        Span::styled(" hs ", accent(screen.color).add_modifier(Modifier::BOLD)),
        Span::raw("MATRIX  |  "),
        Span::raw(if screen.sound {
            "sound on [b]"
        } else {
            "sound off [b]"
        }),
    ]);
    frame.render_widget(Paragraph::new(title), area);
}

fn context(frame: &mut Frame, area: Rect, screen: &Screen<'_>) {
    let text = if screen.filter.is_empty() {
        " Importance increases up  |  Urgency increases right".to_owned()
    } else {
        format!(
            " Filter: {}  [Esc to clear]",
            crate::cli::visible(screen.filter)
        )
    };
    frame.render_widget(Paragraph::new(text).style(subdued(screen.color)), area);
}

fn matrix(frame: &mut Frame, area: Rect, screen: &Screen<'_>) {
    let inbox_height = if area.height >= 15 { 5 } else { 3 };
    let rows = Layout::vertical([Constraint::Min(8), Constraint::Length(inbox_height)]).split(area);
    let quadrants =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[0]);
    let upper = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(quadrants[0]);
    let lower = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(quadrants[1]);
    pane(frame, upper[0], screen, 1);
    pane(frame, upper[1], screen, 0);
    pane(frame, lower[0], screen, 2);
    pane(frame, lower[1], screen, 3);
    pane(frame, rows[1], screen, 4);
}

fn pane(frame: &mut Frame, area: Rect, screen: &Screen<'_>, index: usize) {
    let active = index == screen.active;
    let border_style = if active {
        accent(screen.color).add_modifier(Modifier::BOLD)
    } else {
        subdued(screen.color)
    };
    let title = format!(
        " [{}] {} ({}) ",
        index + 1,
        TITLES[index],
        screen.panes[index].len()
    );
    let block = Block::default()
        .title(title)
        .title_bottom(format!(" {} ", MEANINGS[index]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);
    if screen.panes[index].is_empty() {
        frame.render_widget(
            Paragraph::new(if screen.filter.is_empty() {
                "No tasks. Press n to add one."
            } else {
                "No matching tasks."
            })
            .block(block)
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let selected = active
        .then(|| {
            screen.panes[index]
                .iter()
                .position(|task| Some(task.id) == screen.selected)
        })
        .flatten();
    // Ratatui cannot display an item taller than the list viewport. A compact
    // inbox has one content row, so retain the title and omit its metadata.
    let inner = block.inner(area);
    let show_details = inner.height >= 2;
    // The list reserves two columns for "> " when an item is selected.
    let title_width = inner
        .width
        .saturating_sub(if selected.is_some() { 2 } else { 0 });
    let items: Vec<_> = screen.panes[index]
        .iter()
        .map(|task| {
            let title = format!("#{} {}", task.id, task.title);
            let mut lines = vec![Line::raw(clipped_title(&title, usize::from(title_width)))];
            let detail = task_summary(task, screen.snapshot, screen.today);
            if show_details && !detail.is_empty() {
                lines.push(Line::styled(format!("  {detail}"), subdued(screen.color)));
            }
            ListItem::new(lines)
        })
        .collect();
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_symbol("> ")
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );
}

fn task_summary(task: &Task, snapshot: &Tasks, today: NaiveDate) -> String {
    let mut pieces = Vec::new();
    if let Some(due) = task.due_date {
        let overdue = due < today;
        pieces.push(format!("due {due}{}", if overdue { " !" } else { "" }));
    }
    let seconds = snapshot
        .recorded_seconds
        .get(&task.id)
        .copied()
        .unwrap_or(0);
    if seconds > 0 {
        pieces.push(crate::cli::duration_seconds(seconds));
    }
    if let Some(session) = snapshot
        .status
        .session
        .as_ref()
        .filter(|session| session.state() != State::Stopped && session.task_id == Some(task.id))
    {
        pieces.push(format!(
            "{} {}",
            session.phase().label(),
            session.state().label()
        ));
    } else if snapshot
        .status
        .current
        .as_ref()
        .is_some_and(|entry| entry.task.is_some_and(|link| link.id == task.id))
    {
        pieces.push("tracking".into());
    }
    pieces.join(" | ")
}

fn footer(frame: &mut Frame, area: Rect, screen: &Screen<'_>) {
    let mut message_style = accent(screen.color).add_modifier(Modifier::BOLD);
    if screen.message.starts_with("[FOCUS COMPLETE]")
        || screen.message.starts_with("[BREAK COMPLETE]")
    {
        message_style = message_style.add_modifier(Modifier::REVERSED);
    }
    let (navigation, actions) = if area.width >= 70 {
        (
            "Arrows/Tab/1-5 panes  j/k tasks  Enter details  / search  ? help",
            "n add  e rename  m move  d done  p pomo  Space timer  q quit",
        )
    } else if area.width >= 35 {
        (
            "Arrows/Tab panes  j/k tasks  ? help",
            "n add  Enter details  q quit",
        )
    } else {
        ("Arrows panes  ? help", "j/k tasks  q quit")
    };
    let mut lines = vec![Line::styled(
        crate::cli::visible(screen.message),
        message_style,
    )];
    if area.height >= 3 {
        lines.push(Line::raw(navigation));
    }
    lines.push(Line::raw(actions));
    frame.render_widget(Paragraph::new(lines), area);
}

fn centered(area: Rect, desired_width: u16, desired_height: u16) -> Rect {
    let width = desired_width.min(area.width.saturating_sub(2));
    let height = desired_height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn popup(frame: &mut Frame, area: Rect, screen: &Screen<'_>) -> u16 {
    let (title, content, height, hint) = match screen.modal {
        Modal::None => return 0,
        Modal::Help => (
            "Keyboard help".to_owned(),
            help(),
            25,
            "PgUp/PgDn: scroll  Esc: close",
        ),
        Modal::Details(task) => (
            format!("Task #{}", task.id),
            details(task, screen.snapshot),
            24,
            "PgUp/PgDn: scroll  Esc: close",
        ),
        Modal::Input { title, value, hint } => {
            input(frame, area, screen, title, value, hint);
            return 0;
        }
        Modal::Move { title } => {
            let title = clipped_title(title, usize::from(area.width.saturating_sub(4).min(72)));
            let choices = if area.width >= 60 {
                "1  DO        important, urgent\n2  PLAN      important, not urgent\n3  DROP      not important, not urgent\n4  DELEGATE  not important, urgent\n5  INBOX     unclassified"
            } else {
                "1  DO\n2  PLAN\n3  DROP\n4  DELEGATE\n5  INBOX"
            };
            (
                "Move task".into(),
                Text::from(format!("{title}\n\n{choices}")),
                12,
                "1-5: move  Esc: cancel",
            )
        }
        Modal::Confirm { title, message } => (
            crate::cli::visible(title),
            Text::from(
                message
                    .lines()
                    .map(|line| Line::raw(crate::cli::visible(line)))
                    .collect::<Vec<_>>(),
            ),
            9,
            if area.width >= 50 {
                "y: confirm  n/Esc: cancel  PgUp/PgDn: scroll"
            } else if area.width >= 31 {
                "y:yes n/Esc:no PgUp/PgDn"
            } else {
                "y:yes n/Esc:no"
            },
        ),
    };
    let popup = centered(area, 76, height);
    let block = Block::default()
        .title(format!(" {title} "))
        .title_bottom(format!(" {hint} "))
        .borders(Borders::ALL)
        .border_style(accent(screen.color));
    let inner = block.inner(popup);
    let content = wrapped_lines(content, usize::from(inner.width));
    let maximum = content.len().saturating_sub(usize::from(inner.height));
    let scroll = usize::from(screen.modal_scroll).min(maximum) as u16;
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(content).block(block).scroll((scroll, 0)),
        popup,
    );
    scroll
}

fn clipped_title(text: &str, width: usize) -> String {
    let text = crate::cli::visible(text);
    if UnicodeWidthStr::width(text.as_str()) <= width {
        return text;
    }
    if width < 3 {
        return ".".repeat(width);
    }
    let mut result = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let size = UnicodeWidthStr::width(grapheme);
        if used + size > width.saturating_sub(3) {
            break;
        }
        result.push_str(grapheme);
        used += size;
    }
    result.push_str("...");
    result
}

// Prepare exact rows so modal scrolling stops at the last useful page. Each
// modal source line has one style, retained across its wrapped rows.
fn wrapped_lines(text: Text<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut result = Vec::new();
    for source in text.lines {
        let mut current = String::new();
        let mut used = 0;
        let contents = source.to_string();
        if contents.trim().is_empty() {
            result.push(Line::raw(""));
            continue;
        }
        for word in contents.split_whitespace() {
            let size = UnicodeWidthStr::width(word);
            let spacing = usize::from(!current.is_empty());
            if used + spacing + size > width && !current.is_empty() {
                result.push(Line::styled(std::mem::take(&mut current), source.style));
                used = 0;
            }
            if !current.is_empty() {
                current.push(' ');
                used += 1;
            }
            for grapheme in word.graphemes(true) {
                let size = UnicodeWidthStr::width(grapheme);
                if used + size > width && !current.is_empty() {
                    result.push(Line::styled(std::mem::take(&mut current), source.style));
                    used = 0;
                }
                current.push_str(grapheme);
                used += size;
            }
        }
        if !current.is_empty() {
            result.push(Line::styled(current, source.style));
        }
    }
    result
}

fn input(frame: &mut Frame, area: Rect, screen: &Screen<'_>, title: &str, value: &str, hint: &str) {
    let popup = centered(area, 76, 7);
    let block = Block::default()
        .title(format!(" {} ", crate::cli::visible(title)))
        .title_bottom(" Enter: save  Esc: cancel ")
        .borders(Borders::ALL)
        .border_style(accent(screen.color));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    let fields = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(crate::cli::visible(hint)).wrap(Wrap { trim: true }),
        fields[0],
    );
    let text = crate::cli::visible(value);
    let available = usize::from(fields[1].width.saturating_sub(1));
    let text = input_tail(&text, available);
    let width = UnicodeWidthStr::width(text.as_str());
    frame.render_widget(Paragraph::new(text), fields[1]);
    if fields[1].width > 0 && fields[1].height > 0 {
        frame.set_cursor_position((fields[1].x + width.min(available) as u16, fields[1].y));
    }
}

fn input_tail(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    let mut used = 0;
    let mut tail = Vec::new();
    for grapheme in text.graphemes(true).rev() {
        let size = UnicodeWidthStr::width(grapheme);
        if used + size > width {
            break;
        }
        tail.push(grapheme);
        used += size;
    }
    tail.into_iter().rev().collect()
}

fn details(task: &Task, snapshot: &Tasks) -> Text<'static> {
    let mut text = vec![
        Line::styled(
            crate::cli::visible(&task.title),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(format!(
            "Priority: {}",
            task.quadrant.map_or("Inbox", |quadrant| quadrant.label())
        )),
        Line::raw(format!("State: {}", task.status.key())),
        Line::raw(format!(
            "Due: {}",
            task.due_date
                .map_or_else(|| "(none)".into(), |due| due.to_string())
        )),
        Line::raw(format!(
            "Recorded: {}",
            crate::cli::duration_seconds(
                snapshot
                    .recorded_seconds
                    .get(&task.id)
                    .copied()
                    .unwrap_or(0)
            )
        )),
    ];
    if task.activity != task.title {
        text.push(Line::raw(""));
        text.push(Line::raw(format!(
            "Time label (for reports): {}",
            crate::cli::visible(&task.activity)
        )));
        text.push(Line::raw("New work is recorded under this name."));
    }
    if let Some(category) = &task.category {
        text.push(Line::raw(format!(
            "Category: {}",
            crate::cli::visible(category)
        )));
    }
    if !task.tags.is_empty() {
        text.push(Line::raw(format!(
            "Tags: {}",
            task.tags
                .iter()
                .map(|tag| crate::cli::visible(tag))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if !task.description.is_empty() {
        text.push(Line::raw(""));
        text.push(Line::raw("Notes:"));
        // Notes may legitimately contain line breaks; keep those as presentation
        // structure while escaping every other control character in each line.
        text.extend(
            task.description
                .lines()
                .map(|line| Line::raw(crate::cli::visible(line))),
        );
    }
    Text::from(text)
}

fn help() -> Text<'static> {
    Text::from(vec![
        Line::raw("Tasks are outcomes to finish; the matrix helps prioritize them."),
        Line::raw("Select a task, then Space for focus or s for ordinary tracking."),
        Line::raw("For unplanned work without a task, use the CLI:"),
        Line::raw("  hs start 'Writing@Work'"),
        Line::raw(""),
        Line::raw("Tab / Shift-Tab   Change quadrant or inbox"),
        Line::raw("Arrows            Move between panes in the matrix layout"),
        Line::raw("Up from inbox     Return through the last visited column"),
        Line::raw("h/l               Previous / next pane"),
        Line::raw("1 Do  2 Plan  3 Drop  4 Delegate  5 Inbox"),
        Line::raw("j/k               Select next / previous task"),
        Line::raw("PgUp / PgDn       Move ten tasks"),
        Line::raw("Home / End        First / last task"),
        Line::raw("Enter             Task details (PgUp/PgDn to scroll)"),
        Line::raw("/                 Search tasks; Esc clears the filter"),
        Line::raw("n                 Add a task in the selected pane"),
        Line::raw("e                 Rename selected task"),
        Line::raw("u                 Set or clear its due date"),
        Line::raw("m                 Move it to another quadrant or inbox"),
        Line::raw("d / a             Complete / archive selected task"),
        Line::raw("s                 Start ordinary tracking"),
        Line::raw("p                 Start or continue its Pomodoro"),
        Line::raw("Space             Start / pause / resume / next phase"),
        Line::raw("N                 Start the offered next phase"),
        Line::raw("x                 Stop the current timer"),
        Line::raw("b                 Toggle completion sounds"),
        Line::raw("? / F1            This help"),
        Line::raw("q / Ctrl-C        Quit; keep any running timer saved"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use hs::{
        application::queries::Status,
        domain::{
            tasks::{Quadrant, TaskStatus},
            time::Moment,
        },
    };
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
    use std::collections::BTreeMap;

    fn fixture() -> Tasks {
        let now = Moment::at("2026-11-23T12:00:00Z".parse().unwrap(), chrono_tz::UTC);
        let tasks = [
            ("Do task", Some(Quadrant::Do)),
            ("Plan task", Some(Quadrant::Plan)),
            ("Drop task", Some(Quadrant::Drop)),
            ("Delegate task", Some(Quadrant::Delegate)),
            ("Inbox task", None),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (title, quadrant))| Task {
            id: index as i64 + 1,
            title: title.into(),
            activity: "Writing".into(),
            category: None,
            description: "Notes with \u{1b}[31m control text\nAnother line".into(),
            tags: vec![],
            quadrant,
            due_date: None,
            status: TaskStatus::Open,
            created_utc: now.utc,
            completed_utc: None,
        })
        .collect();
        Tasks {
            tasks,
            recorded_seconds: BTreeMap::new(),
            status: Status {
                current: None,
                session: None,
                as_of: now,
            },
        }
    }

    fn screen(snapshot: &Tasks) -> Screen<'_> {
        let priorities = [
            Some(Quadrant::Do),
            Some(Quadrant::Plan),
            Some(Quadrant::Drop),
            Some(Quadrant::Delegate),
            None,
        ];
        Screen {
            snapshot,
            panes: std::array::from_fn(|index| {
                snapshot
                    .tasks
                    .iter()
                    .filter(|task| task.quadrant == priorities[index])
                    .collect()
            }),
            active: 1,
            selected: Some(2),
            filter: "",
            message: "Ready",
            modal: Modal::None,
            modal_scroll: 0,
            today: "2026-11-23".parse().unwrap(),
            sound: false,
            color: false,
        }
    }

    fn draw(screen: &Screen<'_>, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, screen);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buffer: &Buffer) -> String {
        buffer
            .content
            .chunks(usize::from(buffer.area.width.max(1)))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn position(buffer: &Buffer, needle: &str) -> (usize, usize) {
        text(buffer)
            .lines()
            .enumerate()
            .find_map(|(row, line)| line.find(needle).map(|column| (row, column)))
            .unwrap_or_else(|| panic!("missing {needle}: {}", text(buffer)))
    }

    #[test]
    fn screen_places_tasks_in_cartesian_quadrants_and_inbox_below() {
        let snapshot = fixture();
        let screen = screen(&snapshot);
        let buffer = draw(&screen, 120, 36);
        let plan = position(&buffer, "Plan task");
        let urgent = position(&buffer, "Do task");
        let discard = position(&buffer, "Drop task");
        let delegate = position(&buffer, "Delegate task");
        let inbox = position(&buffer, "Inbox task");
        assert!(plan.1 < urgent.1 && discard.1 < delegate.1);
        assert!(plan.0 < discard.0 && urgent.0 < delegate.0);
        assert!(inbox.0 > discard.0 && inbox.0 > delegate.0);
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
        );
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.modifier.contains(Modifier::REVERSED))
        );
        // The enlarged countdown must not replace the requested matrix/inbox
        // default view at the common 80-column, 24-row terminal size.
        let compact = draw(&screen, 80, 24);
        for title in [
            "POMODORO",
            "25:00",
            "[Space / p] Start focus",
            "Do task",
            "Plan task",
            "Drop task",
            "Delegate task",
            "Inbox task",
        ] {
            assert!(
                text(&compact).contains(title),
                "missing {title}: {}",
                text(&compact)
            );
        }
        assert!(position(&compact, "POMODORO").0 < position(&compact, "II PLAN").0);
    }

    #[test]
    fn narrow_and_tiny_windows_remain_safe_and_selection_is_scrolled_into_view() {
        let mut snapshot = fixture();
        for id in 6..=100 {
            let mut task = snapshot.tasks[1].clone();
            task.id = id;
            task.title = format!("Plan item {id}");
            snapshot.tasks.push(task);
        }
        let mut screen = screen(&snapshot);
        screen.selected = Some(100);
        for (width, height) in [(1, 1), (19, 7), (40, 12), (80, 24), (120, 36)] {
            let buffer = draw(&screen, width, height);
            if width == 19 {
                assert!(text(&buffer).contains("Ctrl-C: quit"));
            }
            if width >= 20 && height >= 8 {
                assert!(
                    text(&buffer).contains("#100 Plan item 100"),
                    "{}",
                    text(&buffer)
                );
            }
        }
    }

    #[test]
    fn compact_inbox_keeps_selected_title_when_tracking_due_and_recorded_metadata_exist() {
        let mut snapshot = fixture();
        snapshot.tasks[4].due_date = Some("2026-11-22".parse().unwrap());
        snapshot.recorded_seconds.insert(5, 60);
        snapshot.status.current = Some(hs::domain::model::Entry {
            id: 1,
            activity: "Inbox work".into(),
            category: None,
            description: String::new(),
            tags: vec![],
            start: snapshot.status.as_of.local().unwrap(),
            end: None,
            clock: None,
            task: Some(hs::domain::model::EntryTask {
                id: 5,
                quadrant: None,
            }),
            running: true,
        });
        let mut ui = screen(&snapshot);
        ui.active = 4;
        ui.selected = Some(5);
        for (width, height) in [(80, 24), (70, 24), (20, 8)] {
            let rendered = text(&draw(&ui, width, height));
            assert!(rendered.contains("> #5 Inbox task"), "{rendered}");
        }
    }

    #[test]
    fn detail_input_and_completion_banner_escape_controls_without_stealing_focus() {
        let snapshot = fixture();
        let mut screen = screen(&snapshot);
        screen.modal = Modal::Details(&snapshot.tasks[1]);
        let details = text(&draw(&screen, 100, 30));
        assert!(details.contains("Notes with \\u{1b}[31m control text"));
        assert!(!details.contains('\u{1b}'));
        let value = "résumé 👩‍💻".repeat(100);
        screen.modal = Modal::Input {
            title: "Rename task",
            value: &value,
            hint: "Full task title",
        };
        screen.message = "[FOCUS COMPLETE] Your work period is finished.";
        let buffer = draw(&screen, 100, 30);
        assert!(text(&buffer).contains("Rename task"));
        let row = position(&buffer, "[FOCUS COMPLETE]").0;
        assert!(
            buffer.content[row * 100..(row + 1) * 100]
                .iter()
                .any(|cell| cell.modifier.contains(Modifier::REVERSED))
        );
    }

    #[test]
    fn modal_scrolling_stops_at_last_page_and_narrow_dialogs_keep_actions_visible() {
        let mut snapshot = fixture();
        snapshot.tasks[1].description = format!(
            "{}LAST NOTE",
            "A long note with several words.\n".repeat(90)
        );
        let mut screen = screen(&snapshot);
        screen.modal = Modal::Details(&snapshot.tasks[1]);
        screen.modal_scroll = u16::MAX;
        assert!(text(&draw(&screen, 40, 16)).contains("LAST NOTE"));
        screen.modal = Modal::Help;
        assert!(text(&draw(&screen, 40, 16)).contains("saved"));
        let message = format!(
            "{} Any timer belonging to this task will stop.",
            "Task title ".repeat(90)
        );
        screen.modal = Modal::Confirm {
            title: "Complete task",
            message: &message,
        };
        let confirmation = text(&draw(&screen, 40, 16));
        assert!(confirmation.contains("stop."), "{confirmation}");
        screen.modal_scroll = 0;
        screen.modal = Modal::Move { title: &message };
        assert!(text(&draw(&screen, 24, 24)).contains("5 INBOX"));
        screen.modal = Modal::None;
        assert!(text(&draw(&screen, 24, 16)).contains("q quit"));
        let value = format!("{}e\u{301}👩‍💻", "x".repeat(70000));
        let tail = input_tail(&value, 10);
        assert!(tail.ends_with("e\u{301}👩‍💻"));
        assert_eq!(UnicodeWidthStr::width(tail.as_str()), 10);
    }

    #[test]
    fn compact_terminal_keeps_title_due_date_timer_and_confirmation_actions_readable() {
        let (_dir, mut store) = timer_store(hs::domain::pomodoro::Config {
            focus_seconds: 10,
            ..Default::default()
        });
        let mut snapshot = timer_snapshot(&mut store, 3);
        let task = snapshot.tasks.iter_mut().find(|task| task.id == 2).unwrap();
        task.title = format!("Quarterly {}", "計画".repeat(40));
        task.due_date = Some("2026-11-22".parse().unwrap());
        let mut ui = screen(&snapshot);
        let confirmation = format!(
            "Complete #2: {}?\nThis will mark the task done.",
            snapshot
                .tasks
                .iter()
                .find(|task| task.id == 2)
                .unwrap()
                .title,
        );
        for (width, height) in [(48, 16), (60, 18), (80, 20)] {
            ui.modal = Modal::None;
            ui.modal_scroll = 0;
            let rendered = text(&draw(&ui, width, height));
            let task_row = rendered
                .lines()
                .find(|row| row.contains("> #2 Quarterly"))
                .unwrap_or_else(|| panic!("selected row missing at {width}x{height}: {rendered}"));
            assert!(task_row.contains("..."), "{task_row}");
            assert!(rendered.contains("due 2026-11-22 !"), "{rendered}");
            assert!(rendered.contains("00:07"), "{rendered}");

            ui.modal = Modal::Confirm {
                title: "Complete task",
                message: &confirmation,
            };
            ui.modal_scroll = u16::MAX;
            let rendered = text(&draw(&ui, width, height));
            assert!(
                rendered.contains("This will mark the task done."),
                "{rendered}"
            );
            assert!(
                rendered.contains("n/Esc") && rendered.contains("PgUp/PgDn"),
                "{rendered}"
            );
            assert!(rendered.contains("y:"), "{rendered}");
        }
    }

    fn moment(seconds: i64) -> Moment {
        Moment::at(
            "2026-11-23T12:00:00Z"
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap()
                + chrono::Duration::seconds(seconds),
            chrono_tz::UTC,
        )
    }

    fn timer_store(config: hs::domain::pomodoro::Config) -> (tempfile::TempDir, hs::Store) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = hs::Store::open(&dir.path().join("timer.db")).unwrap();
        for task in fixture().tasks {
            hs::application::tasks::add(
                &mut store,
                hs::domain::tasks::TaskInput {
                    title: task.title,
                    activity: hs::domain::model::ActivityTemplate {
                        activity: task.activity,
                        category: task.category,
                        description: task.description,
                        tags: task.tags,
                    },
                    quadrant: task.quadrant,
                    due_date: task.due_date,
                },
                &moment(0),
            )
            .unwrap();
        }
        hs::application::pomodoro::start_task(&mut store, 1, config, &moment(0)).unwrap();
        (dir, store)
    }

    fn timer_snapshot(store: &mut hs::Store, seconds: i64) -> Tasks {
        hs::application::queries::task_list(store, Some(TaskStatus::Open), &moment(seconds))
            .unwrap()
    }

    #[test]
    fn timer_card_distinguishes_active_task_and_contextual_phase_controls() {
        use hs::application::pomodoro as workflow;
        let (_dir, mut store) = timer_store(workflow::Config {
            focus_seconds: 10,
            break_seconds: 5,
            long_break_seconds: 8,
            rounds: 2,
        });
        let snapshot = timer_snapshot(&mut store, 3);
        let running = draw(&screen(&snapshot), 80, 24);
        let running_text = text(&running);
        for label in [
            "FOCUS RUNNING",
            "00:07",
            "Current #1: Do task",
            "[Space] Pause",
            "[p] New focus / switch to #2: Plan task",
            "30% elapsed",
        ] {
            assert!(
                running_text.contains(label),
                "missing {label}: {running_text}"
            );
        }
        assert!(
            running
                .content
                .iter()
                .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
        );
        workflow::pause(&mut store, &moment(3)).unwrap();
        let snapshot = timer_snapshot(&mut store, 100);
        let paused = text(&draw(&screen(&snapshot), 80, 24));
        assert!(paused.contains("FOCUS PAUSED") && paused.contains("00:07"));
        assert!(paused.contains("[Space] Resume"));
        workflow::resume(&mut store, &moment(100)).unwrap();
        let snapshot = timer_snapshot(&mut store, 107);
        let ready = text(&draw(&screen(&snapshot), 80, 24));
        for label in [
            "FOCUS COMPLETE",
            "00:00",
            "Next: short break (00:05)",
            "[Space] Start short break",
            "[N] same action",
            "Current #1: Do task",
        ] {
            assert!(ready.contains(label), "missing {label}: {ready}");
        }
        assert!(!ready.contains("% elapsed"));
        workflow::next(&mut store, &moment(108)).unwrap();
        let snapshot = timer_snapshot(&mut store, 110);
        let rest = text(&draw(&screen(&snapshot), 80, 24));
        assert!(rest.contains("SHORT BREAK RUNNING") && rest.contains("00:03"));
        let snapshot = timer_snapshot(&mut store, 113);
        let rest_ready = text(&draw(&screen(&snapshot), 80, 24));
        assert!(
            rest_ready.contains("BREAK COMPLETE") && rest_ready.contains("[Space] Start focus")
        );
        workflow::next(&mut store, &moment(114)).unwrap();
        let snapshot = timer_snapshot(&mut store, 124);
        let long_ready = text(&draw(&screen(&snapshot), 80, 24));
        assert!(
            long_ready.contains("Next: long break (00:08)")
                && long_ready.contains("[Space] Start long break")
        );
        workflow::stop(&mut store, &moment(125)).unwrap();
        workflow::start(
            &mut store,
            hs::domain::model::ActivityTemplate {
                activity: "work".into(),
                category: None,
                description: String::new(),
                tags: vec![],
            }
            .starting_at(moment(126).local().unwrap()),
            workflow::Config {
                focus_seconds: 10,
                ..Default::default()
            },
            &moment(126),
        )
        .unwrap();
        let snapshot = timer_snapshot(&mut store, 136);
        let free_text = text(&draw(&screen(&snapshot), 80, 24));
        assert!(free_text.contains("Current: work"));
        assert!(free_text.contains("[Space] Start short break"));
        assert!(free_text.contains("[p] New focus / switch to #2: Plan task"));
    }

    #[test]
    fn timer_card_keeps_time_and_primary_action_in_small_windows_and_long_durations() {
        use hs::application::pomodoro as workflow;
        let (_dir, mut store) = timer_store(workflow::Config {
            focus_seconds: 10,
            break_seconds: 5,
            long_break_seconds: 8,
            rounds: 2,
        });
        let snapshot = timer_snapshot(&mut store, 10);
        let mut ui = screen(&snapshot);
        ui.filter = "urgent";
        for (width, height) in [(20, 8), (40, 12), (65, 18)] {
            let rendered = text(&draw(&ui, width, height));
            assert!(
                rendered.contains("00:00") && rendered.contains("[Space]"),
                "{rendered}"
            );
            assert!(rendered.contains("Filter: urgent"), "{rendered}");
        }
        let (_dir, mut store) = timer_store(workflow::Config {
            focus_seconds: 36_000_000_000,
            ..Default::default()
        });
        let snapshot = timer_snapshot(&mut store, 0);
        let long = text(&draw(&screen(&snapshot), 80, 24));
        assert!(
            long.contains("10000000:00:00") && long.contains("[Space] Pause"),
            "{long}"
        );
        let mut manual = snapshot;
        manual.status.session = None;
        let mut ui = screen(&manual);
        ui.selected = None;
        assert!(text(&draw(&ui, 80, 24)).contains("[x] Stop tracking"));
        assert!(text(&draw(&ui, 20, 8)).contains("[x] Stop tracking"));
        let snapshot = fixture();
        let mut ui = screen(&snapshot);
        ui.selected = None;
        let idle = text(&draw(&ui, 80, 24));
        assert!(idle.contains("No task selected") && idle.contains("[n] Add a task"));
        assert!(!idle.contains("[Space / p] Start focus"));
    }

    #[test]
    fn multiline_switch_confirmation_preserves_lines_and_escapes_terminal_controls() {
        let snapshot = fixture();
        let mut ui = screen(&snapshot);
        ui.modal = Modal::Confirm {
            title: "Switch timer",
            message: "Current: work\nStart new focus: task #2\n\nEsc then Space: start short break\nUnsafe \u{1b}[31m",
        };
        let rendered = text(&draw(&ui, 100, 30));
        assert!(rendered.contains("Current: work"));
        assert!(rendered.contains("Esc then Space: start short break"));
        assert!(rendered.contains("Unsafe \\u{1b}[31m"));
        assert!(!rendered.contains("\\n") && !rendered.contains('\u{1b}'));
        assert!(
            position(&draw(&ui, 100, 30), "Current: work").0
                < position(&draw(&ui, 100, 30), "Start new focus").0
        );
    }
}
