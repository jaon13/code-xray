use tree_sitter::Node;

use super::cs_syntax_utils::{clean_type_token, is_interesting_type, line_1, node_text};
use super::model::{ExtractedCall, ExtractedDiReg, ExtractedMessengerLink};

pub fn extract_invocation_details(
    node: Node<'_>,
    src: &[u8],
    caller_type: &str,
    caller_member: &str,
    calls: &mut Vec<ExtractedCall>,
    di_regs: &mut Vec<ExtractedDiReg>,
    messenger_links: &mut Vec<ExtractedMessengerLink>,
) {
    let Some(func_node) = node.child_by_field_name("function") else {
        return;
    };
    let line = line_1(node);
    let (receiver_hint, raw_method, type_args) = parse_call_target(func_node, src);
    if raw_method.is_empty() {
        return;
    }

    calls.push(ExtractedCall {
        caller_type: caller_type.to_string(),
        caller_member: caller_member.to_string(),
        callee_name: raw_method.clone(),
        receiver_hint: receiver_hint.clone(),
        line,
    });

    if let Some(lifetime) = match_di_lifetime(&raw_method) {
        if !type_args.is_empty() {
            let service = clean_type_token(&type_args[0]);
            let impl_t = type_args
                .get(1)
                .map(|s| clean_type_token(s))
                .unwrap_or_else(|| service.clone());
            if is_interesting_type(&service) {
                di_regs.push(ExtractedDiReg {
                    lifetime: lifetime.to_string(),
                    service_type: service,
                    impl_type: impl_t,
                    line,
                });
            }
        } else if let Some(arg_list) = node.child_by_field_name("arguments") {
            let txt = node_text(arg_list, src);
            if let Some(first_t) = extract_lambda_return_or_typeof(txt) {
                di_regs.push(ExtractedDiReg {
                    lifetime: lifetime.to_string(),
                    service_type: first_t.clone(),
                    impl_type: first_t,
                    line,
                });
            }
        }
    }

    if raw_method == "Send" || raw_method == "Register" {
        let full_expr = node_text(node, src);
        if full_expr.contains("Messenger") || receiver_hint.contains("Messenger") {
            let direction = if raw_method == "Send" {
                "send"
            } else {
                "receive"
            };
            let msg_type = if raw_method == "Send" {
                type_args.first().map(|s| clean_type_token(s)).or_else(|| {
                    node.child_by_field_name("arguments")
                        .and_then(|a| extract_new_type_in_args(node_text(a, src)))
                })
            } else {
                type_args.last().map(|s| clean_type_token(s))
            };

            if let Some(m) = msg_type.filter(|t| is_interesting_type(t)) {
                messenger_links.push(ExtractedMessengerLink {
                    owner_type: caller_type.to_string(),
                    direction: direction.to_string(),
                    message_type: m,
                    line,
                });
            }
        }
    }
}

fn parse_call_target(func_node: Node<'_>, src: &[u8]) -> (String, String, Vec<String>) {
    let raw = node_text(func_node, src).trim();
    let (expr_part, type_args) = if let Some(lt) = raw.rfind('<') {
        if raw.ends_with('>') {
            let inner = &raw[lt + 1..raw.len() - 1];
            (&raw[..lt], split_top_level_csv(inner))
        } else {
            (raw, Vec::new())
        }
    } else {
        (raw, Vec::new())
    };

    if let Some(dot) = expr_part.rfind('.') {
        let recv = expr_part[..dot].trim();
        let recv_short = recv.rsplit('.').next().unwrap_or(recv).trim().to_string();
        let meth = expr_part[dot + 1..].trim().to_string();
        (recv_short, meth, type_args)
    } else {
        (String::new(), expr_part.to_string(), type_args)
    }
}

fn split_top_level_csv(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                let s = text[start..idx].trim();
                if !s.is_empty() {
                    parts.push(s.to_string());
                }
                start = idx + 1;
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    parts
}

fn match_di_lifetime(method: &str) -> Option<&'static str> {
    match method {
        "AddSingleton" | "TryAddSingleton" => Some("Singleton"),
        "AddTransient" | "TryAddTransient" => Some("Transient"),
        "AddScoped" | "TryAddScoped" => Some("Scoped"),
        _ => None,
    }
}

fn extract_lambda_return_or_typeof(args_txt: &str) -> Option<String> {
    if let Some(pos) = args_txt.find("typeof(") {
        let after = &args_txt[pos + 7..];
        if let Some(end) = after.find(')') {
            let tok = clean_type_token(&after[..end]);
            if is_interesting_type(&tok) {
                return Some(tok);
            }
        }
    }
    extract_new_type_in_args(args_txt)
}

fn extract_new_type_in_args(args_txt: &str) -> Option<String> {
    let pos = args_txt.find("new ")?;
    let after = args_txt[pos + 4..].trim_start();
    let raw: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
        .collect();
    let tok = clean_type_token(&raw);
    is_interesting_type(&tok).then_some(tok)
}
