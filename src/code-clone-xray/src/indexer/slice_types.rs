use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

use super::model::PartialCluster;
use super::query_models::{DiLinkRow, MessengerRow};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SliceFileEntry {
    pub project: String,
    pub file: String,
    pub line_count: usize,
    pub matched_types: Vec<String>,
    pub reason: String,
    pub score: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SliceReport {
    pub query: String,
    pub hops: usize,
    pub seed_types: Vec<String>,
    pub related_types: Vec<String>,
    pub files_to_read: Vec<SliceFileEntry>,
    pub partial_clusters: Vec<PartialCluster>,
    pub di_links: Vec<DiLinkRow>,
    pub messenger_flows: Vec<MessengerRow>,
}

pub struct FileAcc {
    pub project: String,
    pub line_count: usize,
    pub score: usize,
    pub matched_types: Vec<String>,
    pub reason: String,
}

pub fn short_symbol_name(full_or_short: &str) -> &str {
    full_or_short
        .rsplit(['.', ':', '/'])
        .find(|s| !s.is_empty())
        .unwrap_or(full_or_short)
}

pub fn enqueue_partner(
    full_or_short: String,
    depth: usize,
    visited_short: &mut FxHashSet<String>,
    related_types: &mut Vec<String>,
    queue: &mut VecDeque<(String, usize)>,
) {
    let short = short_symbol_name(&full_or_short).to_string();
    if visited_short.insert(short.to_ascii_lowercase()) {
        related_types.push(full_or_short);
        queue.push_back((short, depth + 1));
    }
}

pub fn record_file(
    map: &mut FxHashMap<String, FileAcc>,
    project: &str,
    rel_path: &str,
    line_count: usize,
    type_name: &str,
    reason: &str,
    score: usize,
) {
    let entry = map.entry(rel_path.to_string()).or_insert_with(|| FileAcc {
        project: project.to_string(),
        line_count,
        score: 0,
        matched_types: Vec::new(),
        reason: reason.to_string(),
    });
    entry.score += score;
    if !type_name.is_empty() && !entry.matched_types.iter().any(|t| t == type_name) {
        entry.matched_types.push(type_name.to_string());
    }
}
