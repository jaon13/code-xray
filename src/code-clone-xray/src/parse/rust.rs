use std::cell::RefCell;

use tree_sitter::{Node, Parser};

use crate::normalize::{
    fnv1a_u32, NormalizedToken, SYM_BOOL_NULL_LIT, SYM_IDENT, SYM_NUM_LIT, SYM_STR_LIT,
};

thread_local! {
    static RS_PARSER: RefCell<Option<Parser>> = RefCell::new({
        let mut parser = Parser::new();
        if parser.set_language(&tree_sitter_rust::LANGUAGE.into()).is_ok() {
            Some(parser)
        } else {
            None
        }
    });
}

pub fn tokenize_rust(source: &[u8], include_tests: bool) -> Vec<NormalizedToken> {
    let tree = RS_PARSER.with(|cell| {
        let mut guard = cell.borrow_mut();
        guard.as_mut().and_then(|parser| parser.parse(source, None))
    });
    let Some(tree) = tree else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(source.len() / 8);
    emit_rs_children(tree.root_node(), source, include_tests, &mut out);
    out
}

fn emit_rs_children(
    parent: Node<'_>,
    source: &[u8],
    include_tests: bool,
    out: &mut Vec<NormalizedToken>,
) {
    let mut cursor = parent.walk();
    let mut skip_next_item_for_cfg_test = false;

    for child in parent.children(&mut cursor) {
        if child.kind() == "attribute_item" {
            if !include_tests && is_cfg_test_attr(child, source) {
                skip_next_item_for_cfg_test = true;
            }
            continue;
        }
        if skip_next_item_for_cfg_test {
            if !child.is_extra() {
                skip_next_item_for_cfg_test = false;
                continue;
            }
            continue;
        }
        emit_rs_node(child, source, include_tests, out);
    }
}

fn emit_rs_node(
    node: Node<'_>,
    source: &[u8],
    include_tests: bool,
    out: &mut Vec<NormalizedToken>,
) {
    if node.is_extra() || node.is_missing() {
        return;
    }

    let kind = node.kind();
    match kind {
        "line_comment"
        | "block_comment"
        | "use_declaration"
        | "attribute_item"
        | "inner_attribute_item"
        | "enum_item"
        | "extern_crate_declaration" => return,
        "string_literal"
        | "raw_string_literal"
        | "char_literal"
        | "byte_literal"
        | "byte_string_literal"
        | "raw_byte_string_literal" => {
            push_token(out, node, SYM_STR_LIT);
            return;
        }
        "integer_literal" | "float_literal" => {
            push_token(out, node, SYM_NUM_LIT);
            return;
        }
        "boolean_literal" => {
            push_token(out, node, SYM_BOOL_NULL_LIT);
            return;
        }
        "type_identifier" | "primitive_type" => {
            let text = std::str::from_utf8(slice_node(node, source)).unwrap_or("");
            let mut buf = Vec::with_capacity(2 + text.len());
            buf.extend_from_slice(b"T:");
            buf.extend_from_slice(text.as_bytes());
            push_token(out, node, fnv1a_u32(0x30_002, &buf));
            return;
        }
        "identifier" | "field_identifier" => {
            let text = std::str::from_utf8(slice_node(node, source)).unwrap_or("");
            let symbol = if text.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                let mut buf = Vec::with_capacity(2 + text.len());
                buf.extend_from_slice(b"T:");
                buf.extend_from_slice(text.as_bytes());
                fnv1a_u32(0x30_002, &buf)
            } else {
                SYM_IDENT
            };
            push_token(out, node, symbol);
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

    emit_rs_children(node, source, include_tests, out);
}

#[inline]
fn is_cfg_test_attr(attr_node: Node<'_>, source: &[u8]) -> bool {
    let raw = std::str::from_utf8(slice_node(attr_node, source)).unwrap_or("");
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("cfg(test)") || compact == "#[test]"
}

#[inline]
fn slice_node<'a>(node: Node<'_>, source: &'a [u8]) -> &'a [u8] {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or(&[])
}

#[inline]
fn push_token(out: &mut Vec<NormalizedToken>, node: Node<'_>, symbol: u32) {
    let start_line = (node.start_position().row as u32) + 1;
    let end_row = node.end_position().row as u32;
    let end_line = (end_row + 1).max(start_line);
    out.push(NormalizedToken {
        symbol,
        start_line,
        end_line,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_cfg_test_module_when_include_tests_is_false() {
        let src = br#"
pub fn compute(a: &[f64]) -> f64 {
    let mut sum = 0.0;
    for &v in a {
        sum += v;
    }
    sum
}

#[cfg(test)]
mod tests {
    #[test]
    fn check_sum() {
        assert_eq!(super::compute(&[1.0, 2.0]), 3.0);
    }
}
"#;
        let prod_only = tokenize_rust(src, false);
        let with_tests = tokenize_rust(src, true);
        assert!(!prod_only.is_empty());
        assert!(with_tests.len() > prod_only.len());
    }
}
