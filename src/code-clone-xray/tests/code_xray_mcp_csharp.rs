//! Self-hosted Rust coverage for CodeXrayMcp C# sources (tokenize + clone scan smoke).

use std::fs;
use std::path::{Path, PathBuf};

use code_clone_xray::cli::{CliArgs, OutputFormat};
use code_clone_xray::detect::{detect_clones, DetectConfig};
use code_clone_xray::normalize::parse_files_parallel;
use code_clone_xray::parse::csharp::tokenize_csharp;
use code_clone_xray::walk::collect_source_files;

fn code_xray_mcp_src_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../CodeXrayMcp")
        .canonicalize()
        .expect("../CodeXrayMcp should exist beside code-clone-xray")
}

fn collect_cs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).expect("read CodeXrayMcp src") {
        let entry = entry.expect("dirent");
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.eq_ignore_ascii_case("bin") || name.eq_ignore_ascii_case("obj") {
                continue;
            }
            out.extend(collect_cs_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("cs") {
            out.push(path);
        }
    }
    out
}

#[test]
fn tokenizes_all_code_xray_mcp_csharp_sources() {
    let root = code_xray_mcp_src_root();
    let files = collect_cs_files(&root);
    assert!(
        files.len() >= 6,
        "expected CodeXrayMcp .cs files under {}, got {}",
        root.display(),
        files.len()
    );

    for path in &files {
        let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let tokens = tokenize_csharp(&bytes);
        assert!(
            !tokens.is_empty(),
            "tokenize_csharp produced 0 tokens for {}",
            path.display()
        );
    }
}

#[test]
fn scan_code_xray_mcp_tree_produces_clone_report_contract() {
    let root = code_xray_mcp_src_root();
    let args = CliArgs {
        path: vec![root],
        min_lines: 8,
        min_similarity: 80.0,
        window: 25,
        extensions: "cs".into(),
        ignore: "bin,obj".into(),
        include_tests: true,
        top: 20,
        format: OutputFormat::Json,
        output: None,
    };
    let files = collect_source_files(&args);
    assert!(!files.is_empty(), "walk found no CodeXrayMcp sources");
    let parsed = parse_files_parallel(&files);
    assert!(!parsed.is_empty());

    let config = DetectConfig {
        paths: args.path.iter().map(|p| p.display().to_string()).collect(),
        min_lines: args.min_lines,
        min_similarity: args.min_similarity,
        window: args.window,
        top: args.top,
    };
    let report = detect_clones(&parsed, &config);
    assert_eq!(report.schema_version, "1.0");
    assert_eq!(report.tool, "structural-clone");
    assert!(report.statistics.total.files >= 1);
}
