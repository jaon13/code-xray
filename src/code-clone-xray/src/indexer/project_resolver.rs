use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rustc_hash::FxHashSet;

use super::manifest_discovery::{
    build_cargo_project, build_package_json_project, build_pyproject_project, scan_repo_manifests,
    SKIP_DIRS,
};
use super::model::{DiscoveredFile, ProjectInfo};
use super::path_utils::{
    classify_source_file, clean_canonical, file_metadata, guess_layer, is_test_project,
    make_rel_path, normalize_join, to_slash,
};

pub fn resolve_workspace(
    repo_root: &Path,
    include_external: bool,
) -> (Vec<ProjectInfo>, Vec<DiscoveredFile>) {
    let norm_root = clean_canonical(repo_root);
    let (projects, project_dirs) = discover_projects(&norm_root, include_external);
    let files = collect_project_files(&norm_root, &project_dirs);
    (projects, files)
}

pub fn discover_projects(
    repo_root: &Path,
    include_external: bool,
) -> (Vec<ProjectInfo>, Vec<(PathBuf, String)>) {
    let norm_root = clean_canonical(repo_root);
    let scan = scan_repo_manifests(&norm_root);

    let mut projects: Vec<ProjectInfo> = Vec::new();
    let mut project_dirs: Vec<(PathBuf, String)> = Vec::new();
    let mut seen_names: FxHashSet<String> = FxHashSet::default();

    discover_dotnet_projects(
        &norm_root,
        include_external,
        &scan.csproj_files,
        &mut seen_names,
        &mut projects,
        &mut project_dirs,
    );

    for cargo_path in &scan.cargo_tomls {
        if let Some((proj, dir)) = build_cargo_project(&norm_root, cargo_path) {
            if seen_names.insert(proj.name.clone()) {
                project_dirs.push((dir, proj.name.clone()));
                projects.push(proj);
            }
        }
    }

    let known_js_names: FxHashSet<String> = scan
        .package_jsons
        .iter()
        .filter_map(|p| read_pkg_name(p))
        .collect();
    for pkg_path in &scan.package_jsons {
        if let Some((proj, dir)) = build_package_json_project(&norm_root, pkg_path, &known_js_names)
        {
            if seen_names.insert(proj.name.clone()) {
                project_dirs.push((dir, proj.name.clone()));
                projects.push(proj);
            }
        }
    }

    for py_path in &scan.pyprojects {
        if let Some((proj, dir)) = build_pyproject_project(&norm_root, py_path) {
            if seen_names.insert(proj.name.clone()) {
                project_dirs.push((dir, proj.name.clone()));
                projects.push(proj);
            }
        }
    }

    let has_unowned_sources = scan
        .source_dirs
        .iter()
        .any(|sd| !project_dirs.iter().any(|(pd, _)| sd.starts_with(pd)));
    if has_unowned_sources && !project_dirs.iter().any(|(pd, _)| pd == &norm_root) {
        let fallback_name = unique_fallback_name(&norm_root, &seen_names);
        seen_names.insert(fallback_name.clone());
        let is_test = is_test_project(&fallback_name, ".");
        let layer = guess_layer(&fallback_name, ".", false, is_test);
        project_dirs.push((norm_root.clone(), fallback_name.clone()));
        projects.push(ProjectInfo {
            name: fallback_name,
            csproj_path: String::new(),
            root_dir: to_slash(&norm_root),
            layer,
            is_external: false,
            is_test,
            refs: Vec::new(),
        });
    }

    projects.sort_by_key(|a| a.name.to_lowercase());
    project_dirs.sort_by(|a, b| {
        b.0.as_os_str()
            .len()
            .cmp(&a.0.as_os_str().len())
            .then_with(|| a.1.cmp(&b.1))
    });

    (projects, project_dirs)
}

fn discover_dotnet_projects(
    norm_root: &Path,
    include_external: bool,
    root_csprojs: &[PathBuf],
    seen_names: &mut FxHashSet<String>,
    projects: &mut Vec<ProjectInfo>,
    project_dirs: &mut Vec<(PathBuf, String)>,
) {
    let mut seeds = root_csprojs.to_vec();
    for sln_proj in collect_solution_csprojs(norm_root) {
        if !seeds.contains(&sln_proj) {
            seeds.push(sln_proj);
        }
    }

    let mut visited_csproj: FxHashSet<PathBuf> = FxHashSet::default();
    let mut queue: VecDeque<(PathBuf, bool)> = VecDeque::new();
    for cp in seeds {
        let is_ext = !cp.starts_with(norm_root);
        if (include_external || !is_ext) && visited_csproj.insert(cp.clone()) {
            queue.push_back((cp, is_ext));
        }
    }

    while let Some((csproj_path, is_external)) = queue.pop_front() {
        let Some(proj_dir) = csproj_path.parent().map(Path::to_path_buf) else {
            continue;
        };
        let proj_name = csproj_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();
        if proj_name.contains("_wpftmp") || !seen_names.insert(proj_name.clone()) {
            continue;
        }

        let refs_paths = parse_project_references(&csproj_path);
        let mut ref_names: Vec<String> = Vec::with_capacity(refs_paths.len());
        for ref_path in refs_paths {
            let ref_name = ref_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if !ref_name.is_empty() {
                ref_names.push(ref_name);
            }
            let ref_is_ext = !ref_path.starts_with(norm_root);
            if (include_external || !ref_is_ext) && visited_csproj.insert(ref_path.clone()) {
                queue.push_back((ref_path, ref_is_ext));
            }
        }
        ref_names.sort();
        ref_names.dedup();

        let rel_csproj = make_rel_path(norm_root, &csproj_path);
        let is_test = is_test_project(&proj_name, &rel_csproj);
        let layer = guess_layer(&proj_name, &rel_csproj, is_external, is_test);
        project_dirs.push((proj_dir.clone(), proj_name.clone()));

        projects.push(ProjectInfo {
            name: proj_name,
            csproj_path: to_slash(&csproj_path),
            root_dir: to_slash(&proj_dir),
            layer,
            is_external,
            is_test,
            refs: ref_names,
        });
    }
}

fn read_pkg_name(pkg_path: &Path) -> Option<String> {
    let text = fs::read_to_string(pkg_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&text).ok()?;
    let name = val.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn unique_fallback_name(norm_root: &Path, seen_names: &FxHashSet<String>) -> String {
    let base = norm_root
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Repository");
    if !seen_names.contains(base) {
        return base.to_string();
    }
    let candidate = format!("{base}.root");
    if !seen_names.contains(&candidate) {
        return candidate;
    }
    format!("{base}_workspace")
}

fn collect_project_files(
    repo_root: &Path,
    project_dirs: &[(PathBuf, String)],
) -> Vec<DiscoveredFile> {
    let mut seen: FxHashSet<PathBuf> = FxHashSet::default();
    let mut out: Vec<DiscoveredFile> = Vec::new();
    let mut dir_paths: Vec<PathBuf> = project_dirs.iter().map(|(d, _)| d.clone()).collect();
    dir_paths.sort_by_key(|d| d.as_os_str().len());

    let mut scanned_roots: Vec<PathBuf> = Vec::new();
    for dir in dir_paths {
        if !scanned_roots.iter().any(|r| dir.starts_with(r)) {
            scanned_roots.push(dir);
        }
    }

    for scan_dir in scanned_roots {
        let walker = WalkBuilder::new(&scan_dir)
            .hidden(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
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
            let path = clean_canonical(entry.path());
            let Some(lang) = classify_source_file(&path) else {
                continue;
            };
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some(proj_name) = project_dirs
                .iter()
                .find(|(d, _)| path.starts_with(d))
                .map(|(_, n)| n.clone())
            else {
                continue;
            };
            let (mtime_ns, size_bytes) = file_metadata(&path);
            out.push(DiscoveredFile {
                project_name: proj_name,
                abs_path: to_slash(&path),
                rel_path: make_rel_path(repo_root, &path),
                lang,
                mtime_ns,
                size_bytes,
            });
        }
    }

    out.sort_by(|a, b| a.abs_path.cmp(&b.abs_path));
    out
}

fn collect_solution_csprojs(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !(ext.eq_ignore_ascii_case("slnx") || ext.eq_ignore_ascii_case("sln")) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&p) else {
            continue;
        };
        for part in text.split('"') {
            let trimmed = part.trim();
            if trimmed.to_ascii_lowercase().ends_with(".csproj") && !trimmed.contains('\n') {
                let joined = normalize_join(root, trimmed);
                if joined.is_file() {
                    found.push(joined);
                }
            }
        }
    }
    found
}

fn parse_project_references(csproj_path: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(csproj_path) else {
        return Vec::new();
    };
    let Some(base_dir) = csproj_path.parent() else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    let mut rest = text.as_str();
    while let Some(pos) = rest.find("<ProjectReference") {
        rest = &rest[pos..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        if let Some(i) = tag.find("Include=\"") {
            let after = &tag[i + 9..];
            if let Some(q) = after.find('"') {
                let resolved = normalize_join(base_dir, &after[..q]);
                if resolved.is_file() {
                    refs.push(resolved);
                }
            }
        }
        rest = &rest[end..];
    }
    refs
}
