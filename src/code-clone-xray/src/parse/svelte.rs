use crate::normalize::NormalizedToken;
use crate::parse::{css, ts_js};

#[derive(Clone, Debug)]
pub struct SvelteBlock<'a> {
    pub kind: SvelteBlockKind,
    pub is_ts: bool,
    pub body: &'a [u8],
    pub start_line_0: usize,
    pub tag_line_1: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvelteBlockKind {
    Script,
    Style,
}

pub fn extract_svelte_blocks(src: &[u8]) -> Vec<SvelteBlock<'_>> {
    let Ok(text) = std::str::from_utf8(src) else {
        return Vec::new();
    };
    let lower = text.to_ascii_lowercase();
    let mut blocks = Vec::new();
    let mut pos = 0usize;

    while pos < lower.len() {
        let Some(rel_lt) = lower[pos..].find('<') else {
            break;
        };
        let lt = pos + rel_lt;
        if lower[lt..].starts_with("<!--") {
            if let Some(end_c) = lower[lt + 4..].find("-->") {
                pos = lt + 4 + end_c + 3;
                continue;
            }
            break;
        }

        let (kind, tag_name, close_tag) = if starts_with_tag(&lower[lt..], "<script") {
            (SvelteBlockKind::Script, "<script", "</script>")
        } else if starts_with_tag(&lower[lt..], "<style") {
            (SvelteBlockKind::Style, "<style", "</style>")
        } else {
            pos = lt + 1;
            continue;
        };

        let Some(rel_gt) = find_tag_close(&text[lt..]) else {
            break;
        };
        let header_end = lt + rel_gt + 1;
        let header_low = &lower[lt + tag_name.len()..header_end - 1];
        if header_low.trim_end().ends_with('/') {
            pos = header_end;
            continue;
        }

        let is_ts = kind == SvelteBlockKind::Script && is_typescript_attr(header_low);
        let Some(rel_close) = lower[header_end..].find(close_tag) else {
            pos = header_end;
            continue;
        };
        let body_end = header_end + rel_close;
        let start_line_0 = src[..header_end].iter().filter(|&&b| b == b'\n').count();
        let tag_line_1 = (src[..lt].iter().filter(|&&b| b == b'\n').count() + 1) as u32;

        blocks.push(SvelteBlock {
            kind,
            is_ts,
            body: &src[header_end..body_end],
            start_line_0,
            tag_line_1,
        });
        pos = body_end + close_tag.len();
    }

    blocks
}

pub fn tokenize_svelte(content: &[u8]) -> Vec<NormalizedToken> {
    let mut out = Vec::new();
    for block in extract_svelte_blocks(content) {
        let mut tokens = match block.kind {
            SvelteBlockKind::Script => {
                if block.is_ts {
                    ts_js::tokenize_typescript(block.body, false)
                } else {
                    ts_js::tokenize_javascript(block.body)
                }
            }
            SvelteBlockKind::Style => css::tokenize_css(block.body),
        };
        let offset = block.start_line_0 as u32;
        for tok in &mut tokens {
            tok.start_line += offset;
            tok.end_line += offset;
        }
        out.extend(tokens);
    }
    out
}

fn starts_with_tag(slice: &str, prefix: &str) -> bool {
    let Some(after) = slice.strip_prefix(prefix) else {
        return false;
    };
    after
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_whitespace() || c == '>' || c == '/')
}

fn find_tag_close(slice: &str) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (idx, b) in slice.bytes().enumerate() {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => return Some(idx),
            None => {}
        }
    }
    None
}

fn is_typescript_attr(header_low: &str) -> bool {
    let compact: String = header_low
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    compact.contains("lang=\"ts\"")
        || compact.contains("lang='ts'")
        || compact.contains("lang=\"typescript\"")
        || compact.contains("lang='typescript'")
}
