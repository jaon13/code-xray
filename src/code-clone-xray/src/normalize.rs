use std::fs;

use rayon::prelude::*;

use crate::parse::{csharp, css, html, python, rust, svelte, ts_js, xaml};
use crate::walk::{ScannedFile, SourceLang};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NormalizedToken {
    pub symbol: u32,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug)]
pub struct ParsedSourceFile {
    pub id: u32,
    pub display_path: String,
    pub project: String,
    pub lang: SourceLang,
    pub total_lines: usize,
    pub tokens: Vec<NormalizedToken>,
}

/// Abstract token symbol space (above tree-sitter kind IDs)
pub const SYM_IDENT: u32 = 0x10_001;
pub const SYM_STR_LIT: u32 = 0x10_002;
pub const SYM_NUM_LIT: u32 = 0x10_003;
pub const SYM_BOOL_NULL_LIT: u32 = 0x10_004;
pub const SYM_XAML_ATTR_VAL: u32 = 0x20_001;
pub const SYM_XAML_BINDING: u32 = 0x20_002;
pub const SYM_XAML_STATIC_RES: u32 = 0x20_003;
pub const SYM_XAML_DYN_RES: u32 = 0x20_004;
pub const SYM_XAML_TMPL_BIND: u32 = 0x20_005;
pub const SYM_XAML_TEXT: u32 = 0x20_006;

pub fn fnv1a_u32(tag: u32, bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5 ^ tag;
    for &b in bytes {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash | 0x8000_0000
}

pub fn parse_files_parallel(files: &[ScannedFile]) -> Vec<ParsedSourceFile> {
    files
        .par_iter()
        .filter_map(|file| {
            let bytes = fs::read(&file.path).ok()?;
            let content = strip_utf8_bom(&bytes);
            let total_lines = count_lines(content);
            let tokens = match file.lang {
                SourceLang::Cs => csharp::tokenize_csharp(content),
                SourceLang::Xaml => xaml::tokenize_xaml(content),
                SourceLang::Rs => rust::tokenize_rust(content, file.include_tests),
                SourceLang::Ts => {
                    let is_tsx = file
                        .path
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("tsx"));
                    ts_js::tokenize_typescript(content, is_tsx)
                }
                SourceLang::Js => ts_js::tokenize_javascript(content),
                SourceLang::Py => python::tokenize_python(content),
                SourceLang::Svelte => svelte::tokenize_svelte(content),
                SourceLang::Html => html::tokenize_html(content),
                SourceLang::Css => css::tokenize_css(content),
            };
            Some(ParsedSourceFile {
                id: file.id,
                display_path: file.display_path.clone(),
                project: file.project.clone(),
                lang: file.lang,
                total_lines,
                tokens,
            })
        })
        .collect()
}

fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

fn count_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    if bytes.last() == Some(&b'\n') {
        newlines
    } else {
        newlines + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a_u32(0, b"T:Foo"), fnv1a_u32(0, b"T:Foo"));
        assert_ne!(fnv1a_u32(0, b"T:Foo"), fnv1a_u32(0, b"T:Bar"));
    }
}
