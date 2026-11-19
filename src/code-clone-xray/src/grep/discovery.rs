use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::indexer::path_utils::{clean_canonical, make_rel_path};
use crate::indexer::project_resolver::discover_projects;
use crate::project_name::resolve_project_name;

const SKIP_DIRS: &[&str] = &[
    ".git",
    "bin",
    "obj",
    "target",
    "target-final",
    ".venv",
    "venv",
    "node_modules",
    ".xray",
    "build",
    ".vs",
    ".agents",
    ".cursor",
    ".gemini",
    ".code_graph",
    "testresults",
    "__pycache__",
    ".svelte-kit",
    "dist",
    "dist-ext",
];

const BINARY_EXTS: &[&str] = &[
    "exe", "dll", "pdb", "so", "dylib", "lib", "a", "obj", "o", "png", "jpg", "jpeg", "gif", "ico",
    "bmp", "webp", "woff", "woff2", "ttf", "eot", "zip", "7z", "tar", "gz", "db", "sqlite",
    "sqlite3", "pdf", "bin", "nupkg", "snk", "pfx",
];

const DOC_EXTS: &[&str] = &["md", "mdx", "rst", "txt", "adoc"];

const CONFIG_EXTS: &[&str] = &[
    "json",
    "jsonc",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "config",
    "props",
    "targets",
    "csproj",
    "slnx",
    "sln",
    "editorconfig",
    "xml",
    "manifest",
    "ruleset",
    "resx",
];

pub(crate) struct CandidateFile {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub project_name: String,
    pub is_external: bool,
    pub base_category: &'static str,
}

pub(crate) fn parse_ext_filter(raw: Option<&str>) -> Vec<String> {
    let Some(s) = raw else {
        return Vec::new();
    };
    let mut out: Vec<String> = s
        .split(',')
        .map(|part| part.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

pub(crate) fn collect_candidate_files(
    canon_root: &Path,
    include_external: bool,
    ext_filter: &[String],
) -> Vec<CandidateFile> {
    let mut roots: Vec<(PathBuf, bool)> = vec![(canon_root.to_path_buf(), false)];
    let mut csproj_dirs: Vec<(PathBuf, String)> = Vec::new();

    if include_external {
        let (projects, dirs) = discover_projects(canon_root, true);
        csproj_dirs = dirs;
        for proj in projects {
            if proj.is_external {
                let ext_dir = clean_canonical(Path::new(&proj.root_dir));
                if ext_dir.is_dir() && !roots.iter().any(|(r, _)| ext_dir.starts_with(r)) {
                    roots.push((ext_dir, true));
                }
            }
        }
    }

    let mut seen: FxHashSet<PathBuf> = FxHashSet::default();
    let mut raw_paths: Vec<(PathBuf, bool)> = Vec::new();

    for (scan_root, is_ext_root) in roots {
        if scan_root.is_file() {
            if is_candidate_path(&scan_root, ext_filter) && seen.insert(scan_root.clone()) {
                raw_paths.push((scan_root, is_ext_root));
            }
            continue;
        }
        let walker = WalkBuilder::new(&scan_root)
            .hidden(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
            .filter_entry(|entry| {
                if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                    return !SKIP_DIRS.contains(&name.as_str()) && !name.contains("_wpftmp");
                }
                true
            })
            .build();

        for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            let path = clean_canonical(entry.path());
            if is_candidate_path(&path, ext_filter) && seen.insert(path.clone()) {
                let is_ext = is_ext_root || !path.starts_with(canon_root);
                raw_paths.push((path, is_ext));
            }
        }
    }

    raw_paths.sort_by(|a, b| a.0.cmp(&b.0));

    let mut proj_cache: FxHashMap<PathBuf, Option<String>> = FxHashMap::default();
    raw_paths
        .into_iter()
        .map(|(abs_path, is_external)| {
            let rel_path = make_rel_path(canon_root, &abs_path);
            let project_name = csproj_dirs
                .iter()
                .find(|(d, _)| abs_path.starts_with(d))
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| resolve_project_name(&abs_path, &mut proj_cache));
            let base_category = classify_category(&abs_path, &rel_path, &project_name);
            CandidateFile {
                abs_path,
                rel_path,
                project_name,
                is_external,
                base_category,
            }
        })
        .collect()
}

fn is_candidate_path(path: &Path, ext_filter: &[String]) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower_name = name.to_ascii_lowercase();
    if lower_name.ends_with(".g.cs")
        || lower_name.ends_with(".g.i.cs")
        || lower_name.ends_with(".designer.cs")
        || lower_name.ends_with(".assemblyinfo.cs")
        || lower_name.ends_with(".assemblyattributes.cs")
        || lower_name.starts_with("temporarygeneratedfile_")
        || lower_name.contains("_wpftmp")
    {
        return false;
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    if !ext_filter.is_empty() {
        return ext_filter.iter().any(|allowed| allowed == &ext);
    }

    !BINARY_EXTS.contains(&ext.as_str())
}

fn classify_category(path: &Path, rel_path: &str, project_name: &str) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let file_low = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let rel_low = format!("/{}", rel_path.to_ascii_lowercase());
    let proj_low = project_name.to_ascii_lowercase();

    if DOC_EXTS.contains(&ext.as_str()) && file_low != "bannedsymbols.txt" {
        return "docs";
    }
    if CONFIG_EXTS.contains(&ext.as_str())
        || file_low.starts_with('.')
        || file_low == "bannedsymbols.txt"
        || file_low == "dockerfile"
    {
        return "config";
    }

    let is_test = proj_low.ends_with(".tests")
        || proj_low.ends_with(".test")
        || proj_low.ends_with("_tests")
        || rel_low.contains("/tests/")
        || rel_low.contains("/test/")
        || rel_low.contains("/testdata/")
        || rel_low.contains("/__tests__/")
        || rel_low.contains(".tests/")
        || file_low.ends_with("tests.cs")
        || file_low.ends_with("test.cs")
        || file_low.ends_with("_test.rs")
        || file_low.ends_with("_tests.rs")
        || file_low == "tests.rs"
        || file_low.ends_with(".test.ts")
        || file_low.ends_with(".spec.ts")
        || file_low.ends_with(".test.js")
        || file_low.ends_with(".spec.js")
        || file_low.starts_with("test_")
        || file_low.ends_with("_test.py");

    if is_test {
        "test"
    } else {
        "code"
    }
}
