use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{ContextLine, GrepReport, GrepSummary};
use crate::indexer::path_utils::fnv1a_128_hex;

static RUN_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewMatchEntry {
    pub id: String,
    pub file: String,
    pub abs_path: String,
    pub line: usize,
    pub column: usize,
    pub match_text: String,
    #[serde(rename = "enclosing_symbol", alias = "enclosingSymbol")]
    pub enclosing_symbol: Option<String>,
    #[serde(rename = "project_name", alias = "projectName")]
    pub project_name: String,
    pub is_external: bool,
    pub category: String,
    pub snippet: String,
    pub context_before: Vec<ContextLine>,
    pub context_after: Vec<ContextLine>,
    pub status: String,
    pub disposition: String,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewManifest {
    pub schema_version: String,
    pub run_id: String,
    #[serde(alias = "createdAtUtc")]
    pub created_at: String,
    pub repo_root: String,
    pub pattern: String,
    pub is_regex: bool,
    pub case_insensitive: bool,
    pub extensions: Vec<String>,
    pub include_external: bool,
    pub total_files_scanned: usize,
    pub total_matches: usize,
    pub truncated: bool,
    pub summary: GrepSummary,
    pub matches: Vec<ReviewMatchEntry>,
}

pub fn utc_timestamps_now() -> (String, String) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc_timestamp(secs)
}

pub fn format_utc_timestamp(epoch_secs: u64) -> (String, String) {
    let days = (epoch_secs / 86_400) as i64;
    let sod = epoch_secs % 86_400;
    let hour = sod / 3600;
    let min = (sod % 3600) / 60;
    let sec = sod % 60;

    // Howard Hinnant civil-from-days algorithm
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    let compact = format!("{year:04}{m:02}{d:02}_{hour:02}{min:02}{sec:02}");
    let iso = format!("{year:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z");
    (compact, iso)
}

pub fn new_run_id(pattern: &str, compact_ts: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = RUN_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let seed = format!("{compact_ts}|{pid}|{nanos}|{seq}|{pattern}");
    let short = &fnv1a_128_hex(seed.as_bytes())[..8];
    format!("grep_{compact_ts}_{short}")
}

pub fn save_review_manifest(
    repo_root: &str,
    review_dir_override: Option<&Path>,
    report: &GrepReport,
) -> Result<(PathBuf, PathBuf)> {
    let dir = review_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(repo_root).join(".xray").join("reviews"));
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create review directory {}", dir.display()))?;

    let matches: Vec<ReviewMatchEntry> = report
        .groups
        .iter()
        .flat_map(|g| g.matches.iter())
        .map(|m| ReviewMatchEntry {
            id: m.id.clone(),
            file: m.file.clone(),
            abs_path: m.abs_path.clone(),
            line: m.line,
            column: m.column,
            match_text: m.match_text.clone(),
            enclosing_symbol: m.enclosing_symbol.clone(),
            project_name: m.project_name.clone(),
            is_external: m.is_external,
            category: m.category.clone(),
            snippet: m.snippet.clone(),
            context_before: m.context_before.clone(),
            context_after: m.context_after.clone(),
            status: m.status.clone(),
            disposition: m.disposition.clone(),
            note: String::new(),
        })
        .collect();

    let mut run_id = report.run_id.clone();
    let mut attempt = 0u32;
    loop {
        let json_path = dir.join(format!("{run_id}.json"));
        let md_path = dir.join(format!("{run_id}.md"));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&json_path)
        {
            Ok(mut file) => {
                let manifest = ReviewManifest {
                    schema_version: report.schema_version.clone(),
                    run_id: run_id.clone(),
                    created_at: report.created_at.clone(),
                    repo_root: report.repo_root.clone(),
                    pattern: report.pattern.clone(),
                    is_regex: report.is_regex,
                    case_insensitive: report.case_insensitive,
                    extensions: report.extensions.clone(),
                    include_external: report.include_external,
                    total_files_scanned: report.total_files_scanned,
                    total_matches: matches.len(),
                    truncated: report.truncated,
                    summary: report.summary.clone(),
                    matches,
                };
                let json_text = serde_json::to_string_pretty(&manifest)?;
                file.write_all(json_text.as_bytes())?;
                file.write_all(b"\n")?;

                let mut report_copy = report.clone();
                report_copy.run_id = run_id;
                report_copy.review_manifest =
                    Some(crate::indexer::path_utils::to_slash(&json_path));
                report_copy.review_markdown = Some(crate::indexer::path_utils::to_slash(&md_path));
                let md_text = render_markdown_report(&report_copy);
                fs::write(&md_path, md_text).with_context(|| {
                    format!("Failed to write review markdown {}", md_path.display())
                })?;

                return Ok((json_path, md_path));
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists && attempt < 16 => {
                attempt += 1;
                let (compact, _) = utc_timestamps_now();
                run_id = new_run_id(&report.pattern, &compact);
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("Failed to create review manifest {}", json_path.display())
                });
            }
        }
    }
}

pub fn render_markdown_report(report: &GrepReport) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("# Grep Review Checklist (`{}`)", report.run_id));
    lines.push(format!(
        "- Pattern: `{}` (regex={}, caseInsensitive={})",
        report.pattern, report.is_regex, report.case_insensitive
    ));
    lines.push(format!("- Repo: `{}`", report.repo_root));
    lines.push(format!(
        "- Scanned: {} files | Matched: {} files, {} matches{}",
        report.total_files_scanned,
        report.returned_files,
        report.total_matches,
        if report.truncated { " (truncated)" } else { "" }
    ));
    if let Some(manifest) = &report.review_manifest {
        lines.push(format!("- Manifest: `{manifest}`"));
    }
    lines.push(String::new());

    for g in &report.groups {
        let ext_tag = if g.is_external { " [external]" } else { "" };
        lines.push(format!(
            "## `{}` ({}{}, category={})",
            g.file, g.project_name, ext_tag, g.category
        ));
        for m in &g.matches {
            let sym = m.enclosing_symbol.as_deref().unwrap_or("-");
            lines.push(format!(
                "- [ ] `{}` L{}:{} `{}` [{}] ({}) — `{}`",
                m.id,
                m.line,
                m.column,
                sym,
                m.category,
                m.disposition,
                m.snippet.trim()
            ));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_utc_timestamp() {
        let (compact, iso) = format_utc_timestamp(1_700_000_000);
        assert_eq!(compact, "20231114_221320");
        assert_eq!(iso, "2023-11-14T22:13:20Z");
    }
}
