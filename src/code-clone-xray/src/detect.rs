use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::hash::{fingerprint_tokens, window_hashes};
use crate::model::{CloneReport, DuplicatePair, FileSpan, ReportConfig, SCHEMA_VERSION};
use crate::normalize::{NormalizedToken, ParsedSourceFile};
use crate::stats::{build_groups, compute_stats, RawPair};

const MAX_GAP_TOKENS: usize = 16;

#[derive(Clone, Debug)]
pub struct DetectConfig {
    pub paths: Vec<String>,
    pub min_lines: usize,
    pub min_similarity: f64,
    pub window: usize,
    pub top: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ExactRun {
    a0: usize,
    a1: usize,
    b0: usize,
    b1: usize,
}

pub fn detect_clones(files: &[ParsedSourceFile], config: &DetectConfig) -> CloneReport {
    let min_tokens = config.window.max(5);
    let seed_w = if config.min_similarity >= 99.9 {
        min_tokens
    } else {
        (min_tokens * 2 / 5).clamp(5, 14).min(min_tokens)
    };

    let per_file_hashes: Vec<(usize, Vec<(usize, u64)>)> = files
        .par_iter()
        .enumerate()
        .filter_map(|(idx, f)| {
            if f.tokens.len() < min_tokens {
                return None;
            }
            let wh = window_hashes(&f.tokens, seed_w);
            (!wh.is_empty()).then_some((idx, wh))
        })
        .collect();

    let mut buckets: FxHashMap<u64, Vec<(usize, usize)>> = FxHashMap::default();
    for (file_idx, wh) in per_file_hashes {
        for (pos, hash) in wh {
            buckets.entry(hash).or_default().push((file_idx, pos));
        }
    }

    let mut covered_diag: FxHashMap<(usize, usize, isize), usize> = FxHashMap::default();
    let mut pair_runs: FxHashMap<(usize, usize), Vec<ExactRun>> = FxHashMap::default();

    for hits in buckets.values() {
        if hits.len() < 2 || hits.len() > 80 {
            continue;
        }
        for i in 0..hits.len() {
            for j in (i + 1)..hits.len() {
                let ((fa, sa), (fb, sb)) = order_hit(hits[i], hits[j]);
                if files[fa].lang != files[fb].lang {
                    continue;
                }
                if fa == fb && sb < sa + seed_w {
                    continue;
                }
                let diag = (sb as isize) - (sa as isize);
                if covered_diag
                    .get(&(fa, fb, diag))
                    .is_some_and(|&cov_end| sa < cov_end)
                {
                    continue;
                }
                if !tokens_equal_window(&files[fa].tokens, sa, &files[fb].tokens, sb, seed_w) {
                    continue;
                }

                let run = extend_match(
                    &files[fa].tokens,
                    sa,
                    &files[fb].tokens,
                    sb,
                    seed_w,
                    fa == fb,
                );
                covered_diag
                    .entry((fa, fb, diag))
                    .and_modify(|e| *e = (*e).max(run.a1))
                    .or_insert(run.a1);
                pair_runs.entry((fa, fb)).or_default().push(run);
            }
        }
    }

    let mut raw_pairs: Vec<RawPair> = pair_runs
        .into_par_iter()
        .flat_map_iter(|((fa, fb), runs)| {
            chain_and_filter_pair(fa, fb, &files[fa], &files[fb], runs, min_tokens, config)
        })
        .collect();

    raw_pairs.sort_by(|a, b| {
        b.tokens
            .cmp(&a.tokens)
            .then(b.lines.cmp(&a.lines))
            .then(a.a_file.cmp(&b.a_file))
    });
    let pairs = dedupe_pairs(raw_pairs);

    let mut duplicates: Vec<DuplicatePair> = pairs
        .iter()
        .map(|p| DuplicatePair {
            id: format!("{:08x}", p.fingerprint as u32),
            format: p.format.format_name().to_string(),
            similarity: p.similarity,
            lines: p.lines,
            tokens: p.tokens,
            first_file: FileSpan {
                path: files[p.a_file].display_path.clone(),
                start_line: p.a_start_line,
                end_line: p.a_end_line,
            },
            second_file: FileSpan {
                path: files[p.b_file].display_path.clone(),
                start_line: p.b_start_line,
                end_line: p.b_end_line,
            },
        })
        .collect();

    let mut clone_groups = build_groups(&pairs, files);
    duplicates.sort_by_key(|d| std::cmp::Reverse(d.lines));
    clone_groups.sort_by(|a, b| {
        b.instance_count.cmp(&a.instance_count).then_with(|| {
            let la = a
                .occurrences
                .first()
                .map_or(0, |o| o.end_line.saturating_sub(o.start_line));
            let lb = b
                .occurrences
                .first()
                .map_or(0, |o| o.end_line.saturating_sub(o.start_line));
            lb.cmp(&la)
        })
    });

    // Compute corpus-wide statistics BEFORE truncating display lists to `--top`
    let statistics = compute_stats(files, &pairs, &clone_groups);

    if config.top > 0 {
        duplicates.truncate(config.top);
        clone_groups.truncate(config.top);
    }

    CloneReport {
        schema_version: SCHEMA_VERSION.to_string(),
        tool: "structural-clone".to_string(),
        config: ReportConfig {
            paths: config.paths.clone(),
            min_lines: config.min_lines,
            min_similarity: config.min_similarity,
            window: config.window,
        },
        statistics,
        duplicates,
        clone_groups,
    }
}

fn chain_and_filter_pair(
    fa: usize,
    fb: usize,
    file_a: &ParsedSourceFile,
    file_b: &ParsedSourceFile,
    mut runs: Vec<ExactRun>,
    min_tokens: usize,
    config: &DetectConfig,
) -> Vec<RawPair> {
    runs.sort_by(|x, y| x.a0.cmp(&y.a0).then(x.b0.cmp(&y.b0)).then(y.a1.cmp(&x.a1)));
    runs.dedup();

    let same_file = fa == fb;
    let mut consumed = vec![false; runs.len()];
    let mut out = Vec::new();

    for i in 0..runs.len() {
        if consumed[i] {
            continue;
        }
        let seed = runs[i];
        let (ca0, mut ca1, cb0, mut cb1) = (seed.a0, seed.a1, seed.b0, seed.b1);
        let mut matched = seed.a1 - seed.a0;

        for j in (i + 1)..runs.len() {
            let r = runs[j];
            let trim = ca1.saturating_sub(r.a0).max(cb1.saturating_sub(r.b0));
            if r.a1 <= r.a0 + trim || r.b1 <= r.b0 + trim {
                if r.a0 >= ca0 && r.a1 <= ca1 && r.b0 >= cb0 && r.b1 <= cb1 {
                    consumed[j] = true;
                }
                continue;
            }
            let (na0, nb0) = (r.a0 + trim, r.b0 + trim);
            if na0 < ca1 || nb0 < cb1 {
                continue;
            }
            let (gap_a, gap_b) = (na0 - ca1, nb0 - cb1);
            if gap_a > MAX_GAP_TOKENS || gap_b > MAX_GAP_TOKENS {
                if gap_a > MAX_GAP_TOKENS && r.a0 > ca1 + MAX_GAP_TOKENS {
                    break;
                }
                continue;
            }
            if same_file && r.a1 > cb0 {
                continue;
            }
            matched +=
                small_gap_lcs(&file_a.tokens[ca1..na0], &file_b.tokens[cb1..nb0]) + (r.a1 - na0);
            ca1 = r.a1;
            cb1 = r.b1;
            consumed[j] = true;
        }

        let (len_a, len_b) = (ca1 - ca0, cb1 - cb0);
        let max_tok = len_a.max(len_b);
        if max_tok < min_tokens || (same_file && ca1 > cb0) {
            continue;
        }

        let (a_sl, a_el) = line_span(&file_a.tokens, ca0, ca1);
        let (b_sl, b_el) = line_span(&file_b.tokens, cb0, cb1);
        if same_file && a_el >= b_sl {
            continue;
        }
        let lines = ((a_el - a_sl + 1).min(b_el - b_sl + 1)) as usize;
        if lines < config.min_lines {
            continue;
        }

        let sim = (((2.0 * matched as f64) / ((len_a + len_b) as f64) * 100.0).min(100.0) * 10.0)
            .round()
            / 10.0;
        if sim + f64::EPSILON < config.min_similarity {
            continue;
        }

        let fp = if sim >= 99.9 {
            fingerprint_tokens(&file_a.tokens[ca0..ca1])
        } else {
            fingerprint_tokens(&file_a.tokens[seed.a0..seed.a1])
        };

        let (r_fa, r_asl, r_ael, r_fb, r_bsl, r_bel) =
            if (&file_a.display_path, a_sl) <= (&file_b.display_path, b_sl) {
                (fa, a_sl, a_el, fb, b_sl, b_el)
            } else {
                (fb, b_sl, b_el, fa, a_sl, a_el)
            };

        out.push(RawPair {
            format: file_a.lang,
            similarity: sim,
            lines,
            tokens: max_tok,
            fingerprint: fp,
            a_file: r_fa,
            a_start_line: r_asl,
            a_end_line: r_ael,
            b_file: r_fb,
            b_start_line: r_bsl,
            b_end_line: r_bel,
        });
    }
    out
}

#[inline]
fn order_hit(x: (usize, usize), y: (usize, usize)) -> ((usize, usize), (usize, usize)) {
    if x <= y {
        (x, y)
    } else {
        (y, x)
    }
}

#[inline]
fn tokens_equal_window(
    a: &[NormalizedToken],
    sa: usize,
    b: &[NormalizedToken],
    sb: usize,
    w: usize,
) -> bool {
    sa + w <= a.len() && sb + w <= b.len() && (0..w).all(|k| a[sa + k].symbol == b[sb + k].symbol)
}

fn extend_match(
    a: &[NormalizedToken],
    sa: usize,
    b: &[NormalizedToken],
    sb: usize,
    w: usize,
    same_file: bool,
) -> ExactRun {
    let (mut a0, mut b0, mut a1, mut b1) = (sa, sb, sa + w, sb + w);
    while a0 > 0 && b0 > 0 && (!same_file || a1 < b0) && a[a0 - 1].symbol == b[b0 - 1].symbol {
        a0 -= 1;
        b0 -= 1;
    }
    while a1 < a.len() && b1 < b.len() && (!same_file || a1 < b0) && a[a1].symbol == b[b1].symbol {
        a1 += 1;
        b1 += 1;
    }
    ExactRun { a0, a1, b0, b1 }
}

fn small_gap_lcs(a: &[NormalizedToken], b: &[NormalizedToken]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let mut prev = [0u16; 18];
    let mut curr = [0u16; 18];
    let m = b.len().min(17);
    for tok_a in a.iter().take(17) {
        for j in 0..m {
            curr[j + 1] = if tok_a.symbol == b[j].symbol {
                prev[j] + 1
            } else {
                prev[j + 1].max(curr[j])
            };
        }
        prev[..=m].copy_from_slice(&curr[..=m]);
    }
    usize::from(prev[m])
}

#[inline]
fn line_span(tokens: &[NormalizedToken], start: usize, end: usize) -> (u32, u32) {
    let s = tokens[start].start_line;
    let e = tokens[end - 1].end_line.max(s);
    (s, e)
}

fn dedupe_pairs(pairs: Vec<RawPair>) -> Vec<RawPair> {
    let mut kept: Vec<RawPair> = Vec::new();
    for p in pairs {
        let overlaps = kept.iter().any(|k| {
            (p.a_file == k.a_file
                && p.b_file == k.b_file
                && p.a_start_line <= k.a_end_line
                && k.a_start_line <= p.a_end_line
                && p.b_start_line <= k.b_end_line
                && k.b_start_line <= p.b_end_line)
                || (p.a_file == k.b_file
                    && p.b_file == k.a_file
                    && p.a_start_line <= k.b_end_line
                    && k.b_start_line <= p.a_end_line
                    && p.b_start_line <= k.a_end_line
                    && k.a_start_line <= p.b_end_line)
        });
        if !overlaps {
            kept.push(p);
        }
    }
    kept
}
