use anyhow::{Context, Result};
use rustc_hash::FxHashSet;
use tree_sitter::{Node, Parser};

use super::cs_member_extractor::FileCtx;
use super::cs_syntax_utils::{compact_whitespace, count_lines, line_1, node_text, truncate_sig};
use super::extract_utils::{
    build_file_data, collect_pascal_refs_from_text, derive_file_stem, derive_module_namespace,
    push_unique_ref, split_callee, strip_utf8_bom,
};
use super::model::{
    DiscoveredFile, ExtractedCall, ExtractedDiReg, ExtractedFileData, ExtractedMember,
    ExtractedMessengerLink, ExtractedType,
};
use super::state_extractor::{extract_assignment_states, StateContext};

struct PyWalker<'a> {
    src: &'a [u8],
    module_name: String,
    type_stack: Vec<String>,
    member_stack: Vec<String>,
    seen_refs: FxHashSet<(String, String, String)>,
    has_module_type: bool,
    ctx: FileCtx,
}

impl<'a> PyWalker<'a> {
    fn current_owner(&self) -> String {
        self.type_stack
            .last()
            .cloned()
            .unwrap_or_else(|| self.module_name.clone())
    }

    fn ensure_module_type(&mut self, line_count: u32) {
        if self.has_module_type {
            return;
        }
        self.has_module_type = true;
        let short = self
            .module_name
            .rsplit('.')
            .next()
            .unwrap_or(&self.module_name)
            .to_string();
        self.ctx.types.push(ExtractedType {
            namespace: self.module_name.clone(),
            name: short,
            full_name: self.module_name.clone(),
            kind: "module".to_string(),
            accessibility: "public".to_string(),
            is_partial: false,
            base_types: Vec::new(),
            start_line: 1,
            end_line: line_count.max(1),
        });
    }
}

pub fn extract_python_file(file: &DiscoveredFile, bytes: &[u8]) -> Result<ExtractedFileData> {
    let content = strip_utf8_bom(bytes);
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .context("Failed to initialize tree-sitter-python")?;
    let tree = parser
        .parse(content, None)
        .context("tree-sitter-python failed to parse source")?;

    let module_name = derive_module_namespace(&file.rel_path, ".");
    let line_count = count_lines(content);
    let mut walker = PyWalker {
        src: content,
        module_name,
        type_stack: Vec::new(),
        member_stack: Vec::new(),
        seen_refs: FxHashSet::default(),
        has_module_type: false,
        ctx: FileCtx::default(),
    };

    walk_py(tree.root_node(), &mut walker, line_count as u32);
    if walker.ctx.types.is_empty() {
        let fallback_short = derive_file_stem(&file.rel_path);
        walker.ctx.types.push(ExtractedType {
            namespace: walker.module_name.clone(),
            name: fallback_short,
            full_name: walker.module_name.clone(),
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

fn walk_py(node: Node<'_>, w: &mut PyWalker<'_>, total_lines: u32) {
    match node.kind() {
        "class_definition" => {
            handle_py_class(node, w, total_lines);
            return;
        }
        "function_definition" => {
            handle_py_function(node, w, total_lines);
            return;
        }
        "import_statement" | "import_from_statement" => handle_py_import(node, w),
        "assignment" => {
            let owner = w.current_owner();
            let member = w.member_stack.last().cloned().unwrap_or_default();
            extract_assignment_states(
                node,
                StateContext {
                    src: w.src,
                    line_offset: 0,
                    owner_type: &owner,
                    trigger_member: &member,
                },
                &mut w.ctx.seen_states,
                &mut w.ctx.state_transitions,
            );
        }
        "call" => handle_py_call(node, w),
        _ => {}
    }
    walk_py_children(node, w, total_lines);
}

fn walk_py_children(node: Node<'_>, w: &mut PyWalker<'_>, total_lines: u32) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_py(child, w, total_lines);
    }
}

fn handle_py_class(node: Node<'_>, w: &mut PyWalker<'_>, total_lines: u32) {
    let Some(name_n) = node.child_by_field_name("name") else {
        walk_py_children(node, w, total_lines);
        return;
    };
    let name = node_text(name_n, w.src).trim().to_string();
    let full = format!("{}.{}", w.module_name, name);
    let line = line_1(node);
    let mut base_types = Vec::new();
    if let Some(sup) = node.child_by_field_name("superclasses") {
        let raw = node_text(sup, w.src).trim_matches(|c| c == '(' || c == ')');
        for base in raw.split(',') {
            let clean = base.trim();
            if !clean.is_empty() {
                base_types.push(clean.to_string());
                collect_pascal_refs_from_text(
                    clean,
                    &full,
                    "inherits",
                    line,
                    &mut w.ctx,
                    &mut w.seen_refs,
                );
            }
        }
    }
    let acc = if name.starts_with('_') {
        "private"
    } else {
        "public"
    };
    w.ctx.types.push(ExtractedType {
        namespace: w.module_name.clone(),
        name,
        full_name: full.clone(),
        kind: "class".to_string(),
        accessibility: acc.to_string(),
        is_partial: false,
        base_types,
        start_line: line,
        end_line: (node.end_position().row as u32) + 1,
    });
    w.type_stack.push(full);
    walk_py_children(node, w, total_lines);
    w.type_stack.pop();
}

fn handle_py_function(node: Node<'_>, w: &mut PyWalker<'_>, total_lines: u32) {
    let Some(name_n) = node.child_by_field_name("name") else {
        walk_py_children(node, w, total_lines);
        return;
    };
    let name = node_text(name_n, w.src).trim().to_string();
    if w.type_stack.is_empty() {
        w.ensure_module_type(total_lines);
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
        format!("def {name}{params}")
    } else {
        format!("def {name}{params} -> {ret}")
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
    walk_py_children(node, w, total_lines);
    w.member_stack.pop();
}

fn handle_py_import(node: Node<'_>, w: &mut PyWalker<'_>) {
    let owner = w.current_owner();
    let line = line_1(node);
    if let Some(mod_n) = node.child_by_field_name("module_name") {
        let mod_txt = node_text(mod_n, w.src).trim().trim_start_matches('.');
        let tail = mod_txt.rsplit('.').next().unwrap_or(mod_txt);
        if !tail.is_empty() {
            push_unique_ref(&mut w.ctx, &mut w.seen_refs, &owner, tail, "import", line);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "dotted_name" | "aliased_import") {
            let txt = node_text(child, w.src);
            let sym = txt.split_whitespace().next().unwrap_or(txt);
            let tail = sym.rsplit('.').next().unwrap_or(sym).trim();
            if !tail.is_empty() {
                push_unique_ref(&mut w.ctx, &mut w.seen_refs, &owner, tail, "import", line);
            }
        }
    }
}

fn handle_py_call(node: Node<'_>, w: &mut PyWalker<'_>) {
    let Some(func_n) = node.child_by_field_name("function") else {
        return;
    };
    let (recv, callee) = split_callee(&compact_whitespace(node_text(func_n, w.src)));
    if callee.is_empty() {
        return;
    }
    let owner = w.current_owner();
    let line = line_1(node);
    w.ctx.calls.push(ExtractedCall {
        caller_type: owner.clone(),
        caller_member: w.member_stack.last().cloned().unwrap_or_default(),
        callee_name: callee.clone(),
        receiver_hint: recv,
        line,
    });
    if callee == "Depends" {
        let dep = node
            .child_by_field_name("arguments")
            .map(|a| {
                compact_whitespace(node_text(a, w.src))
                    .trim_matches(|c| c == '(' || c == ')')
                    .to_string()
            })
            .unwrap_or_default();
        if !dep.is_empty() {
            w.ctx.di_regs.push(ExtractedDiReg {
                lifetime: "Depends".to_string(),
                service_type: truncate_sig(&dep, 80),
                impl_type: owner,
                line,
            });
        }
    } else if matches!(callee.as_str(), "emit" | "send" | "publish" | "connect") {
        let msg = node
            .child_by_field_name("arguments")
            .map(|a| compact_whitespace(node_text(a, w.src)))
            .unwrap_or_default();
        let dir = if callee == "connect" {
            "receive"
        } else {
            "send"
        };
        w.ctx.messenger_links.push(ExtractedMessengerLink {
            owner_type: owner,
            direction: dir.to_string(),
            message_type: truncate_sig(&msg, 80),
            line,
        });
    }
}
