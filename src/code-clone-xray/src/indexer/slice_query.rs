use std::collections::VecDeque;

use anyhow::Result;
use rusqlite::{params, Connection};
use rustc_hash::{FxHashMap, FxHashSet};

use super::model::PartialCluster;
use super::query_models::{DiLinkRow, MessengerRow};
use super::slice_types::{
    enqueue_partner, record_file, short_symbol_name, FileAcc, SliceFileEntry, SliceReport,
};
use super::sqlite_query::query_partials;

pub fn query_feature_slice(
    conn: &Connection,
    query: &str,
    hops: usize,
    limit: usize,
) -> Result<SliceReport> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    let first_pat = terms
        .first()
        .map(|t| format!("%{t}%"))
        .unwrap_or_else(|| "%".to_string());

    let mut seed_types: Vec<String> = Vec::new();
    let mut seed_set: FxHashSet<String> = FxHashSet::default();
    let mut file_scores: FxHashMap<String, FileAcc> = FxHashMap::default();

    seed_from_types(
        conn,
        &first_pat,
        &terms,
        &mut seed_types,
        &mut seed_set,
        &mut file_scores,
    )?;
    seed_from_members(
        conn,
        &first_pat,
        &terms,
        &mut seed_types,
        &mut seed_set,
        &mut file_scores,
    )?;

    let mut visited_short: FxHashSet<String> = FxHashSet::default();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    for st in &seed_types {
        let short = short_symbol_name(st).to_string();
        if visited_short.insert(short.to_ascii_lowercase()) {
            queue.push_back((short, 0));
        }
    }

    let mut related_types: Vec<String> = Vec::new();
    let mut di_links: Vec<DiLinkRow> = Vec::new();
    let mut messenger_flows: Vec<MessengerRow> = Vec::new();

    expand_bfs(
        conn,
        hops,
        &mut queue,
        &mut visited_short,
        &mut related_types,
        &mut di_links,
        &mut messenger_flows,
        &mut file_scores,
    )?;

    let mut files_to_read: Vec<SliceFileEntry> = file_scores
        .into_iter()
        .map(|(file, acc)| SliceFileEntry {
            project: acc.project,
            file,
            line_count: acc.line_count,
            matched_types: acc.matched_types,
            reason: acc.reason,
            score: acc.score,
        })
        .collect();

    files_to_read.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.file.cmp(&b.file)));
    files_to_read.truncate(limit.clamp(1, 100));

    let active_names: FxHashSet<String> = seed_types
        .iter()
        .chain(related_types.iter())
        .map(|s| short_symbol_name(s).to_ascii_lowercase())
        .collect();
    let partial_clusters: Vec<PartialCluster> = query_partials(conn, None, 2)?
        .into_iter()
        .filter(|pc| {
            let short = short_symbol_name(&pc.full_name);
            active_names.contains(&short.to_ascii_lowercase())
        })
        .take(15)
        .collect();

    Ok(SliceReport {
        query: query.to_string(),
        hops,
        seed_types,
        related_types,
        files_to_read,
        partial_clusters,
        di_links,
        messenger_flows,
    })
}

fn seed_from_types(
    conn: &Connection,
    first_pat: &str,
    terms: &[String],
    seed_types: &mut Vec<String>,
    seed_set: &mut FxHashSet<String>,
    file_scores: &mut FxHashMap<String, FileAcc>,
) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT t.full_name, f.project_name, f.rel_path, f.line_count FROM types t JOIN files f ON f.id = t.file_id WHERE LOWER(t.full_name) LIKE ?1 OR LOWER(f.rel_path) LIKE ?1 LIMIT 80",
    )?;
    for row in stmt
        .query_map(params![first_pat], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?.max(0) as usize,
            ))
        })?
        .flatten()
    {
        let hay = format!("{} {}", row.0, row.2).to_ascii_lowercase();
        if terms.iter().all(|t| hay.contains(t)) {
            if seed_set.insert(row.0.clone()) {
                seed_types.push(row.0.clone());
            }
            record_file(file_scores, &row.1, &row.2, row.3, &row.0, "seed-type", 100);
        }
    }
    Ok(())
}

fn seed_from_members(
    conn: &Connection,
    first_pat: &str,
    terms: &[String],
    seed_types: &mut Vec<String>,
    seed_set: &mut FxHashSet<String>,
    file_scores: &mut FxHashMap<String, FileAcc>,
) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT m.owner_type, m.name, m.kind, f.project_name, f.rel_path, f.line_count FROM members m JOIN files f ON f.id = m.file_id WHERE LOWER(m.name) LIKE ?1 LIMIT 80",
    )?;
    for row in stmt
        .query_map(params![first_pat], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?.max(0) as usize,
            ))
        })?
        .flatten()
    {
        let hay = format!("{} {} {}", row.0, row.1, row.4).to_ascii_lowercase();
        if terms.iter().all(|t| hay.contains(t)) {
            if !row.0.is_empty() && seed_set.insert(row.0.clone()) {
                seed_types.push(row.0.clone());
            }
            let reason = format!("seed-{}", row.2);
            record_file(file_scores, &row.3, &row.4, row.5, &row.0, &reason, 90);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn expand_bfs(
    conn: &Connection,
    hops: usize,
    queue: &mut VecDeque<(String, usize)>,
    visited_short: &mut FxHashSet<String>,
    related_types: &mut Vec<String>,
    di_links: &mut Vec<DiLinkRow>,
    messenger_flows: &mut Vec<MessengerRow>,
    file_scores: &mut FxHashMap<String, FileAcc>,
) -> Result<()> {
    let mut neighbor_stmt = conn.prepare(
        "SELECT target_type, ref_kind FROM type_refs WHERE LOWER(source_type) = ?1 OR LOWER(source_type) LIKE ?2 OR LOWER(source_type) LIKE ?3 UNION SELECT source_type, ref_kind FROM type_refs WHERE LOWER(target_type) = ?1 OR LOWER(target_type) LIKE ?2 OR LOWER(target_type) LIKE ?3 LIMIT 30",
    )?;
    let mut resolve_file_stmt = conn.prepare(
        "SELECT t.full_name, f.project_name, f.rel_path, f.line_count FROM types t JOIN files f ON f.id = t.file_id WHERE LOWER(t.name) = ?1 OR LOWER(t.full_name) = ?1 OR LOWER(t.full_name) LIKE ?2 OR LOWER(t.full_name) LIKE ?3 LIMIT 25",
    )?;
    let mut msg_stmt = conn.prepare(
        "SELECT m.owner_type, m.direction, m.message_type, f.rel_path, m.line FROM messenger_links m JOIN files f ON f.id = m.file_id WHERE LOWER(m.owner_type) = ?1 OR LOWER(m.owner_type) LIKE ?2 OR LOWER(m.owner_type) LIKE ?3 OR LOWER(m.message_type) = ?1 LIMIT 20",
    )?;
    let mut di_stmt = conn.prepare(
        "SELECT d.lifetime, d.service_type, d.impl_type, f.rel_path, d.line FROM di_registrations d JOIN files f ON f.id = d.file_id WHERE LOWER(d.service_type) = ?1 OR LOWER(d.impl_type) = ?1 LIMIT 20",
    )?;

    while let Some((short_name, depth)) = queue.pop_front() {
        if depth >= hops || related_types.len() >= 80 {
            continue;
        }
        let low = short_name.to_ascii_lowercase();
        let dot_like = format!("%.{low}");
        let colon_like = format!("%::{low}");
        let weight = 50usize.saturating_sub(depth * 20).max(10);

        for mr in msg_stmt
            .query_map(params![&low, &dot_like, &colon_like], |r| {
                Ok(MessengerRow {
                    owner_type: r.get(0)?,
                    direction: r.get(1)?,
                    message_type: r.get(2)?,
                    file: r.get(3)?,
                    line: r.get(4)?,
                })
            })?
            .flatten()
        {
            let owner_short = short_symbol_name(&mr.owner_type);
            let partner = if owner_short.eq_ignore_ascii_case(&short_name) {
                mr.message_type.clone()
            } else {
                owner_short.to_string()
            };
            messenger_flows.push(mr);
            enqueue_partner(partner, depth, visited_short, related_types, queue);
        }

        for dr in di_stmt
            .query_map(params![&low], |r| {
                Ok(DiLinkRow {
                    lifetime: r.get(0)?,
                    service_type: r.get(1)?,
                    impl_type: r.get(2)?,
                    file: r.get(3)?,
                    line: r.get(4)?,
                })
            })?
            .flatten()
        {
            let partner = if dr.service_type.eq_ignore_ascii_case(&short_name) {
                dr.impl_type.clone()
            } else {
                dr.service_type.clone()
            };
            di_links.push(dr);
            enqueue_partner(partner, depth, visited_short, related_types, queue);
        }

        for (nbr_full, ref_kind) in neighbor_stmt
            .query_map(params![&low, &dot_like, &colon_like], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .flatten()
        {
            let nbr_short = short_symbol_name(&nbr_full).to_string();
            let nbr_low = nbr_short.to_ascii_lowercase();
            let nbr_dot = format!("%.{nbr_low}");
            let nbr_colon = format!("%::{nbr_low}");
            for frow in resolve_file_stmt
                .query_map(params![&nbr_low, &nbr_dot, &nbr_colon], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?.max(0) as usize,
                    ))
                })?
                .flatten()
            {
                record_file(
                    file_scores,
                    &frow.1,
                    &frow.2,
                    frow.3,
                    &frow.0,
                    &format!("hop{}:{ref_kind}", depth + 1),
                    weight,
                );
            }
            enqueue_partner(nbr_full, depth, visited_short, related_types, queue);
        }
    }
    Ok(())
}
