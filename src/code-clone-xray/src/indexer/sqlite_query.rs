use anyhow::Result;
use rusqlite::{params, Connection};
use rustc_hash::FxHashMap;

use super::model::{PartialCluster, PartialFileSpan};
pub use super::query_models::{
    DepsReport, DiLinkRow, MessengerRow, SearchHit, StatusReport, TypeRefRow,
};

pub fn query_status(conn: &Connection) -> Result<StatusReport> {
    let count = |sql: &str| -> usize {
        conn.query_row(sql, [], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
            .max(0) as usize
    };
    Ok(StatusReport {
        repo_root: get_meta(conn, "repo_root"),
        updated_epoch: get_meta(conn, "updated_epoch"),
        projects: count("SELECT COUNT(*) FROM projects"),
        external_projects: count("SELECT COUNT(*) FROM projects WHERE is_external = 1"),
        files: count("SELECT COUNT(*) FROM files"),
        types: count("SELECT COUNT(*) FROM types"),
        members: count("SELECT COUNT(*) FROM members"),
        calls: count("SELECT COUNT(*) FROM calls"),
        type_refs: count("SELECT COUNT(*) FROM type_refs"),
        di_registrations: count("SELECT COUNT(*) FROM di_registrations"),
        messenger_links: count("SELECT COUNT(*) FROM messenger_links"),
    })
}

pub fn query_partials(
    conn: &Connection,
    filter: Option<&str>,
    min_files: usize,
) -> Result<Vec<PartialCluster>> {
    let filter_pat = filter
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s.to_ascii_lowercase()))
        .unwrap_or_else(|| "%".to_string());

    let mut stmt = conn.prepare(
        "SELECT t.full_name, t.kind, f.project_name, f.rel_path, f.line_count, t.start_line, t.end_line FROM types t JOIN files f ON f.id = t.file_id WHERE t.is_partial = 1 AND t.kind != 'xaml_view' AND LOWER(t.full_name) LIKE ?1 ORDER BY t.full_name, f.rel_path",
    )?;

    let mut grouped: FxHashMap<String, (String, Vec<PartialFileSpan>)> = FxHashMap::default();
    let rows = stmt.query_map(params![filter_pat], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            PartialFileSpan {
                project: r.get(2)?,
                file: r.get(3)?,
                line_count: r.get::<_, i64>(4)?.max(0) as usize,
                start_line: r.get::<_, i64>(5)?.max(0) as u32,
                end_line: r.get::<_, i64>(6)?.max(0) as u32,
            },
        ))
    })?;

    for row in rows.flatten() {
        let entry = grouped.entry(row.0).or_insert_with(|| (row.1, Vec::new()));
        if !entry.1.iter().any(|f| f.file == row.2.file) {
            entry.1.push(row.2);
        }
    }

    let mut out: Vec<PartialCluster> = grouped
        .into_iter()
        .filter(|(_, (_, files))| files.len() >= min_files)
        .map(|(full_name, (kind, files))| {
            let total_lines = files.iter().map(|f| f.line_count).sum();
            let mut projects: Vec<String> = files.iter().map(|f| f.project.clone()).collect();
            projects.sort();
            projects.dedup();
            PartialCluster {
                full_name,
                kind,
                file_count: files.len(),
                total_lines,
                projects,
                files,
            }
        })
        .collect();

    out.sort_by(|a, b| {
        b.file_count
            .cmp(&a.file_count)
            .then_with(|| b.total_lines.cmp(&a.total_lines))
    });
    Ok(out)
}

pub fn query_deps(conn: &Connection, target: &str) -> Result<DepsReport> {
    let pat = format!("%{}%", target.trim().to_ascii_lowercase());
    let mut di_stmt = conn.prepare(
        "SELECT d.lifetime, d.service_type, d.impl_type, f.rel_path, d.line FROM di_registrations d JOIN files f ON f.id = d.file_id WHERE LOWER(d.service_type) LIKE ?1 OR LOWER(d.impl_type) LIKE ?1 OR LOWER(f.project_name) LIKE ?1 LIMIT 80",
    )?;
    let di_registrations = di_stmt
        .query_map(params![&pat], |r| {
            Ok(DiLinkRow {
                lifetime: r.get(0)?,
                service_type: r.get(1)?,
                impl_type: r.get(2)?,
                file: r.get(3)?,
                line: r.get(4)?,
            })
        })?
        .flatten()
        .collect();

    let mut out_stmt = conn.prepare(
        "SELECT tr.source_type, tr.target_type, tr.ref_kind, f.rel_path, tr.line FROM type_refs tr JOIN files f ON f.id = tr.file_id WHERE LOWER(tr.source_type) LIKE ?1 LIMIT 100",
    )?;
    let type_refs_outgoing = out_stmt
        .query_map(params![&pat], map_type_ref)?
        .flatten()
        .collect();

    let mut in_stmt = conn.prepare(
        "SELECT tr.source_type, tr.target_type, tr.ref_kind, f.rel_path, tr.line FROM type_refs tr JOIN files f ON f.id = tr.file_id WHERE LOWER(tr.target_type) LIKE ?1 LIMIT 100",
    )?;
    let type_refs_incoming = in_stmt
        .query_map(params![&pat], map_type_ref)?
        .flatten()
        .collect();

    let mut msg_stmt = conn.prepare(
        "SELECT m.owner_type, m.direction, m.message_type, f.rel_path, m.line FROM messenger_links m JOIN files f ON f.id = m.file_id WHERE LOWER(m.owner_type) LIKE ?1 OR LOWER(m.message_type) LIKE ?1 LIMIT 80",
    )?;
    let messenger_links = msg_stmt
        .query_map(params![&pat], |r| {
            Ok(MessengerRow {
                owner_type: r.get(0)?,
                direction: r.get(1)?,
                message_type: r.get(2)?,
                file: r.get(3)?,
                line: r.get(4)?,
            })
        })?
        .flatten()
        .collect();

    Ok(DepsReport {
        target: target.to_string(),
        di_registrations,
        type_refs_outgoing,
        type_refs_incoming,
        messenger_links,
    })
}

pub fn query_search(
    conn: &Connection,
    query: &str,
    kind: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let pat = format!("%{}%", query.trim().to_ascii_lowercase());
    let cap = limit.clamp(1, 200) as i64;
    let mut hits = Vec::new();

    if kind.is_none() || kind == Some("type") {
        let mut s = conn.prepare(
            "SELECT t.kind, t.full_name, t.namespace, f.project_name, f.rel_path, t.start_line FROM types t JOIN files f ON f.id = t.file_id WHERE LOWER(t.full_name) LIKE ?1 OR LOWER(t.name) LIKE ?1 LIMIT ?2",
        )?;
        hits.extend(s.query_map(params![&pat, cap], map_search_hit)?.flatten());
    }
    if (kind.is_none() || kind == Some("member")) && hits.len() < limit {
        let mut s = conn.prepare(
            "SELECT m.kind, m.signature, m.owner_type, f.project_name, f.rel_path, m.start_line FROM members m JOIN files f ON f.id = m.file_id WHERE LOWER(m.name) LIKE ?1 LIMIT ?2",
        )?;
        hits.extend(s.query_map(params![&pat, cap], map_search_hit)?.flatten());
    }
    if (kind.is_none() || kind == Some("file")) && hits.len() < limit {
        let mut s = conn.prepare(
            "SELECT lang, rel_path, project_name, project_name, rel_path, 1 FROM files WHERE LOWER(rel_path) LIKE ?1 LIMIT ?2",
        )?;
        hits.extend(s.query_map(params![&pat, cap], map_search_hit)?.flatten());
    }

    hits.truncate(limit);
    Ok(hits)
}

fn map_type_ref(r: &rusqlite::Row<'_>) -> rusqlite::Result<TypeRefRow> {
    Ok(TypeRefRow {
        source_type: r.get(0)?,
        target_type: r.get(1)?,
        ref_kind: r.get(2)?,
        file: r.get(3)?,
        line: r.get(4)?,
    })
}

fn map_search_hit(r: &rusqlite::Row<'_>) -> rusqlite::Result<SearchHit> {
    Ok(SearchHit {
        kind: r.get(0)?,
        name: r.get(1)?,
        owner: r.get(2)?,
        project: r.get(3)?,
        file: r.get(4)?,
        line: r.get(5)?,
    })
}

fn get_meta(conn: &Connection, key: &str) -> String {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
        r.get::<_, String>(0)
    })
    .unwrap_or_default()
}
