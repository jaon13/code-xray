use tree_sitter::Node;

pub(crate) struct SymbolSpan {
    pub start_line: usize,
    pub end_line: usize,
    pub label: String,
    pub is_test: bool,
}

pub(crate) fn node_str<'a>(node: Node<'_>, src: &'a [u8]) -> &'a str {
    src.get(node.start_byte()..node.end_byte())
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
        .trim()
}

pub(crate) fn push_span(node: Node<'_>, label: String, is_test: bool, spans: &mut Vec<SymbolSpan>) {
    spans.push(SymbolSpan {
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        label,
        is_test,
    });
}

pub(crate) fn collect_ts_js_spans(
    node: Node<'_>,
    src: &[u8],
    scopes: &mut Vec<String>,
    spans: &mut Vec<SymbolSpan>,
) {
    let pushed = match node.kind() {
        "class_declaration"
        | "class"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration"
        | "function_declaration"
        | "generator_function_declaration"
        | "method_definition"
        | "method_signature"
        | "abstract_method_signature"
        | "public_field_definition"
        | "property_definition" => node
            .child_by_field_name("name")
            .map(|n| node_str(n, src).to_string()),
        "variable_declarator" => ts_declarator_name(node, src),
        _ => None,
    };

    if let Some(name) = pushed.filter(|s| !s.is_empty()) {
        scopes.push(name);
        push_span(node, scopes.join("."), false, spans);
        walk_ts_children(node, src, scopes, spans);
        scopes.pop();
        return;
    }

    walk_ts_children(node, src, scopes, spans);
}

fn walk_ts_children(
    node: Node<'_>,
    src: &[u8],
    scopes: &mut Vec<String>,
    spans: &mut Vec<SymbolSpan>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_ts_js_spans(child, src, scopes, spans);
    }
}

fn ts_declarator_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let val = node.child_by_field_name("value")?;
    if matches!(
        val.kind(),
        "arrow_function" | "function_expression" | "function" | "class" | "object"
    ) {
        return node
            .child_by_field_name("name")
            .map(|n| node_str(n, src).to_string());
    }
    None
}

pub(crate) fn collect_py_spans(
    node: Node<'_>,
    src: &[u8],
    scopes: &mut Vec<String>,
    spans: &mut Vec<SymbolSpan>,
) {
    let pushed = match node.kind() {
        "class_definition" | "function_definition" => node
            .child_by_field_name("name")
            .map(|n| node_str(n, src).to_string()),
        _ => None,
    };

    if let Some(name) = pushed.filter(|s| !s.is_empty()) {
        let label = if scopes.is_empty() && node.kind() == "function_definition" {
            format!("def {name}")
        } else if scopes.is_empty() {
            name.clone()
        } else {
            format!("{}.{name}", scopes.join("."))
        };
        push_span(node, label, false, spans);
        scopes.push(name);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_py_spans(child, src, scopes, spans);
        }
        scopes.pop();
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_py_spans(child, src, scopes, spans);
    }
}

pub(crate) fn collect_xaml_spans(
    node: Node<'_>,
    src: &[u8],
    scopes: &mut Vec<String>,
    spans: &mut Vec<SymbolSpan>,
) {
    if node.kind() == "element" {
        if let Some(label_part) = extract_xaml_element_label(node, src, scopes.is_empty()) {
            scopes.push(label_part);
            push_span(node, scopes.join(" > "), false, spans);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_xaml_spans(child, src, scopes, spans);
            }
            scopes.pop();
            return;
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_xaml_spans(child, src, scopes, spans);
    }
}

fn extract_xaml_element_label(node: Node<'_>, src: &[u8], is_root: bool) -> Option<String> {
    let mut cursor = node.walk();
    let tag_node = node
        .children(&mut cursor)
        .find(|c| matches!(c.kind(), "STag" | "EmptyElemTag"))?;

    let mut tag_name = String::new();
    let mut x_class = String::new();
    let mut key_or_name = String::new();

    let mut tc = tag_node.walk();
    for child in tag_node.children(&mut tc) {
        if child.kind() == "Name" {
            tag_name = node_str(child, src).to_string();
        } else if child.kind() == "Attribute" {
            let (k, v) = parse_xml_attr(child, src);
            if k == "x:Class" {
                x_class = v.rsplit('.').next().unwrap_or(&v).to_string();
            } else if key_or_name.is_empty() && matches!(k.as_str(), "x:Key" | "x:Name" | "Name") {
                key_or_name = v;
            }
        }
    }

    if !x_class.is_empty() {
        return Some(x_class);
    }
    if !key_or_name.is_empty() {
        return Some(format!("{tag_name}[{key_or_name}]"));
    }
    if is_root
        || matches!(
            tag_name.as_str(),
            "Style" | "ControlTemplate" | "DataTemplate" | "ResourceDictionary"
        )
    {
        return Some(tag_name);
    }
    None
}

fn parse_xml_attr(attr_node: Node<'_>, src: &[u8]) -> (String, String) {
    let mut key = String::new();
    let mut val = String::new();
    let mut cursor = attr_node.walk();
    for child in attr_node.children(&mut cursor) {
        if child.kind() == "Name" {
            key = node_str(child, src).to_string();
        } else if child.kind() == "AttValue" {
            let raw = node_str(child, src);
            val = raw.trim_matches('"').trim_matches('\'').to_string();
        }
    }
    (key, val)
}
