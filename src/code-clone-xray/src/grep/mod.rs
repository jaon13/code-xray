pub mod context;
mod context_langs;
mod discovery;
pub mod engine;
pub mod review;

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrepFormat {
    Json,
    Markdown,
}

#[derive(Args, Clone, Debug)]
pub struct GrepArgs {
    /// Repository or solution root directory to search
    #[arg(short = 'r', long, default_value = ".")]
    pub root: PathBuf,

    /// Pattern to search for (literal substring by default, or regex with --regex)
    #[arg(short = 'p', long, visible_alias = "query")]
    pub pattern: String,

    /// Treat pattern as a regular expression
    #[arg(long, default_value_t = false)]
    pub regex: bool,

    /// Case-insensitive matching
    #[arg(short = 'i', long, default_value_t = false)]
    pub case_insensitive: bool,

    /// Comma-separated file extensions to include (e.g. cs,xaml,rs,json,md)
    #[arg(long)]
    pub ext: Option<String>,

    /// Include recursive external .csproj ProjectReferences outside --root
    #[arg(long, default_value_t = false)]
    pub include_external: bool,

    /// Number of context lines before and after each matching line
    #[arg(short = 'C', long, default_value_t = 2)]
    pub context: usize,

    /// Maximum matched files to include in results (0 = unlimited)
    #[arg(long, default_value_t = 100)]
    pub max_files: usize,

    /// Maximum total matches to include in results (0 = unlimited)
    #[arg(long, default_value_t = 500)]
    pub max_matches: usize,

    /// Output format (json or markdown)
    #[arg(long, value_enum, default_value_t = GrepFormat::Json)]
    pub format: GrepFormat,

    /// Write an isolated review manifest under <repo>/.xray/reviews/grep_<ts>_<id>.json (.md)
    #[arg(long, default_value_t = false)]
    pub save_review: bool,

    /// Optional override directory for saved review manifests
    #[arg(long)]
    pub review_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextLine {
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepMatch {
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepFileGroup {
    pub project_name: String,
    pub file: String,
    pub abs_path: String,
    pub is_external: bool,
    pub category: String,
    pub match_count: usize,
    pub matches: Vec<GrepMatch>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepSummary {
    pub by_category: BTreeMap<String, usize>,
    pub by_project: BTreeMap<String, usize>,
    pub by_disposition: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepReport {
    pub schema_version: String,
    pub tool: String,
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
    pub total_matched_files: usize,
    pub returned_files: usize,
    pub total_matches_found: usize,
    pub total_matches: usize,
    pub truncated: bool,
    pub elapsed_ms: u128,
    pub review_manifest: Option<String>,
    pub review_markdown: Option<String>,
    pub summary: GrepSummary,
    pub groups: Vec<GrepFileGroup>,
}

/// Runs Review-First grep and optionally writes an isolated `.xray/reviews/` manifest.
pub fn run_grep_command(args: &GrepArgs) -> Result<String> {
    let mut report = engine::execute_grep(args)?;
    if args.save_review {
        let (json_path, md_path) =
            review::save_review_manifest(&report.repo_root, args.review_dir.as_deref(), &report)?;
        report.review_manifest = Some(crate::indexer::path_utils::to_slash(&json_path));
        report.review_markdown = Some(crate::indexer::path_utils::to_slash(&md_path));
    }

    match args.format {
        GrepFormat::Json => Ok(serde_json::to_string_pretty(&report)?),
        GrepFormat::Markdown => Ok(review::render_markdown_report(&report)),
    }
}
