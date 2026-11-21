use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

use super::cs_syntax_utils::{
    clean_type_token, count_lines, is_interesting_type, line_1, node_text,
};
use super::model::{DiscoveredFile, ExtractedFileData, ExtractedType, ExtractedTypeRef};
use super::path_utils::fnv1a_128_hex;

pub fn extract_xaml_file(file: &DiscoveredFile, bytes: &[u8]) -> Result<ExtractedFileData> {
    let content = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_xml::LANGUAGE_XML.into())
        .context("Failed to initialize tree-sitter-xml")?;

    let tree = parser
        .parse(content, None)
        .context("tree-sitter-xml failed to parse XAML")?;

    let mut x_class = String::new();
    let mut types = Vec::new();
    let mut type_refs = Vec::new();

    walk_xaml(tree.root_node(), content, &mut x_class, &mut type_refs);

    if !x_class.is_empty() {
        let short = x_class.rsplit('.').next().unwrap_or(&x_class).to_string();
        let ns = x_class
            .rsplit_once('.')
            .map(|(n, _)| n.to_string())
            .unwrap_or_default();
        types.push(ExtractedType {
            namespace: ns,
            name: short,
            full_name: x_class.clone(),
            kind: "xaml_view".to_string(),
            accessibility: "public".to_string(),
            is_partial: true,
            base_types: Vec::new(),
            start_line: 1,
            end_line: count_lines(content) as u32,
        });
    }

    Ok(ExtractedFileData {
        file: file.clone(),
        content_hash: fnv1a_128_hex(bytes),
        line_count: count_lines(content),
        types,
        members: Vec::new(),
        calls: Vec::new(),
        type_refs,
        di_regs: Vec::new(),
        messenger_links: Vec::new(),
        state_transitions: Vec::new(),
    })
}

fn walk_xaml(
    node: Node<'_>,
    src: &[u8],
    x_class: &mut String,
    type_refs: &mut Vec<ExtractedTypeRef>,
) {
    match node.kind() {
        "STag" | "EmptyElemTag" => {
            let line = line_1(node);
            let mut tag_name = String::new();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                match child.kind() {
                    "Name" => tag_name = node_text(child, src).to_string(),
                    "Attribute" => {
                        handle_attr(child, src, x_class, line, type_refs);
                    }
                    _ => {}
                }
            }
            if let Some((_, local)) = tag_name.split_once(':') {
                let owner = if x_class.is_empty() {
                    "XamlRoot"
                } else {
                    x_class.as_str()
                };
                if is_interesting_type(local) {
                    type_refs.push(ExtractedTypeRef {
                        source_type: owner.to_string(),
                        target_type: local.to_string(),
                        ref_kind: "xaml_control".to_string(),
                        line,
                    });
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_xaml(child, src, x_class, type_refs);
    }
}

fn handle_attr(
    attr_node: Node<'_>,
    src: &[u8],
    x_class: &mut String,
    line: u32,
    type_refs: &mut Vec<ExtractedTypeRef>,
) {
    let mut attr_name = String::new();
    let mut attr_val = String::new();
    let mut cursor = attr_node.walk();
    for child in attr_node.children(&mut cursor) {
        match child.kind() {
            "Name" => attr_name = node_text(child, src).to_string(),
            "AttValue" => {
                let raw = node_text(child, src).trim();
                attr_val = raw.trim_matches('"').trim_matches('\'').trim().to_string();
            }
            _ => {}
        }
    }

    if attr_name == "x:Class" && x_class.is_empty() && !attr_val.is_empty() {
        *x_class = attr_val;
        return;
    }

    let owner = if x_class.is_empty() {
        "XamlRoot"
    } else {
        x_class.as_str()
    };

    if attr_name == "DataType"
        || attr_name == "d:DataContext"
        || attr_name == "x:DataType"
        || attr_name == "TargetType"
    {
        if let Some(target) = extract_xtype_target(&attr_val) {
            type_refs.push(ExtractedTypeRef {
                source_type: owner.to_string(),
                target_type: target,
                ref_kind: "xaml_binding_type".to_string(),
                line,
            });
        }
    }
}

fn extract_xtype_target(val: &str) -> Option<String> {
    let candidate = if let Some(pos) = val.find("x:Type") {
        Some(val[pos + 6..].trim())
    } else if let Some(pos) = val.find("Type=") {
        Some(val[pos + 5..].trim())
    } else if let Some(pos) = val.find("DesignInstance") {
        Some(val[pos + 14..].trim().trim_start_matches("Type=").trim())
    } else if val.contains(':') && !val.contains('{') {
        Some(val.trim())
    } else {
        None
    }?;

    let ident = candidate
        .split(|c: char| c == '}' || c == ',' || c.is_whitespace())
        .next()
        .unwrap_or("");
    let short = ident.split(':').next_back().unwrap_or(ident);
    let tok = clean_type_token(short);
    is_interesting_type(&tok).then_some(tok)
}
