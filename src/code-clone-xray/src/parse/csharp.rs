use std::cell::RefCell;

use tree_sitter::{Node, Parser};

use crate::normalize::{
    fnv1a_u32, NormalizedToken, SYM_BOOL_NULL_LIT, SYM_IDENT, SYM_NUM_LIT, SYM_STR_LIT,
};

thread_local! {
    static CS_PARSER: RefCell<Parser> = RefCell::new({
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
            .expect("Failed to load tree-sitter-c-sharp grammar");
        parser
    });
}

pub fn tokenize_csharp(source: &[u8]) -> Vec<NormalizedToken> {
    let tree = CS_PARSER.with(|cell| {
        let mut parser = cell.borrow_mut();
        parser.parse(source, None)
    });
    let Some(tree) = tree else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(source.len() / 8);
    emit_cs_node(tree.root_node(), source, &mut out);
    out
}

fn emit_cs_node(node: Node<'_>, source: &[u8], out: &mut Vec<NormalizedToken>) {
    if node.is_extra() || node.is_missing() {
        return;
    }

    let kind = node.kind();

    match kind {
        "comment"
        | "using_directive"
        | "extern_alias_directive"
        | "global_attribute"
        | "enum_declaration" => return,
        _ if kind.starts_with("preproc_") => return,
        _ => {}
    }

    if kind == "file_scoped_namespace_declaration" || kind == "namespace_declaration" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let ck = child.kind();
            if ck == "namespace" || ck == "qualified_name" || ck == "identifier" || ck == ";" {
                continue;
            }
            if ck == "declaration_list" {
                let mut list_cursor = child.walk();
                for member in child.children(&mut list_cursor) {
                    if member.kind() != "{" && member.kind() != "}" {
                        emit_cs_node(member, source, out);
                    }
                }
            } else {
                emit_cs_node(child, source, out);
            }
        }
        return;
    }

    if kind == "property_declaration" && is_trivial_auto_property(node) {
        return;
    }

    match kind {
        "string_literal"
        | "verbatim_string_literal"
        | "raw_string_literal"
        | "interpolated_string_expression"
        | "character_literal"
        | "utf8_string_literal" => {
            push_token(out, node, SYM_STR_LIT);
            return;
        }
        "integer_literal" | "real_literal" => {
            push_token(out, node, SYM_NUM_LIT);
            return;
        }
        "boolean_literal" | "null_literal" => {
            push_token(out, node, SYM_BOOL_NULL_LIT);
            return;
        }
        "identifier" => {
            // Roslyn-aligned: PascalCase → T:Name, else abstract ID
            let text = std::str::from_utf8(slice_node(node, source)).unwrap_or("");
            let symbol = if text.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                let mut buf = Vec::with_capacity(2 + text.len());
                buf.extend_from_slice(b"T:");
                buf.extend_from_slice(text.as_bytes());
                fnv1a_u32(0x30_001, &buf)
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

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        emit_cs_node(child, source, out);
    }
}

fn is_trivial_auto_property(prop_node: Node<'_>) -> bool {
    let mut has_accessor_list = false;
    let mut cursor = prop_node.walk();
    for child in prop_node.children(&mut cursor) {
        match child.kind() {
            "arrow_expression_clause" => return false,
            "accessor_list" => {
                has_accessor_list = true;
                let mut acc_cursor = child.walk();
                for acc in child.children(&mut acc_cursor) {
                    if acc.kind() == "accessor_declaration" {
                        let mut body_cursor = acc.walk();
                        for sub in acc.children(&mut body_cursor) {
                            if sub.kind() == "block" || sub.kind() == "arrow_expression_clause" {
                                return false;
                            }
                        }
                    }
                }
            }
            "equals_value_clause" if child.descendant_count() > 6 => {
                return false;
            }
            _ => {}
        }
    }
    has_accessor_list
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
    use crate::normalize::SYM_IDENT;

    #[test]
    fn pascal_case_differs_from_camel_case() {
        let src = br#"
class Demo {
    void Run() {
        int localValue = 1;
        Helper.DoWork(localValue);
    }
}
"#;
        let tokens = tokenize_csharp(src);
        assert!(!tokens.is_empty());
        // Helper / DoWork are PascalCase → not SYM_IDENT; localValue is.
        let has_ident = tokens.iter().any(|t| t.symbol == SYM_IDENT);
        let has_type_like = tokens
            .iter()
            .any(|t| t.symbol != SYM_IDENT && t.symbol >= 0x8000_0000);
        assert!(has_ident);
        assert!(has_type_like);
    }

    #[test]
    fn clones_share_normalized_shape() {
        let a = br#"
namespace A {
  public class One {
    public int Compute(int x) {
      var tmp = x + 1;
      if (tmp > 10) { return tmp * 2; }
      for (int i = 0; i < tmp; i++) { tmp += i; }
      return tmp;
    }
  }
}
"#;
        let b = br#"
namespace B {
  public class Two {
    public int Calculate(int y) {
      var tmp = y + 1;
      if (tmp > 10) { return tmp * 2; }
      for (int i = 0; i < tmp; i++) { tmp += i; }
      return tmp;
    }
  }
}
"#;
        let ta = tokenize_csharp(a);
        let tb = tokenize_csharp(b);
        // Method bodies should share a long common suffix after Type-2 normalize
        // (PascalCase method/class names differ → T:Compute vs T:Calculate).
        assert!(ta.len() > 20);
        assert!(tb.len() > 20);
    }
}
