use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusReport {
    pub repo_root: String,
    pub updated_epoch: String,
    pub projects: usize,
    pub external_projects: usize,
    pub files: usize,
    pub types: usize,
    pub members: usize,
    pub calls: usize,
    pub type_refs: usize,
    pub di_registrations: usize,
    pub messenger_links: usize,
    pub state_transitions: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateTransitionRow {
    pub owner_type: String,
    pub machine_name: String,
    pub from_state: String,
    pub to_state: String,
    pub trigger_member: String,
    pub guard_expr: String,
    pub file: String,
    pub line: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatesReport {
    pub target: String,
    pub machines: Vec<String>,
    pub orphan_terminal_states: Vec<String>,
    pub transitions: Vec<StateTransitionRow>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiLinkRow {
    pub lifetime: String,
    pub service_type: String,
    pub impl_type: String,
    pub file: String,
    pub line: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessengerRow {
    pub owner_type: String,
    pub direction: String,
    pub message_type: String,
    pub file: String,
    pub line: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeRefRow {
    pub source_type: String,
    pub target_type: String,
    pub ref_kind: String,
    pub file: String,
    pub line: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepsReport {
    pub target: String,
    pub di_registrations: Vec<DiLinkRow>,
    pub type_refs_outgoing: Vec<TypeRefRow>,
    pub type_refs_incoming: Vec<TypeRefRow>,
    pub messenger_links: Vec<MessengerRow>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub kind: String,
    pub name: String,
    pub owner: String,
    pub project: String,
    pub file: String,
    pub line: i64,
}
