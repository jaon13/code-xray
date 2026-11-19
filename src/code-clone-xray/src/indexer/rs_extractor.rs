use anyhow::{Context, Result};
use rustc_hash::FxHashSet;
use tree_sitter::{Node, Parser};

use super::cs_member_extractor::FileCtx;
use super::cs_syntax_utils::{compact_whitespace, count_lines, line_1, node_text, truncate_sig};
use super::extract_utils::{
    build_file_data, collect_pascal_refs_from_text, derive_file_stem, derive_module_namespace,
    push_unique_ref, strip_generic_args, strip_utf8_bom,
};
use super::model::{DiscoveredFile, ExtractedFileData, ExtractedMember, ExtractedType};
use super::rs_links::{extract_rs_call_links, extract_rs_use_refs, rs_visibility};

struct RsWalker<'a> {
    src: &'a [u8],
    base_mod: String,
    module_stack: Vec<String>,
    type_stack: Vec<String>,
    member_stack: Vec<String>,
    seen_refs: FxHashSet<(String, String, String)>,
    has_module_container: bool,
    ctx: FileCtx,
}

impl<'a> RsWalker<'a> {
    fn current_mod(&self) -> String {
        if self.module_stack.is_empty() {
            self.base_mod.clone()
        } else {
            format!("{}::{}", self.base_mod, self.module_stack.join("::"))
        }
    }

    fn current_owner(&self) -> String {
        self.type_stack
            .last()
            .cloned()
            .unwrap_or_else(|| self.current_mod())
    }

    fn qualify(&self, short: &str) -> String {
        let cur = self.current_mod();
        if cur.is_empty() || short.contains("::") {
            short.to_string()
        } else {
            format!("{cur}::{short}")
        }
    }

    fn ensure_module_container(&mut self, line_count: u32) {
        if self.has_module_container {
            return;
        }
        self.has_module_container = true;
        let full = self.current_mod();
        let short = full.rsplit("::").next().unwrap_or(&full).to_string();
        self.ctx.types.push(ExtractedType {
            namespace: full.clone(),
            name: short,
            full_name: full,
            kind: "module".to_string(),
            accessibility: "public".to_string(),
            is_partial: false,
            base_types: Vec::new(),
            start_line: 1,
            end_line: line_count.max(1),
        });
    }
}

pub fn extract_rust_file(file: &DiscoveredFile, bytes: &[u8]) -> Result<ExtractedFileData> {
    let content = strip_utf8_bom(bytes);
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("Failed to initialize tree-sitter-rust")?;
    let tree = parser
        .parse(content, None)
        .context("tree-sitter-rust failed to parse source")?;

    let base_mod = derive_module_namespace(&file.rel_path, "::");
    let line_count = count_lines(content);
    let mut walker = RsWalker {
        src: content,
        base_mod,
        module_stack: Vec::new(),
        type_stack: Vec::new(),
        member_stack: Vec::new(),
        seen_refs: FxHashSet::default(),
        has_module_container: false,
        ctx: FileCtx::default(),
    };

    walk_rs(tree.root_node(), &mut walker, line_count as u32);
    if walker.ctx.types.is_empty() && !walker.ctx.members.is_empty() {
        walker.ensure_module_container(line_count as u32);
    } else if walker.ctx.types.is_empty() {
        let fallback_name = derive_file_stem(&file.rel_path);
        walker.ctx.types.push(ExtractedType {
            namespace: walker.base_mod.clone(),
            name: fallback_name,
            full_name: walker.base_mod.clone(),
            kind: "module".to_string(),
            accessibility: "public".to_string(),
            is_partial: false,
            base_types: Vec::new(),
            start_line: 1,
            end_line: line_count.max(1) as u32,
        });
    }
    Ok(build_file_data(file, bytes, line_count, walker.ctx))
}

fn walk_rs(node: Node<'_>, w: &mut RsWalker<'_>, total_lines: u32) {
    let kind = node.kind();
    match kind {
        "mod_item" => {
            if let Some(name_n) = node.child_by_field_name("name") {
                let name = node_text(name_n, w.src).trim().to_string();
                if !name.is_empty() && node.child_by_field_name("body").is_some() {
                    let full = w.qualify(&name);
                    w.ctx.types.push(ExtractedType {
                        namespace: w.current_mod(),
                        name: name.clone(),
                        full_name: full,
                        kind: "module".to_string(),
                        accessibility: rs_visibility(node, w.src),
                        is_partial: false,
                        base_types: Vec::new(),
                        start_line: line_1(node),
                        end_line: (node.end_position().row as u32) + 1,
                    });
                    w.module_stack.push(name);
                    walk_rs_children(node, w, total_lines);
                    w.module_stack.pop();
                    return;
                }
            }
        }
        "struct_item" | "enum_item" | "trait_item" | "type_item" | "union_item" => {
            handle_rs_type(node, w, total_lines);
            return;
        }
        "impl_item" => {
            handle_rs_impl(node, w, total_lines);
            return;
        }
        "function_item" | "function_signature_item" => {
            handle_rs_function(node, w, total_lines);
            return;
        }
        "field_declaration" => handle_rs_field(node, w),
        "enum_variant" => handle_rs_enum_variant(node, w),
        "const_item" | "static_item" => handle_rs_const(node, w, total_lines),
        "use_declaration" => {
            let owner = w.current_owner();
            extract_rs_use_refs(node, w.src, &owner, &mut w.ctx, &mut w.seen_refs);
        }
        "call_expression" => {
            let caller = w.current_owner();
            let member = w.member_stack.last().cloned().unwrap_or_default();
            extract_rs_call_links(node, w.src, &caller, &member, &mut w.ctx);
        }
        _ => {}
    }
    walk_rs_children(node, w, total_lines);
}

fn walk_rs_children(node: Node<'_>, w: &mut RsWalker<'_>, total_lines: u32) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_rs(child, w, total_lines);
    }
}

fn handle_rs_type(node: Node<'_>, w: &mut RsWalker<'_>, total_lines: u32) {
    let Some(name_n) = node.child_by_field_name("name") else {
        walk_rs_children(node, w, total_lines);
        return;
    };
    let name = node_text(name_n, w.src).trim().to_string();
    let ns = w.current_mod();
    let full = w.qualify(&name);
    let line = line_1(node);
    let kind = match node.kind() {
        "struct_item" => "struct",
        "enum_item" => "enum",
        "trait_item" => "trait",
        "type_item" => "type_alias",
        "union_item" => "union",
        other => other,
    }
    .to_string();
    if let Some(ty) = node.child_by_field_name("type") {
        collect_pascal_refs_from_text(
            node_text(ty, w.src),
            &full,
            "alias",
            line,
            &mut w.ctx,
            &mut w.seen_refs,
        );
    }
    w.ctx.types.push(ExtractedType {
        namespace: ns,
        name,
        full_name: full.clone(),
        kind,
        accessibility: rs_visibility(node, w.src),
        is_partial: false,
        base_types: Vec::new(),
        start_line: line,
        end_line: (node.end_position().row as u32) + 1,
    });
    w.type_stack.push(full);
    walk_rs_children(node, w, total_lines);
    w.type_stack.pop();
}

fn handle_rs_impl(node: Node<'_>, w: &mut RsWalker<'_>, total_lines: u32) {
    let raw_target = node
        .child_by_field_name("type")
        .map(|n| strip_generic_args(node_text(n, w.src)))
        .unwrap_or("")
        .to_string();
    if raw_target.is_empty() {
        walk_rs_children(node, w, total_lines);
        return;
    }
    let owner = w.qualify(&raw_target);
    let line = line_1(node);
    if let Some(tr_node) = node.child_by_field_name("trait") {
        let trait_name = strip_generic_args(node_text(tr_node, w.src));
        if !trait_name.is_empty() {
            push_unique_ref(
                &mut w.ctx,
                &mut w.seen_refs,
                &owner,
                trait_name,
                "implements",
                line,
            );
        }
    }
    w.type_stack.push(owner);
    walk_rs_children(node, w, total_lines);
    w.type_stack.pop();
}

fn handle_rs_function(node: Node<'_>, w: &mut RsWalker<'_>, total_lines: u32) {
    let Some(name_n) = node.child_by_field_name("name") else {
        walk_rs_children(node, w, total_lines);
        return;
    };
    let name = node_text(name_n, w.src).trim().to_string();
    if w.type_stack.is_empty() {
        w.ensure_module_container(total_lines);
    }
    let owner = w.current_owner();
    let params = node
        .child_by_field_name("parameters")
        .map(|p| {
            let text = node_text(p, w.src);
            collect_pascal_refs_from_text(
                text,
                &owner,
                "param",
                line_1(p),
                &mut w.ctx,
                &mut w.seen_refs,
            );
            compact_whitespace(text)
        })
        .unwrap_or_else(|| "()".to_string());
    let ret = node
        .child_by_field_name("return_type")
        .map(|r| {
            let text = node_text(r, w.src).trim();
            collect_pascal_refs_from_text(
                text,
                &owner,
                "return_type",
                line_1(r),
                &mut w.ctx,
                &mut w.seen_refs,
            );
            compact_whitespace(text)
        })
        .unwrap_or_default();
    let sig = if ret.is_empty() {
        format!("fn {name}{params}")
    } else {
        format!("fn {name}{params} -> {ret}")
    };
    let kind = if w.type_stack.is_empty() {
        "function"
    } else {
        "method"
    };
    w.ctx.members.push(ExtractedMember {
        owner_type: owner,
        name: name.clone(),
        kind: kind.to_string(),
        signature: truncate_sig(&sig, 200),
        return_type: ret,
        start_line: line_1(node),
        end_line: (node.end_position().row as u32) + 1,
    });
    w.member_stack.push(name);
    walk_rs_children(node, w, total_lines);
    w.member_stack.pop();
}

fn handle_rs_field(node: Node<'_>, w: &mut RsWalker<'_>) {
    let Some(owner) = w.type_stack.last().cloned() else {
        return;
    };
    let Some(name_n) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_n, w.src).trim().to_string();
    let field_ty = node
        .child_by_field_name("type")
        .map(|t| {
            let raw = node_text(t, w.src);
            collect_pascal_refs_from_text(
                raw,
                &owner,
                "field_type",
                line_1(node),
                &mut w.ctx,
                &mut w.seen_refs,
            );
            compact_whitespace(raw)
        })
        .unwrap_or_default();
    w.ctx.members.push(ExtractedMember {
        owner_type: owner,
        name: name.clone(),
        kind: "field".to_string(),
        signature: truncate_sig(&format!("{name}: {field_ty}"), 180),
        return_type: field_ty,
        start_line: line_1(node),
        end_line: (node.end_position().row as u32) + 1,
    });
}

fn handle_rs_enum_variant(node: Node<'_>, w: &mut RsWalker<'_>) {
    let Some(owner) = w.type_stack.last().cloned() else {
        return;
    };
    let Some(name_n) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_n, w.src).trim().to_string();
    w.ctx.members.push(ExtractedMember {
        owner_type: owner,
        name: name.clone(),
        kind: "enum_member".to_string(),
        signature: name,
        return_type: String::new(),
        start_line: line_1(node),
        end_line: (node.end_position().row as u32) + 1,
    });
}

fn handle_rs_const(node: Node<'_>, w: &mut RsWalker<'_>, total_lines: u32) {
    let Some(name_n) = node.child_by_field_name("name") else {
        return;
    };
    if w.type_stack.is_empty() {
        w.ensure_module_container(total_lines);
    }
    let name = node_text(name_n, w.src).trim().to_string();
    let owner = w.current_owner();
    let ret = node
        .child_by_field_name("type")
        .map(|t| compact_whitespace(node_text(t, w.src)))
        .unwrap_or_default();
    w.ctx.members.push(ExtractedMember {
        owner_type: owner,
        name: name.clone(),
        kind: "const".to_string(),
        signature: truncate_sig(&format!("{name}: {ret}"), 180),
        return_type: ret,
        start_line: line_1(node),
        end_line: (node.end_position().row as u32) + 1,
    });
}
