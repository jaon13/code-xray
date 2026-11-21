use tree_sitter::Node;

use super::cs_syntax_utils::{compact_whitespace, node_text, truncate_sig};

const STATE_TOKENS: &[&str] = &[
    "state",
    "status",
    "phase",
    "step",
    "stage",
    "mode",
    "workflow",
    "lifecycle",
];

const IGNORED_RHS_MEMBERS: &[&str] = &[
    "Empty", "Length", "Count", "Size", "Name", "Type", "Value", "Keys", "Values", "ToString",
    "Clone", "Default", "MaxValue", "MinValue",
];

fn split_ident_tokens(ident: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;

    for ch in ident.chars() {
        if !ch.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                tokens.push(cur.to_ascii_lowercase());
                cur.clear();
            }
            prev_lower = false;
            continue;
        }
        if prev_lower && ch.is_ascii_uppercase() && !cur.is_empty() {
            tokens.push(cur.to_ascii_lowercase());
            cur.clear();
        }
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        cur.push(ch);
    }
    if !cur.is_empty() {
        tokens.push(cur.to_ascii_lowercase());
    }
    tokens
}

/// Checks whether a field, property, or variable identifier denotes a state machine holder.
pub fn is_state_identifier(raw: &str) -> bool {
    let leaf = extract_leaf_ident(raw);
    if leaf.is_empty() {
        return false;
    }
    split_ident_tokens(leaf)
        .iter()
        .any(|tok| STATE_TOKENS.contains(&tok.as_str()))
}

pub(crate) fn extract_leaf_ident(raw: &str) -> &str {
    let trimmed = raw.trim();
    let tail = trimmed
        .rsplit(['.', ':'])
        .next()
        .unwrap_or(trimmed)
        .trim_start_matches('_')
        .trim_start_matches('$');
    tail.trim()
}

fn is_valid_state_literal(text: &str) -> bool {
    let clean = text.trim();
    if clean.is_empty() || clean.len() > 32 {
        return false;
    }
    let mut has_alpha = false;
    for ch in clean.chars() {
        if ch.is_ascii_alphabetic() {
            has_alpha = true;
        } else if !(ch.is_ascii_digit() || ch == '_' || ch == '-') {
            return false;
        }
    }
    has_alpha
        && !matches!(
            clean.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "none" | "undefined" | "nan" | "default"
        )
}

/// Parses an enum member (`RunState.Running`, `JobState::Idle`), `$state(...)`, or string literal.
pub fn parse_state_rhs(raw: &str) -> Option<(String, String)> {
    let trimmed = raw.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }

    let clean = unwrap_option_like(trimmed);
    if (clean.starts_with('"') && clean.ends_with('"'))
        || (clean.starts_with('\'') && clean.ends_with('\''))
    {
        if clean.len() >= 3 {
            let inner = &clean[1..clean.len() - 1];
            if is_valid_state_literal(inner) {
                return Some((String::new(), inner.to_string()));
            }
        }
        return None;
    }

    if clean.contains('(') || clean.contains(' ') {
        return None;
    }

    let (prefix, variant) = if let Some((p, v)) = clean.rsplit_once("::") {
        (p.trim(), v.trim())
    } else if let Some((p, v)) = clean.rsplit_once('.') {
        (p.trim(), v.trim())
    } else {
        return None;
    };

    let enum_tail = prefix.rsplit(['.', ':']).next().unwrap_or(prefix).trim();
    let starts_upper = enum_tail
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase());
    if !starts_upper || IGNORED_RHS_MEMBERS.contains(&variant) || !is_valid_state_literal(variant) {
        return None;
    }

    Some((enum_tail.to_string(), variant.to_string()))
}

fn unwrap_option_like(raw: &str) -> &str {
    for prefix in ["Some(", "Ok(", "Err("] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            if let Some(inner) = rest.strip_suffix(')') {
                return inner.trim();
            }
        }
    }
    if let Some(rest) = raw.strip_prefix("$state") {
        let after_gen = if rest.starts_with('<') {
            rest.find('>').map(|idx| &rest[idx + 1..]).unwrap_or(rest)
        } else {
            rest
        };
        if let Some(inner) = after_gen.trim().strip_prefix('(') {
            if let Some(val) = inner.strip_suffix(')') {
                return val.trim();
            }
        }
    }
    raw
}

pub(crate) fn short_type_name(full: &str) -> &str {
    full.rsplit(['.', ':']).next().unwrap_or(full).trim()
}

pub(crate) fn derive_machine_name(lhs_leaf: &str, enum_hint: &str, owner_type: &str) -> String {
    if !enum_hint.is_empty() {
        return enum_hint.to_string();
    }
    let owner_short = short_type_name(owner_type);
    if owner_short.is_empty() {
        lhs_leaf.to_string()
    } else {
        format!("{owner_short}.{lhs_leaf}")
    }
}

fn is_ctor_trigger(trigger: &str, owner_type: &str) -> bool {
    let owner_short = short_type_name(owner_type);
    matches!(trigger, "new" | "__init__" | "constructor" | "<init>")
        || trigger.starts_with('.')
        || (!owner_short.is_empty() && trigger == owner_short)
}

fn is_boundary_kind(kind: &str) -> bool {
    matches!(
        kind,
        "method_declaration"
            | "constructor_declaration"
            | "function_item"
            | "function_definition"
            | "method_definition"
            | "function_declaration"
            | "class_declaration"
            | "struct_item"
            | "impl_item"
            | "class_definition"
    )
}

fn extract_state_token_in_pattern(raw: &str, lhs_leaf: &str) -> Option<(String, String)> {
    let clean = raw.trim();
    if let Some((enum_hint, val)) = parse_state_rhs(clean) {
        return Some((enum_hint, val));
    }
    if clean.contains("==") {
        let mut parts = clean.split("==");
        let left = parts.next()?.trim();
        let right = parts.next()?.trim();
        if extract_leaf_ident(left).eq_ignore_ascii_case(lhs_leaf) {
            return parse_state_rhs(right);
        }
        if extract_leaf_ident(right).eq_ignore_ascii_case(lhs_leaf) {
            return parse_state_rhs(left);
        }
    }
    None
}

pub(crate) fn infer_from_state_ancestors(
    node: Node<'_>,
    src: &[u8],
    lhs_leaf: &str,
    trigger: &str,
    owner_type: &str,
) -> (String, String) {
    let mut cur = node.parent();
    while let Some(p) = cur {
        let kind = p.kind();
        if is_boundary_kind(kind) {
            break;
        }
        if let Some((from_state, guard)) = match_branch_guard(p, src, lhs_leaf) {
            return (from_state, guard);
        }
        cur = p.parent();
    }
    if is_ctor_trigger(trigger, owner_type) || trigger.is_empty() {
        return ("(init)".to_string(), String::new());
    }
    ("*".to_string(), String::new())
}

fn match_branch_guard(node: Node<'_>, src: &[u8], lhs_leaf: &str) -> Option<(String, String)> {
    match node.kind() {
        "switch_section" | "switch_case" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if matches!(child.kind(), "case_switch_label" | "constant_pattern") {
                    let raw = node_text(child, src)
                        .trim()
                        .trim_start_matches("case")
                        .trim_end_matches(':')
                        .trim();
                    if let Some((_, val)) = parse_state_rhs(raw) {
                        return Some((val, compact_whitespace(node_text(child, src))));
                    }
                }
            }
            if let Some(val_n) = node.child_by_field_name("value") {
                if let Some((_, val)) = parse_state_rhs(node_text(val_n, src)) {
                    return Some((val.clone(), truncate_sig(&format!("case {val}"), 80)));
                }
            }
            None
        }
        "match_arm" => {
            let pat = node.child_by_field_name("pattern")?;
            let txt = compact_whitespace(node_text(pat, src));
            let first = txt.split('|').next().unwrap_or(&txt).trim();
            let (_, val) = parse_state_rhs(first)?;
            Some((val, truncate_sig(&txt, 80)))
        }
        "if_statement" | "if_expression" => {
            let cond = node.child_by_field_name("condition")?;
            let cond_txt = compact_whitespace(node_text(cond, src));
            let trimmed = cond_txt.trim_matches(|c| c == '(' || c == ')').trim();
            let (_, val) = extract_state_token_in_pattern(trimmed, lhs_leaf)?;
            Some((val, truncate_sig(trimmed, 80)))
        }
        _ => None,
    }
}
