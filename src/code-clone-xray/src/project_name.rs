use std::fs;
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

pub(crate) fn resolve_project_name(
    file_path: &Path,
    cache: &mut FxHashMap<PathBuf, Option<String>>,
) -> String {
    let mut chain: Vec<PathBuf> = Vec::new();
    let mut current = file_path.parent();

    let found = loop {
        let Some(dir) = current else {
            break None;
        };
        if let Some(cached) = cache.get(dir) {
            break cached.clone();
        }
        chain.push(dir.to_path_buf());
        if let Some(proj_name) = find_manifest_project_in_dir(dir) {
            break Some(proj_name);
        }
        current = dir.parent();
    };

    for dir in chain {
        cache.insert(dir, found.clone());
    }

    found.unwrap_or_else(|| {
        file_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Unknown".to_string())
    })
}

fn find_manifest_project_in_dir(dir: &Path) -> Option<String> {
    if let Some(name) = read_cargo_name(dir) {
        return Some(name);
    }
    if let Some(name) = read_csproj_stem(dir) {
        return Some(name);
    }
    read_package_json_name(dir)
}

fn read_cargo_name(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name") {
            let after_eq = rest.trim_start().strip_prefix('=')?.trim();
            let unquoted = after_eq.trim_matches('"').trim_matches('\'');
            if !unquoted.is_empty() {
                return Some(unquoted.to_string());
            }
        }
    }
    None
}

fn read_csproj_stem(dir: &Path) -> Option<String> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("csproj"))
        {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if !stem.contains("_wpftmp") {
                    return Some(stem.to_string());
                }
            }
        }
    }
    None
}

fn read_package_json_name(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("package.json")).ok()?;
    let val = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let name = val.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}
