//! Lexer that finds `#include` directives in a C/C++ translation unit,
//! ignoring matches inside comments, string literals, and character literals.
//!
//! v1 scope and intentional omissions:
//!
//! - Line continuations *within* an `#include` directive (backslash + newline
//!   between tokens) are not handled. Real-world `#include`s are virtually
//!   always single-line; if we hit this in practice we can extend.
//! - C++11 raw string literals (`R"(...)"`) are not recognized; the lexer
//!   treats them as a regular quote string, which is the common case for
//!   most C/C++ source. We can extend if it bites.
//! - Trigraphs and digraphs are out of scope.
//! - `#if 0` / `#ifdef` branches are not evaluated; includes inside them are
//!   reported. Per the design, that is intentional.

use std::ops::Range;

use crate::config::schema::{CommentStyle, IncludeForm};

/// One `#include` directive recovered from a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Include {
    /// Quote / angle / macro.
    pub form: IncludeForm,
    /// The textual content between the delimiters (no quotes/angles). For
    /// `Macro` form this is the verbatim argument token(s).
    pub content: String,
    /// 1-based line number of the `#` character.
    pub line: usize,
    /// Byte range covering the *argument* — the closed-delimiter text for
    /// quote / angle (delimiters included), or the macro identifier(s) for
    /// macro form. This is what a rewrite replaces.
    pub argument_range: Range<usize>,
    /// Byte range covering the same-line trailing bytes after the include
    /// argument. When a recognized same-line trailing comment is present,
    /// this spans from the argument end through the physical line end so
    /// action-layer transforms can edit the first comment while preserving
    /// any later same-line suffix bytes.
    /// Empty (start == end) when there is no trailing comment, or when
    /// the first comment opens with `/*` but doesn't close on the same
    /// line (per refactor.md, cross-line block comments are NOT trailing
    /// comments and are skipped by trailing-comment processing).
    /// Carriage returns at end of line, if any, are excluded.
    pub trailing_range: Range<usize>,
    /// Delimiter style of the trailing comment, when one is present and
    /// closes on the same line. `None` for no trailing comment or for
    /// any text after the argument that isn't a recognized comment.
    pub trailing_comment_style: Option<CommentStyle>,
    /// `true` when the same-line trailing bytes after the include argument
    /// contain a block comment (`/*`) that does NOT close on the same
    /// physical line.
    /// Per refactor.md §"Trailing comment 的定义": such cross-line block
    /// comments are not trailing comments; the trailing_comment.transform
    /// AND the trailing_comment.append_if_absent paths both no-op for
    /// this include.
    pub has_cross_line_block_trailing: bool,
}

/// Per-line lex notes that downstream callers can surface as warnings.
/// Each entry is `(1-based line, reason)`.
#[derive(Debug, Default, Clone)]
pub struct ScanReport {
    pub skipped_lines: Vec<(usize, String)>,
}

/// Scan `src` for `#include` directives. Quiet variant: any per-line
/// parse anomalies (unterminated quote / angle, `#includefoo` token,
/// etc.) are dropped silently. Prefer [`scan_with_report`] when callers
/// want those surfaced.
pub fn scan(src: &str) -> Vec<Include> {
    scan_with_report(src).0
}

/// Scan `src` and also return a [`ScanReport`] describing per-line
/// anomalies the lex chose to skip. Callers (e.g. the pipeline) can
/// surface these as warnings without aborting the run.
pub fn scan_with_report(src: &str) -> (Vec<Include>, ScanReport) {
    let mut lexer = Lexer::new(src.as_bytes());
    let includes = lexer.run();
    (includes, lexer.report)
}

/// Compute a byte-range-per-physical-line table for `src`. Each entry is
/// `[line_start, line_end_excl_newline)` — the newline itself is not part
/// of the range. Used by the engine to map `#include` lines to off-limits
/// suppression regions.
pub fn line_table(src: &str) -> Vec<Range<usize>> {
    let bytes = src.as_bytes();
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            let mut end = i;
            if end > start && bytes[end - 1] == b'\r' {
                end -= 1;
            }
            out.push(start..end);
            start = i + 1;
        }
        i += 1;
    }
    // Trailing line without terminator.
    if start <= bytes.len() {
        let mut end = bytes.len();
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        out.push(start..end);
    }
    out
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
    report: ScanReport,
}

enum DirectiveGap {
    SameLine(usize),
    CrossLineBlock { start: usize },
}

impl<'a> Lexer<'a> {
    fn new(src: &'a [u8]) -> Self {
        Lexer {
            src,
            pos: 0,
            line: 1,
            report: ScanReport::default(),
        }
    }

    fn run(&mut self) -> Vec<Include> {
        let mut out = Vec::new();

        // We track `at_line_start` so that `#` is only treated as the start
        // of a preprocessor directive when it occurs as the first
        // non-whitespace character on a line.
        let mut at_line_start = true;

        while self.pos < self.src.len() {
            let b = self.src[self.pos];

            // ---- Skip comments ----
            if b == b'/' && self.peek(1) == Some(b'/') {
                self.skip_line_comment();
                at_line_start = false; // any text on this line breaks "line start"
                continue;
            }
            if b == b'/' && self.peek(1) == Some(b'*') {
                let crossed_line = self.skip_block_comment();
                if crossed_line {
                    at_line_start = true;
                }
                continue;
            }

            // ---- Skip string / char literals ----
            if b == b'"' {
                self.skip_quoted(b'"');
                at_line_start = false;
                continue;
            }
            if b == b'\'' {
                self.skip_quoted(b'\'');
                at_line_start = false;
                continue;
            }

            // ---- Newline ----
            if b == b'\n' {
                self.pos += 1;
                self.line += 1;
                at_line_start = true;
                continue;
            }

            // ---- Whitespace within a line ----
            if b == b' ' || b == b'\t' || b == b'\r' {
                self.pos += 1;
                continue;
            }

            // ---- Possible directive ----
            if at_line_start && b == b'#' {
                let (inc, next_at_line_start) = self.try_include_directive();
                if let Some(inc) = inc {
                    out.push(inc);
                }
                at_line_start = next_at_line_start;
                continue;
            }

            // Otherwise advance one byte.
            self.pos += 1;
            at_line_start = false;
        }

        out
    }

    fn peek(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn skip_line_comment(&mut self) {
        debug_assert_eq!(self.src[self.pos], b'/');
        debug_assert_eq!(self.src[self.pos + 1], b'/');
        self.pos += 2;
        while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
            self.pos += 1;
        }
    }

    fn skip_block_comment(&mut self) -> bool {
        debug_assert_eq!(self.src[self.pos], b'/');
        debug_assert_eq!(self.src[self.pos + 1], b'*');
        self.pos += 2;
        let mut crossed_line = false;
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            if b == b'*' && self.peek(1) == Some(b'/') {
                self.pos += 2;
                return crossed_line;
            }
            if b == b'\n' {
                self.line += 1;
                crossed_line = true;
            }
            self.pos += 1;
        }
        crossed_line
    }

    fn skip_quoted(&mut self, delim: u8) {
        debug_assert_eq!(self.src[self.pos], delim);
        self.pos += 1;
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            if b == b'\\' && self.peek(1).is_some() {
                self.pos += 2;
                continue;
            }
            if b == delim {
                self.pos += 1;
                return;
            }
            if b == b'\n' {
                self.line += 1;
            }
            self.pos += 1;
        }
    }

    /// Called when `at_line_start && src[pos] == '#'`. Tries to parse an
    /// `#include` directive. Returns `Some(...)` if one was found. In all
    /// cases, advances `pos` past whatever it consumed and returns the next
    /// `at_line_start` state for the main scanner.
    fn try_include_directive(&mut self) -> (Option<Include>, bool) {
        let directive_start_line = self.line;
        let start = self.pos;
        debug_assert_eq!(self.src[start], b'#');

        // Step past `#`.
        let mut p = start + 1;

        // Skip whitespace/comments between `#` and keyword. Comments are
        // whitespace in C/C++ preprocessing, but a block comment that crosses
        // a newline ends this directive line instead of continuing it.
        p = match self.skip_directive_gap_on_line(p) {
            DirectiveGap::SameLine(p) => p,
            DirectiveGap::CrossLineBlock { start } => {
                self.pos = start;
                let crossed_line = self.skip_block_comment();
                return (None, crossed_line);
            }
        };

        // Match "include".
        const KEY: &[u8] = b"include";
        if !self.src[p..].starts_with(KEY) {
            // Not an include directive. Not flagged: `#define`, `#pragma`,
            // etc. are normal directives, not lex errors.
            self.pos = p;
            let next_at_line_start = self.skip_rest_of_directive_line();
            return (None, next_at_line_start);
        }
        p += KEY.len();

        // The next byte must be a preprocessing-token separator. Otherwise
        // it's an identifier like `#includefoo`, not the include directive.
        if !self.is_include_keyword_delimiter(p) {
            self.report.skipped_lines.push((
                directive_start_line,
                "looks like `#include<identifier>` (missing whitespace after `include`) — skipped"
                    .to_string(),
            ));
            self.pos = p;
            let next_at_line_start = self.skip_rest_of_directive_line();
            return (None, next_at_line_start);
        }

        // Skip whitespace/comments before the argument.
        p = match self.skip_directive_gap_on_line(p) {
            DirectiveGap::SameLine(p) => p,
            DirectiveGap::CrossLineBlock { start } => {
                self.report.skipped_lines.push((
                    directive_start_line,
                    "missing argument in `#include` directive".to_string(),
                ));
                self.pos = start;
                let crossed_line = self.skip_block_comment();
                return (None, crossed_line);
            }
        };

        if self.is_end_of_line(p) || self.starts_line_comment(p) {
            self.report.skipped_lines.push((
                directive_start_line,
                "missing argument in `#include` directive".to_string(),
            ));
            self.pos = p;
            let next_at_line_start = self.skip_rest_of_directive_line();
            return (None, next_at_line_start);
        }

        // Parse the argument.
        let arg_start = p;
        let (form, content, arg_end) = match self.src.get(p) {
            Some(&b'"') => {
                let close = self.find_byte_on_line(p + 1, b'"');
                match close {
                    Some(end) => (
                        IncludeForm::Quote,
                        std::str::from_utf8(&self.src[p + 1..end])
                            .unwrap_or("")
                            .to_string(),
                        end + 1,
                    ),
                    None => {
                        self.report.skipped_lines.push((
                            directive_start_line,
                            "unterminated quote `\"` in `#include` argument".to_string(),
                        ));
                        self.skip_to_end_of_line();
                        return (None, false);
                    }
                }
            }
            Some(&b'<') => {
                let close = self.find_byte_on_line(p + 1, b'>');
                match close {
                    Some(end) => (
                        IncludeForm::Angle,
                        std::str::from_utf8(&self.src[p + 1..end])
                            .unwrap_or("")
                            .to_string(),
                        end + 1,
                    ),
                    None => {
                        self.report.skipped_lines.push((
                            directive_start_line,
                            "unterminated angle `<` in `#include` argument".to_string(),
                        ));
                        self.skip_to_end_of_line();
                        return (None, false);
                    }
                }
            }
            Some(_) => {
                // Macro form: read identifier/macro-invocation up to
                // whitespace or end-of-line, ignoring any trailing comment.
                let end = self.find_macro_end(p);
                (
                    IncludeForm::Macro,
                    std::str::from_utf8(&self.src[p..end])
                        .unwrap_or("")
                        .trim_end()
                        .to_string(),
                    end,
                )
            }
            None => {
                // EOF directly after `#include`.
                return (None, false);
            }
        };

        // Keep the main cursor at the end of the argument. The main scanner
        // will consume the rest of the line, including any block comment that
        // crosses lines, so includes inside that comment remain ignored.
        self.pos = arg_end;

        // Find this physical line's end and trim a trailing
        // `\r` for the EOL marker so we can reason about printable bytes.
        let mut eol_end = self.find_line_end(arg_end);
        if eol_end > arg_end && self.src.get(eol_end - 1) == Some(&b'\r') {
            eol_end -= 1;
        }

        let (trailing_range, trailing_comment_style, has_cross_line_block_trailing) =
            classify_trailing(self.src, arg_end, eol_end);

        (
            Some(Include {
                form,
                content,
                line: directive_start_line,
                argument_range: arg_start..arg_end,
                trailing_range,
                trailing_comment_style,
                has_cross_line_block_trailing,
            }),
            false,
        )
    }

    fn skip_directive_gap_on_line(&self, mut p: usize) -> DirectiveGap {
        while p < self.src.len() {
            match self.src[p] {
                b' ' | b'\t' | b'\r' => p += 1,
                b'/' if self.src.get(p + 1) == Some(&b'*') => {
                    match self.find_block_comment_end_on_line(p) {
                        Some(end) => p = end,
                        None => return DirectiveGap::CrossLineBlock { start: p },
                    }
                }
                _ => return DirectiveGap::SameLine(p),
            }
        }
        DirectiveGap::SameLine(p)
    }

    fn find_block_comment_end_on_line(&self, start: usize) -> Option<usize> {
        debug_assert_eq!(self.src[start], b'/');
        debug_assert_eq!(self.src[start + 1], b'*');
        let mut i = start + 2;
        while i < self.src.len() {
            if self.src[i] == b'\n' {
                return None;
            }
            if self.src[i] == b'*' && self.src.get(i + 1) == Some(&b'/') {
                return Some(i + 2);
            }
            i += 1;
        }
        None
    }

    fn is_include_keyword_delimiter(&self, p: usize) -> bool {
        match self.src.get(p) {
            None | Some(&b' ' | &b'\t' | &b'\r' | &b'\n') => true,
            Some(&b'/')
                if self.src.get(p + 1) == Some(&b'*') || self.src.get(p + 1) == Some(&b'/') =>
            {
                true
            }
            _ => false,
        }
    }

    fn is_end_of_line(&self, p: usize) -> bool {
        p >= self.src.len() || self.src[p] == b'\n' || self.src[p] == b'\r'
    }

    fn starts_line_comment(&self, p: usize) -> bool {
        self.src.get(p) == Some(&b'/') && self.src.get(p + 1) == Some(&b'/')
    }

    fn find_byte_on_line(&self, from: usize, byte: u8) -> Option<usize> {
        let mut i = from;
        while i < self.src.len() && self.src[i] != b'\n' {
            if self.src[i] == byte {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    fn find_macro_end(&self, from: usize) -> usize {
        // The macro argument ends at the first run of whitespace + comment
        // or at end-of-line, whichever comes first.
        let mut i = from;
        while i < self.src.len() {
            let b = self.src[i];
            if b == b'\n' {
                break;
            }
            // A trailing comment terminates the argument.
            if b == b'/'
                && (self.src.get(i + 1) == Some(&b'/') || self.src.get(i + 1) == Some(&b'*'))
            {
                break;
            }
            i += 1;
        }
        i
    }

    fn find_line_end(&self, from: usize) -> usize {
        let mut i = from;
        while i < self.src.len() && self.src[i] != b'\n' {
            i += 1;
        }
        i
    }

    fn skip_to_end_of_line(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
            self.pos += 1;
        }
    }

    fn skip_rest_of_directive_line(&mut self) -> bool {
        while self.pos < self.src.len() {
            if self.src[self.pos] == b'\n' {
                return false;
            }
            if self.src[self.pos] == b'/' && self.peek(1) == Some(b'/') {
                self.skip_line_comment();
                return false;
            }
            if self.src[self.pos] == b'/' && self.peek(1) == Some(b'*') {
                let crossed_line = self.skip_block_comment();
                if crossed_line {
                    return true;
                }
                continue;
            }
            self.pos += 1;
        }
        false
    }
}

/// Classify the bytes between `arg_end` and `eol_end` (exclusive of EOL).
/// Returns the trailing range and the detected comment style.
///
/// - All whitespace / empty → empty range, `None`.
/// - Starts (after whitespace) with `//` → `Line`, range = `[arg_end, eol_end)`.
/// - Starts with `/*` and the first block comment closes on the same line
///   → `Block`, range = `[arg_end, eol_end)`. Any later same-line bytes
///   are preserved by action-layer trailing-comment processing. If those
///   later bytes contain an unterminated block comment, the cross-line flag
///   is also set so transform/append logic no-ops.
/// - Starts with `/*` but the first block comment has no `*/` on the same
///   line → empty range, `None`.
///   The block comment continues to be skipped by the main lexer loop on
///   the next iteration; we deliberately drop it from `trailing_range` so
///   M4's trailing-comment processing leaves it alone.
/// - Anything else (e.g. `;` or stray tokens) → keep the range so it's
///   visible to downstream code, but style is `None`.
fn classify_trailing(
    src: &[u8],
    arg_end: usize,
    eol_end: usize,
) -> (Range<usize>, Option<CommentStyle>, bool) {
    if eol_end <= arg_end {
        return (arg_end..arg_end, None, false);
    }
    let slice = &src[arg_end..eol_end];
    // Find the first non-whitespace byte within the trailing slice.
    let mut i = 0usize;
    while i < slice.len() && (slice[i] == b' ' || slice[i] == b'\t') {
        i += 1;
    }
    if i == slice.len() {
        return (arg_end..arg_end, None, false);
    }
    if slice[i] == b'/' && slice.get(i + 1) == Some(&b'/') {
        return (arg_end..eol_end, Some(CommentStyle::Line), false);
    }
    if slice[i] == b'/' && slice.get(i + 1) == Some(&b'*') {
        // Look for `*/` strictly within the remaining bytes of this line.
        let mut j = i + 2;
        while j + 1 < slice.len() {
            if slice[j] == b'*' && slice[j + 1] == b'/' {
                let first_block_end = j + 2;
                let has_cross_line_block =
                    suffix_has_unclosed_block_comment(&slice[first_block_end..]);
                return (
                    arg_end..eol_end,
                    Some(CommentStyle::Block),
                    has_cross_line_block,
                );
            }
            j += 1;
        }
        // Cross-line block comment — drop the trailing range entirely and
        // flag it so trailing-comment processing can short-circuit.
        return (arg_end..arg_end, None, true);
    }
    (arg_end..eol_end, None, false)
}

fn suffix_has_unclosed_block_comment(slice: &[u8]) -> bool {
    let mut i = 0usize;
    'outer: while i < slice.len() {
        if slice[i] == b'/' && slice.get(i + 1) == Some(&b'/') {
            return false;
        }
        if slice[i] == b'/' && slice.get(i + 1) == Some(&b'*') {
            let mut j = i + 2;
            while j + 1 < slice.len() {
                if slice[j] == b'*' && slice[j + 1] == b'/' {
                    i = j + 2;
                    continue 'outer;
                }
                j += 1;
            }
            return true;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::IncludeForm;

    #[test]
    fn empty_source_yields_no_includes() {
        assert!(scan("").is_empty());
    }

    #[test]
    fn single_quote_include() {
        let incs = scan(r#"#include "foo.h""#);
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].form, IncludeForm::Quote);
        assert_eq!(incs[0].content, "foo.h");
        assert_eq!(incs[0].line, 1);
    }

    #[test]
    fn single_angle_include() {
        let incs = scan("#include <stdio.h>");
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].form, IncludeForm::Angle);
        assert_eq!(incs[0].content, "stdio.h");
    }

    #[test]
    fn argument_range_covers_delimiters() {
        let src = "#include \"foo.h\"";
        let incs = scan(src);
        let r = &incs[0].argument_range;
        assert_eq!(&src[r.clone()], "\"foo.h\"");
    }

    #[test]
    fn ignores_includes_in_line_comments() {
        let incs = scan("// #include \"x.h\"\n");
        assert!(incs.is_empty());
    }

    #[test]
    fn ignores_includes_in_block_comments_across_lines() {
        let src = "/* foo\n#include \"bar.h\"\nbaz */\n";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn ignores_includes_in_string_literals() {
        let src = "const char* s = \"#include \\\"bar.h\\\"\";\n";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn ignores_includes_when_not_at_line_start() {
        // The `#` is not the first non-whitespace token on its line.
        let src = "x = 1; #include \"foo.h\"\n";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn allows_whitespace_between_hash_and_include_keyword() {
        let incs = scan("#  \tinclude \"foo.h\"\n");
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].content, "foo.h");
    }

    #[test]
    fn allows_leading_whitespace_before_hash() {
        let incs = scan("   #include <foo.h>\n");
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].content, "foo.h");
    }

    #[test]
    fn allows_block_comment_before_hash_at_line_start() {
        let incs = scan("/* banner */ #include \"foo.h\"\n");
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].content, "foo.h");
        assert_eq!(incs[0].line, 1);
    }

    #[test]
    fn closed_block_comment_after_code_does_not_restore_line_start() {
        let src = "int x; /* banner */ #include \"foo.h\"\n";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn allows_block_comments_between_directive_tokens() {
        let incs = scan("#/**/include/* gap */ \"foo.h\"\n#include/* gap */\"bar.h\"\n");
        assert_eq!(incs.len(), 2);
        assert_eq!(incs[0].content, "foo.h");
        assert_eq!(incs[1].content, "bar.h");
    }

    #[test]
    fn cross_line_block_between_include_and_argument_is_not_joined() {
        let src = "#include /* gap\n*/ \"foo.h\"\n#include \"bar.h\"\n";
        let (incs, report) = scan_with_report(src);
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].content, "bar.h");
        assert_eq!(incs[0].line, 3);
        assert_eq!(report.skipped_lines.len(), 1);
        assert!(report.skipped_lines[0].1.contains("missing argument"));
    }

    #[test]
    fn ignores_other_preprocessor_directives() {
        let src = "#define FOO 1\n#pragma once\n#include \"foo.h\"\n";
        let incs = scan(src);
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].content, "foo.h");
        assert_eq!(incs[0].line, 3);
    }

    #[test]
    fn ignores_hash_includefoo_token() {
        // `#includefoo` is not the `#include` directive.
        assert!(scan("#includefoo\n").is_empty());
    }

    #[test]
    fn handles_multiple_includes_and_tracks_lines() {
        let src = "#include \"a.h\"\n#include <b.h>\n\n#include \"c.h\"\n";
        let incs = scan(src);
        assert_eq!(incs.len(), 3);
        assert_eq!((incs[0].line, &incs[0].content[..]), (1, "a.h"));
        assert_eq!((incs[1].line, &incs[1].content[..]), (2, "b.h"));
        assert_eq!((incs[2].line, &incs[2].content[..]), (4, "c.h"));
    }

    #[test]
    fn macro_form_is_recognized() {
        let incs = scan("#include MY_HEADER\n");
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].form, IncludeForm::Macro);
        assert_eq!(incs[0].content, "MY_HEADER");
    }

    #[test]
    fn trailing_comment_after_include_is_not_part_of_argument() {
        let src = "#include \"foo.h\" // pulled in for FOO\n";
        let incs = scan(src);
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].content, "foo.h");
        let r = &incs[0].argument_range;
        assert_eq!(&src[r.clone()], "\"foo.h\"");
    }

    #[test]
    fn trailing_range_covers_to_eol() {
        let src = "#include \"foo.h\"  // pulled in for FOO\n";
        let incs = scan(src);
        let t = &incs[0].trailing_range;
        assert_eq!(&src[t.clone()], "  // pulled in for FOO");
    }

    #[test]
    fn trailing_range_empty_when_no_trailing() {
        let src = "#include \"foo.h\"\n";
        let incs = scan(src);
        let t = &incs[0].trailing_range;
        assert_eq!(t.start, t.end);
        assert_eq!(&src[t.clone()], "");
    }

    #[test]
    fn trailing_range_excludes_carriage_return() {
        let src = "#include \"foo.h\"\r\n";
        let incs = scan(src);
        let t = &incs[0].trailing_range;
        assert_eq!(&src[t.clone()], "");
    }

    #[test]
    fn trailing_range_excludes_carriage_return_after_comment() {
        let src = "#include \"foo.h\"  // note\r\n";
        let incs = scan(src);
        let t = &incs[0].trailing_range;
        assert_eq!(&src[t.clone()], "  // note");
    }

    #[test]
    fn trailing_range_handles_block_comment() {
        let src = "#include \"foo.h\" /* note */\n";
        let incs = scan(src);
        let t = &incs[0].trailing_range;
        assert_eq!(&src[t.clone()], " /* note */");
        assert_eq!(incs[0].trailing_comment_style, Some(CommentStyle::Block));
    }

    #[test]
    fn trailing_range_preserves_suffix_after_first_block_comment() {
        let src = "#include \"foo.h\" /*1st*/ /*2nd*/\n";
        let incs = scan(src);
        let t = &incs[0].trailing_range;
        assert_eq!(&src[t.clone()], " /*1st*/ /*2nd*/");
        assert_eq!(incs[0].trailing_comment_style, Some(CommentStyle::Block));
        assert!(!incs[0].has_cross_line_block_trailing);
    }

    #[test]
    fn first_block_comment_with_later_open_block_sets_cross_line_flag() {
        let src = "#include \"foo.h\" /*1st*/ /* open\n*/\n";
        let incs = scan(src);
        let t = &incs[0].trailing_range;
        assert_eq!(&src[t.clone()], " /*1st*/ /* open");
        assert_eq!(incs[0].trailing_comment_style, Some(CommentStyle::Block));
        assert!(incs[0].has_cross_line_block_trailing);
    }

    #[test]
    fn trailing_line_comment_classified_as_line_style() {
        let src = "#include \"foo.h\" // note\n";
        let incs = scan(src);
        assert_eq!(incs[0].trailing_comment_style, Some(CommentStyle::Line));
    }

    #[test]
    fn trailing_no_comment_has_none_style() {
        let src = "#include \"foo.h\"\n";
        let incs = scan(src);
        assert_eq!(incs[0].trailing_comment_style, None);
    }

    #[test]
    fn cross_line_block_comment_is_not_a_trailing_comment() {
        let src = "#include \"foo.h\" /* this\ncontinues */\n";
        let incs = scan(src);
        assert_eq!(incs.len(), 1);
        // Cross-line block: trailing_range collapses to empty, style is None.
        let t = &incs[0].trailing_range;
        assert_eq!(t.start, t.end);
        assert_eq!(incs[0].trailing_comment_style, None);
    }

    #[test]
    fn trailing_cross_line_block_hides_includes_until_close() {
        let src = "#include \"foo.h\" /* opens\n#include \"hidden.h\"\n*/\n#include \"bar.h\"\n";
        let incs = scan(src);
        assert_eq!(incs.len(), 2);
        assert_eq!((incs[0].line, incs[0].content.as_str()), (1, "foo.h"));
        assert_eq!((incs[1].line, incs[1].content.as_str()), (4, "bar.h"));
        assert!(incs[0].has_cross_line_block_trailing);
    }

    #[test]
    fn trailing_unterminated_block_hides_includes_to_eof() {
        let src = "#include \"foo.h\" /* opens\n#include \"hidden.h\"\n";
        let incs = scan(src);
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].content, "foo.h");
        assert!(incs[0].has_cross_line_block_trailing);
    }

    #[test]
    fn unterminated_block_before_include_hides_to_eof() {
        let src = "/* opens\n#include \"hidden.h\"\n";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn other_directive_cross_line_block_hides_include_inside_and_recovers() {
        let src = "#define X /* opens\n#include \"hidden.h\"\n*/\n#include \"bar.h\"\n";
        let incs = scan(src);
        assert_eq!(incs.len(), 1);
        assert_eq!((incs[0].line, incs[0].content.as_str()), (4, "bar.h"));
    }

    #[test]
    fn whitespace_only_trailing_has_empty_range_and_no_style() {
        let src = "#include \"foo.h\"   \n";
        let incs = scan(src);
        let t = &incs[0].trailing_range;
        assert_eq!(t.start, t.end);
        assert_eq!(incs[0].trailing_comment_style, None);
    }

    #[test]
    fn line_table_basic_lf() {
        let src = "a\nbb\nccc\n";
        let lines = line_table(src);
        assert_eq!(lines, vec![0..1, 2..4, 5..8, 9..9]);
    }

    #[test]
    fn line_table_strips_crlf() {
        let src = "a\r\nbb\r\nccc";
        let lines = line_table(src);
        assert_eq!(lines, vec![0..1, 3..5, 7..10]);
    }

    #[test]
    fn line_table_includes_trailing_line_without_newline() {
        let src = "hello";
        let lines = line_table(src);
        assert_eq!(lines, vec![0..5]);
    }

    #[test]
    fn line_table_empty_source() {
        assert_eq!(line_table(""), vec![0..0]);
    }

    #[test]
    fn trailing_range_for_last_line_without_newline() {
        let src = "#include \"foo.h\" // tail";
        let incs = scan(src);
        let t = &incs[0].trailing_range;
        assert_eq!(&src[t.clone()], " // tail");
    }

    #[test]
    fn unterminated_quote_is_skipped() {
        let src = "#include \"foo.h\n";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn unterminated_angle_is_skipped() {
        let src = "#include <foo.h\n";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn unterminated_quote_emits_skip_warning() {
        let (incs, report) = scan_with_report("#include \"foo.h\n");
        assert!(incs.is_empty());
        assert_eq!(report.skipped_lines.len(), 1);
        assert_eq!(report.skipped_lines[0].0, 1);
        assert!(report.skipped_lines[0].1.contains("unterminated"));
    }

    #[test]
    fn unterminated_angle_emits_skip_warning() {
        let (incs, report) = scan_with_report("#include <foo.h\n");
        assert!(incs.is_empty());
        assert_eq!(report.skipped_lines.len(), 1);
        assert!(report.skipped_lines[0].1.contains("unterminated"));
    }

    #[test]
    fn includefoo_token_emits_skip_warning() {
        let (incs, report) = scan_with_report("#includefoo\n");
        assert!(incs.is_empty());
        assert_eq!(report.skipped_lines.len(), 1);
        assert!(report.skipped_lines[0].1.contains("missing whitespace"));
    }

    #[test]
    fn cross_line_block_trailing_flag_is_set() {
        let src = "#include \"foo.h\" /* opens\nbut never closes\n";
        let (incs, _) = scan_with_report(src);
        assert_eq!(incs.len(), 1);
        assert!(incs[0].has_cross_line_block_trailing);
        let t = &incs[0].trailing_range;
        assert_eq!(t.start, t.end);
    }

    #[test]
    fn same_line_block_does_not_set_cross_line_flag() {
        let src = "#include \"foo.h\" /* same line */\n";
        let (incs, _) = scan_with_report(src);
        assert_eq!(incs.len(), 1);
        assert!(!incs[0].has_cross_line_block_trailing);
    }

    #[test]
    fn char_literals_dont_swallow_subsequent_includes() {
        let src = "char c = '\"';\n#include \"foo.h\"\n";
        let incs = scan(src);
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].content, "foo.h");
    }

    #[test]
    fn line_count_is_correct_after_block_comment() {
        let src = "/*\n\n\n*/\n#include \"foo.h\"\n";
        let incs = scan(src);
        assert_eq!(incs[0].line, 5);
    }
}
