use std::fs;
use std::path::Path;

use code_clone_xray::cli::{CliArgs, OutputFormat};
use code_clone_xray::grep::context::resolve_enclosing_symbols;
use code_clone_xray::indexer::{
    build_or_refresh_index, open_db,
    slice_query::query_feature_slice,
    sqlite_query::{query_deps, query_search, query_status},
};
use code_clone_xray::normalize::parse_files_parallel;
use code_clone_xray::walk::{collect_source_files, SourceLang};

fn write_sample(base: &Path, rel: &str, body: &str) {
    let p = base.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).expect("create sample parent");
    }
    fs::write(&p, body).expect("write sample file");
}

#[test]
fn indexes_polyglot_workspace_across_rust_ts_svelte_python_and_csharp() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // Rust crate
    write_sample(
        root,
        "crates/core_engine/Cargo.toml",
        "[package]\nname = \"core_engine\"\nversion = \"0.1.0\"\n",
    );
    write_sample(
        root,
        "crates/core_engine/src/service.rs",
        r#"
use crate::model::OrderRequest;

pub trait Processor {
    fn process(&self, req: &OrderRequest);
}

pub struct OrderProcessor {
    pub retries: u32,
}

impl Processor for OrderProcessor {
    fn process(&self, req: &OrderRequest) {
        self.emit(req);
    }
}

impl OrderProcessor {
    pub fn emit(&self, _req: &OrderRequest) {}
}
"#,
    );

    // Web package (TS + Svelte)
    write_sample(
        root,
        "web/package.json",
        "{\n  \"name\": \"web-dashboard\",\n  \"version\": \"0.1.0\"\n}\n",
    );
    write_sample(
        root,
        "web/src/api.ts",
        r#"
export interface OrderPayload {
    id: string;
}

export class OrderApiClient {
    async submitOrder(payload: OrderPayload): Promise<void> {
        await fetch('/api/orders');
    }
}
"#,
    );
    write_sample(
        root,
        "web/src/OrderPanel.svelte",
        r#"<script lang="ts">
  import { OrderApiClient } from './api';
  let { title } = $props();
  const client = new OrderApiClient();
  function handleSubmit() {
    client.submitOrder({ id: '1' });
  }
</script>
<div class="panel">{title}</div>
"#,
    );

    // Python worker
    write_sample(
        root,
        "py_worker/pyproject.toml",
        "[project]\nname = \"py-worker\"\nversion = \"0.1.0\"\n",
    );
    write_sample(
        root,
        "py_worker/worker.py",
        r#"
from fastapi import Depends

class OrderWorker:
    def run_job(self, req: OrderRequest, svc=Depends(get_store)) -> bool:
        return True
"#,
    );

    let summary = build_or_refresh_index(root, None, true, false).expect("build polyglot index");
    assert_eq!(summary.total_projects, 3);
    assert_eq!(summary.total_files, 4);

    let db_path = root.join(".xray").join("index.db");
    let conn = open_db(&db_path).expect("open built db");
    let status = query_status(&conn).expect("status");
    assert_eq!(status.projects, 3);
    assert_eq!(status.files, 4);
    assert!(
        status.types >= 4,
        "expected >= 4 types, got {}",
        status.types
    );
    assert!(status.members >= 4);
    assert!(status.type_refs >= 2);
    assert!(status.di_registrations >= 1);

    let rs_hits = query_search(&conn, "OrderProcessor", None, 20).expect("search rs");
    assert!(rs_hits.iter().any(|h| h.kind == "struct"));

    let svelte_hits = query_search(&conn, "handleSubmit", None, 20).expect("search svelte");
    assert!(svelte_hits.iter().any(|h| h.owner.contains("OrderPanel")));

    let deps = query_deps(&conn, "OrderProcessor").expect("deps");
    assert!(
        deps.type_refs_outgoing
            .iter()
            .any(|r| r.target_type == "Processor" && r.ref_kind == "implements"),
        "expected trait impl edge for OrderProcessor (found {} outgoing refs)",
        deps.type_refs_outgoing.len()
    );

    let slice = query_feature_slice(&conn, "OrderApiClient", 2, 20).expect("slice");
    assert!(
        slice
            .files_to_read
            .iter()
            .any(|f| f.file.ends_with("OrderPanel.svelte")),
        "expected OrderPanel.svelte via import hop: {:?}",
        slice
            .files_to_read
            .iter()
            .map(|f| &f.file)
            .collect::<Vec<_>>()
    );
}

#[test]
fn resolves_svelte_grep_enclosing_symbols_and_structural_tokens() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let svelte_path = tmp.path().join("SensorCard.svelte");
    let code = r#"<!-- comment with <script> fake -->
<script lang="ts">
  export class Formatter {
    formatValue(v: number): string {
      return v.toFixed(2);
    }
  }
  function refreshSensor() {
    return 42;
  }
</script>
<section>
  <span>{refreshSensor()}</span>
</section>
"#;
    fs::write(&svelte_path, code).expect("write svelte");

    let info = resolve_enclosing_symbols(&svelte_path, code.as_bytes(), &[5, 9, 13]);
    assert_eq!(
        info.symbols.get(&5).map(String::as_str),
        Some("SensorCard.Formatter.formatValue")
    );
    assert_eq!(
        info.symbols.get(&9).map(String::as_str),
        Some("SensorCard.refreshSensor")
    );
    assert_eq!(
        info.symbols.get(&13).map(String::as_str),
        Some("SensorCard")
    );

    let args = CliArgs {
        path: vec![tmp.path().to_path_buf()],
        min_lines: 3,
        min_similarity: 80.0,
        window: 10,
        extensions: "svelte".into(),
        ignore: String::new(),
        include_tests: true,
        top: 10,
        format: OutputFormat::Json,
        output: None,
    };
    let files = collect_source_files(&args);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].lang, SourceLang::Svelte);
    let parsed = parse_files_parallel(&files);
    assert_eq!(parsed.len(), 1);
    assert!(!parsed[0].tokens.is_empty());
}

#[test]
fn standalone_python_repo_without_manifest_indexes_cleanly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_sample(
        tmp.path(),
        "helper.py",
        "def compute_checksum(data: bytes) -> int:\n    return len(data)\n",
    );
    let summary = build_or_refresh_index(tmp.path(), None, true, false).expect("index py");
    assert_eq!(summary.total_projects, 1);
    assert_eq!(summary.total_files, 1);
    let conn = open_db(&tmp.path().join(".xray").join("index.db")).expect("db");
    let hits = query_search(&conn, "compute_checksum", Some("member"), 10).expect("search");
    assert_eq!(hits.len(), 1);
}

#[test]
fn extracts_polyglot_state_transitions_for_rust_svelte_ts_and_python() {
    use code_clone_xray::indexer::sqlite_query::query_states;

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    write_sample(
        root,
        "worker.rs",
        r#"
pub enum WorkerState { Idle, Running, Paused, Alarm }
pub struct Worker { pub state: WorkerState }
impl Worker {
    pub fn new() -> Self { Self { state: WorkerState::Idle } }
    pub fn start(&mut self) {
        match self.state {
            WorkerState::Idle => { self.state = WorkerState::Running; }
            _ => {}
        }
    }
    pub fn trip(&mut self) {
        self.state = match self.state {
            WorkerState::Running | WorkerState::Paused => WorkerState::Alarm,
            _ => WorkerState::Idle,
        };
    }
}
"#,
    );
    write_sample(
        root,
        "Panel.svelte",
        r#"<script lang="ts">
let status = $state("Idle");
export function begin() {
    if (status == "Idle") {
        status = "Running";
    }
}
export function finish() {
    if (status == "Running") {
        status = "Done";
    }
}
</script>
<div>{status}</div>
"#,
    );
    write_sample(
        root,
        "controller.ts",
        r#"export class Controller {
    phase: string = "Boot";
    ready() {
        switch (this.phase) {
            case "Boot":
                this.phase = "Ready";
                break;
        }
    }
}
"#,
    );
    write_sample(
        root,
        "job.py",
        r#"class Job:
    def __init__(self):
        self.phase = "Queued"
    def run(self):
        if self.phase == "Queued":
            self.phase = "Active"
    def fail(self):
        self.phase = "Aborted"
"#,
    );

    let summary = build_or_refresh_index(root, None, true, false).expect("build index");
    assert!(
        summary.total_state_transitions >= 9,
        "expected >= 9 state transitions, got {}",
        summary.total_state_transitions
    );

    let conn = open_db(&root.join(".xray").join("index.db")).expect("open db");
    let status = query_status(&conn).expect("status");
    assert!(status.state_transitions >= 9);

    let rs_states = query_states(&conn, Some("Worker")).expect("rs states");
    assert!(rs_states
        .transitions
        .iter()
        .any(|t| t.machine_name == "WorkerState"
            && t.from_state == "(init)"
            && t.to_state == "Idle"));
    assert!(rs_states
        .transitions
        .iter()
        .any(|t| t.machine_name == "WorkerState"
            && t.from_state == "Idle"
            && t.to_state == "Running"));
    assert!(rs_states
        .transitions
        .iter()
        .any(|t| t.machine_name == "WorkerState"
            && t.from_state == "Running"
            && t.to_state == "Alarm"));
    assert!(rs_states
        .orphan_terminal_states
        .contains(&"WorkerState.Alarm".to_string()));

    let svelte_states = query_states(&conn, Some("Panel.svelte")).expect("svelte states");
    assert!(svelte_states
        .transitions
        .iter()
        .any(|t| t.from_state == "(init)" && t.to_state == "Idle"));
    assert!(svelte_states
        .transitions
        .iter()
        .any(|t| t.from_state == "Idle" && t.to_state == "Running" && t.trigger_member == "begin"));

    let ts_states = query_states(&conn, Some("Controller")).expect("ts states");
    assert!(ts_states
        .transitions
        .iter()
        .any(|t| t.from_state == "(init)" && t.to_state == "Boot"));
    assert!(ts_states
        .transitions
        .iter()
        .any(|t| t.from_state == "Boot" && t.to_state == "Ready"));

    let py_states = query_states(&conn, Some("Job")).expect("py states");
    assert!(py_states
        .transitions
        .iter()
        .any(|t| t.from_state == "(init)" && t.to_state == "Queued"));
    assert!(py_states
        .transitions
        .iter()
        .any(|t| t.from_state == "Queued" && t.to_state == "Active"));
    assert!(py_states
        .orphan_terminal_states
        .iter()
        .any(|s| s.ends_with(".Aborted")));
}
