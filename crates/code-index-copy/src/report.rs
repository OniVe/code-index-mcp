use serde::Serialize;

#[derive(Clone, Copy)]
pub enum ExitKind {
    Ok = 0,
    Args = 2,
    Checks = 3,
    Occupied = 4,
    Copy = 5,
    Database = 6,
}

#[derive(Default, Serialize)]
pub struct Timings {
    pub checks: u128,
    pub copy: u128,
    pub align: u128,
    pub backup: u128,
    pub cleanup: u128,
    pub configs: u128,
    pub total: u128,
}

#[derive(Serialize)]
pub struct ErrorInfo {
    pub stage: String,
    pub message: String,
}

#[derive(Default, Serialize)]
pub struct Rollback {
    pub files_removed: bool,
    pub index_home_removed: bool,
    pub branch_deleted: bool,
    pub branch_note: Option<String>,
}

#[derive(Serialize)]
pub struct Report {
    pub ok: bool,
    pub source: String,
    pub dest: String,
    pub mode: &'static str,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub eol_raw: bool,
    pub files_copied: usize,
    pub files_skipped: usize,
    pub mtime_aligned: usize,
    pub not_aligned_size: usize,
    pub not_aligned_modified: usize,
    pub not_aligned_missing: usize,
    pub index_config_copied: bool,
    pub db_transferred: bool,
    pub db_note: Option<String>,
    pub db_paths_removed: usize,
    pub db_paths_stale: usize,
    pub fts_optimized: usize,
    pub configs_written: bool,
    pub index_home: Option<String>,
    pub timings_ms: Timings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback: Option<Rollback>,
}

impl Report {
    pub fn new(
        source: String,
        dest: String,
        mode: &'static str,
        index_home: Option<String>,
    ) -> Self {
        Self {
            ok: false,
            source,
            dest,
            mode,
            branch: None,
            commit: None,
            eol_raw: false,
            files_copied: 0,
            files_skipped: 0,
            mtime_aligned: 0,
            not_aligned_size: 0,
            not_aligned_modified: 0,
            not_aligned_missing: 0,
            index_config_copied: false,
            db_transferred: false,
            db_note: None,
            db_paths_removed: 0,
            db_paths_stale: 0,
            fts_optimized: 0,
            configs_written: false,
            index_home,
            timings_ms: Timings::default(),
            error: None,
            rollback: None,
        }
    }
}
