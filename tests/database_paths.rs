#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
};

fn hs(directory: &Path, database: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hs"))
        .current_dir(directory)
        .env("TZ", "UTC")
        .arg("--db")
        .arg(database)
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn special_sqlite_names_stay_literal_persistent_and_private() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    for filename in [
        ":memory:",
        "file:literal.db",
        "file:temporary.db?mode=memory",
    ] {
        success(hs(dir, filename, &["task", "add", "Keep this task"]));
        assert!(success(hs(dir, filename, &["task", "list"])).contains("Keep this task"));
        let metadata = fs::metadata(dir.join(filename)).unwrap();
        assert!(metadata.len() > 0);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }
    assert!(!dir.join("literal.db").exists());
    assert!(!dir.join("temporary.db").exists());
    // A special-looking path component in the backup directory is also literal.
    success(hs(dir, ":memory:", &["backup", "file:copies/saved.db"]));
    assert!(
        success(hs(dir, "./file:copies/saved.db", &["task", "list"])).contains("Keep this task")
    );
    assert!(!dir.join("copies").exists());
}

#[test]
fn old_uri_placeholder_does_not_hide_existing_history() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    success(hs(dir, "prior.db", &["task", "add", "Earlier task"]));
    fs::write(dir.join("file:prior.db"), []).unwrap();
    let refused = hs(dir, "file:prior.db", &["status"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("plain filesystem path"));
    assert_eq!(fs::metadata(dir.join("file:prior.db")).unwrap().len(), 0);
    assert!(success(hs(dir, "prior.db", &["task", "list"])).contains("Earlier task"));
    success(hs(dir, "./file:prior.db", &["task", "add", "Literal task"]));
    assert!(success(hs(dir, "file:prior.db", &["task", "list"])).contains("Literal task"));
}

#[test]
fn dangling_links_and_nonregular_database_paths_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    std::os::unix::fs::symlink("missing.db", dir.join("link.db")).unwrap();
    assert!(!hs(dir, "link.db", &["status"]).status.success());
    assert!(!dir.join("missing.db").exists());
    fs::create_dir(dir.join("directory")).unwrap();
    let refused = hs(dir, "directory", &["status"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("regular file"));
}
