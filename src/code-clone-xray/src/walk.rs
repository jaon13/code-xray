use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

use crate::cli::CliArgs;
use crate::project_name::resolve_project_name;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceLang {
    Cs,
    Xaml,
    Rs,
    Ts,
    Js,
    Py,
    Svelte,
    Html,
    Css,
}

impl SourceLang {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cs => "cs",
            Self::Xaml => "xaml",
            Self::Rs => "rs",
            Self::Ts => "ts",
            Self::Js => "js",
            Self::Py => "py",
            Self::Svelte => "svelte",
            Self::Html => "html",
            Self::Css => "css",
        }
    }

    pub fn format_name(self) -> &'static str {
        match self {
            Self::Cs => "csharp",
            Self::Xaml => "xaml",
            Self::Rs => "rust",
            Self::Ts => "typescript",
            Self::Js => "javascript",
            Self::Py => "python",
            Self::Svelte => "svelte",
            Self::Html => "html",
            Self::Css => "css",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScannedFile {
    pub id: u32,
    pub path: PathBuf,
    pub display_path: String,
    pub project: String,
    pub lang: SourceLang,
    pub include_tests: bool,
}

const IGNORED_DIRS: &[&str] = &[
    "bin",
    "obj",
    "build",
    ".git",
    ".vs",
    ".agents",
    ".cursor",
    ".gemini",
    "node_modules",
    "target",
    "target-final",
    "dist",
    "dist-ext",
    ".svelte-kit",
    "__pycache__",
    ".venv",
    "venv",
    "testresults",
    "_archive",
];

struct LangFilter {
    cs: bool,
    xaml: bool,
    rs: bool,
    ts: bool,
    js: bool,
    py: bool,
    svelte: bool,
    html: bool,
    css: bool,
}

pub fn collect_source_files(args: &CliArgs) -> Vec<ScannedFile> {
    let filter = LangFilter {
        cs: args.wants_cs(),
        xaml: args.wants_xaml(),
        rs: args.wants_rs(),
        ts: args.wants_ts(),
        js: args.wants_js(),
        py: args.wants_py(),
        svelte: args.wants_svelte(),
        html: args.wants_html(),
        css: args.wants_css(),
    };
    let include_tests = args.include_tests;
    let extra_ignore = args.ignore_fragments();

    let mut seen_paths: FxHashSet<PathBuf> = FxHashSet::default();
    let mut raw_entries: Vec<(PathBuf, SourceLang)> = Vec::new();

    for root in &args.path {
        if !root.exists() {
            continue;
        }
        let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.clone());

        if canonical_root.is_file() {
            if let Some(lang) = classify_file(&canonical_root, &filter) {
                let clean = strip_verbatim_prefix(&canonical_root);
                if seen_paths.insert(clean.clone()) {
                    raw_entries.push((clean, lang));
                }
            }
            continue;
        }

        let extra_ignore_clone = extra_ignore.clone();
        let walker = WalkBuilder::new(&canonical_root)
            .hidden(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
            .filter_entry(move |entry| {
                if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    let name = entry.file_name().to_string_lossy();
                    if is_ignored_dir(&name, include_tests) {
                        return false;
                    }
                    if extra_ignore_clone.iter().any(|frag| {
                        name.to_ascii_lowercase()
                            .contains(&frag.to_ascii_lowercase())
                    }) {
                        return false;
                    }
                }
                true
            })
            .build();

        for result in walker.flatten() {
            if !result.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            let path = result.into_path();
            if is_generated_or_excluded_file(&path, include_tests) {
                continue;
            }
            if let Some(lang) = classify_file(&path, &filter) {
                let clean_path = strip_verbatim_prefix(&path);
                if seen_paths.insert(clean_path.clone()) {
                    raw_entries.push((clean_path, lang));
                }
            }
        }
    }

    raw_entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut proj_cache: FxHashMap<PathBuf, Option<String>> = FxHashMap::default();
    raw_entries
        .into_iter()
        .enumerate()
        .map(|(idx, (path, lang))| {
            let project = resolve_project_name(&path, &mut proj_cache);
            let display_path = path.to_string_lossy().replace('\\', "/");
            ScannedFile {
                id: idx as u32,
                path,
                display_path,
                project,
                lang,
                include_tests,
            }
        })
        .collect()
}

fn classify_file(path: &Path, filter: &LangFilter) -> Option<SourceLang> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "cs" if filter.cs => Some(SourceLang::Cs),
        "xaml" if filter.xaml => Some(SourceLang::Xaml),
        "rs" if filter.rs => Some(SourceLang::Rs),
        "ts" | "tsx" | "mts" | "cts" if filter.ts => Some(SourceLang::Ts),
        "js" | "jsx" | "mjs" | "cjs" if filter.js => Some(SourceLang::Js),
        "py" if filter.py => Some(SourceLang::Py),
        "svelte" if filter.svelte => Some(SourceLang::Svelte),
        "html" | "htm" if filter.html => Some(SourceLang::Html),
        "css" if filter.css => Some(SourceLang::Css),
        _ => None,
    }
}

fn is_ignored_dir(dir_name: &str, include_tests: bool) -> bool {
    let lower = dir_name.to_ascii_lowercase();
    if IGNORED_DIRS.contains(&lower.as_str()) || lower.contains("_wpftmp") {
        return true;
    }
    if !include_tests
        && (lower == "tests"
            || lower == "benches"
            || lower.ends_with(".tests")
            || lower.ends_with(".test"))
    {
        return true;
    }
    false
}

fn is_generated_or_excluded_file(path: &Path, include_tests: bool) -> bool {
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".g.cs")
        || lower.ends_with(".g.i.cs")
        || lower.ends_with(".designer.cs")
        || lower.ends_with(".assemblyinfo.cs")
        || lower.ends_with(".assemblyattributes.cs")
        || lower == "build.rs"
        || lower.starts_with("temporarygeneratedfile_")
        || lower.contains("_wpftmp")
    {
        return true;
    }
    if !include_tests {
        if lower == "tests.rs" || lower.ends_with("_tests.rs") {
            return true;
        }
        if lower.ends_with("tests.cs") || lower.ends_with("test.cs") {
            let path_str = path.to_string_lossy().to_ascii_lowercase();
            if path_str.contains("\\tests\\")
                || path_str.contains("/tests/")
                || path_str.contains(".tests")
            {
                return true;
            }
        }
    }
    false
}

fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}
