use tree_sitter::Node;

use super::model::ExtractedTypeRef;

const BUILTIN_SKIP: &[&str] = &[
    "void",
    "bool",
    "byte",
    "sbyte",
    "short",
    "ushort",
    "int",
    "uint",
    "long",
    "ulong",
    "float",
    "double",
    "decimal",
    "char",
    "string",
    "object",
    "var",
    "dynamic",
    "nint",
    "nuint",
    "Task",
    "ValueTask",
    "CancellationToken",
    "CancellationTokenSource",
    "String",
    "Int32",
    "Int64",
    "Boolean",
    "Double",
    "Object",
    "Type",
    "Guid",
    "DateTime",
    "DateTimeOffset",
    "TimeSpan",
    "Exception",
    "ArgumentNullException",
    "InvalidOperationException",
    "EventArgs",
    "EventHandler",
    "Action",
    "Func",
    "List",
    "IList",
    "IReadOnlyList",
    "Dictionary",
    "IDictionary",
    "IReadOnlyDictionary",
    "HashSet",
    "ISet",
    "IEnumerable",
    "IReadOnlyCollection",
    "ConcurrentDictionary",
    "FrozenDictionary",
    "FrozenSet",
    "Span",
    "ReadOnlySpan",
    "Memory",
    "ReadOnlyMemory",
    "Nullable",
];

pub fn node_text<'a>(node: Node<'_>, src: &'a [u8]) -> &'a str {
    src.get(node.start_byte()..node.end_byte())
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
}

pub fn line_1(node: Node<'_>) -> u32 {
    (node.start_position().row as u32) + 1
}

pub fn clean_type_token(raw: &str) -> String {
    let no_nullable = raw
        .trim()
        .trim_end_matches('?')
        .trim_end_matches("[]")
        .trim();
    let short = no_nullable.rsplit('.').next().unwrap_or(no_nullable).trim();
    if let Some(lt) = short.find('<') {
        short[..lt].trim().to_string()
    } else {
        short.to_string()
    }
}

pub fn is_interesting_type(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && !BUILTIN_SKIP.contains(&name)
}

pub fn split_type_tokens(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            cur.push(ch);
        } else if !cur.is_empty() {
            tokens.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

pub fn collect_type_identifiers(
    type_node: Node<'_>,
    src: &[u8],
    source_type: &str,
    ref_kind: &str,
    out: &mut Vec<ExtractedTypeRef>,
) {
    let raw = node_text(type_node, src);
    let line = line_1(type_node);
    for token in split_type_tokens(raw) {
        if is_interesting_type(&token) && token != source_type {
            out.push(ExtractedTypeRef {
                source_type: source_type.to_string(),
                target_type: token,
                ref_kind: ref_kind.to_string(),
                line,
            });
        }
    }
}

pub fn extract_param_type_refs(
    plist: Node<'_>,
    src: &[u8],
    owner_type: &str,
    ref_kind: &str,
    out: &mut Vec<ExtractedTypeRef>,
) {
    let mut cursor = plist.walk();
    for child in plist.children(&mut cursor) {
        if child.kind() == "parameter" {
            if let Some(ty) = child.child_by_field_name("type") {
                collect_type_identifiers(ty, src, owner_type, ref_kind, out);
            }
        }
    }
}

pub fn extract_modifiers(node: Node<'_>, src: &[u8]) -> (String, bool) {
    let mut access = "internal".to_string();
    let mut is_partial = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier" {
            let txt = node_text(child, src).trim();
            match txt {
                "public" | "private" | "protected" | "internal" | "file" => {
                    access = txt.to_string();
                }
                "partial" => is_partial = true,
                _ => {}
            }
        }
    }
    (access, is_partial)
}

pub fn extract_base_types(node: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut res = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "base_list" {
            let raw = node_text(child, src).trim().trim_start_matches(':').trim();
            for item in raw.split(',') {
                let s = item.trim();
                if !s.is_empty() {
                    res.push(s.to_string());
                }
            }
        }
    }
    res
}

pub fn extract_generic_arg(base_item: &str, interface_name: &str) -> Option<String> {
    let lt = base_item.find('<')?;
    let head = base_item[..lt]
        .rsplit('.')
        .next()
        .unwrap_or(&base_item[..lt])
        .trim();
    if head != interface_name || !base_item.ends_with('>') {
        return None;
    }
    let inner = clean_type_token(&base_item[lt + 1..base_item.len() - 1]);
    if is_interesting_type(&inner) {
        Some(inner)
    } else {
        None
    }
}

pub fn is_constant_key_literal(raw_unquoted: &str) -> bool {
    let len = raw_unquoted.len();
    if !(3..=48).contains(&len) {
        return false;
    }
    let Some(first) = raw_unquoted.chars().next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    let mut has_underscore = false;
    let mut has_lower = false;
    for ch in raw_unquoted.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
            return false;
        }
        if ch == '_' {
            has_underscore = true;
        }
        if ch.is_ascii_lowercase() {
            has_lower = true;
        }
    }
    has_underscore || !has_lower
}

pub fn compact_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn truncate_sig(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push_str("...");
        out
    }
}

pub fn count_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let nl = bytes.iter().filter(|&&b| b == b'\n').count();
    if bytes.last() == Some(&b'\n') {
        nl
    } else {
        nl + 1
    }
}
