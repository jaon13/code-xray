use rustc_hash::FxHashSet;
use tree_sitter::Node;

use super::cs_patterns::extract_invocation_details;
use super::cs_syntax_utils::{
    clean_type_token, collect_type_identifiers, compact_whitespace, extract_param_type_refs,
    is_constant_key_literal, is_interesting_type, line_1, node_text, truncate_sig,
};
use super::model::{
    ExtractedCall, ExtractedDiReg, ExtractedMember, ExtractedMessengerLink, ExtractedType,
    ExtractedTypeRef,
};

#[derive(Default)]
pub struct FileCtx {
    pub ns: String,
    pub type_stack: Vec<String>,
    pub member_stack: Vec<String>,
    pub seen_keys: FxHashSet<(String, String)>,
    pub types: Vec<ExtractedType>,
    pub members: Vec<ExtractedMember>,
    pub calls: Vec<ExtractedCall>,
    pub type_refs: Vec<ExtractedTypeRef>,
    pub di_regs: Vec<ExtractedDiReg>,
    pub messenger_links: Vec<ExtractedMessengerLink>,
}

pub fn handle_invocation(node: Node<'_>, src: &[u8], ctx: &mut FileCtx) {
    let owner = ctx.type_stack.last().cloned().unwrap_or_default();
    let member = ctx.member_stack.last().cloned().unwrap_or_default();
    extract_invocation_details(
        node,
        src,
        &owner,
        &member,
        &mut ctx.calls,
        &mut ctx.di_regs,
        &mut ctx.messenger_links,
    );
}

pub fn handle_object_creation(node: Node<'_>, src: &[u8], ctx: &mut FileCtx) {
    let Some(ty) = node.child_by_field_name("type") else {
        return;
    };
    let target = clean_type_token(node_text(ty, src));
    if !is_interesting_type(&target) {
        return;
    }
    let owner = ctx.type_stack.last().cloned().unwrap_or_default();
    let member = ctx.member_stack.last().cloned().unwrap_or_default();
    ctx.calls.push(ExtractedCall {
        caller_type: owner.clone(),
        caller_member: member,
        callee_name: target,
        receiver_hint: "new".to_string(),
        line: line_1(node),
    });
    if !owner.is_empty() {
        collect_type_identifiers(ty, src, &owner, "new", &mut ctx.type_refs);
    }
}

pub fn handle_callable_member<F>(node: Node<'_>, src: &[u8], ctx: &mut FileCtx, walk_children: F)
where
    F: FnOnce(Node<'_>, &[u8], &mut FileCtx),
{
    let owner = ctx.type_stack.last().cloned().unwrap_or_default();
    let kind = node.kind();
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(n, src).trim().to_string())
        .unwrap_or_else(|| {
            if kind == "constructor_declaration" {
                owner.rsplit('.').next().unwrap_or(".ctor").to_string()
            } else {
                String::new()
            }
        });
    if name.is_empty() {
        walk_children(node, src, ctx);
        return;
    }

    let return_type = node
        .child_by_field_name("returns")
        .or_else(|| node.child_by_field_name("type"))
        .map(|n| node_text(n, src).trim().to_string())
        .unwrap_or_default();
    let params = node
        .child_by_field_name("parameters")
        .map(|p| {
            if kind == "constructor_declaration" {
                extract_param_type_refs(p, src, &owner, "ctor_param", &mut ctx.type_refs);
            }
            compact_whitespace(node_text(p, src))
        })
        .unwrap_or_else(|| "()".to_string());

    if kind != "local_function_statement" && !owner.is_empty() {
        let sig = if return_type.is_empty() {
            format!("{name}{params}")
        } else {
            format!("{return_type} {name}{params}")
        };
        ctx.members.push(ExtractedMember {
            owner_type: owner,
            name: name.clone(),
            kind: if kind == "constructor_declaration" {
                "constructor".to_string()
            } else {
                "method".to_string()
            },
            signature: truncate_sig(&sig, 200),
            return_type,
            start_line: line_1(node),
            end_line: (node.end_position().row as u32) + 1,
        });
    }

    ctx.member_stack.push(name);
    walk_children(node, src, ctx);
    ctx.member_stack.pop();
}

pub fn handle_property<F>(node: Node<'_>, src: &[u8], ctx: &mut FileCtx, walk_children: F)
where
    F: FnOnce(Node<'_>, &[u8], &mut FileCtx),
{
    let owner = ctx.type_stack.last().cloned().unwrap_or_default();
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = node_text(name_node, src).trim().to_string();
        let prop_type = node
            .child_by_field_name("type")
            .map(|t| {
                if !owner.is_empty() {
                    collect_type_identifiers(t, src, &owner, "property_type", &mut ctx.type_refs);
                }
                node_text(t, src).trim().to_string()
            })
            .unwrap_or_default();
        if !owner.is_empty() {
            ctx.members.push(ExtractedMember {
                owner_type: owner,
                name: name.clone(),
                kind: "property".to_string(),
                signature: truncate_sig(&format!("{prop_type} {name}"), 180),
                return_type: prop_type,
                start_line: line_1(node),
                end_line: (node.end_position().row as u32) + 1,
            });
        }
    }
    walk_children(node, src, ctx);
}

pub fn handle_field(node: Node<'_>, src: &[u8], ctx: &mut FileCtx) {
    let Some(owner) = ctx.type_stack.last() else {
        return;
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declaration" {
            if let Some(ty) = child.child_by_field_name("type") {
                collect_type_identifiers(ty, src, owner, "field_type", &mut ctx.type_refs);
            }
        }
    }
}

pub fn handle_enum_member(node: Node<'_>, src: &[u8], ctx: &mut FileCtx) {
    let Some(owner) = ctx.type_stack.last() else {
        return;
    };
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(n, src).trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    ctx.members.push(ExtractedMember {
        owner_type: owner.clone(),
        name: name.clone(),
        kind: "enum_member".to_string(),
        signature: name,
        return_type: owner.rsplit('.').next().unwrap_or(owner).to_string(),
        start_line: line_1(node),
        end_line: line_1(node),
    });
}

pub fn handle_string_literal(node: Node<'_>, src: &[u8], ctx: &mut FileCtx) {
    let Some(owner) = ctx.type_stack.last() else {
        return;
    };
    let raw = node_text(node, src).trim();
    if !(raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 5) {
        return;
    }
    let inner = &raw[1..raw.len() - 1];
    if !is_constant_key_literal(inner) {
        return;
    }
    if ctx.seen_keys.insert((owner.clone(), inner.to_string())) {
        let member = ctx.member_stack.last().cloned().unwrap_or_default();
        ctx.members.push(ExtractedMember {
            owner_type: owner.clone(),
            name: inner.to_string(),
            kind: "key_literal".to_string(),
            signature: if member.is_empty() {
                raw.to_string()
            } else {
                format!("{member}: {raw}")
            },
            return_type: String::new(),
            start_line: line_1(node),
            end_line: line_1(node),
        });
    }
}
