use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rayon::prelude::*;
use rusqlite::{params, Connection, TransactionBehavior};
use rustc_hash::{FxHashMap, FxHashSet};

use super::cs_extractor::extract_csharp_file;
use super::model::{DiscoveredFile, ExtractedFileData, ProjectInfo};
use super::path_utils::fnv1a_128_hex;
use super::py_extractor::extract_python_file;
use super::rs_extractor::extract_rust_file;
use super::sqlite_schema::SCHEMA_VERSION;
use super::ts_js_svelte_extractor::extract_ts_js_svelte_file;
use super::xaml_extractor::extract_xaml_file;

struct ExistingFileRow {
    id: i64,
    mtime_ns: i64,
    size_bytes: i64,
    content_hash: String,
}

enum DeltaResult {
    TouchOnly {
        id: i64,
        mtime_ns: i64,
        size_bytes: i64,
    },
    Parsed {
        old_id: Option<i64>,
        data: Box<ExtractedFileData>,
    },
}

pub struct WriteStats {
    pub parsed_files: usize,
    pub skipped_unchanged: usize,
    pub removed_files: usize,
}

pub fn write_incremental_delta(
    conn: &mut Connection,
    repo_root: &str,
    projects: &[ProjectInfo],
    files: &[DiscoveredFile],
    force: bool,
) -> Result<WriteStats> {
    let existing = load_existing_files(conn)?;
    let current_paths: FxHashSet<&str> = files.iter().map(|f| f.abs_path.as_str()).collect();

    let mut removed_ids: Vec<i64> = Vec::new();
    for (abs_path, row) in &existing {
        if !current_paths.contains(abs_path.as_str()) {
            removed_ids.push(row.id);
        }
    }

    let mut skipped_unchanged = 0usize;
    let mut candidates: Vec<(&DiscoveredFile, Option<&ExistingFileRow>)> = Vec::new();

    for f in files {
        if let Some(prev) = existing.get(&f.abs_path) {
            if !force && prev.mtime_ns == f.mtime_ns && prev.size_bytes == f.size_bytes {
                skipped_unchanged += 1;
                continue;
            }
            candidates.push((f, Some(prev)));
        } else {
            candidates.push((f, None));
        }
    }

    let processed: Vec<DeltaResult> = candidates
        .par_iter()
        .filter_map(|(file, prev_opt)| {
            let bytes = fs::read(&file.abs_path).ok()?;
            let hash = fnv1a_128_hex(&bytes);
            if !force {
                if let Some(prev) = prev_opt {
                    if prev.content_hash == hash {
                        return Some(DeltaResult::TouchOnly {
                            id: prev.id,
                            mtime_ns: file.mtime_ns,
                            size_bytes: file.size_bytes,
                        });
                    }
                }
            }
            let data = match file.lang {
                "xaml" => extract_xaml_file(file, &bytes).ok()?,
                "rs" => extract_rust_file(file, &bytes).ok()?,
                "ts" | "tsx" | "js" | "svelte" => extract_ts_js_svelte_file(file, &bytes).ok()?,
                "py" => extract_python_file(file, &bytes).ok()?,
                _ => extract_csharp_file(file, &bytes).ok()?,
            };
            Some(DeltaResult::Parsed {
                old_id: prev_opt.map(|p| p.id),
                data: Box::new(data),
            })
        })
        .collect();

    let mut parsed_count = 0usize;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    tx.execute("DELETE FROM projects", [])?;
    tx.execute("DELETE FROM project_refs", [])?;
    {
        let mut proj_stmt = tx.prepare(
            "INSERT INTO projects (name, csproj_path, root_dir, layer, is_external, is_test) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut ref_stmt = tx.prepare(
            "INSERT OR IGNORE INTO project_refs (from_project, to_project) VALUES (?1, ?2)",
        )?;
        for p in projects {
            proj_stmt.execute(params![
                p.name,
                p.csproj_path,
                p.root_dir,
                p.layer,
                i32::from(p.is_external),
                i32::from(p.is_test)
            ])?;
            for r in &p.refs {
                ref_stmt.execute(params![p.name, r])?;
            }
        }
    }

    {
        let mut del_stmt = tx.prepare("DELETE FROM files WHERE id = ?1")?;
        for rid in &removed_ids {
            del_stmt.execute(params![rid])?;
        }
    }

    {
        let mut touch_stmt =
            tx.prepare("UPDATE files SET mtime_ns = ?1, size_bytes = ?2 WHERE id = ?3")?;
        let mut del_stmt = tx.prepare("DELETE FROM files WHERE id = ?1")?;
        let mut ins_file = tx.prepare(
            "INSERT INTO files (project_name, abs_path, rel_path, lang, mtime_ns, size_bytes, content_hash, line_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        let mut ins_type = tx.prepare(
            "INSERT INTO types (file_id, project_name, namespace, name, full_name, kind, accessibility, is_partial, base_types, start_line, end_line) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        let mut ins_member = tx.prepare(
            "INSERT INTO members (file_id, owner_type, name, kind, signature, return_type, start_line, end_line) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        let mut ins_call = tx.prepare(
            "INSERT INTO calls (file_id, caller_type, caller_member, callee_name, receiver_hint, line) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut ins_ref = tx.prepare(
            "INSERT INTO type_refs (file_id, source_type, target_type, ref_kind, line) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        let mut ins_di = tx.prepare(
            "INSERT INTO di_registrations (file_id, lifetime, service_type, impl_type, line) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        let mut ins_msg = tx.prepare(
            "INSERT INTO messenger_links (file_id, owner_type, direction, message_type, line) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;

        for item in &processed {
            match item {
                DeltaResult::TouchOnly {
                    id,
                    mtime_ns,
                    size_bytes,
                } => {
                    skipped_unchanged += 1;
                    touch_stmt.execute(params![mtime_ns, size_bytes, id])?;
                }
                DeltaResult::Parsed { old_id, data } => {
                    parsed_count += 1;
                    if let Some(oid) = old_id {
                        del_stmt.execute(params![oid])?;
                    }
                    ins_file.execute(params![
                        data.file.project_name,
                        data.file.abs_path,
                        data.file.rel_path,
                        data.file.lang,
                        data.file.mtime_ns,
                        data.file.size_bytes,
                        data.content_hash,
                        data.line_count as i64
                    ])?;
                    let file_id = tx.last_insert_rowid();

                    for t in &data.types {
                        ins_type.execute(params![
                            file_id,
                            data.file.project_name,
                            t.namespace,
                            t.name,
                            t.full_name,
                            t.kind,
                            t.accessibility,
                            i32::from(t.is_partial),
                            t.base_types.join(", "),
                            t.start_line as i64,
                            t.end_line as i64
                        ])?;
                    }
                    for m in &data.members {
                        ins_member.execute(params![
                            file_id,
                            m.owner_type,
                            m.name,
                            m.kind,
                            m.signature,
                            m.return_type,
                            m.start_line as i64,
                            m.end_line as i64
                        ])?;
                    }
                    for c in &data.calls {
                        ins_call.execute(params![
                            file_id,
                            c.caller_type,
                            c.caller_member,
                            c.callee_name,
                            c.receiver_hint,
                            c.line as i64
                        ])?;
                    }
                    for tr in &data.type_refs {
                        ins_ref.execute(params![
                            file_id,
                            tr.source_type,
                            tr.target_type,
                            tr.ref_kind,
                            tr.line as i64
                        ])?;
                    }
                    for d in &data.di_regs {
                        ins_di.execute(params![
                            file_id,
                            d.lifetime,
                            d.service_type,
                            d.impl_type,
                            d.line as i64
                        ])?;
                    }
                    for ml in &data.messenger_links {
                        ins_msg.execute(params![
                            file_id,
                            ml.owner_type,
                            ml.direction,
                            ml.message_type,
                            ml.line as i64
                        ])?;
                    }
                }
            }
        }
    }

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string());
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1), ('repo_root', ?2), ('updated_epoch', ?3)",
        params![SCHEMA_VERSION, repo_root, now_epoch],
    )?;
    tx.commit()?;

    Ok(WriteStats {
        parsed_files: parsed_count,
        skipped_unchanged,
        removed_files: removed_ids.len(),
    })
}

fn load_existing_files(conn: &Connection) -> Result<FxHashMap<String, ExistingFileRow>> {
    let mut map = FxHashMap::default();
    let mut stmt =
        conn.prepare("SELECT id, abs_path, mtime_ns, size_bytes, content_hash FROM files")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(1)?,
            ExistingFileRow {
                id: r.get(0)?,
                mtime_ns: r.get(2)?,
                size_bytes: r.get(3)?,
                content_hash: r.get(4)?,
            },
        ))
    })?;
    for (path, data) in rows.flatten() {
        map.insert(path, data);
    }
    Ok(map)
}
