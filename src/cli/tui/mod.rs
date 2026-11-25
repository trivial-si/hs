//! Event loop and interaction state. Rendering and terminal ownership are separate;
//! every persisted change goes through the shared application workflows.
mod terminal;
mod view;

use std::{
    env,
    io::{self, Write},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use chrono::NaiveDate;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use hs::{
    Error, Result, Store,
    application::{
        clock,
        interactive::{self, TaskStatusExpectation, TimerAction, TimerExpectation},
        pomodoro, queries, tasks,
    },
    domain::{
        ids::TaskId,
        model::ActivityTemplate,
        pomodoro::{Phase, PhaseState, State},
        tasks::{Quadrant, Task, TaskInput, TaskStatus},
        time::Moment,
    },
};
use unicode_segmentation::UnicodeSegmentation;

pub(super) fn validate() -> Result<()> {
    terminal::validate()
}

const PANES: [Option<Quadrant>; 5] = [
    Some(Quadrant::Do),
    Some(Quadrant::Plan),
    Some(Quadrant::Drop),
    Some(Quadrant::Delegate),
    None,
];
const VISUAL_ORDER: [usize; 5] = [1, 0, 2, 3, 4];
const REFRESH: Duration = Duration::from_secs(1);
const INPUT_LIMIT: usize = 4096;

#[derive(Clone)]
enum InputKind {
    Add(Option<Quadrant>),
    Rename {
        id: i64,
        original: String,
    },
    Due {
        id: i64,
        original: Option<NaiveDate>,
    },
    Search,
}
impl InputKind {
    fn title(&self) -> &'static str {
        match self {
            Self::Add(_) => "Add task",
            Self::Rename { .. } => "Rename task",
            Self::Due { .. } => "Due date",
            Self::Search => "Search tasks",
        }
    }
}

enum Pending {
    Status {
        expected: Box<TaskStatusExpectation>,
        state: TaskStatus,
    },
    Timer {
        action: TimerAction,
        expected: TimerExpectation,
    },
}

enum Mode {
    Browse,
    Help,
    Details(i64),
    Input {
        kind: InputKind,
        text: String,
    },
    Move {
        id: i64,
        original: Option<Quadrant>,
        title: String,
    },
    Confirm {
        action: Pending,
        title: &'static str,
        message: String,
    },
}

struct App {
    snapshot: queries::Tasks,
    active: usize,
    /// Lower quadrant in the most recently visited column, for Up from the inbox.
    inbox_return: usize,
    selections: [Option<i64>; 5],
    filter: String,
    mode: Mode,
    modal_scroll: u16,
    message: String,
    sound: bool,
    color: bool,
    today: NaiveDate,
    watched: Option<(i64, i64)>,
    cues: usize,
    dirty: bool,
}

impl App {
    fn new(snapshot: queries::Tasks, now: &Moment, no_sound: bool) -> Result<Self> {
        let mut app = Self {
            snapshot,
            active: 4,
            inbox_return: 2,
            selections: [None; 5],
            filter: String::new(),
            mode: Mode::Browse,
            modal_scroll: 0,
            message: "Ready. Press ? for keys; q closes the interface and leaves timers saved."
                .into(),
            sound: !no_sound,
            color: env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
            today: now.local()?.date(),
            watched: None,
            cues: 0,
            dirty: false,
        };
        app.reselect();
        let active = VISUAL_ORDER
            .into_iter()
            .find(|i| app.selections[*i].is_some())
            .unwrap_or(4);
        app.focus_pane(active);
        app.follow_running();
        Ok(app)
    }

    fn panes(&self) -> [Vec<&Task>; 5] {
        let needle = self.filter.to_lowercase();
        std::array::from_fn(|index| {
            self.snapshot
                .tasks
                .iter()
                .filter(|task| {
                    task.quadrant == PANES[index]
                        && (needle.is_empty()
                            || task.title.to_lowercase().contains(&needle)
                            || task.activity.to_lowercase().contains(&needle)
                            || task
                                .category
                                .as_deref()
                                .unwrap_or("")
                                .to_lowercase()
                                .contains(&needle)
                            || task
                                .tags
                                .iter()
                                .any(|tag| tag.to_lowercase().contains(&needle)))
                })
                .collect()
        })
    }

    fn selected(&self) -> Option<&Task> {
        self.selections[self.active].and_then(|id| self.snapshot.tasks.iter().find(|t| t.id == id))
    }

    fn reselect(&mut self) {
        let panes = self.panes();
        let selections = std::array::from_fn(|i| {
            self.selections[i]
                .filter(|id| panes[i].iter().any(|t| t.id == *id))
                .or_else(|| panes[i].first().map(|t| t.id))
        });
        self.selections = selections;
    }

    fn select_task(&mut self, id: i64) {
        if let Some(task) = self.snapshot.tasks.iter().find(|t| t.id == id) {
            self.focus_pane(PANES.iter().position(|q| *q == task.quadrant).unwrap_or(4));
            self.selections[self.active] = Some(id);
            self.reselect();
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let panes = self.panes();
        let tasks = &panes[self.active];
        if tasks.is_empty() {
            return;
        }
        let index = tasks
            .iter()
            .position(|t| Some(t.id) == self.selections[self.active])
            .unwrap_or(0);
        let index = index.saturating_add_signed(delta).min(tasks.len() - 1);
        self.selections[self.active] = Some(tasks[index].id);
    }

    fn cycle_pane(&mut self, delta: isize) {
        let index = VISUAL_ORDER
            .iter()
            .position(|i| *i == self.active)
            .unwrap_or(0);
        self.focus_pane(VISUAL_ORDER[(index as isize + delta).rem_euclid(5) as usize]);
    }

    fn focus_pane(&mut self, index: usize) {
        self.active = index;
        match index {
            1 | 2 => self.inbox_return = 2,
            0 | 3 => self.inbox_return = 3,
            _ => {}
        }
    }

    fn navigate_pane(&mut self, direction: KeyCode) {
        // Match the matrix's physical layout; arrows stop at its outer edges.
        // The inbox spans both columns, so Up returns through the last column.
        let next = match (self.active, direction) {
            (0, KeyCode::Left) => 1,
            (1, KeyCode::Right) => 0,
            (3, KeyCode::Left) => 2,
            (2, KeyCode::Right) => 3,
            (0, KeyCode::Down) => 3,
            (1, KeyCode::Down) => 2,
            (2 | 3, KeyCode::Down) => 4,
            (2, KeyCode::Up) => 1,
            (3, KeyCode::Up) => 0,
            (4, KeyCode::Up) => self.inbox_return,
            _ => self.active,
        };
        self.focus_pane(next);
    }

    fn follow_running(&mut self) {
        self.watched = self
            .snapshot
            .status
            .session
            .as_ref()
            .filter(|s| s.state() == State::Running)
            .and_then(|s| s.phase_id().map(|phase| (s.id, phase)));
    }

    /// Return an actual durable phase completion once, even if another client
    /// already advanced it. Never infer completion from a changed phase ID.
    fn refresh(&mut self, store: &mut Store, now: &Moment) -> Result<()> {
        let snapshot = queries::task_list(store, Some(TaskStatus::Open), now)?;
        let mut completed = None;
        if let Some((session_id, phase_id)) = self.watched {
            let still_running = snapshot.status.session.as_ref().is_some_and(|s| {
                s.id == session_id && s.phase_id() == Some(phase_id) && s.state() == State::Running
            });
            if !still_running {
                let status = pomodoro::watch_status(store, Some(phase_id), now)?;
                if status.session.as_ref().is_some_and(|s| s.id == session_id) {
                    completed = status
                        .watched_phase
                        .filter(|p| p.state == PhaseState::Completed)
                        .map(|p| p.phase);
                }
            }
        }
        self.snapshot = snapshot;
        self.today = now.local()?.date();
        self.reselect();
        self.follow_running();
        self.dirty = false;
        if let Mode::Details(id) = self.mode
            && !self.snapshot.tasks.iter().any(|task| task.id == id)
        {
            self.mode = Mode::Browse;
            self.modal_scroll = 0;
            self.message = "That task is no longer open; the matrix has refreshed.".into();
        }
        if let Some(phase) = completed {
            self.message = match phase {
                Phase::Focus => {
                    "[FOCUS COMPLETE] Work finished. Space or N starts the offered break."
                }
                Phase::ShortBreak => {
                    "[BREAK COMPLETE] Short rest finished. Space or N starts the offered focus."
                }
                Phase::LongBreak => {
                    "[BREAK COMPLETE] Long rest finished. Space or N starts the offered focus."
                }
            }
            .into();
            // Do not advise starting another phase when another client already did.
            if self
                .snapshot
                .status
                .session
                .as_ref()
                .is_some_and(|s| s.state() != State::Ready)
            {
                self.message = format!(
                    "{} The session has since changed; see the timer header.",
                    self.message.split('.').next().unwrap_or("Phase complete")
                );
            }
            if self.sound {
                self.cues += 1;
            }
        }
        Ok(())
    }

    fn screen(&self) -> view::Screen<'_> {
        let modal = match &self.mode {
            Mode::Browse => view::Modal::None,
            Mode::Help => view::Modal::Help,
            Mode::Details(id) => self
                .snapshot
                .tasks
                .iter()
                .find(|t| t.id == *id)
                .map_or(view::Modal::None, view::Modal::Details),
            Mode::Input { kind, text } => view::Modal::Input {
                title: kind.title(),
                value: text,
                hint: match kind {
                    InputKind::Due { .. } => {
                        "YYYY-MM-DD or none; Enter saves, Esc cancels, Ctrl-U clears"
                    }
                    _ => "Enter saves; Esc cancels; Backspace edits; Ctrl-U clears",
                },
            },
            Mode::Move { title, .. } => view::Modal::Move { title },
            Mode::Confirm { title, message, .. } => view::Modal::Confirm { title, message },
        };
        view::Screen {
            snapshot: &self.snapshot,
            panes: self.panes(),
            active: self.active,
            selected: self.selections[self.active],
            filter: &self.filter,
            message: &self.message,
            modal,
            modal_scroll: self.modal_scroll,
            sound: self.sound,
            color: self.color,
            today: self.today,
        }
    }

    fn draw(&mut self, frame: &mut ratatui::Frame<'_>) {
        self.modal_scroll = view::render(frame, &self.screen());
    }

    fn input(&mut self, kind: InputKind, text: String) {
        self.mode = Mode::Input { kind, text };
        self.modal_scroll = 0;
    }

    fn submit(
        &mut self,
        store: &mut Store,
        kind: InputKind,
        text: &str,
        now: &Moment,
    ) -> Result<Option<i64>> {
        match kind {
            InputKind::Search => {
                self.filter = text.trim().into();
                self.reselect();
                self.message = if self.filter.is_empty() {
                    "Search cleared.".into()
                } else {
                    format!(
                        "Search: {}. Esc clears it.",
                        crate::cli::visible(&self.filter)
                    )
                };
                Ok(None)
            }
            InputKind::Add(quadrant) => {
                let title = text.trim().to_owned();
                self.dirty = true;
                let task = tasks::add(
                    store,
                    TaskInput {
                        title: title.clone(),
                        activity: ActivityTemplate {
                            activity: title,
                            category: None,
                            description: String::new(),
                            tags: vec![],
                        },
                        quadrant,
                        due_date: None,
                    },
                    now,
                )?;
                self.filter.clear();
                self.message = format!("Created task #{}.", task.id);
                Ok(Some(task.id))
            }
            InputKind::Rename { id, original } => {
                self.dirty = true;
                let task = tasks::edit(store, id, |task| {
                    if task.status != TaskStatus::Open || task.title != original {
                        return Err(Error::Conflict(
                            "This task changed in another terminal. Cancel and reopen the editor."
                                .into(),
                        ));
                    }
                    task.title = text.trim().into();
                    Ok(())
                })?;
                self.message = format!("Renamed task #{}.", task.id);
                Ok(Some(id))
            }
            InputKind::Due { id, original } => {
                let value = text.trim();
                let due = if value.is_empty() || value.eq_ignore_ascii_case("none") {
                    None
                } else {
                    Some(
                        value
                            .parse::<NaiveDate>()
                            .map_err(|_| Error::Input("Use YYYY-MM-DD or none.".into()))?,
                    )
                };
                self.dirty = true;
                tasks::edit(store, id, |task| {
                    if task.status != TaskStatus::Open || task.due_date != original {
                        return Err(Error::Conflict(
                            "This task changed in another terminal. Cancel and reopen the editor."
                                .into(),
                        ));
                    }
                    task.due_date = due;
                    Ok(())
                })?;
                self.message = format!("Updated due date for task #{id}.");
                Ok(Some(id))
            }
        }
    }

    fn has_timer(&self) -> bool {
        self.snapshot.status.current.is_some()
            || self
                .snapshot
                .status
                .session
                .as_ref()
                .is_some_and(|s| s.state() != State::Stopped)
    }

    fn task_label(&self, id: TaskId) -> String {
        self.snapshot
            .tasks
            .iter()
            .find(|task| task.id == id.get())
            .map_or_else(
                || format!("task #{id}"),
                |task| format!("#{} {}", task.id, task.title),
            )
    }

    fn timer_description(&self) -> String {
        if let Some(session) = self
            .snapshot
            .status
            .session
            .as_ref()
            .filter(|session| session.state() != State::Stopped)
        {
            let state = if session.state() == State::Ready {
                format!("ready for {}", session.phase().label())
            } else {
                format!("{} {}", session.phase().label(), session.state().label())
            };
            let owner = session
                .task_id
                .and_then(|id| self.snapshot.tasks.iter().find(|task| task.id == id))
                .map_or_else(
                    || format!("\"{}\"", session.activity),
                    |task| format!("#{} {}", task.id, task.title),
                );
            return format!("Pomodoro for {owner} ({state})");
        }
        self.snapshot.status.current.as_ref().map_or_else(
            || "the displayed timer".into(),
            |entry| format!("tracking \"{}\"", entry.activity),
        )
    }

    /// The selected-task shortcut is separate from the current session's primary
    /// control: browsing another task must not redirect pause/resume/next.
    fn selected_pomodoro(&mut self, store: &mut Store, now: &Moment) -> Result<()> {
        let Some(task) = self.selected() else {
            self.message = "Select a task with Tab / 1-5 and j/k, or press n to add one.".into();
            return Ok(());
        };
        let id = TaskId::new(task.id)?;
        let own = self.snapshot.status.session.as_ref().filter(|session| {
            session.task_id == Some(id.get()) && session.state() != State::Stopped
        });
        match own.map(|session| session.state()) {
            Some(State::Running) => {
                self.message = "This task's Pomodoro is running. Space pauses it.".into();
                Ok(())
            }
            Some(State::Paused) => self.request_timer(store, TimerAction::Resume, now, false),
            Some(State::Ready) => self.request_timer(store, TimerAction::Next, now, false),
            _ => self.request_timer(store, TimerAction::Pomodoro(id), now, self.has_timer()),
        }
    }

    fn request_timer(
        &mut self,
        store: &mut Store,
        action: TimerAction,
        now: &Moment,
        confirm: bool,
    ) -> Result<()> {
        let expected = TimerExpectation::from_status(&self.snapshot.status);
        if confirm {
            let (title, message) = match action {
                TimerAction::Stop => (
                    "Stop timer",
                    format!("Stop {}? Recorded work is kept.", self.timer_description()),
                ),
                TimerAction::Track(id) => (
                    "Switch timer",
                    format!(
                        "End {} and track {}? Recorded work is kept.",
                        self.timer_description(),
                        self.task_label(id)
                    ),
                ),
                TimerAction::Pomodoro(id) => {
                    let mut message = format!(
                        "End {}?\nStart a new {}-minute focus for {}.\nRecorded work is kept.",
                        self.timer_description(),
                        pomodoro::Config::default().focus_seconds / 60,
                        self.task_label(id)
                    );
                    if let Some(session) = self.snapshot.status.session.as_ref()
                        && session.state() == State::Ready
                    {
                        message.push_str(&format!(
                            "\nTo start the offered {} instead: Esc, then Space.",
                            session.phase().label()
                        ));
                    }
                    ("Switch timer", message)
                }
                _ => unreachable!("only starts/stops require a confirmation"),
            };
            self.modal_scroll = 0;
            self.mode = Mode::Confirm {
                action: Pending::Timer { action, expected },
                title,
                message,
            };
        } else {
            self.dirty = true;
            interactive::timer(store, &expected, action, now)?;
            self.message = "Timer updated.".into();
        }
        Ok(())
    }

    fn execute_pending(&mut self, store: &mut Store, action: Pending, now: &Moment) -> Result<()> {
        self.dirty = true;
        match action {
            Pending::Status { expected, state } => {
                let task = interactive::task_status(store, &expected, state, now)?;
                self.message = format!("Task #{} is {}.", task.id, state.key());
            }
            Pending::Timer { action, expected } => {
                interactive::timer(store, &expected, action, now)?;
                self.message = "Timer updated.".into();
            }
        }
        Ok(())
    }

    /// Handle one key. Return true only for leaving the interface.
    fn key(&mut self, store: &mut Store, key: KeyEvent, now: &Moment) -> Result<bool> {
        if key.kind == KeyEventKind::Release {
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(true);
        }
        // Modified letters are different shortcuts. Do not turn e.g. Ctrl-P
        // into starting a timer or Alt-Q into quitting. Shift remains valid for
        // uppercase actions and BackTab, and Ctrl-U belongs to text editing.
        let clear_input = key.modifiers == KeyModifiers::CONTROL
            && key.code == KeyCode::Char('u')
            && matches!(self.mode, Mode::Input { .. });
        if !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() && !clear_input {
            return Ok(false);
        }
        match std::mem::replace(&mut self.mode, Mode::Browse) {
            Mode::Input { kind, mut text } => {
                match key.code {
                    KeyCode::Esc => {
                        self.message = "Cancelled.".into();
                        return Ok(false);
                    }
                    KeyCode::Enter => match self.submit(store, kind.clone(), &text, now) {
                        Ok(id) => {
                            self.refresh(store, now)?;
                            if let Some(id) = id {
                                self.select_task(id);
                            }
                            return Ok(false);
                        }
                        Err(error) => self.message = error.to_string(),
                    },
                    KeyCode::Backspace => {
                        pop_grapheme(&mut text);
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        text.clear()
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                            && text.chars().count() < INPUT_LIMIT
                            && !c.is_control() =>
                    {
                        text.push(c);
                    }
                    _ => {}
                }
                self.mode = Mode::Input { kind, text };
                return Ok(false);
            }
            Mode::Move {
                id,
                original,
                title,
            } => {
                match key.code {
                    KeyCode::Esc => self.message = "Cancelled.".into(),
                    KeyCode::Char(c @ '1'..='5') => {
                        let quadrant = PANES[c as usize - '1' as usize];
                        self.dirty = true;
                        tasks::edit(store, id, |task| {
                            if task.status != TaskStatus::Open || task.quadrant != original {
                                return Err(Error::Conflict(
                                    "This task's priority changed. Reopen the move dialog.".into(),
                                ));
                            }
                            task.quadrant = quadrant;
                            Ok(())
                        })?;
                        self.message = format!("Moved task #{id}.");
                        self.refresh(store, now)?;
                        self.select_task(id);
                    }
                    _ => {
                        self.mode = Mode::Move {
                            id,
                            original,
                            title,
                        }
                    }
                }
                return Ok(false);
            }
            Mode::Confirm {
                action,
                title,
                message,
            } => {
                match key.code {
                    KeyCode::Enter | KeyCode::Char('y' | 'Y') => {
                        self.modal_scroll = 0;
                        self.execute_pending(store, action, now)?
                    }
                    KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                        self.modal_scroll = 0;
                        self.message = "Cancelled.".into();
                    }
                    KeyCode::PageDown | KeyCode::Down => {
                        self.modal_scroll = self.modal_scroll.saturating_add(5);
                        self.mode = Mode::Confirm {
                            action,
                            title,
                            message,
                        };
                    }
                    KeyCode::PageUp | KeyCode::Up => {
                        self.modal_scroll = self.modal_scroll.saturating_sub(5);
                        self.mode = Mode::Confirm {
                            action,
                            title,
                            message,
                        };
                    }
                    _ => {
                        self.mode = Mode::Confirm {
                            action,
                            title,
                            message,
                        }
                    }
                }
                return Ok(false);
            }
            mode @ (Mode::Help | Mode::Details(_)) => {
                match key.code {
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.modal_scroll = 0,
                    KeyCode::PageDown | KeyCode::Down | KeyCode::Char('j') => {
                        self.modal_scroll = self.modal_scroll.saturating_add(5);
                        self.mode = mode;
                    }
                    KeyCode::PageUp | KeyCode::Up | KeyCode::Char('k') => {
                        self.modal_scroll = self.modal_scroll.saturating_sub(5);
                        self.mode = mode;
                    }
                    _ => self.mode = mode,
                }
                return Ok(false);
            }
            Mode::Browse => {}
        }
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.mode = Mode::Help;
                self.modal_scroll = 0;
            }
            KeyCode::Tab | KeyCode::Char('l') => self.cycle_pane(1),
            KeyCode::BackTab | KeyCode::Char('h') => self.cycle_pane(-1),
            direction @ (KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right) => {
                self.navigate_pane(direction)
            }
            KeyCode::Char(c @ '1'..='5') => self.focus_pane(c as usize - '1' as usize),
            KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::Home => self.move_selection(isize::MIN),
            KeyCode::End => self.move_selection(isize::MAX),
            KeyCode::Esc => {
                self.filter.clear();
                self.reselect();
                self.message = "Search cleared.".into();
            }
            KeyCode::Char('/') => self.input(InputKind::Search, self.filter.clone()),
            KeyCode::Char('n') => self.input(InputKind::Add(PANES[self.active]), String::new()),
            KeyCode::Char('b') => {
                self.sound = !self.sound;
                self.message = if self.sound {
                    "Completion sound enabled."
                } else {
                    "Completion sound muted."
                }
                .into();
            }
            KeyCode::Char(' ') => match self.snapshot.status.session.as_ref().map(|s| s.state()) {
                Some(State::Running) => {
                    self.request_timer(store, TimerAction::Pause, now, false)?
                }
                Some(State::Paused) => {
                    self.request_timer(store, TimerAction::Resume, now, false)?
                }
                Some(State::Ready) => self.request_timer(store, TimerAction::Next, now, false)?,
                _ => self.selected_pomodoro(store, now)?,
            },
            KeyCode::Char('p') => self.selected_pomodoro(store, now)?,
            KeyCode::Char('N') => self.request_timer(store, TimerAction::Next, now, false)?,
            KeyCode::Char('x') => {
                if self.has_timer() {
                    self.request_timer(store, TimerAction::Stop, now, true)?;
                } else {
                    self.message = "No active timer.".into();
                }
            }
            key @ (KeyCode::Enter | KeyCode::Char('e' | 'u' | 'm' | 'd' | 'a' | 's')) => {
                let Some(task) = self.selected().cloned() else {
                    self.message =
                        "No task selected. Press n to create one, or change panes with Tab.".into();
                    return Ok(false);
                };
                let id = TaskId::new(task.id)?;
                match key {
                    KeyCode::Enter => {
                        self.mode = Mode::Details(task.id);
                        self.modal_scroll = 0;
                    }
                    KeyCode::Char('e') => self.input(
                        InputKind::Rename {
                            id: task.id,
                            original: task.title.clone(),
                        },
                        task.title,
                    ),
                    KeyCode::Char('u') => self.input(
                        InputKind::Due {
                            id: task.id,
                            original: task.due_date,
                        },
                        task.due_date.map(|d| d.to_string()).unwrap_or_default(),
                    ),
                    KeyCode::Char('m') => {
                        self.mode = Mode::Move {
                            id: task.id,
                            original: task.quadrant,
                            title: format!(
                                "Move task #{}: {}",
                                task.id,
                                crate::cli::visible(&task.title)
                            ),
                        }
                    }
                    KeyCode::Char('d' | 'a') => {
                        let state = if key == KeyCode::Char('d') {
                            TaskStatus::Done
                        } else {
                            TaskStatus::Archived
                        };
                        self.modal_scroll = 0;
                        self.mode = Mode::Confirm {
                            action: Pending::Status {
                                expected: Box::new(TaskStatusExpectation::from_task(
                                    &task,
                                    &self.snapshot.status,
                                )),
                                state,
                            },
                            title: if state == TaskStatus::Done {
                                "Complete task"
                            } else {
                                "Archive task"
                            },
                            message: format!(
                                "Mark #{} {} as {}? Any timer belonging to this task will stop.",
                                task.id,
                                crate::cli::visible(&task.title),
                                state.key()
                            ),
                        };
                    }
                    KeyCode::Char('s') => {
                        self.request_timer(store, TimerAction::Track(id), now, self.has_timer())?
                    }
                    _ => unreachable!(),
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn paste(&mut self, value: &str) {
        if let Mode::Input { text, .. } = &mut self.mode {
            let available = INPUT_LIMIT.saturating_sub(text.chars().count());
            for c in value.chars().take(available) {
                if c == '\n' || c == '\r' || c == '\t' {
                    text.push(' ');
                } else if !c.is_control() {
                    text.push(c);
                }
            }
        }
    }
}

fn pop_grapheme(text: &mut String) {
    if let Some((index, _)) = text.grapheme_indices(true).next_back() {
        text.truncate(index);
    }
}

/// Sound runs outside the rendering thread. The worker returns only a fallback
/// decision; it never writes to the terminal while Ratatui owns its contents.
#[derive(Default)]
struct Sounds {
    workers: Vec<JoinHandle<bool>>,
}
impl Sounds {
    fn play(&mut self) {
        self.workers
            .push(thread::spawn(crate::cli::notification::ring));
    }
    fn poll(&mut self, enabled: bool) -> Result<()> {
        let mut index = 0;
        while index < self.workers.len() {
            if self.workers[index].is_finished() {
                let native = self.workers.swap_remove(index).join().unwrap_or(false);
                if !native && enabled {
                    write!(io::stdout(), "\x07")?;
                    io::stdout().flush()?;
                }
            } else {
                index += 1;
            }
        }
        Ok(())
    }
}
impl Drop for Sounds {
    fn drop(&mut self) {
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

pub(super) fn run(store: &mut Store, no_sound: bool) -> Result<()> {
    store.set_lock_timeout(Duration::from_millis(75))?;
    let now = clock::now()?;
    let initial = queries::task_list(store, Some(TaskStatus::Open), &now)?;
    let mut app = App::new(initial, &now, no_sound)?;
    // Drop terminal guard first on every exit; audio shutdown cannot leave raw
    // mode active while waiting for a bounded native playback subprocess.
    let mut sounds = Sounds::default();
    let mut terminal = terminal::TerminalGuard::enter()?;
    let mut refreshed = Instant::now();
    let mut redraw = true;
    loop {
        if terminal.interrupted() {
            return Ok(());
        }
        if refreshed.elapsed() >= REFRESH {
            let now = clock::now()?;
            if let Err(error) = app.refresh(store, &now) {
                app.message = format!("Refresh failed: {error}. Retrying; q still quits.");
            }
            refreshed = Instant::now();
            redraw = true;
        }
        if redraw {
            terminal.draw(|frame| app.draw(frame))?;
            redraw = false;
        }
        for _ in 0..std::mem::take(&mut app.cues) {
            if app.sound {
                sounds.play();
            }
        }
        sounds.poll(app.sound)?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        redraw = true;
        match event::read()? {
            Event::Key(key) => {
                let now = clock::now()?;
                // Navigation and text editing never wait for a database lock.
                // Mutations carry stable IDs and transaction-checked expectations.
                match app.key(store, key, &now) {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(error) => {
                        app.message = error.to_string();
                        app.dirty = true;
                    }
                }
                if app.dirty {
                    if let Err(error) = app.refresh(store, &now) {
                        app.message = error.to_string();
                    }
                    refreshed = Instant::now();
                }
            }
            Event::Paste(text) => app.paste(&text),
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moment(seconds: i64) -> Moment {
        Moment::at(
            "2026-11-23T12:00:00Z"
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap()
                + chrono::Duration::seconds(seconds),
            chrono_tz::UTC,
        )
    }

    fn add_task(store: &mut Store) -> Task {
        tasks::add(
            store,
            TaskInput {
                title: "Review".into(),
                activity: ActivityTemplate {
                    activity: "Reviewing".into(),
                    category: None,
                    description: String::new(),
                    tags: vec![],
                },
                quadrant: None,
                due_date: None,
            },
            &moment(0),
        )
        .unwrap()
    }

    #[test]
    fn inbox_returns_to_the_column_set_by_initial_focus_and_other_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("ui.db")).unwrap();
        let snapshot = queries::task_list(&mut store, Some(TaskStatus::Open), &moment(0)).unwrap();
        let mut app = App::new(snapshot, &moment(0), true).unwrap();
        app.key(
            &mut store,
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &moment(0),
        )
        .unwrap();
        assert_eq!(PANES[app.active], Some(Quadrant::Drop));

        let task = add_task(&mut store);
        tasks::edit(&mut store, task.id, |task| {
            task.quadrant = Some(Quadrant::Do);
            Ok(())
        })
        .unwrap();
        let snapshot = queries::task_list(&mut store, Some(TaskStatus::Open), &moment(0)).unwrap();
        let mut app = App::new(snapshot, &moment(0), true).unwrap();
        assert_eq!(PANES[app.active], Some(Quadrant::Do));
        for (keys, expected) in [
            (vec![KeyCode::Char('5'), KeyCode::Up], Quadrant::Delegate),
            (
                vec![KeyCode::Char('2'), KeyCode::Char('5'), KeyCode::Up],
                Quadrant::Drop,
            ),
            (
                vec![KeyCode::Tab, KeyCode::Tab, KeyCode::Up],
                Quadrant::Delegate,
            ),
            (
                vec![KeyCode::BackTab, KeyCode::Char('5'), KeyCode::Up],
                Quadrant::Drop,
            ),
            (
                vec![KeyCode::Char('l'), KeyCode::Char('5'), KeyCode::Up],
                Quadrant::Delegate,
            ),
            (
                vec![KeyCode::Char('h'), KeyCode::Char('5'), KeyCode::Up],
                Quadrant::Drop,
            ),
        ] {
            for key in keys {
                app.key(
                    &mut store,
                    KeyEvent::new(key, KeyModifiers::NONE),
                    &moment(0),
                )
                .unwrap();
            }
            assert_eq!(PANES[app.active], Some(expected));
        }
        app.select_task(task.id);
        app.focus_pane(4);
        app.navigate_pane(KeyCode::Up);
        assert_eq!(PANES[app.active], Some(Quadrant::Delegate));
    }

    #[test]
    fn completion_survives_an_external_next_without_interrupting_the_editor_or_repeating() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("ui.db")).unwrap();
        let task = add_task(&mut store);
        pomodoro::start_task(
            &mut store,
            task.id,
            pomodoro::Config {
                focus_seconds: 10,
                break_seconds: 5,
                long_break_seconds: 10,
                rounds: 4,
            },
            &moment(0),
        )
        .unwrap();
        let snapshot = queries::task_list(&mut store, Some(TaskStatus::Open), &moment(0)).unwrap();
        let mut app = App::new(snapshot, &moment(0), false).unwrap();
        app.input(InputKind::Search, "unfinished search".into());
        pomodoro::next(&mut store, &moment(12)).unwrap();
        app.refresh(&mut store, &moment(12)).unwrap();
        assert_eq!(app.cues, 1);
        assert!(app.message.contains("[FOCUS COMPLETE]"));
        assert!(app.message.contains("session has since changed"));
        assert!(matches!(&app.mode, Mode::Input { text, .. } if text == "unfinished search"));
        app.refresh(&mut store, &moment(12)).unwrap();
        assert_eq!(app.cues, 1);
        pomodoro::pause(&mut store, &moment(13)).unwrap();
        app.refresh(&mut store, &moment(13)).unwrap();
        assert_eq!(app.cues, 1, "a pause is not a completion");
        pomodoro::resume(&mut store, &moment(20)).unwrap();
        app.refresh(&mut store, &moment(20)).unwrap();
        app.sound = false;
        app.refresh(&mut store, &moment(24)).unwrap();
        assert!(app.message.contains("[BREAK COMPLETE]"));
        assert_eq!(
            app.cues, 1,
            "mute keeps the visual cue without another sound"
        );
    }

    #[test]
    fn stale_title_editor_preserves_the_other_clients_edit_and_keeps_its_input() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("ui.db")).unwrap();
        let task = add_task(&mut store);
        let snapshot = queries::task_list(&mut store, Some(TaskStatus::Open), &moment(0)).unwrap();
        let mut app = App::new(snapshot, &moment(0), true).unwrap();
        app.input(
            InputKind::Rename {
                id: task.id,
                original: task.title,
            },
            "My new title".into(),
        );
        tasks::edit(&mut store, task.id, |task| {
            task.title = "Other client's title".into();
            Ok(())
        })
        .unwrap();
        app.key(
            &mut store,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &moment(1),
        )
        .unwrap();
        assert_eq!(
            tasks::get(&store, task.id).unwrap().title,
            "Other client's title"
        );
        assert!(matches!(&app.mode, Mode::Input { text, .. } if text == "My new title"));
        assert!(app.message.contains("changed in another terminal"));
    }

    #[test]
    fn text_editing_removes_a_whole_user_visible_grapheme() {
        let mut text = "Cafe\u{301} 👩‍💻".to_owned();
        pop_grapheme(&mut text);
        assert_eq!(text, "Cafe\u{301} ");
        pop_grapheme(&mut text);
        pop_grapheme(&mut text);
        assert_eq!(text, "Caf");
    }

    fn draw_app(app: &mut App, width: u16, height: u16) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
    }

    #[test]
    fn scrolling_back_reacts_immediately_after_repeated_down_and_resize() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("ui.db")).unwrap();
        let task = add_task(&mut store);
        tasks::edit(&mut store, task.id, |task| {
            task.description = "A detailed note with several words.\n".repeat(40);
            Ok(())
        })
        .unwrap();
        let snapshot = queries::task_list(&mut store, Some(TaskStatus::Open), &moment(0)).unwrap();
        let mut app = App::new(snapshot, &moment(0), true).unwrap();
        for mode in [
            Mode::Help,
            Mode::Details(task.id),
            Mode::Confirm {
                action: Pending::Status {
                    expected: Box::new(TaskStatusExpectation::from_task(
                        &task,
                        &app.snapshot.status,
                    )),
                    state: TaskStatus::Done,
                },
                title: "Complete task",
                message: "Long title to review before confirming. ".repeat(40),
            },
        ] {
            app.mode = mode;
            app.modal_scroll = 0;
            for _ in 0..100 {
                app.key(
                    &mut store,
                    KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
                    &moment(0),
                )
                .unwrap();
                draw_app(&mut app, 65, 16);
            }
            let bottom = app.modal_scroll;
            assert!(bottom > 0);
            app.key(
                &mut store,
                KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
                &moment(0),
            )
            .unwrap();
            draw_app(&mut app, 65, 16);
            assert_eq!(app.modal_scroll, bottom.saturating_sub(5));
            app.modal_scroll = u16::MAX;
            draw_app(&mut app, 120, 40);
            assert!(
                app.modal_scroll < bottom,
                "resize must reduce the saved offset"
            );
        }
    }

    #[test]
    fn modified_shortcuts_do_not_trigger_actions_but_documented_controls_work() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("ui.db")).unwrap();
        add_task(&mut store);
        let snapshot = queries::task_list(&mut store, Some(TaskStatus::Open), &moment(0)).unwrap();
        let mut app = App::new(snapshot, &moment(0), true).unwrap();
        let initial_pane = app.active;
        for (code, modifiers) in [
            (KeyCode::Char('p'), KeyModifiers::CONTROL),
            (KeyCode::Char('s'), KeyModifiers::ALT),
            (KeyCode::Char('n'), KeyModifiers::CONTROL),
            (KeyCode::Char('q'), KeyModifiers::ALT),
            (KeyCode::Up, KeyModifiers::CONTROL),
            (KeyCode::Down, KeyModifiers::ALT),
        ] {
            assert!(
                !app.key(&mut store, KeyEvent::new(code, modifiers), &moment(0))
                    .unwrap()
            );
            assert!(matches!(app.mode, Mode::Browse));
            assert!(!app.dirty);
            assert_eq!(app.active, initial_pane);
        }
        assert!(
            queries::status(&mut store, &moment(0))
                .unwrap()
                .session
                .is_none()
        );

        app.mode = Mode::Confirm {
            action: Pending::Status {
                expected: Box::new(TaskStatusExpectation::from_task(
                    &tasks::get(&store, 1).unwrap(),
                    &app.snapshot.status,
                )),
                state: TaskStatus::Done,
            },
            title: "Complete task",
            message: "Confirm".into(),
        };
        app.key(
            &mut store,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::ALT),
            &moment(0),
        )
        .unwrap();
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        assert_eq!(tasks::get(&store, 1).unwrap().status, TaskStatus::Open);
        app.key(
            &mut store,
            KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT),
            &moment(0),
        )
        .unwrap();
        assert!(matches!(app.mode, Mode::Browse));
        let active = app.active;
        app.key(
            &mut store,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &moment(0),
        )
        .unwrap();
        assert_ne!(app.active, active);
        app.input(InputKind::Search, "draft".into());
        app.key(
            &mut store,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            &moment(0),
        )
        .unwrap();
        assert!(matches!(&app.mode, Mode::Input { text, .. } if text.is_empty()));
        assert!(
            app.key(
                &mut store,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &moment(0),
            )
            .unwrap()
        );
    }
}
