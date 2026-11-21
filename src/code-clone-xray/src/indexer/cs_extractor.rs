use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

use super::cs_member_extractor::{
    handle_callable_member, handle_enum_member, handle_field, handle_invocation,
    handle_object_creation, handle_property, handle_string_literal, FileCtx,
};
use super::cs_syntax_utils::{
    count_lines, extract_base_types, extract_generic_arg, extract_modifiers,
    extract_param_type_refs, is_interesting_type, line_1, node_text, split_type_tokens,
};
use super::model::{
    DiscoveredFile, ExtractedFileData, ExtractedMessengerLink, ExtractedType, ExtractedTypeRef,
};
use super::path_utils::fnv1a_128_hex;
use super::state_extractor::{extract_assignment_states, extract_field_init_states, StateContext};

pub fn extract_csharp_file(file: &DiscoveredFile, bytes: &[u8]) -> Result<ExtractedFileData> {
    let content = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .context("Failed to initialize tree-sitter-c-sharp")?;

    let tree = parser
        .parse(content, None)
        .context("tree-sitter-c-sharp failed to parse source")?;

    let mut ctx = FileCtx::default();
    walk_node(tree.root_node(), content, &mut ctx);

    Ok(ExtractedFileData {
        file: file.clone(),
        content_hash: fnv1a_128_hex(bytes),
        line_count: count_lines(content),
        types: ctx.types,
        members: ctx.members,
        calls: ctx.calls,
        type_refs: ctx.type_refs,
        di_regs: ctx.di_regs,
        messenger_links: ctx.messenger_links,
        state_transitions: ctx.state_transitions,
    })
}

fn walk_node(node: Node<'_>, src: &[u8], ctx: &mut FileCtx) {
    let kind = node.kind();
    match kind {
        "file_scoped_namespace_declaration" | "namespace_declaration" => {
            let prev = ctx.ns.clone();
            if let Some(name_node) = node.child_by_field_name("name") {
                let ns_text = node_text(name_node, src).trim();
                if !ns_text.is_empty() {
                    ctx.ns = if prev.is_empty() || kind == "file_scoped_namespace_declaration" {
                        ns_text.to_string()
                    } else {
                        format!("{prev}.{ns_text}")
                    };
                }
            }
            walk_children(node, src, ctx);
            if kind == "namespace_declaration" {
                ctx.ns = prev;
            }
            return;
        }
        "class_declaration"
        | "record_declaration"
        | "struct_declaration"
        | "interface_declaration"
        | "enum_declaration" => {
            handle_type_decl(node, src, ctx);
            return;
        }
        "method_declaration" | "constructor_declaration" | "local_function_statement" => {
            handle_callable_member(node, src, ctx, walk_children);
            return;
        }
        "property_declaration" => {
            handle_property(node, src, ctx, walk_children);
            return;
        }
        "enum_member_declaration" => handle_enum_member(node, src, ctx),
        "field_declaration" | "event_field_declaration" => {
            handle_field(node, src, ctx);
            let owner = ctx.type_stack.last().cloned().unwrap_or_default();
            let member = ctx.member_stack.last().cloned().unwrap_or_default();
            extract_field_init_states(
                node,
                StateContext {
                    src,
                    line_offset: 0,
                    owner_type: &owner,
                    trigger_member: &member,
                },
                &mut ctx.seen_states,
                &mut ctx.state_transitions,
            );
        }
        "assignment_expression" => {
            let owner = ctx.type_stack.last().cloned().unwrap_or_default();
            let member = ctx.member_stack.last().cloned().unwrap_or_default();
            extract_assignment_states(
                node,
                StateContext {
                    src,
                    line_offset: 0,
                    owner_type: &owner,
                    trigger_member: &member,
                },
                &mut ctx.seen_states,
                &mut ctx.state_transitions,
            );
        }
        "string_literal" => handle_string_literal(node, src, ctx),
        "invocation_expression" => handle_invocation(node, src, ctx),
        "object_creation_expression" => handle_object_creation(node, src, ctx),
        _ => {}
    }

    walk_children(node, src, ctx);
}

fn walk_children(node: Node<'_>, src: &[u8], ctx: &mut FileCtx) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(child, src, ctx);
    }
}

fn handle_type_decl(node: Node<'_>, src: &[u8], ctx: &mut FileCtx) {
    let Some(name_node) = node.child_by_field_name("name") else {
        walk_children(node, src, ctx);
        return;
    };
    let name = node_text(name_node, src).trim().to_string();
    let full_name = if let Some(parent) = ctx.type_stack.last() {
        format!("{parent}.{name}")
    } else if ctx.ns.is_empty() {
        name.clone()
    } else {
        format!("{}.{}", ctx.ns, name)
    };

    let (accessibility, is_partial) = extract_modifiers(node, src);
    let base_types = extract_base_types(node, src);
    let line = line_1(node);
    for bt in &base_types {
        if let Some(msg) = extract_generic_arg(bt, "IRecipient") {
            ctx.messenger_links.push(ExtractedMessengerLink {
                owner_type: full_name.clone(),
                direction: "receive".to_string(),
                message_type: msg,
                line,
            });
        }
        for tok in split_type_tokens(bt) {
            if is_interesting_type(&tok) && tok != name {
                ctx.type_refs.push(ExtractedTypeRef {
                    source_type: full_name.clone(),
                    target_type: tok,
                    ref_kind: "inherits".to_string(),
                    line,
                });
            }
        }
    }

    if let Some(plist) = node.child_by_field_name("parameters") {
        extract_param_type_refs(plist, src, &full_name, "ctor_param", &mut ctx.type_refs);
    }

    let kind = node
        .kind()
        .strip_suffix("_declaration")
        .unwrap_or(node.kind())
        .to_string();
    ctx.types.push(ExtractedType {
        namespace: ctx.ns.clone(),
        name,
        full_name: full_name.clone(),
        kind,
        accessibility,
        is_partial,
        base_types,
        start_line: line,
        end_line: (node.end_position().row as u32) + 1,
    });

    ctx.type_stack.push(full_name);
    walk_children(node, src, ctx);
    ctx.type_stack.pop();
}
