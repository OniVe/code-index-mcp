mod configs;
mod database;
mod files;
mod git;
mod report;

use anyhow::{bail, Context, Result};
use clap::{error::ErrorKind, Parser};
use report::{ErrorInfo, ExitKind, Report, Rollback};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "code-index-copy",
    version,
    about = "Копировать проект вместе с готовым индексом"
)]
struct Cli {
    /// Корень исходного рабочего дерева Git
    source: PathBuf,
    /// Каталог копии
    dest: PathBuf,
    /// Создать отдельное рабочее дерево Git
    #[arg(long, requires = "branch")]
    worktree: bool,
    /// Имя новой ветки рабочего дерева
    #[arg(long, requires = "worktree")]
    branch: Option<String>,
    /// Коммит для рабочего дерева (по умолчанию HEAD)
    #[arg(long, requires = "worktree")]
    commit: Option<String>,
    /// Каталог конфигурации индекса копии
    #[arg(long, requires_all = ["port", "alias"])]
    index_home: Option<PathBuf>,
    /// Порт сервера копии
    #[arg(long, requires = "index_home", value_parser = clap::value_parser!(u16).range(1..))]
    port: Option<u16>,
    /// Алиас проекта
    #[arg(long, requires = "index_home")]
    alias: Option<String>,
    /// Язык проекта
    #[arg(long, requires = "index_home")]
    language: Option<String>,
}

struct Failure {
    kind: ExitKind,
    stage: &'static str,
    error: anyhow::Error,
    rollback: bool,
}

impl Failure {
    fn new(kind: ExitKind, stage: &'static str, error: anyhow::Error, rollback: bool) -> Self {
        Self {
            kind,
            stage,
            error,
            rollback,
        }
    }
}

fn valid_label(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
}

fn normalized(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut result = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    if result.exists() {
        return Ok(fs::canonicalize(result)?);
    }
    let parent = result.parent().context("у пути нет родителя")?;
    Ok(normalized(parent)?.join(result.file_name().context("у пути нет имени")?))
}

fn ensure_empty(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    if !path.is_dir() || fs::read_dir(path)?.next().is_some() {
        bail!("каталог {} существует и не пуст", path.display());
    }
    Ok(true)
}

fn rollback(
    cli: &Cli,
    src: &Path,
    dest_existed: bool,
    home_existed: bool,
    branch_created: bool,
) -> Rollback {
    let mut result = Rollback::default();
    if cli.worktree {
        if let Err(error) = git::run(
            src,
            &["worktree", "remove", "--force", &cli.dest.to_string_lossy()],
        ) {
            eprintln!("Ошибка отката рабочего дерева: {error:#}");
            if cli.dest.exists() {
                match fs::remove_dir_all(&cli.dest) {
                    Ok(()) => result.files_removed = true,
                    Err(error) => eprintln!("Ошибка удаления каталога копии: {error}"),
                }
            }
        } else {
            result.files_removed = true;
        }
        if let Err(error) = git::run(src, &["worktree", "prune"]) {
            eprintln!("Ошибка очистки списка рабочих деревьев: {error:#}");
        }
    } else if cli.dest.exists() {
        if let Err(error) = fs::remove_dir_all(&cli.dest) {
            eprintln!("Ошибка удаления копии: {error}");
        } else {
            result.files_removed = true;
        }
    }
    if dest_existed && !cli.dest.exists() {
        if let Err(error) = fs::create_dir_all(&cli.dest) {
            eprintln!("Ошибка восстановления пустого каталога: {error}");
        }
    }
    if let Some(home) = &cli.index_home {
        if home.exists() {
            if let Err(error) = fs::remove_dir_all(home) {
                eprintln!("Ошибка удаления дома индекса: {error}");
            } else {
                result.index_home_removed = true;
            }
        }
        if home_existed && !home.exists() {
            if let Err(error) = fs::create_dir_all(home) {
                eprintln!("Ошибка восстановления дома индекса: {error}");
            }
        }
    }
    if branch_created {
        if let Some(branch) = &cli.branch {
            match git::run(src, &["branch", "-d", branch]) {
                Ok(_) => result.branch_deleted = true,
                Err(error) => {
                    let note = format!("ветка {branch} не удалена: {error:#}");
                    eprintln!("{note}");
                    result.branch_note = Some(note);
                }
            }
        }
    }
    result
}

fn execute(cli: &Cli, report: &mut Report) -> std::result::Result<(), Failure> {
    let started = Instant::now();
    let check_started = Instant::now();
    eprintln!("Проверка исходника и приёмника");
    let checked = (|| -> Result<(PathBuf, bool, bool, Option<String>)> {
        let top = git::top_level(&cli.source).map_err(|error| {
            anyhow::anyhow!("исходник не является рабочим деревом Git: {error:#}")
        })?;
        let root = fs::canonicalize(&top)?;
        let source = fs::canonicalize(&cli.source)?;
        if root != source {
            bail!(
                "исходник — подкаталог рабочего дерева, укажите корень {}",
                root.display()
            );
        }
        let dest = normalized(&cli.dest)?;
        if dest.starts_with(&source) || source.starts_with(&dest) {
            bail!("исходник и приёмник вложены друг в друга");
        }
        if let Some(home) = &cli.index_home {
            if normalized(home)?.starts_with(&dest) {
                bail!("--index-home находится внутри приёмника");
            }
        }
        let dest_existed = ensure_empty(&cli.dest)?;
        let home_existed = cli
            .index_home
            .as_deref()
            .map(ensure_empty)
            .transpose()?
            .unwrap_or(false);
        let sha = if cli.worktree {
            let branch = cli
                .branch
                .as_deref()
                .context("для --worktree нужен --branch")?;
            if !git::success(&source, &["check-ref-format", "--branch", branch])? {
                bail!("недопустимое имя ветки: {branch}");
            }
            if git::success(
                &source,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
            )? {
                bail!("ветка {branch} уже существует");
            }
            Some(git::commit(
                &source,
                cli.commit.as_deref().unwrap_or("HEAD"),
            )?)
        } else {
            None
        };
        Ok((source, dest_existed, home_existed, sha))
    })();
    report.timings_ms.checks = check_started.elapsed().as_millis();
    let (src, dest_existed, home_existed, sha) = match checked {
        Ok(value) => value,
        Err(error) => {
            let message = format!("{error:#}");
            let kind = if message.contains("не является рабочим деревом Git")
                || message.contains("подкаталог рабочего дерева")
            {
                ExitKind::Checks
            } else if message.contains("существует и не пуст") {
                ExitKind::Occupied
            } else {
                ExitKind::Args
            };
            return Err(Failure::new(
                kind,
                if matches!(kind, ExitKind::Checks | ExitKind::Occupied) {
                    "checks"
                } else {
                    "args"
                },
                error,
                false,
            ));
        }
    };
    report.branch = cli.branch.clone();
    report.commit = sha.clone();
    let mut branch_created = false;

    let copy_started = Instant::now();
    eprintln!("Копирование файлов");
    let copied = (|| -> Result<()> {
        if cli.worktree {
            report.eol_raw = git::eol_raw(&src)?;
            git::add_worktree(
                &src,
                &cli.dest,
                cli.branch.as_deref().expect("ветка проверена"),
                sha.as_deref().expect("коммит проверен"),
                report.eol_raw,
            )?;
            branch_created = true;
            for rel in git::paths(&cli.dest, &["ls-files", "-z"])? {
                if fs::symlink_metadata(cli.dest.join(rel))
                    .is_ok_and(|meta| meta.file_type().is_file())
                {
                    report.files_copied += 1;
                } else {
                    report.files_skipped += 1;
                }
            }
        } else {
            fs::create_dir_all(&cli.dest)?;
            files::copy_tracked(&src, &cli.dest, report)?;
        }
        report.index_config_copied = files::copy_index_config(&src, &cli.dest)?;
        Ok(())
    })();
    report.timings_ms.copy = copy_started.elapsed().as_millis();
    if let Err(error) = copied {
        if cli.worktree && !branch_created {
            branch_created = git::success(
                &src,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{}", cli.branch.as_deref().unwrap_or_default()),
                ],
            )
            .unwrap_or(false);
        }
        report.rollback = Some(rollback(
            cli,
            &src,
            dest_existed,
            home_existed,
            branch_created,
        ));
        return Err(Failure::new(ExitKind::Copy, "copy", error, true));
    }

    if let Some(sha) = &sha {
        let step = Instant::now();
        eprintln!("Выравнивание времени файлов");
        let result = files::align(&src, &cli.dest, sha, report);
        report.timings_ms.align = step.elapsed().as_millis();
        if let Err(error) = result {
            report.rollback = Some(rollback(
                cli,
                &src,
                dest_existed,
                home_existed,
                branch_created,
            ));
            return Err(Failure::new(ExitKind::Database, "align", error, true));
        }
    }

    let step = Instant::now();
    eprintln!("Перенос базы индекса");
    let transferred = database::transfer(&src, &cli.dest, report);
    report.timings_ms.backup = step.elapsed().as_millis();
    let transferred = match transferred {
        Ok(value) => value,
        Err(error) => {
            report.rollback = Some(rollback(
                cli,
                &src,
                dest_existed,
                home_existed,
                branch_created,
            ));
            return Err(Failure::new(ExitKind::Database, "backup", error, true));
        }
    };
    if transferred {
        let step = Instant::now();
        eprintln!("Очистка базы копии");
        let result = database::cleanup(&cli.dest, cli.language.as_deref(), report);
        report.timings_ms.cleanup = step.elapsed().as_millis();
        if let Err(error) = result {
            report.rollback = Some(rollback(
                cli,
                &src,
                dest_existed,
                home_existed,
                branch_created,
            ));
            return Err(Failure::new(ExitKind::Database, "cleanup", error, true));
        }
    }
    if let Some(home) = &cli.index_home {
        let step = Instant::now();
        eprintln!("Запись конфигурации");
        let result = configs::write(
            home,
            &cli.dest,
            cli.port.expect("порт проверен"),
            cli.alias.as_deref().expect("алиас проверен"),
            cli.language.as_deref(),
        );
        report.timings_ms.configs = step.elapsed().as_millis();
        if let Err(error) = result {
            report.rollback = Some(rollback(
                cli,
                &src,
                dest_existed,
                home_existed,
                branch_created,
            ));
            return Err(Failure::new(ExitKind::Copy, "configs", error, true));
        }
        report.configs_written = true;
    }
    report.timings_ms.total = started.elapsed().as_millis();
    report.ok = true;
    Ok(())
}

fn main() {
    let started = Instant::now();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if error.kind() == ErrorKind::DisplayHelp
                || error.kind() == ErrorKind::DisplayVersion =>
        {
            print!("{error}");
            return;
        }
        Err(error) => {
            let message = error.to_string();
            eprintln!("{message}");
            let mut report = Report::new(String::new(), String::new(), "copy", None);
            report.error = Some(ErrorInfo {
                stage: "args".into(),
                message,
            });
            println!(
                "{}",
                serde_json::to_string(&report).expect("отчёт сериализуется")
            );
            std::process::exit(ExitKind::Args as i32);
        }
    };
    let mut report = Report::new(
        cli.source.display().to_string(),
        cli.dest.display().to_string(),
        if cli.worktree { "worktree" } else { "copy" },
        cli.index_home
            .as_ref()
            .map(|path| path.display().to_string()),
    );
    let invalid = cli
        .alias
        .as_deref()
        .filter(|label| !valid_label(label))
        .or_else(|| cli.language.as_deref().filter(|label| !valid_label(label)));
    let result = if let Some(label) = invalid {
        Err(Failure::new(
            ExitKind::Args,
            "args",
            anyhow::anyhow!("недопустимый алиас или язык: {label}"),
            false,
        ))
    } else {
        execute(&cli, &mut report)
    };
    let code = match result {
        Ok(()) => ExitKind::Ok,
        Err(failure) => {
            let message = format!("{:#}", failure.error);
            eprintln!("{message}");
            report.error = Some(ErrorInfo {
                stage: failure.stage.into(),
                message,
            });
            let _ = failure.rollback;
            failure.kind
        }
    };
    report.timings_ms.total = started.elapsed().as_millis();
    println!(
        "{}",
        serde_json::to_string(&report).expect("отчёт сериализуется")
    );
    std::process::exit(code as i32);
}
