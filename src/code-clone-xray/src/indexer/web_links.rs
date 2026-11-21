use rustc_hash::FxHashSet;
use tree_sitter::Node;

use super::cs_member_extractor::FileCtx;
use super::cs_syntax_utils::{compact_whitespace, node_text, truncate_sig};
use super::extract_utils::{collect_pascal_refs_from_text, push_unique_ref, split_callee};
use super::model::{ExtractedCall, ExtractedDiReg, ExtractedMessengerLink};

/// Extracts import references from a TS/JS/Svelte `import_statement`.
pub fn extract_web_import(
    node: Node<'_>,
    src: &[u8],
    owner: &str,
    line: u32,
    ctx: &mut FileCtx,
    seen_refs: &mut FxHashSet<(String, String, String)>,
) {
    if let Some(src_n) = node.child_by_field_name("source") {
        let mod_spec = node_text(src_n, src).trim_matches(|c| c == '"' || c == '\'');
        let seg = mod_spec.rsplit('/').next().unwrap_or(mod_spec).trim();
        let clean = seg.split('.').next().unwrap_or(seg);
        if !clean.is_empty() && clean != "." && clean != ".." {
            push_unique_ref(ctx, seen_refs, owner, clean, "import", line);
        }
    }
    collect_pascal_refs_from_text(node_text(node, src), owner, "import", line, ctx, seen_refs);
}

/// Extracts calls, DI context providers, and event/messenger emissions from TS/JS/Svelte calls.
pub fn extract_web_call(
    node: Node<'_>,
    src: &[u8],
    owner: &str,
    caller_member: &str,
    line: u32,
    ctx: &mut FileCtx,
) {
    let target_node = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("constructor"));
    let Some(func_n) = target_node else {
        return;
    };
    let (recv, callee) = split_callee(&compact_whitespace(node_text(func_n, src)));
    if callee.is_empty() {
        return;
    }
    ctx.calls.push(ExtractedCall {
        caller_type: owner.to_string(),
        caller_member: caller_member.to_string(),
        callee_name: callee.clone(),
        receiver_hint: if node.kind() == "new_expression" {
            "new".to_string()
        } else {
            recv
        },
        line,
    });
    if matches!(callee.as_str(), "setContext" | "provide") {
        let arg = node
            .child_by_field_name("arguments")
            .map(|a| compact_whitespace(node_text(a, src)))
            .unwrap_or_default();
        ctx.di_regs.push(ExtractedDiReg {
            lifetime: "Context".to_string(),
            service_type: truncate_sig(&arg, 80),
            impl_type: owner.to_string(),
            line,
        });
    } else if matches!(
        callee.as_str(),
        "emit" | "dispatch" | "dispatchEvent" | "addEventListener"
    ) {
        let arg = node
            .child_by_field_name("arguments")
            .map(|a| compact_whitespace(node_text(a, src)))
            .unwrap_or_default();
        let dir = if callee == "addEventListener" {
            "receive"
        } else {
            "send"
        };
        ctx.messenger_links.push(ExtractedMessengerLink {
            owner_type: owner.to_string(),
            direction: dir.to_string(),
            message_type: truncate_sig(&arg, 80),
            line,
        });
    }
}
