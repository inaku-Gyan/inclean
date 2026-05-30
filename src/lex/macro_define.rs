//! Lexer for simple header-like `#define` directives.
//!
//! This is intentionally not a C preprocessor. It only recovers object-like
//! macros whose replacement list is exactly one header name token:
//!
//! - `#define NAME "foo.h"`
//! - `#define NAME <foo.h>`
//!
//! Function-like macros, token concatenation, multi-token replacements, and
//! line continuations are skipped.

use std::ops::Range;

use crate::config::schema::IncludeForm;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderMacroDefinition {
    pub name: String,
    pub form: IncludeForm,
    pub content: String,
    pub line: usize,
    /// Byte range covering the replacement header name, including delimiters.
    pub value_range: Range<usize>,
}

pub fn scan(src: &str) -> Vec<HeaderMacroDefinition> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut line_start = 0usize;
    let mut line_no = 1usize;
    let mut in_block_comment = false;

    while line_start <= bytes.len() {
        let mut line_end = line_start;
        while line_end < bytes.len() && bytes[line_end] != b'\n' {
            line_end += 1;
        }
        let mut logical_end = line_end;
        if logical_end > line_start && bytes[logical_end - 1] == b'\r' {
            logical_end -= 1;
        }

        if let Some(def) = scan_line(
            bytes,
            line_start,
            logical_end,
            line_no,
            &mut in_block_comment,
        ) {
            out.push(def);
        }

        if line_end == bytes.len() {
            break;
        }
        line_start = line_end + 1;
        line_no += 1;
    }

    out
}

fn scan_line(
    src: &[u8],
    line_start: usize,
    line_end: usize,
    line_no: usize,
    in_block_comment: &mut bool,
) -> Option<HeaderMacroDefinition> {
    let mut p = line_start;

    if *in_block_comment {
        let close = find_block_close(src, p, line_end)?;
        p = close;
        *in_block_comment = false;
    }

    p = skip_line_prefix(src, p, line_end, in_block_comment)?;
    if src.get(p) != Some(&b'#') {
        return None;
    }
    p += 1;
    p = skip_gap(src, p, line_end)?;

    const DEFINE: &[u8] = b"define";
    if !src[p..line_end].starts_with(DEFINE) {
        return None;
    }
    p += DEFINE.len();
    if !is_directive_word_delimiter(src.get(p).copied()) {
        return None;
    }
    p = skip_required_gap(src, p, line_end)?;

    let name_start = p;
    if !is_ident_start(src.get(p).copied()?) {
        return None;
    }
    p += 1;
    while p < line_end && is_ident_continue(src[p]) {
        p += 1;
    }
    let name = std::str::from_utf8(&src[name_start..p]).ok()?.to_string();

    if src.get(p) == Some(&b'(') {
        return None;
    }
    p = skip_required_gap(src, p, line_end)?;

    if has_line_continuation(src, line_start, line_end) {
        return None;
    }

    let value_start = p;
    let (form, content, value_end) = match src.get(p) {
        Some(&b'"') => {
            let close = find_string_close(src, p + 1, line_end)?;
            (
                IncludeForm::Quote,
                std::str::from_utf8(&src[p + 1..close]).ok()?.to_string(),
                close + 1,
            )
        }
        Some(&b'<') => {
            let close = find_byte(src, p + 1, line_end, b'>')?;
            (
                IncludeForm::Angle,
                std::str::from_utf8(&src[p + 1..close]).ok()?.to_string(),
                close + 1,
            )
        }
        _ => return None,
    };

    if !only_gap_or_comment_after(src, value_end, line_end) {
        return None;
    }

    Some(HeaderMacroDefinition {
        name,
        form,
        content,
        line: line_no,
        value_range: value_start..value_end,
    })
}

fn skip_line_prefix(
    src: &[u8],
    mut p: usize,
    line_end: usize,
    in_block_comment: &mut bool,
) -> Option<usize> {
    while p < line_end {
        match src[p] {
            b' ' | b'\t' => p += 1,
            b'/' if src.get(p + 1) == Some(&b'*') => {
                let close = find_block_close(src, p + 2, line_end);
                match close {
                    Some(end) => p = end,
                    None => {
                        *in_block_comment = true;
                        return None;
                    }
                }
            }
            _ => break,
        }
    }
    Some(p)
}

fn skip_gap(src: &[u8], mut p: usize, line_end: usize) -> Option<usize> {
    while p < line_end {
        match src[p] {
            b' ' | b'\t' => p += 1,
            b'/' if src.get(p + 1) == Some(&b'*') => p = find_block_close(src, p + 2, line_end)?,
            _ => break,
        }
    }
    Some(p)
}

fn skip_required_gap(src: &[u8], p: usize, line_end: usize) -> Option<usize> {
    let skipped = skip_gap(src, p, line_end)?;
    if skipped == p { None } else { Some(skipped) }
}

fn only_gap_or_comment_after(src: &[u8], mut p: usize, line_end: usize) -> bool {
    while p < line_end {
        match src[p] {
            b' ' | b'\t' => p += 1,
            b'/' if src.get(p + 1) == Some(&b'/') => return true,
            b'/' if src.get(p + 1) == Some(&b'*') => match find_block_close(src, p + 2, line_end) {
                Some(end) => p = end,
                None => return false,
            },
            _ => return false,
        }
    }
    true
}

fn has_line_continuation(src: &[u8], line_start: usize, line_end: usize) -> bool {
    let mut p = line_end;
    while p > line_start && (src[p - 1] == b' ' || src[p - 1] == b'\t') {
        p -= 1;
    }
    p > line_start && src[p - 1] == b'\\'
}

fn find_block_close(src: &[u8], mut p: usize, line_end: usize) -> Option<usize> {
    while p + 1 < line_end {
        if src[p] == b'*' && src[p + 1] == b'/' {
            return Some(p + 2);
        }
        p += 1;
    }
    None
}

fn find_string_close(src: &[u8], mut p: usize, line_end: usize) -> Option<usize> {
    while p < line_end {
        match src[p] {
            b'\\' => p += 2,
            b'"' => return Some(p),
            _ => p += 1,
        }
    }
    None
}

fn find_byte(src: &[u8], mut p: usize, line_end: usize, needle: u8) -> Option<usize> {
    while p < line_end {
        if src[p] == needle {
            return Some(p);
        }
        p += 1;
    }
    None
}

fn is_directive_word_delimiter(b: Option<u8>) -> bool {
    matches!(b, None | Some(b' ' | b'\t' | b'/' | b'\r'))
}

fn is_ident_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_ident_continue(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_quote_header_macro() {
        let src = "#define CMSIS_device_header \"stm32f4xx.h\"\n";
        let defs = scan(src);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "CMSIS_device_header");
        assert_eq!(defs[0].form, IncludeForm::Quote);
        assert_eq!(defs[0].content, "stm32f4xx.h");
        assert_eq!(&src[defs[0].value_range.clone()], "\"stm32f4xx.h\"");
    }

    #[test]
    fn scans_angle_header_macro_with_comments() {
        let src = "/* banner */ # define FOO <foo.h> /* tail */\n";
        let defs = scan(src);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "FOO");
        assert_eq!(defs[0].form, IncludeForm::Angle);
        assert_eq!(defs[0].content, "foo.h");
    }

    #[test]
    fn skips_function_like_and_complex_replacements() {
        let src = "\
#define FOO(x) \"foo.h\"
#define BAR \"bar.h\" other
#define BAZ \"baz.h\" \\
  continued
";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn keeps_multiple_definitions_for_indexing_policy() {
        let src = "\
#define DEVICE \"stm32f4xx.h\"
#define DEVICE <stm32f407xx.h>
";
        let defs = scan(src);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "DEVICE");
        assert_eq!(defs[0].content, "stm32f4xx.h");
        assert_eq!(defs[1].name, "DEVICE");
        assert_eq!(defs[1].content, "stm32f407xx.h");
    }

    #[test]
    fn ignores_defines_inside_cross_line_block_comments() {
        let src = "/*\n#define HIDDEN \"hidden.h\"\n*/\n#define SHOWN \"shown.h\"\n";
        let defs = scan(src);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "SHOWN");
    }
}
