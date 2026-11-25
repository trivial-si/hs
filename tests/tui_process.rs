#![cfg(target_os = "linux")]

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process},
    pty::{OpenptFlags, grantpt, ioctl_tiocgptpeer, openpt, unlockpt},
    termios::{LocalModes, Termios, Winsize, tcgetattr, tcsetwinsize},
};
use std::{
    fs::{self, File},
    io::{ErrorKind, Read, Write},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(15);

struct Fixture {
    dir: TempDir,
    db: PathBuf,
}

#[derive(Debug, PartialEq)]
struct Task {
    title: String,
    quadrant: Option<String>,
    status: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            db: dir.path().join("tui.db"),
            dir,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hs"));
        command
            .arg("--db")
            .arg(&self.db)
            .args(args)
            .env("TZ", "UTC")
            .env("TERM", "xterm-256color")
            .env("NO_COLOR", "1");
        command
    }

    fn cli(&self, args: &[&str]) {
        let mut child = self
            .command(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + TIMEOUT;
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("CLI command hung: {args:?}");
            }
            thread::sleep(POLL);
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn read(&self) -> Option<Connection> {
        let db = Connection::open_with_flags(&self.db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        db.busy_timeout(Duration::from_secs(1)).unwrap();
        Some(db)
    }

    fn task(&self, id: i64) -> Option<Task> {
        self.read()?
            .query_row(
                "SELECT title, quadrant, status FROM tasks WHERE id=?",
                [id],
                |row| {
                    Ok(Task {
                        title: row.get(0)?,
                        quadrant: row.get(1)?,
                        status: row.get(2)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    fn session(&self) -> Option<(String, String)> {
        self.read()?
            .query_row(
                "SELECT state,phase FROM pomodoro_sessions ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .ok()
            .flatten()
    }

    fn session_owner(&self) -> Option<(i64, Option<i64>)> {
        self.read()?
            .query_row(
                "SELECT id,task_id FROM pomodoro_sessions ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .ok()
            .flatten()
    }

    fn current_activity(&self) -> Option<String> {
        self.read()?
            .query_row(
                "SELECT a.name FROM entries e JOIN activities a ON a.id=e.activity_id
                 WHERE e.end_time IS NULL ORDER BY e.id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    fn tui(&self) -> Terminal {
        Self::open_tui(self.command(&["tui", "--no-sound"]))
    }

    fn tui_with_mock_audio(&self) -> Terminal {
        let player = self.dir.path().join("paplay");
        fs::write(
            &player,
            "#!/bin/sh\nset -eu\n/bin/cat > \"$HS_TEST_CUES/$$.tmp\"\n: > \"$HS_TEST_CUES/$$.playing\"\n/bin/mv \"$HS_TEST_CUES/$$.tmp\" \"$HS_TEST_CUES/$$.wav\"\n/bin/sleep 1.5\n/bin/rm \"$HS_TEST_CUES/$$.playing\"\n",
        )
        .unwrap();
        fs::set_permissions(&player, fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = self.command(&["tui"]);
        command
            .env("PATH", self.dir.path())
            .env("HS_TEST_CUES", self.dir.path());
        Self::open_tui(command)
    }

    fn audio_files(&self, extension: &str) -> Vec<PathBuf> {
        self.dir
            .path()
            .read_dir()
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == extension))
            .collect()
    }

    fn open_tui(command: Command) -> Terminal {
        let mut terminal = Terminal::spawn(command, true, true);
        terminal.until(
            "TUI did not enter raw mode and draw its first frame",
            |terminal| {
                !tcgetattr(&terminal.slave)
                    .unwrap()
                    .local_modes
                    .contains(LocalModes::ICANON)
                    && terminal.text().contains("INBOX")
            },
        );
        terminal
    }
}

struct Terminal {
    child: Child,
    master: File,
    slave: File,
    original: Termios,
    bytes: Vec<u8>,
}

impl Terminal {
    fn spawn(mut command: Command, tty_input: bool, tty_output: bool) -> Self {
        let flags = OpenptFlags::RDWR | OpenptFlags::CLOEXEC | OpenptFlags::NOCTTY;
        let master = openpt(flags).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let slave = File::from(ioctl_tiocgptpeer(&master, flags).unwrap());
        tcsetwinsize(
            &slave,
            Winsize {
                ws_row: 40,
                ws_col: 120,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap();
        let original = tcgetattr(&slave).unwrap();
        fcntl_setfl(&master, fcntl_getfl(&master).unwrap() | OFlags::NONBLOCK).unwrap();
        let child = command
            .stdin(if tty_input {
                Stdio::from(slave.try_clone().unwrap())
            } else {
                Stdio::null()
            })
            .stdout(if tty_output {
                Stdio::from(slave.try_clone().unwrap())
            } else {
                Stdio::piped()
            })
            .stderr(slave.try_clone().unwrap())
            .spawn()
            .unwrap();
        Self {
            child,
            master: File::from(master),
            slave,
            original,
            bytes: Vec::new(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    fn read_available(&mut self) {
        let mut buf = [0; 16 * 1024];
        loop {
            match self.master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => self.bytes.extend_from_slice(&buf[..n]),
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock
                        || error.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) =>
                {
                    break;
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => panic!("PTY read: {error}"),
            }
        }
    }

    fn until(&mut self, message: &str, mut condition: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            self.read_available();
            if condition(self) {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none() && Instant::now() < deadline,
                "{message}: {:?}",
                self.text()
            );
            thread::sleep(POLL);
        }
    }

    fn send(&mut self, keys: &str) {
        self.master.write_all(keys.as_bytes()).unwrap();
    }

    fn settle(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.read_available();
            assert!(self.child.try_wait().unwrap().is_none(), "{}", self.text());
            thread::sleep(POLL);
        }
    }

    fn resize(&self, columns: u16, rows: u16) {
        tcsetwinsize(
            &self.slave,
            Winsize {
                ws_row: rows,
                ws_col: columns,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap();
        kill_process(Pid::from_child(&self.child), Signal::WINCH).unwrap();
    }

    fn finish(mut self, success: bool) -> String {
        let deadline = Instant::now() + TIMEOUT;
        let status: ExitStatus = loop {
            self.read_available();
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "TUI hung: {}", self.text());
            thread::sleep(POLL);
        };
        self.read_available();
        if let Some(mut stdout) = self.child.stdout.take() {
            stdout.read_to_end(&mut self.bytes).unwrap();
        }
        assert_eq!(status.success(), success, "{status}: {}", self.text());
        let restored = tcgetattr(&self.slave).unwrap();
        assert_eq!(restored.input_modes, self.original.input_modes);
        assert_eq!(restored.output_modes, self.original.output_modes);
        assert_eq!(restored.control_modes, self.original.control_modes);
        assert_eq!(restored.local_modes, self.original.local_modes);
        let text = self.text();
        if success {
            assert!(text.contains("\x1b[?1049h"), "alternate screen not entered");
            assert!(
                text.contains("\x1b[?1049l"),
                "alternate screen not restored"
            );
            assert!(text.contains("\x1b[?2004l"), "bracketed paste not disabled");
            assert!(text.contains("\x1b[?25h"), "cursor not restored");
            assert!(
                !text.contains('\x07'),
                "TUI emitted an unexpected terminal bell"
            );
        }
        text
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn noninteractive_and_dumb_terminals_are_rejected_without_creating_a_database() {
    for (tty_input, tty_output, dumb) in [
        (false, true, false),
        (true, false, false),
        (true, true, true),
    ] {
        let fixture = Fixture::new();
        let mut command = fixture.command(&["tui", "--no-sound"]);
        if dumb {
            command.env("TERM", "dumb");
        }
        let text = Terminal::spawn(command, tty_input, tty_output).finish(false);
        assert!(!text.contains('\x1b'), "refusal polluted output: {text:?}");
        assert!(!fixture.db.exists(), "refusal created the database");
    }
}

#[test]
fn keyboard_task_workflow_creates_renames_moves_completes_and_archives() {
    let fixture = Fixture::new();
    let mut tui = fixture.tui();
    tui.send("5nReview notes\r");
    tui.until("task was not created", |_| fixture.task(1).is_some());
    tui.send("e\x15Review design 👩‍💻🚀\x7f\r");
    tui.until("rename or grapheme backspace failed", |_| {
        fixture
            .task(1)
            .is_some_and(|task| task.title == "Review design 👩‍💻")
    });
    tui.send("m2");
    tui.until("task was not moved to PLAN", |_| {
        fixture
            .task(1)
            .is_some_and(|task| task.quadrant.as_deref() == Some("plan"))
    });
    tui.send("d\x1b");
    tui.settle(Duration::from_millis(150));
    assert_eq!(fixture.task(1).unwrap().status, "open");
    tui.send("dy");
    tui.until("task was not completed", |_| {
        fixture.task(1).is_some_and(|task| task.status == "done")
    });
    tui.send("5n\x1b[200~Archive me\x1b[201~\r");
    tui.until("second task was not captured", |_| {
        fixture
            .task(2)
            .is_some_and(|task| task.title == "Archive me")
    });
    tui.send("a\x1b");
    tui.settle(Duration::from_millis(150));
    assert_eq!(fixture.task(2).unwrap().status, "open");
    tui.send("ay");
    tui.until("archive was not confirmed", |_| {
        fixture
            .task(2)
            .is_some_and(|task| task.status == "archived")
    });
    tui.send("q");
    tui.finish(true);
}

#[test]
fn arrow_keys_follow_matrix_rows_and_remember_the_inbox_column() {
    let fixture = Fixture::new();
    let mut tui = fixture.tui();
    // Empty panes must remain reachable. At the outer edges, arrows stay put
    // rather than wrapping to an unrelated quadrant.
    for (keys, id, title, quadrant) in [
        ("\x1b[B\x1b[D\x1b[CnInbox task\r", 1, "Inbox task", None),
        ("\x1b[AnDrop task\r", 2, "Drop task", Some("drop")),
        ("\x1b[AnPlan task\r", 3, "Plan task", Some("plan")),
        ("\x1b[A\x1b[De\x15Plan edge\r", 3, "Plan edge", Some("plan")),
        ("\x1b[CnDo task\r", 4, "Do task", Some("do")),
        ("\x1b[A\x1b[Ce\x15Do edge\r", 4, "Do edge", Some("do")),
        (
            "\x1b[BnDelegate task\r",
            5,
            "Delegate task",
            Some("delegate"),
        ),
        (
            "\x1b[B\x1b[Ae\x15Delegate return\r",
            5,
            "Delegate return",
            Some("delegate"),
        ),
        ("\x1b[De\x15Drop left\r", 2, "Drop left", Some("drop")),
        (
            "\x1b[B\x1b[Ae\x15Drop return\r",
            2,
            "Drop return",
            Some("drop"),
        ),
    ] {
        tui.send(keys);
        tui.until("arrow navigation targeted the wrong pane", |_| {
            fixture.task(id)
                == Some(Task {
                    title: title.into(),
                    quadrant: quadrant.map(str::to_owned),
                    status: "open".into(),
                })
        });
    }

    // The compact layout only shows one pane but keeps the same navigation.
    tui.resize(60, 18);
    tui.settle(Duration::from_millis(100));
    tui.send("\x1b[A\x1b[C\x1b[B\x1b[BnCompact inbox\r");
    tui.until("compact navigation did not reach the inbox", |_| {
        fixture.task(6)
            == Some(Task {
                title: "Compact inbox".into(),
                quadrant: None,
                status: "open".into(),
            })
    });
    tui.send("\x1b[Ae\x15Compact delegate\r");
    tui.until("compact inbox forgot the right column", |_| {
        fixture
            .task(5)
            .is_some_and(|task| task.title == "Compact delegate")
    });
    tui.send("q");
    tui.finish(true);
}

#[test]
fn pane_arrows_preserve_task_selection_and_do_not_escape_dialogs() {
    let fixture = Fixture::new();
    fixture.cli(&["task", "add", "Alpha", "--quadrant", "plan"]);
    fixture.cli(&["task", "add", "Bravo", "--quadrant", "plan"]);
    fixture.cli(&["task", "add", "Inbox task"]);
    let mut tui = fixture.tui();

    for (keys, id, title) in [
        ("2je\x15Bravo selected\r", 2, "Bravo selected"),
        (
            "\x1b[B\x1b[B\x1b[A\x1b[Ae\x15Bravo returned\r",
            2,
            "Bravo returned",
        ),
        ("ke\x15Alpha selected\r", 1, "Alpha selected"),
        ("je\x15Bravo reselected\r", 2, "Bravo reselected"),
        (
            "?\x1b[B\x1b[A\x1b[C\x1b[Dqe\x15After help\r",
            2,
            "After help",
        ),
        (
            "\r\x1b[B\x1b[A\x1b[C\x1b[D\re\x15After details\r",
            2,
            "After details",
        ),
        (
            "d\x1b[B\x1b[A\x1b[C\x1b[Dne\x15After confirmation\r",
            2,
            "After confirmation",
        ),
        (
            "e\x1b[B\x1b[A\x1b[C\x1b[D\x15After editor\r",
            2,
            "After editor",
        ),
    ] {
        tui.send(keys);
        tui.until("navigation changed the selected task unexpectedly", |_| {
            fixture.task(id).is_some_and(|task| task.title == title)
        });
    }
    assert_eq!(fixture.task(1).unwrap().title, "Alpha selected");
    assert_eq!(fixture.task(2).unwrap().status, "open");
    assert_eq!(fixture.task(3).unwrap().title, "Inbox task");
    tui.send("q");
    tui.finish(true);
}

#[test]
fn timer_controls_pause_resume_and_quit_preserve_the_running_timer() {
    let fixture = Fixture::new();
    fixture.cli(&["task", "add", "Write report", "--quadrant", "plan"]);
    let mut tui = fixture.tui();
    tui.send("2 ");
    tui.until("focus did not start", |_| {
        fixture.session() == Some(("running".into(), "focus".into()))
    });
    tui.send(" ");
    tui.until("focus did not pause", |_| {
        fixture.session() == Some(("paused".into(), "focus".into()))
    });
    tui.send(" ");
    tui.until("focus did not resume", |_| {
        fixture.session() == Some(("running".into(), "focus".into()))
    });
    tui.send("q");
    tui.finish(true);
    assert_eq!(fixture.session(), Some(("running".into(), "focus".into())));
}

#[test]
fn space_continues_the_current_session_while_p_can_switch_to_the_selected_task() {
    let fixture = Fixture::new();
    fixture.cli(&["task", "add", "Current focus", "--quadrant", "do"]);
    fixture.cli(&["task", "add", "Different selection"]);
    fixture.cli(&[
        "pomo", "start", "--task", "1", "--focus", "2s", "--break", "5m",
    ]);
    let owner = fixture.session_owner().unwrap();
    let mut tui = fixture.tui();
    tui.send("5");
    tui.until("focus did not finish", |_| {
        fixture.session() == Some(("ready".into(), "short_break".into()))
    });

    // p targets the selected task, and must offer a guarded switch instead of
    // silently discarding the current session's offered break.
    tui.send("p");
    tui.until("switch confirmation did not open", |terminal| {
        terminal.text().contains("confirm")
    });
    assert_eq!(fixture.session_owner(), Some(owner));
    assert_eq!(
        fixture.session(),
        Some(("ready".into(), "short_break".into()))
    );

    // Cancelling and then pressing Space must act on the displayed session,
    // even though a different task is selected in the inbox.
    tui.send("n ");
    tui.until("Space did not start the current session's break", |_| {
        fixture.session() == Some(("running".into(), "short_break".into()))
    });
    assert_eq!(fixture.session_owner(), Some(owner));
    assert!(fixture.current_activity().is_none(), "break recorded work");

    // A deliberate, confirmed p switch still starts focus on the selection.
    tui.send("py");
    tui.until("confirmed switch did not start selected task", |_| {
        fixture.session() == Some(("running".into(), "focus".into()))
            && fixture
                .session_owner()
                .is_some_and(|(_, task)| task == Some(2))
    });
    assert_ne!(fixture.session_owner().unwrap().0, owner.0);
    assert_eq!(
        fixture.current_activity().as_deref(),
        Some("Different selection")
    );
    tui.send("q");
    tui.finish(true);
}

#[test]
fn starting_pomodoro_with_space_confirms_replacing_manual_tracking() {
    let fixture = Fixture::new();
    fixture.cli(&["task", "add", "Selected focus", "--quadrant", "plan"]);
    fixture.cli(&["start", "Existing manual work"]);
    let mut tui = fixture.tui();
    tui.send(" ");
    tui.until(
        "Space did not ask before replacing manual work",
        |terminal| terminal.text().contains("confirm"),
    );
    assert_eq!(
        fixture.current_activity().as_deref(),
        Some("Existing manual work")
    );
    assert!(fixture.session().is_none());
    tui.send("nq");
    tui.finish(true);
    assert_eq!(
        fixture.current_activity().as_deref(),
        Some("Existing manual work")
    );
    assert!(fixture.session().is_none());

    let mut tui = fixture.tui();
    tui.send(" y");
    tui.until("confirmed Space start did not begin focus", |_| {
        fixture.session() == Some(("running".into(), "focus".into()))
    });
    assert_eq!(fixture.session_owner().unwrap().1, Some(1));
    assert_eq!(
        fixture.current_activity().as_deref(),
        Some("Selected focus")
    );
    tui.send("q");
    tui.finish(true);
}

#[test]
fn process_termination_signal_restores_terminal_without_stopping_work() {
    let fixture = Fixture::new();
    fixture.cli(&["pomo", "start", "Long session", "--focus", "1h"]);
    let tui = fixture.tui();
    kill_process(Pid::from_child(&tui.child), Signal::TERM).unwrap();
    tui.finish(true);
    assert_eq!(fixture.session(), Some(("running".into(), "focus".into())));
}

#[test]
fn stale_stop_confirmation_cannot_stop_a_replacement_timer() {
    let fixture = Fixture::new();
    fixture.cli(&["start", "Original timer"]);
    let mut tui = fixture.tui();
    tui.send("x");
    tui.until("stop confirmation did not open", |terminal| {
        // The panel also advertises Stop, so wait for the modal-only action.
        terminal.text().contains("confirm")
    });
    fixture.cli(&["start", "Replacement timer"]);
    tui.settle(Duration::from_millis(1300));
    // Process the confirmation before quitting, then inspect the durable state.
    tui.send("yq");
    tui.finish(true);
    assert_eq!(
        fixture.current_activity().as_deref(),
        Some("Replacement timer")
    );
}

#[test]
fn stale_task_confirmation_cannot_complete_renamed_work_or_stop_its_timer() {
    let fixture = Fixture::new();
    fixture.cli(&["task", "add", "Original outcome"]);
    fixture.cli(&["start", "--task", "1"]);
    let mut tui = fixture.tui();
    tui.send("d");
    tui.until("task confirmation did not open", |terminal| {
        terminal.text().contains("Complete")
    });
    fixture.cli(&["task", "set", "1", "--title", "Revised outcome"]);
    tui.settle(Duration::from_millis(1300));
    tui.send("yq");
    tui.finish(true);
    let task = fixture.task(1).unwrap();
    assert_eq!(task.title, "Revised outcome");
    assert_eq!(task.status, "open");
    assert_eq!(
        fixture.current_activity().as_deref(),
        Some("Original outcome")
    );
}

#[test]
fn a_locked_database_does_not_block_quitting_or_terminal_cleanup() {
    let fixture = Fixture::new();
    fixture.cli(&["pomo", "start", "Long session", "--focus", "1h"]);
    let mut tui = fixture.tui();
    let mut connection = Connection::open(&fixture.db).unwrap();
    connection.busy_timeout(Duration::from_secs(1)).unwrap();
    let lock = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    // A live Pomodoro refresh needs the writer lock. Keep it unavailable
    // through a refresh interval, then request an ordinary terminal exit.
    tui.settle(Duration::from_millis(1200));
    let began = Instant::now();
    tui.send("q");
    tui.finish(true);
    assert!(
        began.elapsed() < Duration::from_secs(1),
        "quit waited for database access"
    );
    lock.rollback().unwrap();
    assert_eq!(fixture.session(), Some(("running".into(), "focus".into())));
}

#[test]
fn deadline_next_phase_and_tiny_resizes_remain_usable_and_ctrl_c_restores_terminal() {
    let fixture = Fixture::new();
    fixture.cli(&["pomo", "start", "Read", "--focus", "2s", "--break", "5m"]);
    let mut tui = fixture.tui();
    tui.until("deadline did not offer the short rest", |_| {
        fixture.session() == Some(("ready".into(), "short_break".into()))
    });
    tui.until("work completion banner was not drawn", |terminal| {
        terminal.text().contains("[FOCUS")
    });
    tui.send(" ");
    tui.until("next did not start the short rest", |_| {
        fixture.session() == Some(("running".into(), "short_break".into()))
    });
    tui.resize(10, 4);
    tui.settle(Duration::from_millis(150));
    tui.resize(1, 1);
    tui.settle(Duration::from_millis(150));
    tui.resize(120, 40);
    tui.send("?");
    tui.settle(Duration::from_millis(150));
    tui.send("\x1b");
    tui.settle(Duration::from_millis(100));
    tui.send("\x03");
    tui.finish(true);
    assert_eq!(
        fixture.session(),
        Some(("running".into(), "short_break".into()))
    );
}

#[test]
fn work_and_rest_play_once_without_interrupting_or_blocking_the_task_editor() {
    let fixture = Fixture::new();
    fixture.cli(&["pomo", "start", "Read", "--focus", "3s", "--break", "2s"]);
    let mut tui = fixture.tui_with_mock_audio();
    tui.send("5nDuring");
    tui.until("focus did not send its audio payload", |terminal| {
        terminal.text().contains("[FOCUS") && fixture.audio_files("wav").len() == 1
    });
    assert_eq!(fixture.audio_files("playing").len(), 1);
    // The fake player stays busy for1.5s. Input and the resulting task write
    // must finish while it is still playing, proving audio is off the UI loop.
    let began = Instant::now();
    tui.send(" playback\r");
    tui.until("completion discarded or blocked the open editor", |_| {
        fixture
            .task(1)
            .is_some_and(|task| task.title == "During playback")
    });
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(fixture.audio_files("playing").len(), 1);
    tui.send("nCancelled draft\x1b");
    tui.settle(Duration::from_millis(100));
    tui.send("N");
    tui.until("rest did not send its audio payload", |terminal| {
        terminal.text().contains("[BREAK") && fixture.audio_files("wav").len() == 2
    });
    assert_eq!(fixture.session(), Some(("ready".into(), "focus".into())));
    tui.settle(Duration::from_millis(1200));
    assert_eq!(
        fixture.audio_files("wav").len(),
        2,
        "ready refresh repeated a cue"
    );
    assert!(fixture.task(2).is_none(), "cancelled draft was saved");
    tui.send("q");
    tui.finish(true);
    let payloads = fixture.audio_files("wav");
    assert_eq!(payloads.len(), 2);
    for payload in payloads {
        assert_eq!(
            fs::read(payload).unwrap(),
            include_bytes!("../src/cli/phase-complete.wav")
        );
    }
    assert!(fixture.audio_files("playing").is_empty());
}

#[test]
fn refresh_preserves_selected_id_and_filter_targets_only_the_matching_task() {
    let fixture = Fixture::new();
    fixture.cli(&["task", "add", "Alpha", "--quadrant", "plan"]);
    fixture.cli(&["task", "add", "Bravo", "--quadrant", "plan"]);
    let mut tui = fixture.tui();
    tui.send("2j");
    tui.settle(Duration::from_millis(100));
    fixture.cli(&[
        "task",
        "add",
        "Earlier",
        "--quadrant",
        "plan",
        "--due",
        "2000-01-01",
    ]);
    // Give the live UI more than one refresh interval to reorder its list.
    tui.settle(Duration::from_millis(1300));
    tui.send("e\x15Chosen stable task\r");
    tui.until("refresh changed the selected task identity", |_| {
        fixture
            .task(2)
            .is_some_and(|task| task.title == "Chosen stable task")
    });
    assert_eq!(fixture.task(1).unwrap().title, "Alpha");
    assert_eq!(fixture.task(3).unwrap().title, "Earlier");
    tui.send("/Earlier\r");
    tui.send("e\x15Filtered rename\r");
    tui.until("search did not select the matching task", |_| {
        fixture
            .task(3)
            .is_some_and(|task| task.title == "Filtered rename")
    });
    tui.send("\x1b");
    tui.settle(Duration::from_millis(100));
    tui.send("q");
    tui.finish(true);
}
