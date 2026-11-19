use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rustc_hash::FxHashSet;

use super::model::ProjectInfo;
use super::path_utils::{
    classify_source_file, clean_canonical, guess_layer, is_test_project, make_rel_path,
    normalize_join, to_slash,
};

pub const SKIP_DIRS: &[&str] = &[
    "bin",
    "obj",
    "build",
    ".git",
    ".vs",
    ".agents",
    ".cursor",
    ".gemini",
    ".xray",
    ".code_graph",
    "node_modules",
    "target",
    "target-final",
    "testresults",
    "__pycache__",
    ".venv",
    "venv",
    ".svelte-kit",
    "dist",
    "dist-ext",
];

#[derive(Default)]
pub struct RepoManifestScan {
    pub csproj_files: Vec<PathBuf>,
    pub cargo_tomls: Vec<PathBuf>,
    pub package_jsons: Vec<PathBuf>,
    pub pyprojects: Vec<PathBuf>,
    pub source_dirs: FxHashSet<PathBuf>,
}

pub fn scan_repo_manifests(root: &Path) -> RepoManifestScan {
    let mut scan = RepoManifestScan::default();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .filter_entry(|e| {
            if e.file_type().is_some_and(|ft| ft.is_dir()) {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                return !SKIP_DIRS.contains(&name.as_str()) && !name.contains("_wpftmp");
            }
            true
        })
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if fname.eq_ignore_ascii_case("Cargo.toml") {
            scan.cargo_tomls.push(clean_canonical(path));
        } else if fname.eq_ignore_ascii_case("package.json") {
            scan.package_jsons.push(clean_canonical(path));
        } else if fname.eq_ignore_ascii_case("pyproject.toml") {
            scan.pyprojects.push(clean_canonical(path));
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("csproj"))
        {
            let clean = clean_canonical(path);
            if !clean.to_string_lossy().contains("_wpftmp") {
                scan.csproj_files.push(clean);
            }
        }
        if classify_source_file(path).is_some() {
            if let Some(parent) = path.parent() {
                scan.source_dirs.insert(clean_canonical(parent));
            }
        }
    }
    scan
}

pub fn build_cargo_project(repo_root: &Path, cargo_path: &Path) -> Option<(ProjectInfo, PathBuf)> {
    let text = fs::read_to_string(cargo_path).ok()?;
    let pkg_name = parse_toml_section_name(&text, "package")?;
    let proj_dir = cargo_path.parent()?.to_path_buf();
    let rel = make_rel_path(repo_root, cargo_path);
    let mut refs = parse_cargo_path_refs(&proj_dir, &text);
    refs.sort();
    refs.dedup();
    let is_test = is_test_project(&pkg_name, &rel);
    let layer = guess_layer(&pkg_name, &rel, false, is_test);
    Some((
        ProjectInfo {
            name: pkg_name,
            csproj_path: to_slash(cargo_path),
            root_dir: to_slash(&proj_dir),
            layer,
            is_external: false,
            is_test,
            refs,
        },
        proj_dir,
    ))
}

pub fn build_package_json_project(
    repo_root: &Path,
    pkg_path: &Path,
    known_names: &FxHashSet<String>,
) -> Option<(ProjectInfo, PathBuf)> {
    let text = fs::read_to_string(pkg_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&text).ok()?;
    let name = val.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let proj_dir = pkg_path.parent()?.to_path_buf();
    let rel = make_rel_path(repo_root, pkg_path);
    let mut refs = Vec::new();
    for key in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(map) = val.get(key).and_then(|v| v.as_object()) {
            for (dep_name, dep_ver) in map {
                let ver_str = dep_ver.as_str().unwrap_or("");
                if ver_str.starts_with("workspace:")
                    || ver_str.starts_with("file:")
                    || known_names.contains(dep_name)
                {
                    refs.push(dep_name.clone());
                }
            }
        }
    }
    refs.sort();
    refs.dedup();
    let is_test = is_test_project(name, &rel);
    let layer = guess_layer(name, &rel, false, is_test);
    Some((
        ProjectInfo {
            name: name.to_string(),
            csproj_path: to_slash(pkg_path),
            root_dir: to_slash(&proj_dir),
            layer,
            is_external: false,
            is_test,
            refs,
        },
        proj_dir,
    ))
}

pub fn build_pyproject_project(
    repo_root: &Path,
    pyproj_path: &Path,
) -> Option<(ProjectInfo, PathBuf)> {
    let text = fs::read_to_string(pyproj_path).ok()?;
    let name = parse_toml_section_name(&text, "project")
        .or_else(|| parse_toml_section_name(&text, "tool.poetry"))?;
    let proj_dir = pyproj_path.parent()?.to_path_buf();
    let rel = make_rel_path(repo_root, pyproj_path);
    let is_test = is_test_project(&name, &rel);
    let layer = guess_layer(&name, &rel, false, is_test);
    Some((
        ProjectInfo {
            name,
            csproj_path: to_slash(pyproj_path),
            root_dir: to_slash(&proj_dir),
            layer,
            is_external: false,
            is_test,
            refs: Vec::new(),
        },
        proj_dir,
    ))
}

fn parse_toml_section_name(text: &str, target_section: &str) -> Option<String> {
    let mut cur_section = "";
    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            cur_section = inner.trim();
            continue;
        }
        if cur_section == target_section {
            if let Some(rest) = line.strip_prefix("name") {
                if let Some(val) = rest.trim_start().strip_prefix('=') {
                    let clean = val.trim().trim_matches('"').trim_matches('\'');
                    if !clean.is_empty() {
                        return Some(clean.to_string());
                    }
                }
            }
        }
    }
    None
}

fn parse_cargo_path_refs(proj_dir: &Path, text: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut in_deps = false;
    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            let sec = line.trim_matches(|c| c == '[' || c == ']').trim();
            in_deps = matches!(
                sec,
                "dependencies" | "dev-dependencies" | "build-dependencies"
            ) || sec.ends_with(".dependencies");
            continue;
        }
        if !in_deps {
            continue;
        }
        if let Some(pos) = line.find("path") {
            let after = line[pos + 4..].trim_start();
            if let Some(eq_after) = after.strip_prefix('=') {
                let quoted = eq_after.trim_start();
                if let Some(q_char) = quoted.chars().next().filter(|c| *c == '"' || *c == '\'') {
                    if let Some(end_q) = quoted[1..].find(q_char) {
                        let rel_dir = &quoted[1..1 + end_q];
                        let dep_cargo = normalize_join(proj_dir, rel_dir).join("Cargo.toml");
                        if let Ok(dep_text) = fs::read_to_string(&dep_cargo) {
                            if let Some(dep_name) = parse_toml_section_name(&dep_text, "package") {
                                refs.push(dep_name);
                                continue;
                            }
                        }
                    }
                }
            }
        }
        if let Some((key, _)) = line.split_once('=') {
            if line.contains("workspace") && line.contains("true") {
                refs.push(key.trim().to_string());
            }
        }
    }
    refs
}
