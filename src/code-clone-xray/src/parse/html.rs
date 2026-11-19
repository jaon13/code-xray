use std::cell::RefCell;

use tree_sitter::{Node, Parser};

use super::common::{emit_children_with, init_parser, push_token, slice_node};
use crate::normalize::{fnv1a_u32, NormalizedToken, SYM_STR_LIT, SYM_XAML_TEXT};

const HTML_TAG_SALT: u32 = 0x40_001;
const HTML_ATTR_SALT: u32 = 0x40_002;

thread_local! {
    static HTML_PARSER: RefCell<Option<Parser>> =
        RefCell::new(init_parser(&tree_sitter_html::LANGUAGE.into()));
}

pub fn tokenize_html(source: &[u8]) -> Vec<NormalizedToken> {
    let tree = HTML_PARSER.with(|c| {
        let mut guard = c.borrow_mut();
        guard.as_mut().and_then(|p| p.parse(source, None))
    });
    let Some(tree) = tree else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(source.len() / 10);
    emit_html_node(tree.root_node(), source, &mut out);
    out
}

fn emit_html_node(node: Node<'_>, source: &[u8], out: &mut Vec<NormalizedToken>) {
    if node.is_extra() || node.is_missing() {
        return;
    }

    match node.kind() {
        "comment" | "doctype" => return,
        "tag_name" => {
            let bytes = slice_node(node, source);
            push_token(out, node, fnv1a_u32(HTML_TAG_SALT, bytes));
            return;
        }
        "attribute_name" => {
            let bytes = slice_node(node, source);
            push_token(out, node, fnv1a_u32(HTML_ATTR_SALT, bytes));
            return;
        }
        "attribute_value" | "quoted_attribute_value" => {
            push_token(out, node, SYM_STR_LIT);
            return;
        }
        "text" | "raw_text" => {
            if slice_node(node, source)
                .iter()
                .any(|b| !b.is_ascii_whitespace())
            {
                push_token(out, node, SYM_XAML_TEXT);
            }
            return;
        }
        _ => {}
    }

    emit_children_with(node, source, out, emit_html_node);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_html_structure() {
        let src = br#"
<section class="card">
  <h2>Title A</h2>
  <p>Body text</p>
</section>
"#;
        assert!(!tokenize_html(src).is_empty());
    }
}
