//! Structural clone detection & multi-language code intelligence library.
//! CLI binaries: `structural-clone`, `xray-index`, `xray-grep`.

pub mod cli;
pub mod detect;
pub mod grep;
pub mod hash;
pub mod indexer;
pub mod model;
pub mod normalize;
pub mod parse;
pub(crate) mod project_name;
pub mod report;
pub(crate) mod stats;
pub mod walk;

pub use cli::{CliArgs, OutputFormat};
pub use detect::{detect_clones, DetectConfig};
pub use model::{CloneReport, SCHEMA_VERSION};
pub use normalize::{parse_files_parallel, ParsedSourceFile};
pub use report::{print_console, render_json};
pub use walk::{collect_source_files, ScannedFile, SourceLang};
