#![cfg(target_os = "linux")]

use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, WaitOptions, kill_process, waitpid},
    pty::{OpenptFlags, grantpt, ioctl_tiocgptpeer, openpt, unlockpt},
};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{ErrorKind, Read},
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(8);
const POLL: Duration = Duration::from_millis(15);
const FOCUS: &str = "[FOCUS COMPLETE]";
const REST: &str = "[BREAK COMPLETE]";

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let player = dir.path().join("paplay");
        // Each launched player publishes its input only after receiving EOF.
        fs::write(&player, "#!/bin/sh\nset -eu\n/bin/cat > \"$TEST_CUES/$$.tmp\"\n/bin/mv \"$TEST_CUES/$$.tmp\" \"$TEST_CUES/$$.wav\"\n").unwrap();
        fs::set_permissions(player, fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hs"));
        cmd.arg("--db")
            .arg(self.dir.path().join("hs.sqlite"))
            .args(args)
            .env("TZ", "UTC")
            .env("TERM", "xterm-256color")
            .env_remove("NO_COLOR")
            .env("PATH", self.dir.path())
            .env("TEST_CUES", self.dir.path());
        cmd
    }

    fn run(&self, args: &[&str]) -> String {
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
                panic!("noninteractive command did not return: {args:?}");
            }
            thread::sleep(POLL);
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn status(&self) -> Value {
        let text = self.run(&["--json", "pomo", "status"]);
        assert!(!text.contains(['\x07', '\x1b']));
        serde_json::from_str::<Value>(&text).unwrap()["session"].clone()
    }

    fn cues(&self) -> usize {
        let mut count = 0;
        for path in self
            .dir
            .path()
            .read_dir()
            .unwrap()
            .map(|e| e.unwrap().path())
        {
            if path.extension().is_some_and(|ext| ext == "wav") {
                let audio = fs::read(&path).unwrap();
                assert!(
                    audio.len() > 44 && audio.starts_with(b"RIFF") && &audio[8..12] == b"WAVE",
                    "fake paplay did not receive WAV audio"
                );
                count += 1;
            }
        }
        count
    }
}

struct Terminal {
    child: Child,
    master: File,
    bytes: Vec<u8>,
}

impl Terminal {
    fn spawn(mut command: Command, tty_input: bool) -> Self {
        let flags = OpenptFlags::RDWR | OpenptFlags::CLOEXEC | OpenptFlags::NOCTTY;
        let master = openpt(flags).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let slave = File::from(ioctl_tiocgptpeer(&master, flags).unwrap());
        fcntl_setfl(&master, fcntl_getfl(&master).unwrap() | OFlags::NONBLOCK).unwrap();
        let input = if tty_input {
            Stdio::from(slave.try_clone().unwrap())
        } else {
            Stdio::null()
        };
        let child = command
            .stdin(input)
            .stdout(slave.try_clone().unwrap())
            .stderr(slave)
            .spawn()
            .unwrap();
        Self {
            child,
            master: File::from(master),
            bytes: Vec::new(),
        }
    }

    fn read_available(&mut self) {
        let mut buf = [0; 4096];
        loop {
            match self.master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => self.bytes.extend_from_slice(&buf[..n]),
                Err(e)
                    if e.kind() == ErrorKind::WouldBlock
                        || e.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) =>
                {
                    break;
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => panic!("read PTY: {e}"),
            }
        }
    }

    fn running(&mut self) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            self.read_available();
            let text = String::from_utf8_lossy(&self.bytes);
            if text.contains("\r\x1b[2K") {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none() && Instant::now() < deadline,
                "watcher never displayed its running phase: {text:?}"
            );
            thread::sleep(POLL);
        }
    }

    fn signal(&self, sig: Signal) {
        kill_process(Pid::from_child(&self.child), sig).unwrap();
    }

    fn finish(mut self) -> (ExitStatus, String) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            self.read_available();
            if let Some(status) = self.child.try_wait().unwrap() {
                self.read_available();
                return (status, String::from_utf8_lossy(&self.bytes).into_owned());
            }
            assert!(
                Instant::now() < deadline,
                "PTY child hung: {:?}",
                String::from_utf8_lossy(&self.bytes)
            );
            thread::sleep(POLL);
        }
    }

    fn success(self) -> String {
        let (status, text) = self.finish();
        assert!(status.success(), "PTY child {status}: {text:?}");
        text
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Also release a SIGSTOPed child when an assertion fails.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn assert_no_alert(text: &str) {
    assert!(
        !text.contains(FOCUS) && !text.contains(REST) && !text.contains('\x07'),
        "{text:?}"
    );
}

fn suspend_watcher_until_deadline(f: &Fixture, watcher: &Terminal) {
    // Take the lock before suspending the watcher so it cannot freeze while
    // holding a transaction needed by the second command.
    let mut db = rusqlite::Connection::open_with_flags(
        f.dir.path().join("hs.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .unwrap();
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)
        .unwrap();
    let deadline: i64 = tx
        .query_row(
            "SELECT deadline_utc FROM pomodoro_sessions WHERE state = 'running'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let timeout = Instant::now() + TIMEOUT;
    watcher.signal(Signal::STOP);
    loop {
        if let Some((_, status)) = waitpid(
            Some(Pid::from_child(&watcher.child)),
            WaitOptions::UNTRACED | WaitOptions::NOHANG,
        )
        .unwrap()
        {
            assert!(
                status.stopped(),
                "watcher exited before suspension: {status:?}"
            );
            break;
        }
        assert!(Instant::now() < timeout, "watcher did not suspend");
        thread::sleep(POLL);
    }
    tx.commit().unwrap();
    while chrono::Utc::now().timestamp() < deadline {
        assert!(
            Instant::now() < timeout,
            "wall clock did not reach phase deadline"
        );
        thread::sleep(POLL);
    }
}

#[test]
fn foreground_focus_short_break_and_long_break_each_emit_one_visible_and_audible_cue() {
    let f = Fixture::new();
    let start = [
        "pomo",
        "start",
        "Draft",
        "--focus",
        "2s",
        "--break",
        "1s",
        "--long-break",
        "1s",
        "--rounds",
        "2",
    ];
    for (n, (args, banner, next)) in [
        (start.as_slice(), FOCUS, "short_break"),
        (&["pomo", "next"], REST, "focus"),
        (&["pomo", "next"], FOCUS, "long_break"),
        (&["pomo", "next"], REST, "focus"),
    ]
    .into_iter()
    .enumerate()
    {
        let text = Terminal::spawn(f.command(args), true).success();
        assert_eq!(text.matches(banner).count(), 1, "{text:?}");
        assert!(!text.contains(if banner == FOCUS { REST } else { FOCUS }));
        assert!(text.contains(&format!("\x1b[1;7m {banner} \x1b[0m")));
        assert!(
            !text.contains('\x07'),
            "successful audio also rang the terminal bell"
        );
        let session = f.status();
        assert_eq!(session["phase"], next);
        assert_eq!(session["state"], "ready");
        assert_eq!(f.cues(), n + 1, "{text:?}");
    }
    let text = Terminal::spawn(f.command(&["pomo", "watch"]), true).success();
    assert_no_alert(&text);
    assert_eq!(f.cues(), 4, "reattaching to a ready phase must be silent");
}

#[test]
fn detached_and_redirected_starts_return_and_a_silent_foreground_resume_keeps_the_banner() {
    let f = Fixture::new();
    // A long phase makes accidental foreground attachment time out, rather
    // than allowing a blocking start to pass when the phase expires.
    for (args, tty_input) in [
        (
            vec!["pomo", "start", "First", "--focus", "5m", "--detach"],
            true,
        ),
        (vec!["pomo", "start", "Second", "--focus", "5m"], false),
    ] {
        let text = Terminal::spawn(f.command(&args), tty_input).success();
        assert_no_alert(&text);
        assert_eq!(f.status()["state"], "running");
        f.run(&["pomo", "stop"]);
    }
    f.run(&["pomo", "start", "Review", "--focus", "2s", "--break", "1s"]);
    f.run(&["pomo", "pause"]);
    let mut resume = f.command(&["pomo", "resume", "--no-sound"]);
    resume.env("TERM", "dumb").env("NO_COLOR", "1");
    let text = Terminal::spawn(resume, true).success();
    assert_eq!(text.matches(FOCUS).count(), 1, "{text:?}");
    assert!(!text.contains('\x07'));
    assert!(
        !text
            .lines()
            .find(|line| line.contains(FOCUS))
            .unwrap()
            .contains("\x1b[1;7m")
    );
    let mut next = f.command(&["pomo", "next", "--no-bell"]);
    next.env("NO_COLOR", "1");
    let text = Terminal::spawn(next, true).success();
    assert_eq!(text.matches(REST).count(), 1, "{text:?}");
    assert!(!text.contains('\x07'));
    assert!(!text.contains("\x1b[1;7m"));
    assert_eq!(f.cues(), 0);
}

#[test]
fn missing_audio_player_falls_back_to_one_terminal_bell() {
    let f = Fixture::new();
    fs::remove_file(f.dir.path().join("paplay")).unwrap();
    let text = Terminal::spawn(
        f.command(&["pomo", "start", "Draft", "--focus", "1s"]),
        true,
    )
    .success();
    assert_eq!(text.matches(FOCUS).count(), 1, "{text:?}");
    assert_eq!(text.matches('\x07').count(), 1, "{text:?}");
}

#[test]
fn interruption_is_quiet_and_ctrl_c_leaves_the_timer_running() {
    let f = Fixture::new();
    for control in ["pause", "stop"] {
        let mut watcher = Terminal::spawn(
            f.command(&["pomo", "start", "Draft", "--focus", "5m"]),
            true,
        );
        watcher.running();
        f.run(&["pomo", control]);
        assert_no_alert(&watcher.success());
        if control == "pause" {
            f.run(&["pomo", "stop"]);
        }
    }
    let mut watcher = Terminal::spawn(
        f.command(&["pomo", "start", "Continue", "--focus", "5m"]),
        true,
    );
    watcher.running();
    watcher.signal(Signal::INT);
    let (status, text) = watcher.finish();
    assert!(
        status.signal() == Some(Signal::INT.as_raw()) || status.code() == Some(130),
        "{status}"
    );
    assert_no_alert(&text);
    assert_eq!(f.status()["state"], "running");
    assert_eq!(f.cues(), 0);
}

#[test]
fn watcher_uses_its_original_phase_when_another_terminal_advances_or_replaces_it() {
    let f = Fixture::new();
    for replacement in [false, true] {
        let mut watcher = Terminal::spawn(
            f.command(&["pomo", "start", "Read", "--focus", "2s", "--break", "5m"]),
            true,
        );
        watcher.running();
        suspend_watcher_until_deadline(&f, &watcher);
        if replacement {
            f.run(&["pomo", "stop"]);
            f.run(&["pomo", "start", "Another session", "--focus", "5m"]);
        } else {
            f.run(&["pomo", "next"]);
        }
        watcher.signal(Signal::CONT);
        let text = watcher.success();
        if replacement {
            assert_no_alert(&text);
        } else {
            assert_eq!(text.matches(FOCUS).count(), 1, "{text:?}");
            assert!(!text.contains(REST));
        }
        assert_eq!(f.cues(), 1);
        f.run(&["pomo", "stop"]);
    }
}
