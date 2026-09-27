use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Output};

pub fn run(dir: &Path, args: &[&str]) -> Result<Output> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("не удалось запустить git в {}", dir.display()))?;
    if !output.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}

pub fn success(dir: &Path, args: &[&str]) -> Result<bool> {
    Ok(Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?
        .status
        .success())
}

pub fn paths(dir: &Path, args: &[&str]) -> Result<Vec<String>> {
    let output = run(dir, args)?;
    Ok(output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect())
}

pub fn top_level(src: &Path) -> Result<String> {
    let output = run(src, &["rev-parse", "--show-toplevel"])?;
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

pub fn commit(src: &Path, name: &str) -> Result<String> {
    let output = run(
        src,
        &["rev-parse", "--verify", &format!("{name}^{{commit}}")],
    )?;
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

pub fn eol_raw(src: &Path) -> Result<bool> {
    let output = run(src, &["ls-files", "--eol", "-z"])?;
    let mut lf = false;
    let mut crlf = false;
    for line in output.stdout.split(|b| *b == 0) {
        if line.starts_with(b"i/lf ") {
            lf |= line.windows(4).any(|w| w == b"w/lf");
            crlf |= line.windows(6).any(|w| w == b"w/crlf");
        }
    }
    Ok(lf && !crlf)
}

pub fn add_worktree(src: &Path, dest: &Path, branch: &str, sha: &str, raw: bool) -> Result<()> {
    let hooks = tempfile::tempdir()?;
    let hooks_path = hooks.path().to_string_lossy().replace('\\', "/");
    let mut cmd = Command::new("git");
    cmd.current_dir(src)
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg(format!("core.hooksPath={hooks_path}"));
    if raw {
        cmd.arg("-c").arg("core.autocrlf=false");
    }
    let output = cmd
        .args(["worktree", "add", "-b", branch])
        .arg(dest)
        .arg(sha)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    if !output.status.success() {
        bail!(
            "git worktree add: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}
