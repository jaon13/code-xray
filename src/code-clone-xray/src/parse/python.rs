use std::cell::RefCell;

use tree_sitter::{Node, Parser};

use super::common::{emit_children_with, init_parser, normalize_pascal_or_ident, push_token};
use crate::normalize::{NormalizedToken, SYM_BOOL_NULL_LIT, SYM_NUM_LIT, SYM_STR_LIT};

thread_local! {
    static PY_PARSER: RefCell<Option<Parser>> =
        RefCell::new(init_parser(&tree_sitter_python::LANGUAGE.into()));
}

pub fn tokenize_python(source: &[u8]) -> Vec<NormalizedToken> {
    let tree = PY_PARSER.with(|c| {
        let mut guard = c.borrow_mut();
        guard.as_mut().and_then(|p| p.parse(source, None))
    });
    let Some(tree) = tree else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(source.len() / 8);
    emit_py_node(tree.root_node(), source, &mut out);
    out
}

fn emit_py_node(node: Node<'_>, source: &[u8], out: &mut Vec<NormalizedToken>) {
    if node.is_extra() || node.is_missing() {
        return;
    }

    let kind = node.kind();
    match kind {
        "comment" | "import_statement" | "import_from_statement" | "future_import_statement" => {
            return;
        }
        "string" | "concatenated_string" => {
            push_token(out, node, SYM_STR_LIT);
            return;
        }
        "integer" | "float" => {
            push_token(out, node, SYM_NUM_LIT);
            return;
        }
        "true" | "false" | "none" => {
            push_token(out, node, SYM_BOOL_NULL_LIT);
            return;
        }
        "identifier" => {
            push_token(out, node, normalize_pascal_or_ident(node, source));
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

    emit_children_with(node, source, out, emit_py_node);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_python_functions() {
        let a = br#"
from pathlib import Path
def calc_total(items: list[int]) -> int:
    total = 0
    for item in items:
        total += item
    return total
"#;
        let b = br#"
import sys
def sum_values(nums: list[int]) -> int:
    acc = 10
    for n in nums:
        acc += n
    return acc
"#;
        let ta: Vec<u32> = tokenize_python(a).into_iter().map(|t| t.symbol).collect();
        let tb: Vec<u32> = tokenize_python(b).into_iter().map(|t| t.symbol).collect();
        assert!(!ta.is_empty());
        assert_eq!(ta, tb);
    }
}
