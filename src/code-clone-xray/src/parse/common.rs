use tree_sitter::{Language, Node, Parser};

use crate::normalize::{fnv1a_u32, NormalizedToken, SYM_IDENT};

#[inline]
pub(crate) fn init_parser(lang: &Language) -> Option<Parser> {
    let mut parser = Parser::new();
    if parser.set_language(lang).is_ok() {
        Some(parser)
    } else {
        None
    }
}

#[inline]
pub(crate) fn slice_node<'a>(node: Node<'_>, source: &'a [u8]) -> &'a [u8] {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or(&[])
}

#[inline]
pub(crate) fn push_token(out: &mut Vec<NormalizedToken>, node: Node<'_>, symbol: u32) {
    let start_line = (node.start_position().row as u32) + 1;
    let end_row = node.end_position().row as u32;
    let end_line = (end_row + 1).max(start_line);
    out.push(NormalizedToken {
        symbol,
        start_line,
        end_line,
    });
}

#[inline]
pub(crate) fn hash_prefixed(salt: u32, prefix: &[u8], bytes: &[u8]) -> u32 {
    let mut buf = Vec::with_capacity(prefix.len() + bytes.len());
    buf.extend_from_slice(prefix);
    buf.extend_from_slice(bytes);
    fnv1a_u32(salt, &buf)
}

#[inline]
pub(crate) fn normalize_pascal_or_ident(node: Node<'_>, source: &[u8]) -> u32 {
    let bytes = slice_node(node, source);
    if bytes.first().is_some_and(|b| b.is_ascii_uppercase()) {
        hash_prefixed(0x30_002, b"T:", bytes)
    } else {
        SYM_IDENT
    }
}

#[inline]
pub(crate) fn emit_children_with(
    node: Node<'_>,
    source: &[u8],
    out: &mut Vec<NormalizedToken>,
    visit: fn(Node<'_>, &[u8], &mut Vec<NormalizedToken>),
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit(child, source, out);
    }
}
