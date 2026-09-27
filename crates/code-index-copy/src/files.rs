use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::Path;

use crate::{git, report::Report};

// `set_readonly(false)` вызывается только вне Unix (там это снятие атрибута
// «только для чтения»); на Unix права выставляются через `set_mode`.
#[allow(clippy::permissions_set_readonly_false)]
pub fn set_mtime(src: &Path, dest: &Path) -> Result<()> {
    let modified = fs::metadata(src)?.modified()?;
    let permissions = fs::metadata(dest)?.permissions();
    let readonly = permissions.readonly();
    if readonly {
        let mut writable = permissions.clone();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            writable.set_mode(writable.mode() | 0o200);
        }
        #[cfg(not(unix))]
        writable.set_readonly(false);
        fs::set_permissions(dest, writable)?;
    }
    let result = File::options()
        .write(true)
        .open(dest)
        .and_then(|file| file.set_modified(modified));
    if readonly {
        fs::set_permissions(dest, permissions)?;
    }
    result.with_context(|| format!("не удалось установить время {}", dest.display()))
}

pub fn copy_file(src: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, dest).with_context(|| format!("не удалось скопировать {}", src.display()))?;
    set_mtime(src, dest)
}

pub fn copy_tracked(src: &Path, dest: &Path, report: &mut Report) -> Result<()> {
    for rel in git::paths(src, &["ls-files", "-z"])? {
        let from = src.join(&rel);
        if fs::symlink_metadata(&from).is_ok_and(|m| m.file_type().is_file()) {
            copy_file(&from, &dest.join(&rel))?;
            report.files_copied += 1;
        } else {
            report.files_skipped += 1;
        }
    }
    Ok(())
}

pub fn copy_index_config(src: &Path, dest: &Path) -> Result<bool> {
    let from = src.join(".code-index/config.json");
    if from.is_file() {
        copy_file(&from, &dest.join(".code-index/config.json"))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn align(src: &Path, dest: &Path, sha: &str, report: &mut Report) -> Result<()> {
    let changed: HashSet<_> = git::paths(
        src,
        &["diff", "--name-only", "-z", "--no-renames", sha, "--"],
    )?
    .into_iter()
    .collect();
    for rel in git::paths(dest, &["ls-files", "-z"])? {
        if changed.contains(&rel) {
            report.not_aligned_modified += 1;
            continue;
        }
        let from = src.join(&rel);
        let to = dest.join(&rel);
        let Ok(from_meta) = fs::symlink_metadata(&from) else {
            report.not_aligned_missing += 1;
            continue;
        };
        let Ok(to_meta) = fs::symlink_metadata(&to) else {
            report.not_aligned_missing += 1;
            continue;
        };
        if !from_meta.file_type().is_file() || !to_meta.file_type().is_file() {
            report.not_aligned_missing += 1;
        } else if from_meta.len() != to_meta.len() {
            report.not_aligned_size += 1;
        } else {
            set_mtime(&from, &to)?;
            report.mtime_aligned += 1;
        }
    }
    Ok(())
}
