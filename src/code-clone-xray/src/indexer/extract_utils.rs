use rustc_hash::FxHashSet;

use super::cs_member_extractor::FileCtx;
use super::cs_syntax_utils::{is_interesting_type, split_type_tokens};
use super::model::{DiscoveredFile, ExtractedFileData, ExtractedTypeRef};
use super::path_utils::fnv1a_128_hex;

pub fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

pub fn derive_module_namespace(rel_path: &str, sep: &str) -> String {
    let norm = rel_path.replace('\\', "/");
    let trimmed = norm.trim_start_matches("./");
    let no_ext = match trimmed.rsplit_once('.') {
        Some((head, _)) if !head.ends_with('/') => head,
        _ => trimmed,
    };
    let mut parts: Vec<&str> = no_ext
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if matches!(parts.first().copied(), Some("src" | "lib" | "app")) && parts.len() > 1 {
        parts.remove(0);
    }
    if parts.len() > 1
        && matches!(
            parts.last().copied(),
            Some("mod" | "index" | "__init__" | "lib" | "main")
        )
    {
        parts.pop();
    }
    if parts.is_empty() {
        "root".to_string()
    } else {
        parts.join(sep)
    }
}

pub fn derive_file_stem(rel_path: &str) -> String {
    let norm = rel_path.replace('\\', "/");
    let fname = norm.rsplit('/').next().unwrap_or(&norm);
    fname
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .filter(|s| !s.is_empty())
        .unwrap_or("module")
        .to_string()
}

pub fn push_unique_ref(
    ctx: &mut FileCtx,
    seen_refs: &mut FxHashSet<(String, String, String)>,
    source_type: &str,
    target_type: &str,
    ref_kind: &str,
    line: u32,
) {
    let src = source_type.trim();
    let tgt = target_type.trim();
    if src.is_empty() || tgt.is_empty() || src == tgt {
        return;
    }
    let key = (src.to_string(), tgt.to_string(), ref_kind.to_string());
    if seen_refs.insert(key.clone()) {
        ctx.type_refs.push(ExtractedTypeRef {
            source_type: key.0,
            target_type: key.1,
            ref_kind: key.2,
            line,
        });
    }
}

pub fn collect_pascal_refs_from_text(
    text: &str,
    source_type: &str,
    ref_kind: &str,
    line: u32,
    ctx: &mut FileCtx,
    seen_refs: &mut FxHashSet<(String, String, String)>,
) {
    for tok in split_type_tokens(text) {
        if is_interesting_type(&tok) {
            push_unique_ref(ctx, seen_refs, source_type, &tok, ref_kind, line);
        }
    }
}

pub fn split_callee(raw: &str) -> (String, String) {
    let clean = raw.trim();
    if let Some((recv, name)) = clean.rsplit_once('.') {
        return (recv.trim().to_string(), name.trim().to_string());
    }
    if let Some((recv, name)) = clean.rsplit_once("::") {
        return (recv.trim().to_string(), name.trim().to_string());
    }
    (String::new(), clean.to_string())
}

pub fn strip_generic_args(raw: &str) -> &str {
    raw.split('<').next().unwrap_or(raw).trim()
}

pub fn build_file_data(
    file: &DiscoveredFile,
    bytes: &[u8],
    line_count: usize,
    ctx: FileCtx,
) -> ExtractedFileData {
    ExtractedFileData {
        file: file.clone(),
        content_hash: fnv1a_128_hex(bytes),
        line_count,
        types: ctx.types,
        members: ctx.members,
        calls: ctx.calls,
        type_refs: ctx.type_refs,
        di_regs: ctx.di_regs,
        messenger_links: ctx.messenger_links,
    }
}
