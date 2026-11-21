use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use code_clone_xray::grep::{run_grep_command, GrepArgs};
use code_clone_xray::indexer::{
    build_or_refresh_index, ensure_fresh_db,
    slice_query::query_feature_slice,
    sqlite_query::{query_deps, query_partials, query_search, query_states, query_status},
};

#[derive(Parser)]
#[command(
    name = "xray-index",
    version,
    about = "High-speed SQLite multi-language (C#, XAML, Rust, TS/JS/Svelte, Python) codebase indexer & slice query CLI"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build {
        #[arg(short = 'r', long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        force: bool,
        #[arg(long, default_value_t = false)]
        no_external: bool,
    },
    Status {
        #[arg(short = 'r', long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        refresh: bool,
    },
    Slice {
        #[arg(short = 'r', long, default_value = ".")]
        root: PathBuf,
        #[arg(short = 'q', long)]
        query: String,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long, default_value_t = 2)]
        hops: usize,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long, default_value_t = false)]
        no_refresh: bool,
    },
    Partials {
        #[arg(short = 'r', long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        filter: Option<String>,
        #[arg(long, default_value_t = 2)]
        min_files: usize,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        no_refresh: bool,
    },
    Deps {
        #[arg(short = 'r', long, default_value = ".")]
        root: PathBuf,
        #[arg(short = 't', long)]
        target: String,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        no_refresh: bool,
    },
    Search {
        #[arg(short = 'r', long, default_value = ".")]
        root: PathBuf,
        #[arg(short = 'q', long)]
        query: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        no_refresh: bool,
    },
    States {
        #[arg(short = 'r', long, default_value = ".")]
        root: PathBuf,
        #[arg(short = 't', long)]
        target: Option<String>,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        no_refresh: bool,
    },
    Grep(GrepArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let json = match cli.command {
        Commands::Build {
            root,
            db,
            force,
            no_external,
        } => {
            let summary = build_or_refresh_index(&root, db.as_deref(), force, !no_external)?;
            serde_json::to_string_pretty(&summary)?
        }
        Commands::Status { root, db, refresh } => {
            let conn = ensure_fresh_db(&root, db.as_deref(), refresh, true)?;
            let report = query_status(&conn)?;
            serde_json::to_string_pretty(&report)?
        }
        Commands::Slice {
            root,
            query,
            db,
            hops,
            limit,
            no_refresh,
        } => {
            let conn = ensure_fresh_db(&root, db.as_deref(), !no_refresh, true)?;
            let report = query_feature_slice(&conn, &query, hops, limit)?;
            serde_json::to_string_pretty(&report)?
        }
        Commands::Partials {
            root,
            filter,
            min_files,
            db,
            no_refresh,
        } => {
            let conn = ensure_fresh_db(&root, db.as_deref(), !no_refresh, true)?;
            let clusters = query_partials(&conn, filter.as_deref(), min_files)?;
            serde_json::to_string_pretty(&clusters)?
        }
        Commands::Deps {
            root,
            target,
            db,
            no_refresh,
        } => {
            let conn = ensure_fresh_db(&root, db.as_deref(), !no_refresh, true)?;
            let report = query_deps(&conn, &target)?;
            serde_json::to_string_pretty(&report)?
        }
        Commands::Search {
            root,
            query,
            kind,
            limit,
            db,
            no_refresh,
        } => {
            let conn = ensure_fresh_db(&root, db.as_deref(), !no_refresh, true)?;
            let hits = query_search(&conn, &query, kind.as_deref(), limit)?;
            serde_json::to_string_pretty(&hits)?
        }
        Commands::States {
            root,
            target,
            db,
            no_refresh,
        } => {
            let conn = ensure_fresh_db(&root, db.as_deref(), !no_refresh, true)?;
            let report = query_states(&conn, target.as_deref())?;
            serde_json::to_string_pretty(&report)?
        }
        Commands::Grep(args) => run_grep_command(&args)?,
    };

    println!("{json}");
    Ok(())
}
