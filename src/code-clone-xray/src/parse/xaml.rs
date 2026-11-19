use std::cell::RefCell;

use tree_sitter::{Node, Parser};

use crate::normalize::{
    fnv1a_u32, NormalizedToken, SYM_XAML_ATTR_VAL, SYM_XAML_BINDING, SYM_XAML_DYN_RES,
    SYM_XAML_STATIC_RES, SYM_XAML_TEXT, SYM_XAML_TMPL_BIND,
};

thread_local! {
    static XAML_PARSER: RefCell<Parser> = RefCell::new({
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_xml::LANGUAGE_XML.into())
            .expect("Failed to load tree-sitter-xml grammar");
        parser
    });
}

const TAG_OPEN_SALT: u32 = 0x01;
const TAG_EMPTY_SALT: u32 = 0x02;
const TAG_CLOSE_SALT: u32 = 0x03;
const ATTR_NAME_SALT: u32 = 0x04;

pub fn tokenize_xaml(source: &[u8]) -> Vec<NormalizedToken> {
    let tree = XAML_PARSER.with(|cell| {
        let mut parser = cell.borrow_mut();
        parser.parse(source, None)
    });
    let Some(tree) = tree else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(source.len() / 12);
    emit_xaml_node(tree.root_node(), source, &mut out);
    out
}

fn emit_xaml_node(node: Node<'_>, source: &[u8], out: &mut Vec<NormalizedToken>) {
    if node.is_extra() || node.is_missing() {
        return;
    }

    match node.kind() {
        "Comment" | "XMLDecl" | "PI" | "doctypedecl" => return,
        "STag" => {
            emit_tag_with_attrs(node, source, TAG_OPEN_SALT, out);
            return;
        }
        "EmptyElemTag" => {
            emit_tag_with_attrs(node, source, TAG_EMPTY_SALT, out);
            return;
        }
        "ETag" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "Name" {
                    let bytes = slice_node(child, source);
                    let local = local_name(bytes);
                    push_xaml_token(out, child, fnv1a_u32(TAG_CLOSE_SALT, local));
                    break;
                }
            }
            return;
        }
        "CharData" | "CDSect" => {
            let bytes = slice_node(node, source);
            if bytes.iter().any(|b| !b.is_ascii_whitespace()) {
                push_xaml_token(out, node, SYM_XAML_TEXT);
            }
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        emit_xaml_node(child, source, out);
    }
}

fn emit_tag_with_attrs(
    tag_node: Node<'_>,
    source: &[u8],
    tag_salt: u32,
    out: &mut Vec<NormalizedToken>,
) {
    let mut cursor = tag_node.walk();
    for child in tag_node.children(&mut cursor) {
        match child.kind() {
            "Name" => {
                let tag_name = local_name(slice_node(child, source));
                push_xaml_token(out, child, fnv1a_u32(tag_salt, tag_name));
            }
            "Attribute" => {
                emit_attribute(child, source, out);
            }
            _ => {}
        }
    }
}

fn emit_attribute(attr_node: Node<'_>, source: &[u8], out: &mut Vec<NormalizedToken>) {
    let mut name_node: Option<Node<'_>> = None;
    let mut val_node: Option<Node<'_>> = None;

    let mut cursor = attr_node.walk();
    for child in attr_node.children(&mut cursor) {
        match child.kind() {
            "Name" => name_node = Some(child),
            "AttValue" => val_node = Some(child),
            _ => {}
        }
    }

    let Some(n_node) = name_node else {
        return;
    };
    let raw_name = std::str::from_utf8(slice_node(n_node, source)).unwrap_or("");
    if is_ignored_xaml_attribute(raw_name) {
        return;
    }

    push_xaml_token(out, n_node, fnv1a_u32(ATTR_NAME_SALT, raw_name.as_bytes()));

    if let Some(v_node) = val_node {
        let raw_val = std::str::from_utf8(slice_node(v_node, source)).unwrap_or("");
        let trimmed = raw_val.trim_matches(|c| c == '"' || c == '\'').trim_start();
        let val_sym = classify_xaml_value(trimmed);
        push_xaml_token(out, v_node, val_sym);
    }
}

fn local_name(bytes: &[u8]) -> &[u8] {
    if let Some(pos) = bytes.iter().position(|&b| b == b':') {
        &bytes[pos + 1..]
    } else {
        bytes
    }
}

fn is_ignored_xaml_attribute(attr_name: &str) -> bool {
    attr_name == "xmlns"
        || attr_name.starts_with("xmlns:")
        || attr_name == "x:Class"
        || attr_name == "mc:Ignorable"
        || attr_name.starts_with("d:")
}

fn classify_xaml_value(val: &str) -> u32 {
    if val.starts_with("{Binding") || val.starts_with("{x:Bind") {
        SYM_XAML_BINDING
    } else if val.starts_with("{StaticResource") {
        SYM_XAML_STATIC_RES
    } else if val.starts_with("{DynamicResource") {
        SYM_XAML_DYN_RES
    } else if val.starts_with("{TemplateBinding") {
        SYM_XAML_TMPL_BIND
    } else {
        SYM_XAML_ATTR_VAL
    }
}

#[inline]
fn slice_node<'a>(node: Node<'_>, source: &'a [u8]) -> &'a [u8] {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or(&[])
}

#[inline]
fn push_xaml_token(out: &mut Vec<NormalizedToken>, node: Node<'_>, symbol: u32) {
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
    fn parses_simple_xaml() {
        let src = br#"
<UserControl xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation">
  <Grid>
    <Button Content="{Binding Title}" Margin="8"/>
    <TextBlock Text="{Binding Subtitle}"/>
  </Grid>
</UserControl>
"#;
        let tokens = tokenize_xaml(src);
        assert!(tokens.len() >= 8);
    }
}
