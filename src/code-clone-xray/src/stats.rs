use rustc_hash::FxHashMap;

use crate::model::{CloneGroup, FormatStats, FormatStatsMap, Occurrence, Statistics, TotalStats};
use crate::normalize::ParsedSourceFile;
use crate::walk::SourceLang;

#[derive(Clone, Debug)]
pub(crate) struct RawPair {
    pub format: SourceLang,
    pub similarity: f64,
    pub lines: usize,
    pub tokens: usize,
    pub fingerprint: u64,
    pub a_file: usize,
    pub a_start_line: u32,
    pub a_end_line: u32,
    pub b_file: usize,
    pub b_start_line: u32,
    pub b_end_line: u32,
}

#[derive(Default)]
struct LangAcc {
    files: usize,
    lines: usize,
    dup: usize,
    pairs: usize,
}

impl LangAcc {
    fn into_stats(self) -> FormatStats {
        FormatStats {
            files: self.files,
            clone_pairs: self.pairs,
            duplicated_lines: self.dup,
            percentage: pct(self.dup, self.lines),
        }
    }
}

pub(crate) fn build_groups(pairs: &[RawPair], files: &[ParsedSourceFile]) -> Vec<CloneGroup> {
    let mut by_fp: FxHashMap<u64, Vec<Occurrence>> = FxHashMap::default();
    let mut sim_by_fp: FxHashMap<u64, f64> = FxHashMap::default();
    let mut fmt_by_fp: FxHashMap<u64, SourceLang> = FxHashMap::default();

    for p in pairs {
        let entry = by_fp.entry(p.fingerprint).or_default();
        push_unique_occ(
            entry,
            Occurrence {
                path: files[p.a_file].display_path.clone(),
                start_line: p.a_start_line,
                end_line: p.a_end_line,
                project_hint: Some(files[p.a_file].project.clone()),
            },
        );
        push_unique_occ(
            entry,
            Occurrence {
                path: files[p.b_file].display_path.clone(),
                start_line: p.b_start_line,
                end_line: p.b_end_line,
                project_hint: Some(files[p.b_file].project.clone()),
            },
        );
        sim_by_fp
            .entry(p.fingerprint)
            .and_modify(|s| *s = (*s).min(p.similarity))
            .or_insert(p.similarity);
        fmt_by_fp.insert(p.fingerprint, p.format);
    }

    by_fp
        .into_iter()
        .filter(|(_, occs)| occs.len() >= 2)
        .map(|(fp, occurrences)| CloneGroup {
            fingerprint: format!("{fp:012x}"),
            format: fmt_by_fp
                .get(&fp)
                .copied()
                .unwrap_or(SourceLang::Cs)
                .format_name()
                .to_string(),
            similarity: *sim_by_fp.get(&fp).unwrap_or(&100.0),
            instance_count: occurrences.len(),
            occurrences,
        })
        .collect()
}

fn push_unique_occ(list: &mut Vec<Occurrence>, occ: Occurrence) {
    let exists = list.iter().any(|o| {
        o.path == occ.path && o.start_line == occ.start_line && o.end_line == occ.end_line
    });
    if !exists {
        list.push(occ);
    }
}

pub(crate) fn compute_stats(
    files: &[ParsedSourceFile],
    pairs: &[RawPair],
    groups: &[CloneGroup],
) -> Statistics {
    let total_lines: usize = files.iter().map(|f| f.total_lines).sum();
    let total_files = files.len();

    // Per-file line bitsets prevent double-counting overlapping or N-way clone pairs
    let mut masks: Vec<Vec<bool>> = files
        .iter()
        .map(|f| vec![false; f.total_lines + 1])
        .collect();

    let mut accs: [LangAcc; 9] = Default::default();

    for p in pairs {
        accs[lang_slot(p.format)].pairs += 1;
        mark_lines(&mut masks[p.a_file], p.a_start_line, p.a_end_line);
        mark_lines(&mut masks[p.b_file], p.b_start_line, p.b_end_line);
    }

    for (idx, f) in files.iter().enumerate() {
        let dup_in_file = masks[idx].iter().filter(|&&b| b).count();
        let slot = &mut accs[lang_slot(f.lang)];
        slot.files += 1;
        slot.lines += f.total_lines;
        slot.dup += dup_in_file;
    }

    let duplicated_lines: usize = accs.iter().map(|a| a.dup).sum();
    let [cs, xaml, rs, ts, js, py, html, css, svelte] = accs;

    Statistics {
        total: TotalStats {
            files: total_files,
            lines: total_lines,
            clone_groups: groups.len(),
            clone_pairs: pairs.len(),
            duplicated_lines,
            percentage: pct(duplicated_lines, total_lines),
        },
        formats: FormatStatsMap {
            csharp: cs.into_stats(),
            xaml: xaml.into_stats(),
            rust: rs.into_stats(),
            typescript: ts.into_stats(),
            javascript: js.into_stats(),
            python: py.into_stats(),
            svelte: svelte.into_stats(),
            html: html.into_stats(),
            css: css.into_stats(),
        },
    }
}

#[inline]
fn lang_slot(lang: SourceLang) -> usize {
    match lang {
        SourceLang::Cs => 0,
        SourceLang::Xaml => 1,
        SourceLang::Rs => 2,
        SourceLang::Ts => 3,
        SourceLang::Js => 4,
        SourceLang::Py => 5,
        SourceLang::Html => 6,
        SourceLang::Css => 7,
        SourceLang::Svelte => 8,
    }
}

#[inline]
fn mark_lines(mask: &mut [bool], start_line: u32, end_line: u32) {
    if mask.len() <= 1 {
        return;
    }
    let s = (start_line as usize).max(1);
    let e = (end_line as usize).min(mask.len() - 1);
    if s <= e {
        for flag in &mut mask[s..=e] {
            *flag = true;
        }
    }
}

#[inline]
fn pct(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        ((num as f64 * 100.0 / den as f64) * 100.0).round() / 100.0
    }
}
