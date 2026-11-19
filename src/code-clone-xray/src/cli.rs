use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Console,
    Json,
    Both,
}

#[derive(Parser, Debug, Clone)]
#[command(
    name = "structural-clone",
    version,
    about = "Parallel C# + XAML structural clone detector (tree-sitter + Rabin-Karp)"
)]
pub struct CliArgs {
    /// Target directories or files to scan (supports multiple roots)
    #[arg(short = 'p', long = "path", visible_alias = "paths", num_args = 1.., default_value = ".")]
    pub path: Vec<PathBuf>,

    /// Minimum source lines required for a clone block
    #[arg(long, default_value_t = 12)]
    pub min_lines: usize,

    /// Minimum structural similarity percentage (0.0 - 100.0)
    #[arg(long, default_value_t = 80.0)]
    pub min_similarity: f64,

    /// Sliding-window / minimum token size in normalized tokens
    #[arg(long, visible_alias = "min-tokens", default_value_t = 40)]
    pub window: usize,

    /// Comma-separated extensions to include (cs,xaml,rs,ts,js,svelte,py,html,css or 'all')
    #[arg(long, default_value = "cs,xaml,rs,ts,js,svelte,py,html,css")]
    pub extensions: String,

    /// Extra directory/file name fragments to ignore (comma-separated)
    #[arg(long, default_value = "")]
    pub ignore: String,

    /// Include test projects and test folders (*.Tests, tests/)
    #[arg(long, default_value_t = false)]
    pub include_tests: bool,

    /// Maximum clone pairs/groups to include in output (0 = unlimited)
    #[arg(long, default_value_t = 15)]
    pub top: usize,

    /// Output format: console, json, or both
    #[arg(long, value_enum, default_value_t = OutputFormat::Console)]
    pub format: OutputFormat,

    /// Optional file path to write JSON report to
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

impl CliArgs {
    fn has_ext(&self, aliases: &[&str]) -> bool {
        self.extensions.split(',').any(|ext| {
            let trimmed = ext.trim();
            trimmed.eq_ignore_ascii_case("all")
                || aliases.iter().any(|a| trimmed.eq_ignore_ascii_case(a))
        })
    }

    pub fn wants_cs(&self) -> bool {
        self.has_ext(&["cs", "csharp"])
    }

    pub fn wants_xaml(&self) -> bool {
        self.has_ext(&["xaml"])
    }

    pub fn wants_rs(&self) -> bool {
        self.has_ext(&["rs", "rust"])
    }

    pub fn wants_ts(&self) -> bool {
        self.has_ext(&["ts", "tsx", "mts", "cts", "typescript"])
    }

    pub fn wants_js(&self) -> bool {
        self.has_ext(&["js", "jsx", "mjs", "cjs", "javascript"])
    }

    pub fn wants_py(&self) -> bool {
        self.has_ext(&["py", "python"])
    }

    pub fn wants_svelte(&self) -> bool {
        self.has_ext(&["svelte"])
    }

    pub fn wants_html(&self) -> bool {
        self.has_ext(&["html", "htm"])
    }

    pub fn wants_css(&self) -> bool {
        self.has_ext(&["css"])
    }

    pub fn ignore_fragments(&self) -> Vec<String> {
        self.ignore
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.path.is_empty() {
            return Err("--path requires at least one directory or file".into());
        }
        if self.window < 5 {
            return Err("--window must be >= 5".into());
        }
        if !(0.0..=100.0).contains(&self.min_similarity) {
            return Err("--min-similarity must be between 0 and 100".into());
        }
        if self.min_lines == 0 {
            return Err("--min-lines must be >= 1".into());
        }
        Ok(())
    }
}
