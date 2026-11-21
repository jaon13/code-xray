use rustc_hash::FxHashSet;
use tree_sitter::Node;

use super::cs_syntax_utils::{compact_whitespace, node_text, truncate_sig};
use super::model::ExtractedStateTransition;
use super::state_guards::{
    derive_machine_name, extract_leaf_ident, infer_from_state_ancestors, is_state_identifier,
    parse_state_rhs,
};

pub use super::state_guards::parse_state_rhs as parse_state_rhs_token;

/// Enclosing AST context passed into tree-sitter state transition extractors.
#[derive(Clone, Copy)]
pub struct StateContext<'a> {
    pub src: &'a [u8],
    pub line_offset: u32,
    pub owner_type: &'a str,
    pub trigger_member: &'a str,
}

fn line_with_offset(node: Node<'_>, line_offset: u32) -> u32 {
    (node.start_position().row as u32) + 1 + line_offset
}

struct TransitionSpec {
    machine_name: String,
    from_state: String,
    to_state: String,
    guard_expr: String,
    line: u32,
}

fn record_transition(
    sctx: StateContext<'_>,
    spec: TransitionSpec,
    seen: &mut FxHashSet<(String, String, String, String, String, String)>,
    out: &mut Vec<ExtractedStateTransition>,
) {
    if spec.machine_name.is_empty() || spec.to_state.is_empty() {
        return;
    }
    let owner = sctx.owner_type.to_string();
    let trigger = if sctx.trigger_member.is_empty() {
        "<init>".to_string()
    } else {
        sctx.trigger_member.to_string()
    };
    let key = (
        owner.clone(),
        spec.machine_name.clone(),
        spec.from_state.clone(),
        spec.to_state.clone(),
        trigger.clone(),
        spec.guard_expr.clone(),
    );
    if seen.insert(key) {
        out.push(ExtractedStateTransition {
            owner_type: owner,
            machine_name: spec.machine_name,
            from_state: spec.from_state,
            to_state: spec.to_state,
            trigger_member: trigger,
            guard_expr: spec.guard_expr,
            line: spec.line,
        });
    }
}

/// Extracts state transitions from an assignment expression (`lhs = rhs`) or switch/match RHS.
pub fn extract_assignment_states(
    node: Node<'_>,
    sctx: StateContext<'_>,
    seen: &mut FxHashSet<(String, String, String, String, String, String)>,
    out: &mut Vec<ExtractedStateTransition>,
) {
    let Some(left_n) = node.child_by_field_name("left") else {
        return;
    };
    let Some(right_n) = node.child_by_field_name("right") else {
        return;
    };
    let lhs_raw = compact_whitespace(node_text(left_n, sctx.src));
    if !is_state_identifier(&lhs_raw) {
        return;
    }
    let lhs_leaf = extract_leaf_ident(&lhs_raw);
    let line = line_with_offset(node, sctx.line_offset);

    if matches!(right_n.kind(), "switch_expression" | "match_expression") {
        extract_switch_match_rhs(right_n, lhs_leaf, sctx, seen, out);
        return;
    }

    let rhs_raw = compact_whitespace(node_text(right_n, sctx.src));
    let Some((enum_hint, to_state)) = parse_state_rhs(&rhs_raw) else {
        return;
    };
    let (from_state, guard_expr) = infer_from_state_ancestors(
        node,
        sctx.src,
        lhs_leaf,
        sctx.trigger_member,
        sctx.owner_type,
    );
    let machine_name = derive_machine_name(lhs_leaf, &enum_hint, sctx.owner_type);
    record_transition(
        sctx,
        TransitionSpec {
            machine_name,
            from_state,
            to_state,
            guard_expr,
            line,
        },
        seen,
        out,
    );
}

fn extract_switch_match_rhs(
    expr_node: Node<'_>,
    lhs_leaf: &str,
    sctx: StateContext<'_>,
    seen: &mut FxHashSet<(String, String, String, String, String, String)>,
    out: &mut Vec<ExtractedStateTransition>,
) {
    let mut stack = vec![expr_node];
    while let Some(cur) = stack.pop() {
        match cur.kind() {
            "switch_expression_arm" => {
                let mut cursor = cur.walk();
                let named: Vec<Node<'_>> = cur.named_children(&mut cursor).collect();
                if named.len() >= 2 {
                    let pat_txt = compact_whitespace(node_text(named[0], sctx.src));
                    let val_txt = compact_whitespace(node_text(named[named.len() - 1], sctx.src));
                    push_arm_pair(
                        &pat_txt,
                        &val_txt,
                        lhs_leaf,
                        line_with_offset(cur, sctx.line_offset),
                        sctx,
                        seen,
                        out,
                    );
                }
            }
            "match_arm" => {
                if let (Some(pat), Some(val)) = (
                    cur.child_by_field_name("pattern"),
                    cur.child_by_field_name("value"),
                ) {
                    let pat_txt = compact_whitespace(node_text(pat, sctx.src));
                    let val_txt = compact_whitespace(node_text(val, sctx.src));
                    push_arm_pair(
                        &pat_txt,
                        &val_txt,
                        lhs_leaf,
                        line_with_offset(cur, sctx.line_offset),
                        sctx,
                        seen,
                        out,
                    );
                }
            }
            _ => {
                let mut cursor = cur.walk();
                for child in cur.children(&mut cursor) {
                    stack.push(child);
                }
            }
        }
    }
}

fn push_arm_pair(
    pat_txt: &str,
    val_txt: &str,
    lhs_leaf: &str,
    line: u32,
    sctx: StateContext<'_>,
    seen: &mut FxHashSet<(String, String, String, String, String, String)>,
    out: &mut Vec<ExtractedStateTransition>,
) {
    let Some((enum_hint, to_state)) = parse_state_rhs(val_txt) else {
        return;
    };
    let from_state = parse_state_rhs(pat_txt.split('|').next().unwrap_or(pat_txt))
        .map(|(_, v)| v)
        .unwrap_or_else(|| "*".to_string());
    let machine_name = derive_machine_name(lhs_leaf, &enum_hint, sctx.owner_type);
    record_transition(
        sctx,
        TransitionSpec {
            machine_name,
            from_state,
            to_state,
            guard_expr: truncate_sig(pat_txt, 80),
            line,
        },
        seen,
        out,
    );
}

/// Extracts initial state transitions from field, property, struct literal, or let declarations.
pub fn extract_field_init_states(
    node: Node<'_>,
    sctx: StateContext<'_>,
    seen: &mut FxHashSet<(String, String, String, String, String, String)>,
    out: &mut Vec<ExtractedStateTransition>,
) {
    let mut stack = vec![node];
    while let Some(cur) = stack.pop() {
        if matches!(
            cur.kind(),
            "variable_declarator"
                | "public_field_definition"
                | "property_definition"
                | "field_initializer"
                | "let_declaration"
        ) {
            extract_single_field_init(cur, sctx, seen, out);
        }
        let mut cursor = cur.walk();
        for child in cur.children(&mut cursor) {
            stack.push(child);
        }
    }
}

fn extract_single_field_init(
    cur: Node<'_>,
    sctx: StateContext<'_>,
    seen: &mut FxHashSet<(String, String, String, String, String, String)>,
    out: &mut Vec<ExtractedStateTransition>,
) {
    let name_txt = cur
        .child_by_field_name("name")
        .or_else(|| cur.child_by_field_name("field"))
        .or_else(|| cur.child_by_field_name("pattern"))
        .or_else(|| {
            let mut c = cur.walk();
            let found = cur
                .children(&mut c)
                .find(|ch| matches!(ch.kind(), "identifier" | "property_identifier"));
            found
        })
        .map(|n| compact_whitespace(node_text(n, sctx.src)))
        .map(|s| s.strip_prefix("mut ").unwrap_or(&s).trim().to_string())
        .unwrap_or_default();
    if !is_state_identifier(&name_txt) {
        return;
    }

    let name_id = cur.child_by_field_name("name").map(|n| n.id());
    let val_txt = cur
        .child_by_field_name("value")
        .or_else(|| {
            let mut c = cur.walk();
            let found = cur
                .children(&mut c)
                .find(|ch| ch.kind() == "equals_value_clause");
            found
        })
        .or_else(|| {
            let mut c = cur.walk();
            let found = cur.named_children(&mut c).find(|ch| {
                Some(ch.id()) != name_id
                    && !matches!(
                        ch.kind(),
                        "identifier"
                            | "bracketed_argument_list"
                            | "argument_list"
                            | "type_annotation"
                    )
            });
            found
        })
        .map(|v| {
            compact_whitespace(node_text(v, sctx.src))
                .trim_start_matches('=')
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    if let Some((enum_hint, to_state)) = parse_state_rhs(&val_txt) {
        let leaf = extract_leaf_ident(&name_txt);
        let machine_name = derive_machine_name(leaf, &enum_hint, sctx.owner_type);
        record_transition(
            sctx,
            TransitionSpec {
                machine_name,
                from_state: "(init)".to_string(),
                to_state,
                guard_expr: String::new(),
                line: line_with_offset(cur, sctx.line_offset),
            },
            seen,
            out,
        );
    }
}
