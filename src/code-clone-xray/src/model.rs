use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "1.0";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneReport {
    pub schema_version: String,
    pub tool: String,
    pub config: ReportConfig,
    pub statistics: Statistics,
    pub duplicates: Vec<DuplicatePair>,
    pub clone_groups: Vec<CloneGroup>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportConfig {
    pub paths: Vec<String>,
    pub min_lines: usize,
    pub min_similarity: f64,
    pub window: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Statistics {
    pub total: TotalStats,
    pub formats: FormatStatsMap,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TotalStats {
    pub files: usize,
    pub lines: usize,
    pub clone_groups: usize,
    pub clone_pairs: usize,
    pub duplicated_lines: usize,
    pub percentage: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatStatsMap {
    pub csharp: FormatStats,
    pub xaml: FormatStats,
    #[serde(default)]
    pub rust: FormatStats,
    #[serde(default, skip_serializing_if = "FormatStats::is_zero")]
    pub typescript: FormatStats,
    #[serde(default, skip_serializing_if = "FormatStats::is_zero")]
    pub javascript: FormatStats,
    #[serde(default, skip_serializing_if = "FormatStats::is_zero")]
    pub python: FormatStats,
    #[serde(default, skip_serializing_if = "FormatStats::is_zero")]
    pub svelte: FormatStats,
    #[serde(default, skip_serializing_if = "FormatStats::is_zero")]
    pub html: FormatStats,
    #[serde(default, skip_serializing_if = "FormatStats::is_zero")]
    pub css: FormatStats,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatStats {
    pub files: usize,
    pub clone_pairs: usize,
    pub duplicated_lines: usize,
    pub percentage: f64,
}

impl FormatStats {
    pub fn is_zero(&self) -> bool {
        self.files == 0 && self.clone_pairs == 0 && self.duplicated_lines == 0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatePair {
    pub id: String,
    pub format: String,
    pub similarity: f64,
    pub lines: usize,
    pub tokens: usize,
    pub first_file: FileSpan,
    pub second_file: FileSpan,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSpan {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneGroup {
    pub fingerprint: String,
    pub format: String,
    pub similarity: f64,
    pub instance_count: usize,
    pub occurrences: Vec<Occurrence>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Occurrence {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_hint: Option<String>,
}
