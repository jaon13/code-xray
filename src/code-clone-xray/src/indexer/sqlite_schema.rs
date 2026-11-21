use std::time::Duration;

use anyhow::Result;
use rusqlite::Connection;

pub const SCHEMA_VERSION: &str = "2";

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_millis(10_000))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    let existing_version: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .ok();
    if existing_version.as_deref() == Some(SCHEMA_VERSION) {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 10000;

        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS projects (
            name TEXT PRIMARY KEY,
            csproj_path TEXT NOT NULL,
            root_dir TEXT NOT NULL,
            layer TEXT NOT NULL,
            is_external INTEGER NOT NULL,
            is_test INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS project_refs (
            from_project TEXT NOT NULL,
            to_project TEXT NOT NULL,
            PRIMARY KEY (from_project, to_project)
        );

        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_name TEXT NOT NULL,
            abs_path TEXT NOT NULL UNIQUE,
            rel_path TEXT NOT NULL,
            lang TEXT NOT NULL,
            mtime_ns INTEGER NOT NULL,
            size_bytes INTEGER NOT NULL,
            content_hash TEXT NOT NULL,
            line_count INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS types (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            project_name TEXT NOT NULL,
            namespace TEXT NOT NULL,
            name TEXT NOT NULL,
            full_name TEXT NOT NULL,
            kind TEXT NOT NULL,
            accessibility TEXT NOT NULL,
            is_partial INTEGER NOT NULL,
            base_types TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS members (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            owner_type TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            signature TEXT NOT NULL,
            return_type TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS calls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            caller_type TEXT NOT NULL,
            caller_member TEXT NOT NULL,
            callee_name TEXT NOT NULL,
            receiver_hint TEXT NOT NULL,
            line INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS type_refs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            source_type TEXT NOT NULL,
            target_type TEXT NOT NULL,
            ref_kind TEXT NOT NULL,
            line INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS di_registrations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            lifetime TEXT NOT NULL,
            service_type TEXT NOT NULL,
            impl_type TEXT NOT NULL,
            line INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS messenger_links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            owner_type TEXT NOT NULL,
            direction TEXT NOT NULL,
            message_type TEXT NOT NULL,
            line INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS state_transitions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            owner_type TEXT NOT NULL,
            machine_name TEXT NOT NULL,
            from_state TEXT NOT NULL,
            to_state TEXT NOT NULL,
            trigger_member TEXT NOT NULL,
            guard_expr TEXT NOT NULL,
            line INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_files_rel ON files(rel_path);
        CREATE INDEX IF NOT EXISTS idx_files_proj ON files(project_name);
        CREATE INDEX IF NOT EXISTS idx_types_name ON types(name);
        CREATE INDEX IF NOT EXISTS idx_types_full ON types(full_name);
        CREATE INDEX IF NOT EXISTS idx_types_file ON types(file_id);
        CREATE INDEX IF NOT EXISTS idx_members_owner ON members(owner_type);
        CREATE INDEX IF NOT EXISTS idx_members_name ON members(name);
        CREATE INDEX IF NOT EXISTS idx_calls_caller ON calls(caller_type);
        CREATE INDEX IF NOT EXISTS idx_calls_callee ON calls(callee_name);
        CREATE INDEX IF NOT EXISTS idx_typerefs_src ON type_refs(source_type);
        CREATE INDEX IF NOT EXISTS idx_typerefs_tgt ON type_refs(target_type);
        CREATE INDEX IF NOT EXISTS idx_di_svc ON di_registrations(service_type);
        CREATE INDEX IF NOT EXISTS idx_di_impl ON di_registrations(impl_type);
        CREATE INDEX IF NOT EXISTS idx_msg_owner ON messenger_links(owner_type);
        CREATE INDEX IF NOT EXISTS idx_msg_type ON messenger_links(message_type);
        CREATE INDEX IF NOT EXISTS idx_state_owner ON state_transitions(owner_type);
        CREATE INDEX IF NOT EXISTS idx_state_machine ON state_transitions(machine_name);
        "#,
    )?;
    Ok(())
}
