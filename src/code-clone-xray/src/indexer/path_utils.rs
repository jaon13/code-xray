use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

pub fn clean_canonical(path: &Path) -> PathBuf {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| lexical_normalize(path));
    let s = resolved.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        resolved
    }
}

pub fn normalize_join(base: &Path, rel: &str) -> PathBuf {
    let replaced = rel.replace('\\', "/");
    let mut joined = base.to_path_buf();
    for seg in replaced.split('/') {
        if !seg.is_empty() {
            joined.push(seg);
        }
    }
    clean_canonical(&joined)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn to_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn make_rel_path(base: &Path, target: &Path) -> String {
    let b_parts: Vec<String> = base
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let t_parts: Vec<String> = target
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();

    let mut common = 0usize;
    while common < b_parts.len()
        && common < t_parts.len()
        && b_parts[common].eq_ignore_ascii_case(&t_parts[common])
    {
        common += 1;
    }

    if common == 0 {
        return to_slash(target);
    }
    let mut rel_parts: Vec<String> = vec!["..".to_string(); b_parts.len() - common];
    rel_parts.extend(t_parts[common..].iter().cloned());
    if rel_parts.is_empty() {
        ".".to_string()
    } else {
        rel_parts.join("/")
    }
}

pub fn classify_source_file(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".g.cs")
        || name.ends_with(".g.i.cs")
        || name.ends_with(".designer.cs")
        || name.ends_with(".assemblyinfo.cs")
        || name.ends_with(".assemblyattributes.cs")
        || name.ends_with(".min.js")
        || name.ends_with(".bundle.js")
        || name.starts_with("temporarygeneratedfile_")
        || name.contains("_wpftmp")
    {
        return None;
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "cs" => Some("cs"),
        "xaml" => Some("xaml"),
        "rs" => Some("rs"),
        "ts" | "mts" | "cts" => Some("ts"),
        "tsx" => Some("tsx"),
        "js" | "jsx" | "mjs" | "cjs" => Some("js"),
        "svelte" => Some("svelte"),
        "py" | "pyi" => Some("py"),
        _ => None,
    }
}

pub fn file_metadata(path: &Path) -> (i64, i64) {
    let Ok(meta) = fs::metadata(path) else {
        return (0, 0);
    };
    let size = i64::try_from(meta.len()).unwrap_or(0);
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
        .unwrap_or(0);
    (mtime, size)
}

pub fn is_test_project(name: &str, rel_path: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let r = format!("/{}", rel_path.to_ascii_lowercase());
    n.ends_with(".tests")
        || n.ends_with(".test")
        || n.ends_with("_tests")
        || n.ends_with("_test")
        || n.ends_with("-tests")
        || n.ends_with("-test")
        || r.contains("/tests/")
        || r.contains("/test/")
        || r.contains("/__tests__/")
        || r.contains("/benches/")
}

pub fn guess_layer(name: &str, rel_path: &str, is_external: bool, is_test: bool) -> String {
    if is_test {
        return "Tests".to_string();
    }
    if is_external {
        return "External".to_string();
    }
    let hay = format!("{name}|{rel_path}").to_ascii_lowercase();
    if hay.contains("contracts") || hay.contains("models") || hay.contains("schema") {
        return "Contracts".to_string();
    }
    if hay.contains("automation") || hay.contains("windows") || hay.contains("driver") {
        return "Automation".to_string();
    }
    if hay.contains("wpf")
        || hay.contains("ui.")
        || hay.contains("studio")
        || hay.contains("svelte")
        || hay.contains("frontend")
        || hay.contains("components")
    {
        return "UI".to_string();
    }
    if hay.contains("host") || hay.contains("server") || hay.contains("mcp") {
        return "Host".to_string();
    }
    if hay.contains("client") || hay.contains("sdk") {
        return "Client".to_string();
    }
    if hay.contains(".core") || hay.contains("core/") || hay.contains("domain") {
        return "Core".to_string();
    }
    if hay.contains("cli") || hay.contains("script") {
        return "Tooling".to_string();
    }
    "Other".to_string()
}

pub fn fnv1a_128_hex(bytes: &[u8]) -> String {
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0xaf63_bd4c_8601_b7df;
    for &b in bytes {
        h1 ^= u64::from(b);
        h1 = h1.wrapping_mul(0x0100_0000_01b3);
        h2 = h2.rotate_left(7) ^ u64::from(b);
        h2 = h2.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    format!("{h1:016x}{h2:016x}")
}
