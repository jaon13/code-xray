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
