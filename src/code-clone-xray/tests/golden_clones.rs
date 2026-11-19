use std::path::PathBuf;

use code_clone_xray::cli::{CliArgs, OutputFormat};
use code_clone_xray::detect::{detect_clones, DetectConfig};
use code_clone_xray::normalize::parse_files_parallel;
use code_clone_xray::walk::collect_source_files;

fn testdata_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

fn scan(path: PathBuf, window: usize, min_lines: usize) -> code_clone_xray::model::CloneReport {
    let args = CliArgs {
        path: vec![path],
        min_lines,
        min_similarity: 80.0,
        window,
        extensions: "cs,xaml".into(),
        ignore: String::new(),
        include_tests: true,
        top: 50,
        format: OutputFormat::Json,
        output: None,
    };
    let files = collect_source_files(&args);
    let parsed = parse_files_parallel(&files);
    let config = DetectConfig {
        paths: args.path.iter().map(|p| p.display().to_string()).collect(),
        min_lines: args.min_lines,
        min_similarity: args.min_similarity,
        window: args.window,
        top: args.top,
    };
    detect_clones(&parsed, &config)
}

#[test]
fn finds_csharp_structural_clones() {
    let report = scan(testdata_root().join("cs"), 25, 8);
    assert!(
        !report.duplicates.is_empty(),
        "expected C# duplicates, got none (files={}, lines={})",
        report.statistics.total.files,
        report.statistics.total.lines
    );
    assert!(
        report.duplicates.iter().any(|d| d.format == "csharp"),
        "expected csharp format in duplicates: {:?}",
        report.duplicates
    );
    assert_eq!(report.schema_version, "1.0");
    assert_eq!(report.tool, "structural-clone");
}

#[test]
fn finds_xaml_structural_clones() {
    let report = scan(testdata_root().join("xaml"), 20, 6);
    assert!(
        !report.duplicates.is_empty(),
        "expected XAML duplicates, got none (files={}, tokens stats={:?})",
        report.statistics.total.files,
        report.statistics.formats.xaml
    );
    assert!(report.duplicates.iter().any(|d| d.format == "xaml"));
}

#[test]
fn schema_contract_fields_present() {
    let report = scan(testdata_root(), 25, 6);
    let json = serde_json::to_value(&report).expect("serialize");
    assert_eq!(json["schemaVersion"], "1.0");
    assert!(json["statistics"]["total"]["files"].as_u64().unwrap() >= 2);
    assert!(report.statistics.total.percentage <= 100.0);
    assert!(json.get("duplicates").is_some());
    assert!(json.get("cloneGroups").is_some());
}

#[test]
fn detects_type3_near_miss_csharp_clone_with_similarity_below_100() {
    use code_clone_xray::normalize::ParsedSourceFile;
    use code_clone_xray::parse::csharp::tokenize_csharp;
    use code_clone_xray::walk::SourceLang;

    let cs_a = br#"
namespace Demo.A;
public sealed class ProcessorA {
    public int Run(int[] values) {
        int acc = 0;
        for (int i = 0; i < values.Length; i++) {
            int v = values[i];
            if (v > 0) {
                acc += v * 2;
            }
        }
        if (acc > 100) {
            acc = 100;
        }
        return acc;
    }
}
"#;
    let cs_b = br#"
namespace Demo.B;
public sealed class ProcessorB {
    public int Execute(int[] nums) {
        int sum = 0;
        for (int k = 0; k < nums.Length; k++) {
            int n = nums[k];
            if (n > 0 && n < 500) {
                sum += n * 3;
            }
        }
        if (sum > 250) {
            sum = 250;
        }
        return sum;
    }
}
"#;
    let parsed = vec![
        ParsedSourceFile {
            id: 0,
            display_path: "A.cs".into(),
            project: "ProjA".into(),
            lang: SourceLang::Cs,
            total_lines: 18,
            tokens: tokenize_csharp(cs_a),
        },
        ParsedSourceFile {
            id: 1,
            display_path: "B.cs".into(),
            project: "ProjB".into(),
            lang: SourceLang::Cs,
            total_lines: 18,
            tokens: tokenize_csharp(cs_b),
        },
    ];
    let cfg = DetectConfig {
        paths: vec![".".into()],
        min_lines: 8,
        min_similarity: 80.0,
        window: 20,
        top: 10,
    };
    let report = detect_clones(&parsed, &cfg);
    assert!(!report.duplicates.is_empty());
    let sim = report.duplicates[0].similarity;
    assert!(
        (80.0..100.0).contains(&sim),
        "expected near-miss similarity, got {sim}"
    );
}

#[test]
fn detects_rust_structural_clones() {
    use code_clone_xray::normalize::ParsedSourceFile;
    use code_clone_xray::parse::rust::tokenize_rust;
    use code_clone_xray::walk::SourceLang;

    let rs_a = br#"
pub fn solve_linear_a(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    for col in 0..3 {
        let mut pivot = col;
        let mut max_val = a[col][col].abs();
        for row in (col + 1)..3 {
            let v = a[row][col].abs();
            if v > max_val {
                max_val = v;
                pivot = row;
            }
        }
        if max_val < 1e-12 {
            return None;
        }
        if pivot != col {
            a.swap(col, pivot);
            b.swap(col, pivot);
        }
    }
    Some(b)
}
"#;
    let rs_b = br#"
pub fn solve_linear_b(mut mat: [[f64; 3]; 3], mut rhs: [f64; 3]) -> Option<[f64; 3]> {
    for c in 0..3 {
        let mut piv = c;
        let mut best = mat[c][c].abs();
        for r in (c + 1)..3 {
            let val = mat[r][c].abs();
            if val > best {
                best = val;
                piv = r;
            }
        }
        if best < 1e-9 {
            return None;
        }
        if piv != c {
            mat.swap(c, piv);
            rhs.swap(c, piv);
        }
    }
    Some(rhs)
}
"#;
    let parsed = vec![
        ParsedSourceFile {
            id: 0,
            display_path: "fitting.rs".into(),
            project: "vision_kernels".into(),
            lang: SourceLang::Rs,
            total_lines: 24,
            tokens: tokenize_rust(rs_a, false),
        },
        ParsedSourceFile {
            id: 1,
            display_path: "shape_match.rs".into(),
            project: "vision_kernels".into(),
            lang: SourceLang::Rs,
            total_lines: 24,
            tokens: tokenize_rust(rs_b, false),
        },
    ];
    let cfg = DetectConfig {
        paths: vec![".".into()],
        min_lines: 8,
        min_similarity: 85.0,
        window: 20,
        top: 10,
    };
    let report = detect_clones(&parsed, &cfg);
    assert!(!report.duplicates.is_empty(), "expected Rust clone pair");
    assert_eq!(report.duplicates[0].format, "rust");
}
