pub mod cs_extractor;
pub mod cs_member_extractor;
pub mod cs_patterns;
pub mod cs_syntax_utils;
pub mod extract_utils;
pub mod manifest_discovery;
pub mod model;
pub mod path_utils;
pub mod project_resolver;
pub mod py_extractor;
pub mod query_models;
pub mod rs_extractor;
pub mod rs_links;
pub mod slice_query;
pub mod slice_types;
pub mod sqlite_query;
pub mod sqlite_schema;
pub mod sqlite_writer;
pub mod state_extractor;
pub mod state_guards;
pub mod ts_js_svelte_extractor;
pub mod web_links;
pub mod xaml_extractor;

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rusqlite::Connection;

use model::BuildSummary;
use path_utils::{clean_canonical, to_slash};
use project_resolver::resolve_workspace;
use sqlite_schema::{ensure_schema, SCHEMA_VERSION};
use sqlite_writer::write_incremental_delta;

pub fn default_db_path(repo_root: &Path) -> PathBuf {
    clean_canonical(repo_root).join(".xray").join("index.db")
}

pub fn default_lock_path(repo_root: &Path) -> PathBuf {
    clean_canonical(repo_root)
        .join(".xray")
        .join("index.build.lock")
}

struct BuildLockGuard {
    file: fs::File,
}

impl Drop for BuildLockGuard {
    fn drop(&mut self) {
        if self.file.unlock().is_err() {
            // Closing the underlying file handle releases the OS lock as well.
        }
    }
}

fn lock_path_for_db(target_db: &Path) -> PathBuf {
    let parent = target_db.parent().unwrap_or_else(|| Path::new("."));
    if target_db
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("index.db"))
    {
        parent.join("index.build.lock")
    } else {
        let stem = target_db
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("index");
        parent.join(format!("{stem}.build.lock"))
    }
}

fn acquire_build_lock(lock_path: &Path) -> Result<BuildLockGuard> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create lock directory {}", parent.display()))?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .with_context(|| format!("Failed to open build lock {}", lock_path.display()))?;
    file.lock()
        .with_context(|| format!("Failed to acquire OS build lock {}", lock_path.display()))?;
    Ok(BuildLockGuard { file })
}

pub fn open_db(db_path: &Path) -> Result<Connection> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create index directory {}", parent.display()))?;
    }
    let conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open SQLite index {}", db_path.display()))?;
    ensure_schema(&conn)?;
    Ok(conn)
}

pub fn build_or_refresh_index(
    repo_root: &Path,
    db_path: Option<&Path>,
    force: bool,
    include_external: bool,
) -> Result<BuildSummary> {
    let started = Instant::now();
    let canon_root = clean_canonical(repo_root);
    let target_db = db_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_db_path(&canon_root));

    let lock_path = lock_path_for_db(&target_db);
    let _build_lock = acquire_build_lock(&lock_path)?;

    let (projects, files) = resolve_workspace(&canon_root, include_external);
    let mut conn = open_db(&target_db)?;
    let existing_schema: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .ok();
    let effective_force = force || existing_schema.as_deref() != Some(SCHEMA_VERSION);
    let root_str = to_slash(&canon_root);
    let stats = write_incremental_delta(&mut conn, &root_str, &projects, &files, effective_force)?;

    let total_types: usize = conn
        .query_row("SELECT COUNT(*) FROM types", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
        .max(0) as usize;
    let total_calls: usize = conn
        .query_row("SELECT COUNT(*) FROM calls", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
        .max(0) as usize;
    let total_state_transitions: usize = conn
        .query_row("SELECT COUNT(*) FROM state_transitions", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_or(0)
        .max(0) as usize;
    let external_projects = projects.iter().filter(|p| p.is_external).count();

    Ok(BuildSummary {
        repo_root: root_str,
        db_path: to_slash(&target_db),
        elapsed_ms: started.elapsed().as_millis(),
        total_projects: projects.len(),
        external_projects,
        total_files: files.len(),
        parsed_files: stats.parsed_files,
        skipped_unchanged: stats.skipped_unchanged,
        removed_files: stats.removed_files,
        total_types,
        total_calls,
        total_state_transitions,
    })
}

pub fn ensure_fresh_db(
    repo_root: &Path,
    db_path: Option<&Path>,
    refresh: bool,
    include_external: bool,
) -> Result<Connection> {
    let canon_root = clean_canonical(repo_root);
    let target_db = db_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_db_path(&canon_root));

    if refresh || !target_db.is_file() {
        let _summary =
            build_or_refresh_index(&canon_root, Some(&target_db), false, include_external)?;
    }
    open_db(&target_db)
}
