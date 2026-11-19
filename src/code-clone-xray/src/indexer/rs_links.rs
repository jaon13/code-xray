use rustc_hash::FxHashSet;
use tree_sitter::Node;

use super::cs_member_extractor::FileCtx;
use super::cs_syntax_utils::{compact_whitespace, line_1, node_text, truncate_sig};
use super::extract_utils::{push_unique_ref, split_callee};
use super::model::{ExtractedCall, ExtractedDiReg, ExtractedMessengerLink};

pub(super) fn extract_rs_use_refs(
    node: Node<'_>,
    src: &[u8],
    owner: &str,
    ctx: &mut FileCtx,
    seen_refs: &mut FxHashSet<(String, String, String)>,
) {
    let raw = node_text(node, src);
    let line = line_1(node);
    for seg in raw
        .replace([';', '{', '}', '\n', '\r'], " ")
        .split_whitespace()
    {
        if matches!(seg, "use" | "pub" | "as" | "crate" | "super" | "self" | "*") {
            continue;
        }
        let cleaned = seg.trim_end_matches(',');
        let tail = cleaned.rsplit("::").next().unwrap_or(cleaned).trim();
        if tail
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && tail != "self"
            && tail != "*"
        {
            push_unique_ref(ctx, seen_refs, owner, tail, "import", line);
        }
    }
}

pub(super) fn extract_rs_call_links(
    node: Node<'_>,
    src: &[u8],
    caller: &str,
    member: &str,
    ctx: &mut FileCtx,
) {
    let Some(func_n) = node.child_by_field_name("function") else {
        return;
    };
    let (recv, callee) = split_callee(&compact_whitespace(node_text(func_n, src)));
    if callee.is_empty() {
        return;
    }
    let line = line_1(node);
    ctx.calls.push(ExtractedCall {
        caller_type: caller.to_string(),
        caller_member: member.to_string(),
        callee_name: callee.clone(),
        receiver_hint: recv,
        line,
    });
    if matches!(callee.as_str(), "with_state" | "manage") {
        let arg = node
            .child_by_field_name("arguments")
            .map(|a| compact_whitespace(node_text(a, src)))
            .unwrap_or_default();
        if !arg.is_empty() {
            ctx.di_regs.push(ExtractedDiReg {
                lifetime: "State".to_string(),
                service_type: truncate_sig(&arg, 80),
                impl_type: caller.to_string(),
                line,
            });
        }
    } else if matches!(callee.as_str(), "emit" | "listen" | "send") {
        let msg = node
            .child_by_field_name("arguments")
            .map(|a| compact_whitespace(node_text(a, src)))
            .unwrap_or_default();
        if !msg.is_empty() {
            let dir = if callee == "listen" {
                "receive"
            } else {
                "send"
            };
            ctx.messenger_links.push(ExtractedMessengerLink {
                owner_type: caller.to_string(),
                direction: dir.to_string(),
                message_type: truncate_sig(&msg, 80),
                line,
            });
        }
    }
}

pub(super) fn rs_visibility(node: Node<'_>, src: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            let txt = node_text(child, src).trim();
            return if txt == "pub" {
                "public".to_string()
            } else {
                "internal".to_string()
            };
        }
    }
    "private".to_string()
}
