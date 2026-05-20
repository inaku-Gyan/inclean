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

use crate::config::schema::IncludeForm;

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
}

/// Scan `src` for `#include` directives.
pub fn scan(src: &str) -> Vec<Include> {
    Lexer::new(src.as_bytes()).run()
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a [u8]) -> Self {
        Lexer {
            src,
            pos: 0,
            line: 1,
        }
    }

    fn run(mut self) -> Vec<Include> {
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
                self.skip_block_comment();
                at_line_start = false;
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
                if let Some(inc) = self.try_include_directive() {
                    out.push(inc);
                }
                // try_include_directive leaves `pos` on the next interesting
                // byte (typically past the argument or end-of-line). Either
                // way, anything else on this line breaks "line start".
                at_line_start = false;
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

    fn skip_block_comment(&mut self) {
        debug_assert_eq!(self.src[self.pos], b'/');
        debug_assert_eq!(self.src[self.pos + 1], b'*');
        self.pos += 2;
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            if b == b'*' && self.peek(1) == Some(b'/') {
                self.pos += 2;
                return;
            }
            if b == b'\n' {
                self.line += 1;
            }
            self.pos += 1;
        }
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
    /// cases, advances `pos` past whatever it consumed (typically the entire
    /// directive line up to but not including the newline).
    fn try_include_directive(&mut self) -> Option<Include> {
        let directive_start_line = self.line;
        let start = self.pos;
        debug_assert_eq!(self.src[start], b'#');

        // Step past `#`.
        let mut p = start + 1;

        // Skip horizontal whitespace between `#` and keyword.
        while p < self.src.len() && (self.src[p] == b' ' || self.src[p] == b'\t') {
            p += 1;
        }

        // Match "include".
        const KEY: &[u8] = b"include";
        if !self.src[p..].starts_with(KEY) {
            // Not an include directive — skip the rest of the line so we
            // don't re-trigger on the same `#`.
            self.skip_to_end_of_line();
            return None;
        }
        p += KEY.len();

        // The next byte must be whitespace (or end of line). Otherwise it's
        // an identifier like `#includefoo`, not the include directive.
        match self.src.get(p) {
            Some(&b' ' | &b'\t' | &b'\r' | &b'\n') => {}
            _ => {
                self.skip_to_end_of_line();
                return None;
            }
        }

        // Skip whitespace before the argument.
        while p < self.src.len() && (self.src[p] == b' ' || self.src[p] == b'\t') {
            p += 1;
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
                        // Unterminated. Skip line and bail.
                        self.skip_to_end_of_line();
                        return None;
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
                        self.skip_to_end_of_line();
                        return None;
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
                return None;
            }
        };

        // Advance the main cursor past the directive line.
        self.pos = arg_end;
        self.skip_to_end_of_line();

        Some(Include {
            form,
            content,
            line: directive_start_line,
            argument_range: arg_start..arg_end,
        })
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

    fn skip_to_end_of_line(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
            self.pos += 1;
        }
    }
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
