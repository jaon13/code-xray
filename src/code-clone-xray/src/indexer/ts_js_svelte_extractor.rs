use anyhow::{Context, Result};
use rustc_hash::FxHashSet;
use tree_sitter::{Node, Parser};

use super::cs_member_extractor::FileCtx;
use super::cs_syntax_utils::{compact_whitespace, count_lines, node_text, truncate_sig};
use super::extract_utils::{
    build_file_data, collect_pascal_refs_from_text, derive_file_stem, derive_module_namespace,
    push_unique_ref, split_callee, strip_utf8_bom,
};
use super::model::{
    DiscoveredFile, ExtractedCall, ExtractedDiReg, ExtractedFileData, ExtractedMember,
    ExtractedMessengerLink, ExtractedType,
};
use crate::parse::svelte::{extract_svelte_blocks, SvelteBlockKind};

struct WebWalker<'a> {
    src: &'a [u8],
    line_offset: u32,
    namespace: String,
    container_type: String,
    type_stack: Vec<String>,
    member_stack: Vec<String>,
    seen_refs: FxHashSet<(String, String, String)>,
    has_container: bool,
    ctx: FileCtx,
}

impl<'a> WebWalker<'a> {
    fn line_of(&self, node: Node<'_>) -> u32 {
        (node.start_position().row as u32) + 1 + self.line_offset
    }

    fn end_line_of(&self, node: Node<'_>) -> u32 {
        (node.end_position().row as u32) + 1 + self.line_offset
    }

    fn current_owner(&self) -> String {
        self.type_stack
            .last()
            .cloned()
            .unwrap_or_else(|| self.container_type.clone())
    }

    fn ensure_container(&mut self, kind: &str, total_lines: u32) {
        if self.has_container {
            return;
        }
        self.has_container = true;
        let name = self
            .container_type
            .rsplit('.')
            .next()
            .unwrap_or(&self.container_type)
            .to_string();
        self.ctx.types.push(ExtractedType {
            namespace: self.namespace.clone(),
            name,
            full_name: self.container_type.clone(),
            kind: kind.to_string(),
            accessibility: "public".to_string(),
            is_partial: false,
            base_types: Vec::new(),
            start_line: 1,
            end_line: total_lines.max(1),
        });
    }
}

pub fn extract_ts_js_svelte_file(file: &DiscoveredFile, bytes: &[u8]) -> Result<ExtractedFileData> {
    let content = strip_utf8_bom(bytes);
    let total_lines = count_lines(content);
    let ns = derive_module_namespace(&file.rel_path, ".");
    let stem = derive_file_stem(&file.rel_path);
    let container_type = if ns.ends_with(&stem) {
        ns.clone()
    } else {
        format!("{ns}.{stem}")
    };

    let mut walker = WebWalker {
        src: content,
        line_offset: 0,
        namespace: ns,
        container_type,
        type_stack: Vec::new(),
        member_stack: Vec::new(),
        seen_refs: FxHashSet::default(),
        has_container: false,
        ctx: FileCtx::default(),
    };

    if file.lang == "svelte" {
        walker.ensure_container("svelte_component", total_lines as u32);
        for block in extract_svelte_blocks(content) {
            if block.kind != SvelteBlockKind::Script {
                continue;
            }
            let lang = if block.is_ts { "ts" } else { "js" };
            parse_and_walk_script(
                &mut walker,
                block.body,
                lang,
                block.start_line_0 as u32,
                total_lines as u32,
            )?;
        }
    } else {
        parse_and_walk_script(&mut walker, content, file.lang, 0, total_lines as u32)?;
        if walker.ctx.types.is_empty() {
            walker.ensure_container("module", total_lines as u32);
        }
    }

    Ok(build_file_data(file, bytes, total_lines, walker.ctx))
}

fn parse_and_walk_script<'a>(
    walker: &mut WebWalker<'a>,
    code: &'a [u8],
    lang: &str,
    line_offset: u32,
    total_lines: u32,
) -> Result<()> {
    let ts_lang = match lang {
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        _ => tree_sitter_javascript::LANGUAGE.into(),
    };
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("Failed to initialize tree-sitter JS/TS parser")?;
    let tree = parser
        .parse(code, None)
        .context("tree-sitter failed to parse JS/TS source")?;
    let container_kind = if walker.has_container {
        "svelte_component"
    } else {
        "module"
    };
    walker.src = code;
    walker.line_offset = line_offset;
    walk_web(tree.root_node(), walker, false, container_kind, total_lines);
    Ok(())
}

fn walk_web(
    node: Node<'_>,
    w: &mut WebWalker<'_>,
    exported: bool,
    container_kind: &str,
    total_lines: u32,
) {
    let kind = node.kind();
    if kind == "export_statement" {
        walk_web_children(node, w, true, container_kind, total_lines);
        return;
    }
    match kind {
        "class_declaration"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration" => {
            handle_web_type(node, w, exported, container_kind, total_lines);
            return;
        }
        "method_definition"
        | "method_signature"
        | "abstract_method_signature"
        | "function_declaration"
        | "generator_function_declaration" => {
            handle_web_callable(node, w, container_kind, total_lines);
            return;
        }
        "variable_declarator" => {
            if handle_web_declarator(node, w, container_kind, total_lines) {
                return;
            }
        }
        "public_field_definition" | "property_definition" => handle_web_field(node, w),
        "import_statement" => handle_web_import(node, w),
        "call_expression" | "new_expression" => handle_web_call(node, w),
        _ => {}
    }
    walk_web_children(node, w, exported, container_kind, total_lines);
}

fn walk_web_children(
    node: Node<'_>,
    w: &mut WebWalker<'_>,
    exported: bool,
    container_kind: &str,
    total_lines: u32,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_web(child, w, exported, container_kind, total_lines);
    }
}

fn handle_web_type(
    node: Node<'_>,
    w: &mut WebWalker<'_>,
    exported: bool,
    container_kind: &str,
    total_lines: u32,
) {
    let Some(name_n) = node.child_by_field_name("name") else {
        walk_web_children(node, w, false, container_kind, total_lines);
        return;
    };
    let name = node_text(name_n, w.src).trim().to_string();
    let full = format!("{}.{}", w.namespace, name);
    let line = w.line_of(node);
    let kind = match node.kind() {
        "class_declaration" => "class",
        "interface_declaration" => "interface",
        "type_alias_declaration" => "type_alias",
        "enum_declaration" => "enum",
        other => other,
    };

    let mut base_types = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_heritage" | "extends_clause" | "extends_type_clause"
        ) {
            let text = node_text(child, w.src);
            collect_pascal_refs_from_text(
                text,
                &full,
                "inherits",
                line,
                &mut w.ctx,
                &mut w.seen_refs,
            );
            base_types.push(compact_whitespace(text));
        }
    }
    if let Some(val_ty) = node.child_by_field_name("value") {
        collect_pascal_refs_from_text(
            node_text(val_ty, w.src),
            &full,
            "alias",
            line,
            &mut w.ctx,
            &mut w.seen_refs,
        );
    }

    w.ctx.types.push(ExtractedType {
        namespace: w.namespace.clone(),
        name,
        full_name: full.clone(),
        kind: kind.to_string(),
        accessibility: if exported { "public" } else { "internal" }.to_string(),
        is_partial: false,
        base_types,
        start_line: line,
        end_line: w.end_line_of(node),
    });

    w.type_stack.push(full);
    walk_web_children(node, w, false, container_kind, total_lines);
    w.type_stack.pop();
}

fn handle_web_callable(
    node: Node<'_>,
    w: &mut WebWalker<'_>,
    container_kind: &str,
    total_lines: u32,
) {
    let Some(name_n) = node.child_by_field_name("name") else {
        walk_web_children(node, w, false, container_kind, total_lines);
        return;
    };
    let name = node_text(name_n, w.src).trim().to_string();
    if w.type_stack.is_empty() {
        w.ensure_container(container_kind, total_lines);
    }
    let owner = w.current_owner();
    let params = node
        .child_by_field_name("parameters")
        .map(|p| compact_whitespace(node_text(p, w.src)))
        .unwrap_or_else(|| "()".to_string());
    let ret = node
        .child_by_field_name("return_type")
        .map(|r| {
            let txt = node_text(r, w.src).trim().trim_start_matches(':').trim();
            collect_pascal_refs_from_text(
                txt,
                &owner,
                "return_type",
                w.line_of(r),
                &mut w.ctx,
                &mut w.seen_refs,
            );
            compact_whitespace(txt)
        })
        .unwrap_or_default();
    let kind = if w.type_stack.is_empty() {
        "function"
    } else {
        "method"
    };
    w.ctx.members.push(ExtractedMember {
        owner_type: owner,
        name: name.clone(),
        kind: kind.to_string(),
        signature: truncate_sig(&format!("{name}{params}"), 200),
        return_type: ret,
        start_line: w.line_of(node),
        end_line: w.end_line_of(node),
    });
    w.member_stack.push(name);
    walk_web_children(node, w, false, container_kind, total_lines);
    w.member_stack.pop();
}

fn handle_web_declarator(
    node: Node<'_>,
    w: &mut WebWalker<'_>,
    container_kind: &str,
    total_lines: u32,
) -> bool {
    let Some(name_n) = node.child_by_field_name("name") else {
        return false;
    };
    let Some(val_n) = node.child_by_field_name("value") else {
        return false;
    };
    let val_kind = val_n.kind();
    let val_txt = node_text(val_n, w.src);
    let is_svelte_props = container_kind == "svelte_component" && val_txt.contains("$props");
    if !matches!(
        val_kind,
        "arrow_function" | "function_expression" | "function"
    ) && !is_svelte_props
    {
        return false;
    }
    w.ensure_container(container_kind, total_lines);
    let owner = w.current_owner();
    let name = compact_whitespace(node_text(name_n, w.src));
    let kind = if is_svelte_props { "prop" } else { "function" };
    w.ctx.members.push(ExtractedMember {
        owner_type: owner,
        name: name.clone(),
        kind: kind.to_string(),
        signature: truncate_sig(&name, 160),
        return_type: String::new(),
        start_line: w.line_of(node),
        end_line: w.end_line_of(node),
    });
    w.member_stack.push(name);
    walk_web_children(val_n, w, false, container_kind, total_lines);
    w.member_stack.pop();
    true
}

fn handle_web_field(node: Node<'_>, w: &mut WebWalker<'_>) {
    let Some(owner) = w.type_stack.last().cloned() else {
        return;
    };
    let Some(name_n) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_n, w.src).trim().to_string();
    let ty = node
        .child_by_field_name("type")
        .map(|t| compact_whitespace(node_text(t, w.src).trim_start_matches(':')))
        .unwrap_or_default();
    w.ctx.members.push(ExtractedMember {
        owner_type: owner,
        name: name.clone(),
        kind: "property".to_string(),
        signature: truncate_sig(&format!("{name}: {ty}"), 180),
        return_type: ty,
        start_line: w.line_of(node),
        end_line: w.end_line_of(node),
    });
}

fn handle_web_import(node: Node<'_>, w: &mut WebWalker<'_>) {
    let owner = w.current_owner();
    let line = w.line_of(node);
    if let Some(src_n) = node.child_by_field_name("source") {
        let mod_spec = node_text(src_n, w.src).trim_matches(|c| c == '"' || c == '\'');
        let seg = mod_spec.rsplit('/').next().unwrap_or(mod_spec).trim();
        let clean = seg.split('.').next().unwrap_or(seg);
        if !clean.is_empty() && clean != "." && clean != ".." {
            push_unique_ref(&mut w.ctx, &mut w.seen_refs, &owner, clean, "import", line);
        }
    }
    collect_pascal_refs_from_text(
        node_text(node, w.src),
        &owner,
        "import",
        line,
        &mut w.ctx,
        &mut w.seen_refs,
    );
}

fn handle_web_call(node: Node<'_>, w: &mut WebWalker<'_>) {
    let target_node = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("constructor"));
    let Some(func_n) = target_node else {
        return;
    };
    let (recv, callee) = split_callee(&compact_whitespace(node_text(func_n, w.src)));
    if callee.is_empty() {
        return;
    }
    let owner = w.current_owner();
    let line = w.line_of(node);
    w.ctx.calls.push(ExtractedCall {
        caller_type: owner.clone(),
        caller_member: w.member_stack.last().cloned().unwrap_or_default(),
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
            .map(|a| compact_whitespace(node_text(a, w.src)))
            .unwrap_or_default();
        w.ctx.di_regs.push(ExtractedDiReg {
            lifetime: "Context".to_string(),
            service_type: truncate_sig(&arg, 80),
            impl_type: owner.clone(),
            line,
        });
    } else if matches!(
        callee.as_str(),
        "emit" | "dispatch" | "dispatchEvent" | "addEventListener"
    ) {
        let arg = node
            .child_by_field_name("arguments")
            .map(|a| compact_whitespace(node_text(a, w.src)))
            .unwrap_or_default();
        let dir = if callee == "addEventListener" {
            "receive"
        } else {
            "send"
        };
        w.ctx.messenger_links.push(ExtractedMessengerLink {
            owner_type: owner,
            direction: dir.to_string(),
            message_type: truncate_sig(&arg, 80),
            line,
        });
    }
}
