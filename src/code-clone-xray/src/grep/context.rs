use std::cell::RefCell;
use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};
use tree_sitter::{Language, Node, Parser};

use super::context_langs::{
    collect_py_spans, collect_ts_js_spans, collect_xaml_spans, node_str, push_span, SymbolSpan,
};
use crate::parse::svelte::{extract_svelte_blocks, SvelteBlockKind};

#[derive(Default)]
pub struct FileSymbolInfo {
    pub symbols: FxHashMap<usize, String>,
    pub test_lines: FxHashSet<usize>,
}

#[derive(Clone, Copy)]
enum AstLang {
    Cs,
    Xaml,
    Rs,
    Ts,
    Tsx,
    Js,
    Svelte,
    Py,
}

thread_local! {
    static CS_PARSER: RefCell<Option<Parser>> = RefCell::new(init_parser(&tree_sitter_c_sharp::LANGUAGE.into()));
    static XAML_PARSER: RefCell<Option<Parser>> = RefCell::new(init_parser(&tree_sitter_xml::LANGUAGE_XML.into()));
    static RS_PARSER: RefCell<Option<Parser>> = RefCell::new(init_parser(&tree_sitter_rust::LANGUAGE.into()));
    static TS_PARSER: RefCell<Option<Parser>> = RefCell::new(init_parser(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()));
    static TSX_PARSER: RefCell<Option<Parser>> = RefCell::new(init_parser(&tree_sitter_typescript::LANGUAGE_TSX.into()));
    static JS_PARSER: RefCell<Option<Parser>> = RefCell::new(init_parser(&tree_sitter_javascript::LANGUAGE.into()));
    static PY_PARSER: RefCell<Option<Parser>> = RefCell::new(init_parser(&tree_sitter_python::LANGUAGE.into()));
}

fn init_parser(lang: &Language) -> Option<Parser> {
    let mut parser = Parser::new();
    parser.set_language(lang).ok().map(|()| parser)
}

pub fn resolve_enclosing_symbols(
    path: &Path,
    content: &[u8],
    target_lines: &[usize],
) -> FileSymbolInfo {
    let Some(lang) = detect_ast_lang(path) else {
        return FileSymbolInfo::default();
    };
    let mut spans = Vec::new();
    if matches!(lang, AstLang::Svelte) {
        collect_svelte_spans(path, content, &mut spans);
    } else {
        let Some(tree) = parse_with_lang(lang, content) else {
            return FileSymbolInfo::default();
        };
        let mut scopes = Vec::new();
        match lang {
            AstLang::Cs => collect_cs_spans(tree.root_node(), content, &mut scopes, &mut spans),
            AstLang::Rs => {
                collect_rs_spans(tree.root_node(), content, &mut scopes, false, &mut spans)
            }
            AstLang::Ts | AstLang::Tsx | AstLang::Js => {
                collect_ts_js_spans(tree.root_node(), content, &mut scopes, &mut spans);
            }
            AstLang::Py => collect_py_spans(tree.root_node(), content, &mut scopes, &mut spans),
            AstLang::Xaml => collect_xaml_spans(tree.root_node(), content, &mut scopes, &mut spans),
            AstLang::Svelte => {}
        }
    }

    let mut info = FileSymbolInfo::default();
    for &line in target_lines {
        if spans
            .iter()
            .any(|s| s.is_test && s.start_line <= line && line <= s.end_line)
        {
            info.test_lines.insert(line);
        }
        let best = spans
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.label.is_empty() && s.start_line <= line && line <= s.end_line)
            .min_by_key(|(idx, s)| (s.end_line.saturating_sub(s.start_line), usize::MAX - *idx));
        if let Some((_, span)) = best {
            info.symbols.insert(line, span.label.clone());
        }
    }
    info
}

fn detect_ast_lang(path: &Path) -> Option<AstLang> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "cs" => Some(AstLang::Cs),
        "xaml" => Some(AstLang::Xaml),
        "rs" => Some(AstLang::Rs),
        "ts" | "mts" | "cts" => Some(AstLang::Ts),
        "tsx" => Some(AstLang::Tsx),
        "js" | "jsx" | "mjs" | "cjs" => Some(AstLang::Js),
        "svelte" => Some(AstLang::Svelte),
        "py" => Some(AstLang::Py),
        _ => None,
    }
}

fn collect_svelte_spans(path: &Path, content: &[u8], spans: &mut Vec<SymbolSpan>) {
    let comp = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Component");
    let line_count = content.iter().filter(|&&b| b == b'\n').count() + 1;
    spans.push(SymbolSpan {
        start_line: 1,
        end_line: line_count,
        label: comp.to_string(),
        is_test: false,
    });
    for block in extract_svelte_blocks(content) {
        if block.kind != SvelteBlockKind::Script {
            continue;
        }
        let script_lang = if block.is_ts {
            AstLang::Ts
        } else {
            AstLang::Js
        };
        let Some(tree) = parse_with_lang(script_lang, block.body) else {
            continue;
        };
        let mut scopes = Vec::new();
        let mut inner = Vec::new();
        collect_ts_js_spans(tree.root_node(), block.body, &mut scopes, &mut inner);
        for mut span in inner {
            span.start_line += block.start_line_0;
            span.end_line += block.start_line_0;
            if !span.label.is_empty() {
                span.label = format!("{comp}.{}", span.label);
            }
            spans.push(span);
        }
    }
}

fn parse_with_lang(lang: AstLang, content: &[u8]) -> Option<tree_sitter::Tree> {
    let cell = match lang {
        AstLang::Cs => &CS_PARSER,
        AstLang::Xaml => &XAML_PARSER,
        AstLang::Rs => &RS_PARSER,
        AstLang::Ts => &TS_PARSER,
        AstLang::Tsx => &TSX_PARSER,
        AstLang::Js => &JS_PARSER,
        AstLang::Svelte => return None,
        AstLang::Py => &PY_PARSER,
    };
    cell.with(|c| {
        let mut guard = c.borrow_mut();
        guard.as_mut().and_then(|p| p.parse(content, None))
    })
}

fn collect_cs_spans(
    node: Node<'_>,
    src: &[u8],
    scopes: &mut Vec<String>,
    spans: &mut Vec<SymbolSpan>,
) {
    let kind = node.kind();
    let pushed = match kind {
        "class_declaration"
        | "record_declaration"
        | "struct_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "delegate_declaration" => node
            .child_by_field_name("name")
            .map(|n| node_str(n, src).to_string()),
        "method_declaration"
        | "local_function_statement"
        | "property_declaration"
        | "destructor_declaration" => node
            .child_by_field_name("name")
            .map(|n| node_str(n, src).to_string()),
        "constructor_declaration" => Some(
            node.child_by_field_name("name")
                .map(|n| node_str(n, src).to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ".ctor".to_string()),
        ),
        "indexer_declaration" => Some("this[]".to_string()),
        "field_declaration" | "event_field_declaration" => first_cs_var_name(node, src),
        _ => None,
    };

    if let Some(name) = pushed.filter(|s| !s.is_empty()) {
        scopes.push(name);
        push_span(node, scopes.join("."), false, spans);
        walk_cs_children(node, src, scopes, spans);
        scopes.pop();
        return;
    }

    if kind == "file_scoped_namespace_declaration" || kind == "namespace_declaration" {
        if let Some(name_node) = node.child_by_field_name("name") {
            let ns = node_str(name_node, src);
            if !ns.is_empty() {
                push_span(node, ns.to_string(), false, spans);
            }
        }
    }

    walk_cs_children(node, src, scopes, spans);
}

fn walk_cs_children(
    node: Node<'_>,
    src: &[u8],
    scopes: &mut Vec<String>,
    spans: &mut Vec<SymbolSpan>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_cs_spans(child, src, scopes, spans);
    }
}

fn first_cs_var_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declaration" {
            let mut vc = child.walk();
            for sub in child.children(&mut vc) {
                if sub.kind() == "variable_declarator" {
                    return sub
                        .child_by_field_name("name")
                        .map(|n| node_str(n, src).to_string());
                }
            }
        }
    }
    None
}

fn collect_rs_spans(
    node: Node<'_>,
    src: &[u8],
    scopes: &mut Vec<String>,
    in_test: bool,
    spans: &mut Vec<SymbolSpan>,
) {
    let mut cursor = node.walk();
    let mut next_is_cfg_test = false;

    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind == "attribute_item" {
            let txt = node_str(child, src);
            if txt.contains("cfg") && txt.contains("test") {
                next_is_cfg_test = true;
            }
            continue;
        }
        if child.is_extra() {
            continue;
        }
        let child_is_test = in_test || next_is_cfg_test;
        next_is_cfg_test = false;

        let scope_entry = match kind {
            "mod_item" => child
                .child_by_field_name("name")
                .map(|n| (node_str(n, src).to_string(), true)),
            "struct_item" | "enum_item" | "trait_item" | "type_item" => child
                .child_by_field_name("name")
                .map(|n| (node_str(n, src).to_string(), true)),
            "impl_item" => child
                .child_by_field_name("type")
                .map(|n| (compact_type_name(node_str(n, src)), true)),
            "function_item" => child
                .child_by_field_name("name")
                .map(|n| (format!("fn {}", node_str(n, src)), false)),
            "const_item" | "static_item" | "macro_definition" => child
                .child_by_field_name("name")
                .map(|n| (node_str(n, src).to_string(), false)),
            _ => None,
        };

        if let Some((label_part, push_as_container)) = scope_entry.filter(|(s, _)| !s.is_empty()) {
            let full_label = format_rs_scope(scopes, &label_part, push_as_container);
            push_span(child, full_label, child_is_test, spans);
            if push_as_container {
                scopes.push(label_part);
                collect_rs_spans(child, src, scopes, child_is_test, spans);
                scopes.pop();
            } else {
                let fn_short = label_part
                    .strip_prefix("fn ")
                    .unwrap_or(&label_part)
                    .to_string();
                scopes.push(fn_short);
                collect_rs_spans(child, src, scopes, child_is_test, spans);
                scopes.pop();
            }
        } else {
            if child_is_test {
                push_span(child, String::new(), true, spans);
            }
            collect_rs_spans(child, src, scopes, child_is_test, spans);
        }
    }
}

fn format_rs_scope(scopes: &[String], part: &str, is_container: bool) -> String {
    if scopes.is_empty() {
        return part.to_string();
    }
    let clean_part = if !is_container {
        part.strip_prefix("fn ").unwrap_or(part)
    } else {
        part
    };
    format!("{}::{clean_part}", scopes.join("::"))
}

fn compact_type_name(raw: &str) -> String {
    raw.split('<')
        .next()
        .unwrap_or(raw)
        .rsplit("::")
        .next()
        .unwrap_or(raw)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_csharp_and_rust_enclosing_symbols() {
        let cs = br#"
namespace Demo.App;
public sealed class Worker {
    public void RunTask() {
        var key = "ACTION_RUN";
    }
}
"#;
        let cs_info = resolve_enclosing_symbols(Path::new("Worker.cs"), cs, &[5]);
        assert_eq!(
            cs_info.symbols.get(&5).map(String::as_str),
            Some("Worker.RunTask")
        );

        let rs = br#"
struct Engine;
impl Engine {
    fn execute(&self) {
        let x = 1;
    }
}
fn top_helper() {
    let y = 2;
}
#[cfg(test)]
mod tests {
    #[test]
    fn inside_test() {
        let z = 3;
    }
}
"#;
        let rs_info = resolve_enclosing_symbols(Path::new("engine.rs"), rs, &[5, 9, 15]);
        assert_eq!(
            rs_info.symbols.get(&5).map(String::as_str),
            Some("Engine::execute")
        );
        assert_eq!(
            rs_info.symbols.get(&9).map(String::as_str),
            Some("fn top_helper")
        );
        assert!(rs_info.test_lines.contains(&15));
    }

    #[test]
    fn resolves_ts_py_and_xaml_enclosing_symbols() {
        let ts = br#"
export class PanelController {
    handleClick() {
        return "ok";
    }
}
"#;
        let ts_info = resolve_enclosing_symbols(Path::new("panel.ts"), ts, &[4]);
        assert_eq!(
            ts_info.symbols.get(&4).map(String::as_str),
            Some("PanelController.handleClick")
        );

        let py = b"def run_job():\n    return 42\n";
        let py_info = resolve_enclosing_symbols(Path::new("job.py"), py, &[2]);
        assert_eq!(
            py_info.symbols.get(&2).map(String::as_str),
            Some("def run_job")
        );

        let xaml = br#"<UserControl x:Class="Demo.Views.MainView"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
    <UserControl.Resources>
        <Style x:Key="PrimaryBtn" TargetType="Button">
            <Setter Property="Padding" Value="8" />
        </Style>
    </UserControl.Resources>
</UserControl>"#;
        let xaml_info = resolve_enclosing_symbols(Path::new("MainView.xaml"), xaml, &[5]);
        assert_eq!(
            xaml_info.symbols.get(&5).map(String::as_str),
            Some("MainView > Style[PrimaryBtn]")
        );
    }
}
