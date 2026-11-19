use std::cell::RefCell;

use tree_sitter::{Node, Parser};

use super::common::{emit_children_with, init_parser, push_token, slice_node};
use crate::normalize::{fnv1a_u32, NormalizedToken, SYM_IDENT, SYM_NUM_LIT, SYM_STR_LIT};

const CSS_PROP_SALT: u32 = 0x50_001;

thread_local! {
    static CSS_PARSER: RefCell<Option<Parser>> =
        RefCell::new(init_parser(&tree_sitter_css::LANGUAGE.into()));
}

pub fn tokenize_css(source: &[u8]) -> Vec<NormalizedToken> {
    let tree = CSS_PARSER.with(|c| {
        let mut guard = c.borrow_mut();
        guard.as_mut().and_then(|p| p.parse(source, None))
    });
    let Some(tree) = tree else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(source.len() / 8);
    emit_css_node(tree.root_node(), source, &mut out);
    out
}

fn emit_css_node(node: Node<'_>, source: &[u8], out: &mut Vec<NormalizedToken>) {
    if node.is_extra() || node.is_missing() {
        return;
    }

    let kind = node.kind();
    match kind {
        "comment" | "import_statement" | "charset_statement" => return,
        "property_name" => {
            let bytes = slice_node(node, source);
            push_token(out, node, fnv1a_u32(CSS_PROP_SALT, bytes));
            return;
        }
        "class_name" | "id_name" | "identifier" => {
            push_token(out, node, SYM_IDENT);
            return;
        }
        "integer_value" | "float_value" | "color_value" => {
            push_token(out, node, SYM_NUM_LIT);
            return;
        }
        "string_value" => {
            push_token(out, node, SYM_STR_LIT);
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

    emit_children_with(node, source, out, emit_css_node);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_css_rules() {
        let src = br#"
.card-a {
  display: flex;
  gap: 8px;
  color: #111;
}
.card-b {
  display: flex;
  gap: 12px;
  color: #222;
}
"#;
        assert!(!tokenize_css(src).is_empty());
    }
}
