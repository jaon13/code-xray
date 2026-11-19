use std::collections::BTreeMap;
use std::fs;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use regex::{Regex, RegexBuilder};

use super::context::resolve_enclosing_symbols;
use super::discovery::{collect_candidate_files, parse_ext_filter, CandidateFile};
use super::review::{new_run_id, utc_timestamps_now};
use super::{ContextLine, GrepArgs, GrepFileGroup, GrepMatch, GrepReport, GrepSummary};
use crate::indexer::path_utils::{clean_canonical, to_slash};

enum PatternMatcher {
    Literal(String),
    Regex(Regex),
}

impl PatternMatcher {
    fn compile(pattern: &str, is_regex: bool, case_insensitive: bool) -> Result<Self> {
        if pattern.is_empty() {
            bail!("--pattern must not be empty");
        }
        if is_regex {
            let re = RegexBuilder::new(pattern)
                .case_insensitive(case_insensitive)
                .build()
                .with_context(|| format!("Invalid regex pattern '{pattern}'"))?;
            Ok(Self::Regex(re))
        } else if case_insensitive {
            let escaped = regex::escape(pattern);
            let re = RegexBuilder::new(&escaped)
                .case_insensitive(true)
                .build()
                .with_context(|| {
                    format!("Failed to compile case-insensitive pattern '{pattern}'")
                })?;
            Ok(Self::Regex(re))
        } else {
            Ok(Self::Literal(pattern.to_string()))
        }
    }

    fn find_first(&self, line: &str) -> Option<(usize, String)> {
        match self {
            Self::Literal(needle) => line
                .find(needle.as_str())
                .map(|byte_idx| (line[..byte_idx].chars().count() + 1, needle.clone())),
            Self::Regex(re) => re.find(line).map(|m| {
                (
                    line[..m.start()].chars().count() + 1,
                    m.as_str().to_string(),
                )
            }),
        }
    }
}

pub fn execute_grep(args: &GrepArgs) -> Result<GrepReport> {
    let started = Instant::now();
    if !args.root.exists() {
        bail!("Search root does not exist: {}", args.root.display());
    }
    let canon_root = clean_canonical(&args.root);
    let matcher = PatternMatcher::compile(&args.pattern, args.regex, args.case_insensitive)?;
    let exts = parse_ext_filter(args.ext.as_deref());
    let candidates = collect_candidate_files(&canon_root, args.include_external, &exts);
    let total_files_scanned = candidates.len();

    let per_file_cap = if args.max_matches == 0 {
        usize::MAX
    } else {
        args.max_matches
    };
    let mut matched_groups: Vec<GrepFileGroup> = candidates
        .par_iter()
        .filter_map(|cand| scan_single_file(cand, &matcher, args.context, per_file_cap))
        .collect();

    matched_groups.sort_by(|a, b| {
        a.is_external
            .cmp(&b.is_external)
            .then_with(|| a.file.cmp(&b.file))
    });

    let total_matched_files = matched_groups.len();
    let total_matches_found: usize = matched_groups.iter().map(|g| g.matches.len()).sum();

    let (groups, truncated) = apply_caps(matched_groups, args.max_files, args.max_matches);
    let summary = build_summary(&groups);
    let total_matches: usize = groups.iter().map(|g| g.matches.len()).sum();
    let returned_files = groups.len();

    let (compact_ts, created_at) = utc_timestamps_now();
    let run_id = new_run_id(&args.pattern, &compact_ts);

    Ok(GrepReport {
        schema_version: "1.0".to_string(),
        tool: "xray-grep".to_string(),
        run_id,
        created_at,
        repo_root: to_slash(&canon_root),
        pattern: args.pattern.clone(),
        is_regex: args.regex,
        case_insensitive: args.case_insensitive,
        extensions: exts,
        include_external: args.include_external,
        total_files_scanned,
        total_matched_files,
        returned_files,
        total_matches_found,
        total_matches,
        truncated,
        elapsed_ms: started.elapsed().as_millis(),
        review_manifest: None,
        review_markdown: None,
        summary,
        groups,
    })
}

fn scan_single_file(
    cand: &CandidateFile,
    matcher: &PatternMatcher,
    context_lines: usize,
    per_file_cap: usize,
) -> Option<GrepFileGroup> {
    let raw_bytes = fs::read(&cand.abs_path).ok()?;
    if raw_bytes.is_empty() || raw_bytes[..raw_bytes.len().min(8192)].contains(&0) {
        return None;
    }
    let clean_bytes = raw_bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(&raw_bytes);
    let text = String::from_utf8_lossy(clean_bytes);
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();

    let mut raw_hits: Vec<(usize, usize, String)> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if let Some((col, matched)) = matcher.find_first(line) {
            raw_hits.push((idx + 1, col, matched));
            if raw_hits.len() >= per_file_cap {
                break;
            }
        }
    }

    if raw_hits.is_empty() {
        return None;
    }

    let hit_lines: Vec<usize> = raw_hits.iter().map(|(ln, _, _)| *ln).collect();
    let sym_info = resolve_enclosing_symbols(&cand.abs_path, clean_bytes, &hit_lines);
    let abs_str = to_slash(&cand.abs_path);

    let matches: Vec<GrepMatch> = raw_hits
        .into_iter()
        .map(|(line_no, col, match_text)| {
            let line_idx = line_no - 1;
            let start_ctx = line_idx.saturating_sub(context_lines);
            let end_ctx = (line_idx + context_lines + 1).min(lines.len());

            let context_before = (start_ctx..line_idx)
                .map(|i| ContextLine {
                    line: i + 1,
                    text: clip_line(lines[i], 500),
                })
                .collect();
            let context_after = ((line_idx + 1)..end_ctx)
                .map(|i| ContextLine {
                    line: i + 1,
                    text: clip_line(lines[i], 500),
                })
                .collect();

            let category = if sym_info.test_lines.contains(&line_no) {
                "test".to_string()
            } else {
                cand.base_category.to_string()
            };

            GrepMatch {
                id: String::new(),
                file: cand.rel_path.clone(),
                abs_path: abs_str.clone(),
                line: line_no,
                column: col,
                match_text,
                enclosing_symbol: sym_info.symbols.get(&line_no).cloned(),
                project_name: cand.project_name.clone(),
                is_external: cand.is_external,
                category,
                snippet: clip_line(lines[line_idx], 500),
                context_before,
                context_after,
                status: "pending".to_string(),
                disposition: "manual".to_string(),
            }
        })
        .collect();

    let group_category = if matches.iter().all(|m| m.category == "test") {
        "test".to_string()
    } else {
        cand.base_category.to_string()
    };

    Some(GrepFileGroup {
        project_name: cand.project_name.clone(),
        file: cand.rel_path.clone(),
        abs_path: abs_str,
        is_external: cand.is_external,
        category: group_category,
        match_count: matches.len(),
        matches,
    })
}

fn clip_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let clipped: String = line.chars().take(max_chars).collect();
    format!("{clipped}...")
}

fn apply_caps(
    groups: Vec<GrepFileGroup>,
    max_files: usize,
    max_matches: usize,
) -> (Vec<GrepFileGroup>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    let mut match_seq = 0usize;

    for mut g in groups {
        if max_files > 0 && out.len() >= max_files {
            truncated = true;
            break;
        }
        if max_matches > 0 && match_seq >= max_matches {
            truncated = true;
            break;
        }
        if max_matches > 0 && match_seq + g.matches.len() > max_matches {
            let take = max_matches - match_seq;
            g.matches.truncate(take);
            g.match_count = g.matches.len();
            truncated = true;
        }
        for m in &mut g.matches {
            match_seq += 1;
            m.id = format!("M{match_seq:03}");
        }
        out.push(g);
    }
    (out, truncated)
}

fn build_summary(groups: &[GrepFileGroup]) -> GrepSummary {
    let mut by_category: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_project: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_disposition: BTreeMap<String, usize> = BTreeMap::new();

    for g in groups {
        for m in &g.matches {
            *by_category.entry(m.category.clone()).or_insert(0) += 1;
            *by_project.entry(m.project_name.clone()).or_insert(0) += 1;
            *by_disposition.entry(m.disposition.clone()).or_insert(0) += 1;
        }
    }

    GrepSummary {
        by_category,
        by_project,
        by_disposition,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::grep::{run_grep_command, GrepFormat};

    fn make_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("xray_grep_test_{label}_{nanos}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn base_args(root: PathBuf, pattern: &str) -> GrepArgs {
        GrepArgs {
            root,
            pattern: pattern.to_string(),
            regex: false,
            case_insensitive: false,
            ext: None,
            include_external: false,
            context: 1,
            max_files: 100,
            max_matches: 500,
            format: GrepFormat::Json,
            save_review: false,
            review_dir: None,
        }
    }

    #[test]
    fn skips_ignored_dirs_tags_symbols_categories_and_isolates_reviews() {
        let root = make_temp_dir("core");
        let app_dir = root.join("src").join("App");
        let test_dir = root.join("tests").join("App.Tests");
        let docs_dir = root.join("docs");
        for d in [
            app_dir.clone(),
            test_dir.clone(),
            docs_dir.clone(),
            root.join("bin"),
            root.join("obj"),
            root.join("target"),
            root.join(".venv"),
            root.join("node_modules"),
            root.join(".xray"),
            root.join("Build"),
        ] {
            fs::create_dir_all(&d).expect("mkdir");
        }

        fs::write(
            app_dir.join("App.csproj"),
            "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>\n",
        )
        .expect("csproj");
        fs::write(
            app_dir.join("Worker.cs"),
            "namespace App;\npublic sealed class Worker {\n    public string Run() => \"TARGET_TOKEN\";\n}\n",
        )
        .expect("cs");
        fs::write(
            test_dir.join("WorkerTests.cs"),
            "public class WorkerTests {\n    public void TestRun() { var s = \"target_token\"; }\n}\n",
        )
        .expect("test cs");
        fs::write(
            root.join("appsettings.json"),
            "{\n  \"Key\": \"TARGET_TOKEN\"\n}\n",
        )
        .expect("json");
        fs::write(docs_dir.join("guide.md"), "# Guide\nUse TARGET_TOKEN.\n").expect("md");
        fs::write(root.join("bin").join("ignored.cs"), "TARGET_TOKEN\n").expect("ignored");
        fs::write(root.join("Build").join("ignored.cs"), "TARGET_TOKEN\n").expect("ignored");
        fs::write(
            root.join("node_modules").join("ignored.js"),
            "TARGET_TOKEN\n",
        )
        .expect("ignored");

        let mut args = base_args(root.clone(), "target_token");
        args.case_insensitive = true;
        args.save_review = true;

        let json_out = run_grep_command(&args).expect("run grep 1");
        let val: serde_json::Value = serde_json::from_str(&json_out).expect("parse json");
        assert_eq!(val["totalMatches"], 4);
        assert_eq!(val["summary"]["byCategory"]["code"], 1);
        assert_eq!(val["summary"]["byCategory"]["test"], 1);
        assert_eq!(val["summary"]["byCategory"]["config"], 1);
        assert_eq!(val["summary"]["byCategory"]["docs"], 1);

        let manifest1 = PathBuf::from(val["reviewManifest"].as_str().expect("manifest path"));
        assert!(manifest1.is_file());
        let manifest_val: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&manifest1).expect("read manifest"))
                .expect("manifest json");
        assert_eq!(manifest_val["matches"][0]["status"], "pending");
        assert_eq!(manifest_val["matches"][0]["disposition"], "manual");
        assert!(manifest_val["matches"]
            .as_array()
            .expect("array")
            .iter()
            .any(|m| m["enclosing_symbol"] == "Worker.Run"));

        let json_out_2 = run_grep_command(&args).expect("run grep 2");
        let val2: serde_json::Value = serde_json::from_str(&json_out_2).expect("parse json 2");
        let manifest2 = PathBuf::from(val2["reviewManifest"].as_str().expect("manifest 2"));
        assert!(manifest2.is_file());
        assert_ne!(manifest1, manifest2);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn supports_include_external_regex_ext_and_caps() {
        let base = make_temp_dir("ext");
        let repo = base.join("MainRepo");
        let common = base.join("SharedLib");
        fs::create_dir_all(repo.join("src").join("App")).expect("mkdir");
        fs::create_dir_all(common.join("src").join("Shared")).expect("mkdir");

        fs::write(
            repo.join("src").join("App").join("App.csproj"),
            "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup><ProjectReference Include=\"..\\..\\..\\SharedLib\\src\\Shared\\Shared.csproj\" /></ItemGroup></Project>\n",
        )
        .expect("app csproj");
        fs::write(
            common.join("src").join("Shared").join("Shared.csproj"),
            "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>\n",
        )
        .expect("shared csproj");
        fs::write(
            repo.join("src").join("App").join("Local.cs"),
            "class Local { void M1() { CallFoo(1); } }\n",
        )
        .expect("local cs");
        fs::write(
            common.join("src").join("Shared").join("Ext.cs"),
            "class Ext { void M2() { CallFoo(2); } }\n",
        )
        .expect("ext cs");

        let mut args = base_args(repo.clone(), r"CallFoo\(\d+\)");
        args.regex = true;
        args.ext = Some("cs".to_string());
        args.include_external = true;

        let report = execute_grep(&args).expect("grep with external");
        assert_eq!(report.total_matches, 2);
        assert!(report.groups.iter().any(|g| !g.is_external));
        assert!(report
            .groups
            .iter()
            .any(|g| g.is_external && g.project_name == "Shared"));

        args.max_matches = 1;
        let capped = execute_grep(&args).expect("capped");
        assert!(capped.truncated);
        assert_eq!(capped.total_matches, 1);

        let _ = fs::remove_dir_all(&base);
    }
}
