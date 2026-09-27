use anyhow::{Context, Result};
use bsl_extension::BslLanguageProcessor;
use code_index_core::extension::ProcessorRegistry;
use code_index_core::storage::Storage;
use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::report::Report;

pub fn transfer(src: &Path, dest: &Path, report: &mut Report) -> Result<bool> {
    let source = src.join(".code-index/index.db");
    if !source.exists() {
        report.db_note = Some("у исходника нет базы, индекс копии построится с нуля".into());
        return Ok(false);
    }
    let target = dest.join(".code-index/index.db");
    fs::create_dir_all(target.parent().expect("путь базы имеет родителя"))?;
    let source_conn = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("не удалось открыть базу {}", source.display()))?;
    source_conn.busy_timeout(Duration::from_secs(30))?;
    let mut target_conn = Connection::open(&target)?;
    {
        let backup = Backup::new(&source_conn, &mut target_conn)?;
        let mut done = false;
        for attempt in 0..=5 {
            match backup.step(-1)? {
                StepResult::Done => {
                    done = true;
                    break;
                }
                StepResult::Busy | StepResult::Locked if attempt < 5 => {
                    thread::sleep(Duration::from_secs(1));
                }
                StepResult::Busy | StepResult::Locked => {
                    anyhow::bail!("база источника занята после пяти повторов")
                }
                StepResult::More => anyhow::bail!("резервное копирование базы не завершилось"),
                _ => anyhow::bail!("неожиданный результат резервного копирования"),
            }
        }
        if !done {
            anyhow::bail!("резервное копирование базы не завершилось");
        }
    }
    report.db_transferred = true;
    Ok(true)
}

/// Удаляет из базы копии файлы, которых в копии нет, и файлы `stale` — в копии
/// другое содержимое, чем записано в базе; демон копии проиндексирует их заново.
pub fn cleanup(
    dest: &Path,
    language: Option<&str>,
    stale: &HashSet<String>,
    report: &mut Report,
) -> Result<()> {
    let path = dest.join(".code-index/index.db");
    let mut storage = Storage::open_file(&path)?;
    storage.set_secure_delete(true)?;
    let mut stale_found = 0;
    let victims: Vec<_> = storage
        .get_all_files()?
        .into_iter()
        .filter(|file| {
            let absent = !fs::symlink_metadata(dest.join(&file.path))
                .is_ok_and(|meta| meta.file_type().is_file());
            if !absent && stale.contains(&file.path) {
                stale_found += 1;
                return true;
            }
            absent
        })
        .collect();
    report.db_paths_stale = stale_found;
    if !victims.is_empty() {
        storage.begin_batch()?;
        let removal = victims
            .iter()
            .try_for_each(|file| storage.delete_file(file.id.context("запись без id")?));
        if let Err(error) = removal {
            storage.rollback_batch()?;
            return Err(error);
        }
        if let Err(error) = storage.commit_batch() {
            storage.rollback_batch()?;
            return Err(error);
        }
        report.db_paths_removed = victims.len() - stale_found;
        let mut registry = ProcessorRegistry::new();
        registry.register(Arc::new(BslLanguageProcessor::new()));
        if let Some(processor) = registry.resolve(language, dest) {
            if processor.extras_present(&storage) {
                let deleted: Vec<PathBuf> =
                    victims.iter().map(|file| dest.join(&file.path)).collect();
                processor.index_extras_for_files(dest, &mut storage, &[], &deleted)?;
            }
        }
        report.fts_optimized = storage.optimize_fts_tables()?;
    }
    let (busy, _, _) = storage.checkpoint_truncate()?;
    if busy != 0 {
        anyhow::bail!("не удалось усечь WAL после очистки базы");
    }
    Ok(())
}
