use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub name: String,
    pub csproj_path: String,
    pub root_dir: String,
    pub layer: String,
    pub is_external: bool,
    pub is_test: bool,
    pub refs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct DiscoveredFile {
    pub project_name: String,
    pub abs_path: String,
    pub rel_path: String,
    pub lang: &'static str,
    pub mtime_ns: i64,
    pub size_bytes: i64,
}

#[derive(Clone, Debug)]
pub struct ExtractedType {
    pub namespace: String,
    pub name: String,
    pub full_name: String,
    pub kind: String,
    pub accessibility: String,
    pub is_partial: bool,
    pub base_types: Vec<String>,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug)]
pub struct ExtractedMember {
    pub owner_type: String,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub return_type: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug)]
pub struct ExtractedCall {
    pub caller_type: String,
    pub caller_member: String,
    pub callee_name: String,
    pub receiver_hint: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct ExtractedTypeRef {
    pub source_type: String,
    pub target_type: String,
    pub ref_kind: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct ExtractedDiReg {
    pub lifetime: String,
    pub service_type: String,
    pub impl_type: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct ExtractedMessengerLink {
    pub owner_type: String,
    pub direction: String,
    pub message_type: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct ExtractedFileData {
    pub file: DiscoveredFile,
    pub content_hash: String,
    pub line_count: usize,
    pub types: Vec<ExtractedType>,
    pub members: Vec<ExtractedMember>,
    pub calls: Vec<ExtractedCall>,
    pub type_refs: Vec<ExtractedTypeRef>,
    pub di_regs: Vec<ExtractedDiReg>,
    pub messenger_links: Vec<ExtractedMessengerLink>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildSummary {
    pub repo_root: String,
    pub db_path: String,
    pub elapsed_ms: u128,
    pub total_projects: usize,
    pub external_projects: usize,
    pub total_files: usize,
    pub parsed_files: usize,
    pub skipped_unchanged: usize,
    pub removed_files: usize,
    pub total_types: usize,
    pub total_calls: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialFileSpan {
    pub project: String,
    pub file: String,
    pub line_count: usize,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialCluster {
    pub full_name: String,
    pub kind: String,
    pub file_count: usize,
    pub total_lines: usize,
    pub projects: Vec<String>,
    pub files: Vec<PartialFileSpan>,
}
