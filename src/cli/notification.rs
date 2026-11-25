//! Play a short completion chime using the local audio system.
use std::{
    io::{Seek, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const CHIME: &[u8] = include_bytes!("phase-complete.wav");
const PLAYBACK_TIMEOUT: Duration = Duration::from_millis(2500);

#[cfg(all(unix, not(target_os = "macos")))]
const PLAYERS: &[(&str, &[&str])] = &[
    ("paplay", &[]),
    ("pw-play", &["-"]),
    ("aplay", &["-q"]),
    ("play", &["-q", "-t", "wav", "-"]),
];

#[cfg(target_os = "macos")]
const PLAYERS: &[(&str, &[&str])] = &[("afplay", &["/dev/stdin"])];

#[cfg(windows)]
const PLAYERS: &[(&str, &[&str])] = &[(
    "powershell.exe",
    &[
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "$ErrorActionPreference='Stop'; ([System.Media.SoundPlayer]::new([Console]::OpenStandardInput())).PlaySync()",
    ],
)];

#[cfg(not(any(unix, windows)))]
const PLAYERS: &[(&str, &[&str])] = &[];

/// Returns true when a player reports success. Otherwise the caller may emit
/// the terminal bell. The caller owns terminal detection and sound preferences.
pub(super) fn ring() -> bool {
    let players = PLAYERS.iter().map(|(program, args)| {
        let mut command = Command::new(program);
        command.args(*args);
        command
    });
    play(players, PLAYBACK_TIMEOUT)
}

fn play(players: impl IntoIterator<Item = Command>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    // AudioFile on macOS and some WAV readers need seekable input. An anonymous
    // file also lets the OS reclaim the sound if the watcher is interrupted.
    let Ok(mut sound) = tempfile::tempfile() else {
        return false;
    };
    if sound.write_all(CHIME).is_err() {
        return false;
    }
    for mut command in players {
        if Instant::now() >= deadline {
            break;
        }
        let Ok(input) = sound.rewind().and_then(|_| sound.try_clone()) else {
            return false;
        };
        let Ok(mut child) = command
            .stdin(input)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return true,
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(20)),
                    );
                }
                _ => {
                    if child.kill().is_ok() {
                        let _ = child.wait();
                    }
                    return false;
                }
            }
        }
    }
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};

    fn script(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn mock_player(dir: &Path, name: &str) -> Command {
        let mut command = Command::new(name);
        command.env("PATH", dir).current_dir(dir);
        command
    }

    #[test]
    fn unavailable_and_failed_players_fall_through_with_fresh_wav_input() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        script(dir, "reject", "/bin/cat > first.wav; exit 1");
        script(dir, "accept", "/bin/cat > second.wav");
        script(dir, "unexpected", ": > should-not-run");
        let players =
            ["absent", "reject", "accept", "unexpected"].map(|name| mock_player(dir, name));
        assert!(play(players, Duration::from_secs(2)));
        assert_eq!(fs::read(dir.join("first.wav")).unwrap(), CHIME);
        assert_eq!(fs::read(dir.join("second.wav")).unwrap(), CHIME);
        assert!(!dir.join("should-not-run").exists());
        assert!(!play([mock_player(dir, "absent")], Duration::from_secs(1)));
    }

    #[test]
    fn a_stalled_player_is_stopped_and_reaped_before_bell_fallback() {
        let dir = tempfile::tempdir().unwrap();
        script(dir.path(), "stalled", "echo $$ > pid; exec /bin/sleep 30");
        let start = Instant::now();
        assert!(!play(
            [mock_player(dir.path(), "stalled")],
            Duration::from_millis(400)
        ));
        assert!(start.elapsed() < Duration::from_secs(4));
        #[cfg(target_os = "linux")]
        {
            let pid = fs::read_to_string(dir.path().join("pid")).unwrap();
            assert!(!Path::new("/proc").join(pid.trim()).exists());
        }
    }
}
