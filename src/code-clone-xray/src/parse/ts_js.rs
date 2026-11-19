use std::cell::RefCell;

use tree_sitter::{Node, Parser};

use super::common::{
    emit_children_with, hash_prefixed, init_parser, normalize_pascal_or_ident, push_token,
    slice_node,
};
use crate::normalize::{NormalizedToken, SYM_BOOL_NULL_LIT, SYM_IDENT, SYM_NUM_LIT, SYM_STR_LIT};

thread_local! {
    static TS_PARSER: RefCell<Option<Parser>> =
        RefCell::new(init_parser(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()));
    static TSX_PARSER: RefCell<Option<Parser>> =
        RefCell::new(init_parser(&tree_sitter_typescript::LANGUAGE_TSX.into()));
    static JS_PARSER: RefCell<Option<Parser>> =
        RefCell::new(init_parser(&tree_sitter_javascript::LANGUAGE.into()));
}

pub fn tokenize_typescript(source: &[u8], is_tsx: bool) -> Vec<NormalizedToken> {
    let cell = if is_tsx { &TSX_PARSER } else { &TS_PARSER };
    tokenize_with_cell(cell, source)
}

pub fn tokenize_javascript(source: &[u8]) -> Vec<NormalizedToken> {
    tokenize_with_cell(&JS_PARSER, source)
}

fn tokenize_with_cell(
    cell: &'static std::thread::LocalKey<RefCell<Option<Parser>>>,
    source: &[u8],
) -> Vec<NormalizedToken> {
    let tree = cell.with(|c| {
        let mut guard = c.borrow_mut();
        guard.as_mut().and_then(|p| p.parse(source, None))
    });
    let Some(tree) = tree else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(source.len() / 8);
    emit_ts_js_node(tree.root_node(), source, &mut out);
    out
}

fn emit_ts_js_node(node: Node<'_>, source: &[u8], out: &mut Vec<NormalizedToken>) {
    if node.is_extra() || node.is_missing() {
        return;
    }

    let kind = node.kind();
    match kind {
        "comment" | "import_statement" | "hash_bang_line" => return,
        "string" | "template_string" | "regex" => {
            push_token(out, node, SYM_STR_LIT);
            return;
        }
        "number" => {
            push_token(out, node, SYM_NUM_LIT);
            return;
        }
        "true" | "false" | "null" | "undefined" => {
            push_token(out, node, SYM_BOOL_NULL_LIT);
            return;
        }
        "type_identifier" | "predefined_type" => {
            let bytes = slice_node(node, source);
            push_token(out, node, hash_prefixed(0x30_002, b"T:", bytes));
            return;
        }
        "identifier" | "property_identifier" | "shorthand_property_identifier" => {
            push_token(out, node, normalize_pascal_or_ident(node, source));
            return;
        }
        "shorthand_property_identifier_pattern" => {
            push_token(out, node, SYM_IDENT);
            return;
        }
        _ => {}
    }

    if node.child_count() == 0 {
        if !kind.trim().is_empty() {
            push_token(out, node, u32::from(node.kind_id()));
        }
        return;
    }

    emit_children_with(node, source, out, emit_ts_js_node);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_typescript_and_javascript() {
        let ts = br#"
import { foo } from './foo';
export function add(a: number, b: number): number {
    return a + b;
}
"#;
        let js = br#"
import { bar } from './bar';
export function sum(x, y) {
    return x + y;
}
"#;
        assert!(!tokenize_typescript(ts, false).is_empty());
        assert!(!tokenize_javascript(js).is_empty());
    }
}
