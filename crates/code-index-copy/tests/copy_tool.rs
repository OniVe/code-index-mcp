use code_index_core::indexer::Indexer;
use code_index_core::storage::Storage;
use serde_json::Value;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

struct Fixture {
    _temp: TempDir,
    src: PathBuf,
    dest: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("source");
        let dest = temp.path().join("destination");
        let home = temp.path().join("index-home");
        fs::create_dir(&src).unwrap();
        git(&src, &["init"]);
        fs::write(src.join("sample.py"), "def sample():\n    return 10\n").unwrap();
        git(&src, &["add", "."]);
        git(
            &src,
            &[
                "-c",
                "user.name=Example",
                "-c",
                "user.email=example@example.invalid",
                "commit",
                "-m",
                "Initial",
            ],
        );
        Self {
            _temp: temp,
            src,
            dest,
            home,
        }
    }

    fn index(&self) {
        fs::create_dir_all(self.src.join(".code-index")).unwrap();
        let mut storage = Storage::open_file(&self.src.join(".code-index/index.db")).unwrap();
        Indexer::new(&mut storage)
            .full_reindex(&self.src, false)
            .unwrap();
        storage.checkpoint_truncate().unwrap();
    }

    fn old_mtime(&self) {
        File::options()
            .write(true)
            .open(self.src.join("sample.py"))
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(1_577_836_800))
            .unwrap();
    }

    fn run(&self, extra: &[&str]) -> (Output, Value) {
        let output = Command::new(env!("CARGO_BIN_EXE_code-index-copy"))
            .arg(&self.src)
            .arg(&self.dest)
            .args(extra)
            .output()
            .unwrap();
        let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
            panic!(
                "stdout: {} stderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output, report)
    }

    fn assert_unchanged(&self) {
        let mut storage = Storage::open_file(&self.dest.join(".code-index/index.db")).unwrap();
        let result = Indexer::new(&mut storage)
            .full_reindex(&self.dest, false)
            .unwrap();
        assert_eq!(result.files_indexed, 0, "{result:?}");
        assert_eq!(result.files_deleted, 0, "{result:?}");
    }
}

#[test]
fn copy_keeps_index_unchanged() {
    let fixture = Fixture::new();
    fixture.old_mtime();
    fixture.index();
    let (output, report) = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(0));
    assert!(report["db_transferred"].as_bool().unwrap());
    fixture.assert_unchanged();
}

#[test]
fn worktree_keeps_index_and_skips_hook() {
    let fixture = Fixture::new();
    fixture.old_mtime();
    fixture.index();
    let hook = fixture.src.join(".git/hooks/post-checkout");
    fs::write(&hook, "#!/bin/sh\ntouch hook-marker\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (output, report) = fixture.run(&["--worktree", "--branch", "copy-test"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(report["db_transferred"].as_bool().unwrap());
    assert!(!fixture.dest.join("hook-marker").exists());
    git(
        &fixture.src,
        &["rev-parse", "--verify", "refs/heads/copy-test"],
    );
    fixture.assert_unchanged();
}

fn marker_removed(worktree: bool) {
    let fixture = Fixture::new();
    fixture.old_mtime();
    let marker = "raresecrettokenquokka";
    fs::write(
        fixture.src.join("untracked.py"),
        format!("# {marker}\ndef {marker}():\n    return '{marker}'\n"),
    )
    .unwrap();
    fixture.index();
    let source_db = fs::read(fixture.src.join(".code-index/index.db")).unwrap();
    assert!(source_db
        .windows(marker.len())
        .any(|part| part == marker.as_bytes()));
    let args: &[&str] = if worktree {
        &["--worktree", "--branch", "copy-test"]
    } else {
        &[]
    };
    let (output, report) = fixture.run(args);
    assert_eq!(output.status.code(), Some(0));
    assert!(report["db_paths_removed"].as_u64().unwrap() >= 1);
    let storage = Storage::open_file(&fixture.dest.join(".code-index/index.db")).unwrap();
    assert!(storage.get_file_by_path("untracked.py").unwrap().is_none());
    drop(storage);
    for path in [
        fixture.dest.join(".code-index/index.db"),
        fixture.dest.join(".code-index/index.db-wal"),
    ] {
        if path.exists() {
            let bytes = fs::read(path).unwrap();
            assert!(!bytes
                .windows(marker.len())
                .any(|part| part == marker.as_bytes()));
        }
    }
}

#[test]
fn removes_untracked_from_copy_database() {
    marker_removed(false);
}

#[test]
fn removes_untracked_from_worktree_database() {
    marker_removed(true);
}

#[test]
fn modified_same_size_is_not_aligned() {
    let fixture = Fixture::new();
    fixture.old_mtime();
    fixture.index();
    fs::write(
        fixture.src.join("sample.py"),
        "def sample():\n    return 11\n",
    )
    .unwrap();
    fixture.index();
    let (output, report) = fixture.run(&["--worktree", "--branch", "copy-test"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(report["not_aligned_modified"].as_u64().unwrap() >= 1);
    assert_ne!(
        fs::metadata(fixture.src.join("sample.py"))
            .unwrap()
            .modified()
            .unwrap(),
        fs::metadata(fixture.dest.join("sample.py"))
            .unwrap()
            .modified()
            .unwrap()
    );
    let mut storage = Storage::open_file(&fixture.dest.join(".code-index/index.db")).unwrap();
    let result = Indexer::new(&mut storage)
        .full_reindex(&fixture.dest, false)
        .unwrap();
    assert_eq!(result.files_indexed, 1, "{result:?}");
}

#[test]
fn missing_database_is_reported() {
    let fixture = Fixture::new();
    let (output, report) = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(0));
    assert!(!report["db_transferred"].as_bool().unwrap());
    assert!(!report["db_note"].as_str().unwrap().is_empty());
    assert!(!fixture.dest.join(".code-index/index.db").exists());
}

#[test]
fn writes_parseable_configs() {
    let fixture = Fixture::new();
    let home = fixture.home.to_str().unwrap();
    let (output, report) = fixture.run(&[
        "--index-home",
        home,
        "--port",
        "8123",
        "--alias",
        "example",
        "--language",
        "python",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert!(report["configs_written"].as_bool().unwrap());
    let daemon = code_index_core::daemon_core::config::parse_str(
        &fs::read_to_string(fixture.home.join("daemon.toml")).unwrap(),
    )
    .unwrap();
    let serve = code_index_core::federation::config::parse_str(
        &fs::read_to_string(fixture.home.join("serve.toml")).unwrap(),
    )
    .unwrap();
    assert_eq!(daemon.cache_targets[0].url, "http://127.0.0.1:8123");
    assert_eq!(daemon.paths[0].alias.as_deref(), Some("example"));
    assert_eq!(daemon.paths[0].language.as_deref(), Some("python"));
    assert_eq!(
        daemon.paths[0].path.to_string_lossy().replace('\\', "/"),
        std::path::absolute(&fixture.dest)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    );
    assert_eq!(serve.paths[0].alias, "example");
    assert_eq!(serve.paths[0].ip, "127.0.0.1");
    assert_eq!(serve.paths[0].port, Some(8123));
}

#[test]
fn occupied_destination_changes_nothing() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.dest).unwrap();
    fs::write(fixture.dest.join("keep.txt"), "keep").unwrap();
    let home = fixture.home.to_str().unwrap();
    let (output, _) = fixture.run(&[
        "--worktree",
        "--branch",
        "copy-test",
        "--index-home",
        home,
        "--port",
        "8123",
        "--alias",
        "example",
    ]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        fs::read_to_string(fixture.dest.join("keep.txt")).unwrap(),
        "keep"
    );
    assert!(!fixture.home.exists());
    let branch = Command::new("git")
        .arg("-C")
        .arg(&fixture.src)
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/copy-test"])
        .output()
        .unwrap();
    assert!(!branch.status.success());
}

#[test]
fn rejects_subdirectory_and_non_git_source() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.src.join("subdir")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_code-index-copy"))
        .arg(fixture.src.join("subdir"))
        .arg(&fixture.dest)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["error"]["stage"],
        "checks"
    );
    let output = Command::new(env!("CARGO_BIN_EXE_code-index-copy"))
        .arg(fixture._temp.path())
        .arg(&fixture.dest)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn failed_backup_rolls_back_worktree_and_branch() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.src.join(".code-index")).unwrap();
    fs::write(
        fixture.src.join(".code-index/index.db"),
        b"invalid sqlite bytes",
    )
    .unwrap();
    let (output, report) = fixture.run(&["--worktree", "--branch", "copy-test"]);
    assert_eq!(output.status.code(), Some(6));
    assert_eq!(report["error"]["stage"], "backup");
    assert!(!report["error"]["message"].as_str().unwrap().is_empty());
    assert!(!fixture.dest.exists());
    let branch = Command::new("git")
        .arg("-C")
        .arg(&fixture.src)
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/copy-test"])
        .output()
        .unwrap();
    assert!(!branch.status.success());
}
