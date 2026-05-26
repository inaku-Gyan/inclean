//! `@std.*` built-in constants and `@name` substitution.
//!
//! Two expansion modes:
//!
//! - **List spread** (in any string-list field): an item of the form
//!   `"@name"` is replaced by the constant's list contents, spliced into
//!   the surrounding list. Only list-typed constants are allowed.
//! - **String substitution** (in any string field, typically regexes):
//!   substrings of the form `@name` are replaced by the constant's
//!   string value. List-typed constants are usable here too — they are
//!   joined with `|` and wrapped in `(?:...)`, with regex meta-chars
//!   escaped so the result is safe inside a regex.
//!
//! Use `@@` to write a literal `@` in a string field.

use std::sync::LazyLock;

use anyhow::{bail, Result};

/// Marker for whether a constant is naturally list-shaped or scalar.
#[derive(Debug, Clone)]
pub enum Value {
    List(Vec<&'static str>),
    String(String),
}

/// Look up a constant by its dotted name (no leading `@`).
///
/// Recognizes the explicit table below plus the `_or` suffix: if `name` ends
/// in `_or` and the base name resolves to a list, the returned value is a
/// regex alternation `(?:item1|item2|...)` with regex meta-chars escaped.
pub fn lookup(name: &str) -> Option<Value> {
    if let Some(list) = lookup_list(name) {
        return Some(Value::List(list));
    }
    if let Some(base) = name.strip_suffix("_or") {
        if let Some(list) = lookup_list(base) {
            let joined = list
                .iter()
                .map(|item| regex::escape(item))
                .collect::<Vec<_>>()
                .join("|");
            return Some(Value::String(format!("(?:{joined})")));
        }
    }
    None
}

/// Spread `@name` items in a string-list field. Items that do not start
/// with `@` are passed through unchanged.
pub fn expand_list(items: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        if let Some(name) = item.strip_prefix('@') {
            match lookup(name) {
                Some(Value::List(list)) => {
                    out.extend(list.iter().map(|s| (*s).to_string()));
                }
                Some(Value::String(_)) => {
                    bail!(
                        "constant `@{name}` is a string; only list-typed constants can be spread in a list field"
                    );
                }
                None => bail!("unknown constant `@{name}`"),
            }
        } else {
            out.push(item.clone());
        }
    }
    Ok(out)
}

/// Substitute `@name` substrings in arbitrary text (typically a regex).
/// `@@` becomes a literal `@`.
pub fn substitute_in_string(text: &str) -> Result<String> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'@' {
            out.push(b as char);
            i += 1;
            continue;
        }
        // bytes[i] == '@'
        if i + 1 < bytes.len() && bytes[i + 1] == b'@' {
            out.push('@');
            i += 2;
            continue;
        }
        // Read identifier: [A-Za-z_][A-Za-z0-9_.]*
        let start = i + 1;
        let mut end = start;
        if end < bytes.len() && is_ident_start(bytes[end]) {
            end += 1;
            while end < bytes.len() && is_ident_cont(bytes[end]) {
                end += 1;
            }
        }
        if end == start {
            bail!(
                "stray `@` at byte {i} in {:?}; use `@@` to write a literal `@`",
                text
            );
        }
        let name = &text[start..end];
        match lookup(name) {
            Some(Value::String(s)) => out.push_str(&s),
            Some(Value::List(list)) => {
                let joined = list
                    .iter()
                    .map(|item| regex::escape(item))
                    .collect::<Vec<_>>()
                    .join("|");
                out.push_str(&format!("(?:{joined})"));
            }
            None => bail!("unknown constant `@{name}`"),
        }
        i = end;
    }
    Ok(out)
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

// ---------------------------------------------------------------------------
// Static table of list-shaped constants
// ---------------------------------------------------------------------------

fn lookup_list(name: &str) -> Option<Vec<&'static str>> {
    let v = match name {
        // ---- file extensions ----
        "std.c.header_extensions" => C_HEADER_EXTENSIONS.to_vec(),
        "std.c.source_extensions" => C_SOURCE_EXTENSIONS.to_vec(),
        "std.c.extensions" => C_EXTENSIONS.clone(),
        "std.cpp.header_extensions" => CPP_HEADER_EXTENSIONS.to_vec(),
        "std.cpp.source_extensions" => CPP_SOURCE_EXTENSIONS.to_vec(),
        "std.cpp.extensions" => CPP_EXTENSIONS.clone(),

        // ---- C system headers (cumulative per version) ----
        "std.c89.system_headers" => C89_HEADERS.to_vec(),
        "std.c95.system_headers" => C95_HEADERS.clone(),
        "std.c99.system_headers" => C99_HEADERS.clone(),
        "std.c11.system_headers" => C11_HEADERS.clone(),
        "std.c17.system_headers" => C11_HEADERS.clone(), // C17 added no headers
        "std.c23.system_headers" => C23_HEADERS.clone(),

        // ---- C++ system headers (cumulative per version) ----
        "std.cpp.c_compat_headers" => CPP_C_COMPAT.to_vec(),
        "std.cpp98.system_headers" => CPP98_HEADERS.clone(),
        "std.cpp11.system_headers" => CPP11_HEADERS.clone(),
        "std.cpp14.system_headers" => CPP14_HEADERS.clone(),
        "std.cpp17.system_headers" => CPP17_HEADERS.clone(),
        "std.cpp20.system_headers" => CPP20_HEADERS.clone(),
        "std.cpp23.system_headers" => CPP23_HEADERS.clone(),

        _ => return None,
    };
    Some(v)
}

// ---- extensions ----------------------------------------------------------

/// Canonical C header extension. `.h` is also a common C++ header extension
/// in many projects — users targeting such projects can write
/// `extensions = ["@std.c_extensions", "@std.cpp_extensions"]` (the layer-2
/// default) to cover both.
const C_HEADER_EXTENSIONS: &[&str] = &[".h"];
const C_SOURCE_EXTENSIONS: &[&str] = &[".c"];
/// Canonical C++ header extensions (excluding `.h`, which is in `c_*`).
const CPP_HEADER_EXTENSIONS: &[&str] = &[".hh", ".hpp", ".hxx", ".h++"];
const CPP_SOURCE_EXTENSIONS: &[&str] = &[".cc", ".cpp", ".cxx", ".c++", ".inl", ".ipp"];

static C_EXTENSIONS: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| concat(&[C_HEADER_EXTENSIONS, C_SOURCE_EXTENSIONS]));
static CPP_EXTENSIONS: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| concat(&[CPP_HEADER_EXTENSIONS, CPP_SOURCE_EXTENSIONS]));

// ---- C standard headers --------------------------------------------------

const C89_HEADERS: &[&str] = &[
    "assert.h", "ctype.h", "errno.h", "float.h", "limits.h", "locale.h", "math.h", "setjmp.h",
    "signal.h", "stdarg.h", "stddef.h", "stdio.h", "stdlib.h", "string.h", "time.h",
];
const C95_ADDED: &[&str] = &["iso646.h", "wchar.h", "wctype.h"];
const C99_ADDED: &[&str] = &[
    "complex.h",
    "fenv.h",
    "inttypes.h",
    "stdbool.h",
    "stdint.h",
    "tgmath.h",
];
const C11_ADDED: &[&str] = &[
    "stdalign.h",
    "stdatomic.h",
    "stdnoreturn.h",
    "threads.h",
    "uchar.h",
];
const C23_ADDED: &[&str] = &["stdbit.h", "stdckdint.h"];

static C95_HEADERS: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| concat(&[C89_HEADERS, C95_ADDED]));
static C99_HEADERS: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| concat(&[C89_HEADERS, C95_ADDED, C99_ADDED]));
static C11_HEADERS: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| concat(&[C89_HEADERS, C95_ADDED, C99_ADDED, C11_ADDED]));
static C23_HEADERS: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| concat(&[C89_HEADERS, C95_ADDED, C99_ADDED, C11_ADDED, C23_ADDED]));

// ---- C++ standard headers ------------------------------------------------

const CPP_C_COMPAT: &[&str] = &[
    "cassert", "cctype", "cerrno", "cfloat", "ciso646", "climits", "clocale", "cmath", "csetjmp",
    "csignal", "cstdarg", "cstddef", "cstdio", "cstdlib", "cstring", "ctime", "cwchar", "cwctype",
];

const CPP98_NATIVE: &[&str] = &[
    "algorithm",
    "bitset",
    "complex",
    "deque",
    "exception",
    "fstream",
    "functional",
    "iomanip",
    "ios",
    "iosfwd",
    "iostream",
    "istream",
    "iterator",
    "limits",
    "list",
    "locale",
    "map",
    "memory",
    "new",
    "numeric",
    "ostream",
    "queue",
    "set",
    "sstream",
    "stack",
    "stdexcept",
    "streambuf",
    "string",
    "strstream", // deprecated in C++98 but still in <strstream>
    "typeinfo",
    "utility",
    "valarray",
    "vector",
];

const CPP11_NATIVE_ADDED: &[&str] = &[
    "array",
    "atomic",
    "chrono",
    "codecvt",
    "condition_variable",
    "forward_list",
    "future",
    "initializer_list",
    "mutex",
    "random",
    "ratio",
    "regex",
    "scoped_allocator",
    "system_error",
    "thread",
    "tuple",
    "type_traits",
    "typeindex",
    "unordered_map",
    "unordered_set",
];
const CPP11_C_COMPAT_ADDED: &[&str] = &[
    "cfenv",
    "cinttypes",
    "cstdbool",
    "cstdint",
    "ctgmath",
    "cuchar",
];

const CPP14_ADDED: &[&str] = &["shared_mutex"];

const CPP17_ADDED: &[&str] = &[
    "any",
    "charconv",
    "execution",
    "filesystem",
    "memory_resource",
    "optional",
    "string_view",
    "variant",
    "cstdalign",
];

const CPP20_ADDED: &[&str] = &[
    "barrier",
    "bit",
    "compare",
    "concepts",
    "coroutine",
    "format",
    "latch",
    "numbers",
    "ranges",
    "semaphore",
    "source_location",
    "span",
    "stop_token",
    "syncstream",
    "version",
];

const CPP23_ADDED: &[&str] = &[
    "expected",
    "flat_map",
    "flat_set",
    "generator",
    "mdspan",
    "print",
    "spanstream",
    "stacktrace",
    "stdfloat",
];

static CPP98_HEADERS: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| concat(&[CPP98_NATIVE, CPP_C_COMPAT]));

static CPP11_HEADERS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    concat(&[
        CPP98_NATIVE,
        CPP11_NATIVE_ADDED,
        CPP_C_COMPAT,
        CPP11_C_COMPAT_ADDED,
    ])
});

static CPP14_HEADERS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    concat(&[
        CPP98_NATIVE,
        CPP11_NATIVE_ADDED,
        CPP14_ADDED,
        CPP_C_COMPAT,
        CPP11_C_COMPAT_ADDED,
    ])
});

static CPP17_HEADERS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    concat(&[
        CPP98_NATIVE,
        CPP11_NATIVE_ADDED,
        CPP14_ADDED,
        CPP17_ADDED,
        CPP_C_COMPAT,
        CPP11_C_COMPAT_ADDED,
    ])
});

static CPP20_HEADERS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    concat(&[
        CPP98_NATIVE,
        CPP11_NATIVE_ADDED,
        CPP14_ADDED,
        CPP17_ADDED,
        CPP20_ADDED,
        CPP_C_COMPAT,
        CPP11_C_COMPAT_ADDED,
    ])
});

static CPP23_HEADERS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    concat(&[
        CPP98_NATIVE,
        CPP11_NATIVE_ADDED,
        CPP14_ADDED,
        CPP17_ADDED,
        CPP20_ADDED,
        CPP23_ADDED,
        CPP_C_COMPAT,
        CPP11_C_COMPAT_ADDED,
    ])
});

fn concat(parts: &[&'static [&'static str]]) -> Vec<&'static str> {
    let mut v = Vec::new();
    for p in parts {
        v.extend_from_slice(p);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_constant_lookup_returns_list() {
        let v = lookup("std.c.extensions").expect("found");
        match v {
            Value::List(list) => assert!(list.contains(&".c") && list.contains(&".h")),
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn c_and_cpp_extensions_are_disjoint_pairs_with_h_in_c() {
        let c = match lookup("std.c.extensions").unwrap() {
            Value::List(l) => l,
            _ => panic!(),
        };
        let cpp = match lookup("std.cpp.extensions").unwrap() {
            Value::List(l) => l,
            _ => panic!(),
        };
        assert!(c.contains(&".h"));
        assert!(c.contains(&".c"));
        assert!(!cpp.contains(&".h"));
        assert!(!cpp.contains(&".c"));
        assert!(cpp.contains(&".cpp"));
        assert!(cpp.contains(&".hpp"));
    }

    #[test]
    fn separate_header_and_source_lists_exist() {
        let ch = match lookup("std.c.header_extensions").unwrap() {
            Value::List(l) => l,
            _ => panic!(),
        };
        let cs = match lookup("std.c.source_extensions").unwrap() {
            Value::List(l) => l,
            _ => panic!(),
        };
        let ph = match lookup("std.cpp.header_extensions").unwrap() {
            Value::List(l) => l,
            _ => panic!(),
        };
        let ps = match lookup("std.cpp.source_extensions").unwrap() {
            Value::List(l) => l,
            _ => panic!(),
        };
        assert_eq!(ch, vec![".h"]);
        assert_eq!(cs, vec![".c"]);
        assert!(ph.contains(&".hpp"));
        assert!(ps.contains(&".cpp"));
    }

    #[test]
    fn old_underscore_names_no_longer_exist() {
        // v0.3 renamed `std.c_extensions` → `std.c.extensions` etc.
        assert!(lookup("std.c_extensions").is_none());
        assert!(lookup("std.cpp_extensions").is_none());
        assert!(lookup("std.all_extensions").is_none());
    }

    #[test]
    fn or_suffix_produces_regex_alternation() {
        let v = lookup("std.c89.system_headers_or").expect("found");
        let s = match v {
            Value::String(s) => s,
            _ => panic!("expected string"),
        };
        assert!(s.starts_with("(?:"));
        assert!(s.ends_with(')'));
        // Regex meta-character `.` from "stdio.h" must be escaped.
        assert!(s.contains(r"stdio\.h"));
    }

    #[test]
    fn unknown_constant_is_an_error() {
        assert!(lookup("std.nope").is_none());
    }

    #[test]
    fn expand_list_spreads_known_constant_and_keeps_literals() {
        let out = expand_list(&["@std.c.extensions".to_string(), ".inl".to_string()]).unwrap();
        assert!(out.contains(&".c".to_string()));
        assert!(out.contains(&".h".to_string()));
        assert!(out.contains(&".inl".to_string()));
    }

    #[test]
    fn expand_list_rejects_string_constant() {
        let err = expand_list(&["@std.c89.system_headers_or".to_string()]).unwrap_err();
        assert!(format!("{err}").contains("string"));
    }

    #[test]
    fn expand_list_rejects_unknown_constant() {
        let err = expand_list(&["@std.unknown".to_string()]).unwrap_err();
        assert!(format!("{err}").contains("unknown constant"));
    }

    #[test]
    fn substitute_replaces_string_constant_in_regex() {
        let s = substitute_in_string("^(@std.c89.system_headers_or)$").unwrap();
        assert!(s.starts_with("^("));
        assert!(s.contains("stdio"));
        assert!(s.ends_with(")$"));
    }

    #[test]
    fn substitute_replaces_list_constant_as_alternation() {
        // A list constant used in a string field is materialized as a
        // regex alternation, just like _or would do.
        let s = substitute_in_string("@std.c.extensions").unwrap();
        assert_eq!(s, r"(?:\.h|\.c)");
    }

    #[test]
    fn substitute_double_at_escapes_literal() {
        let s = substitute_in_string("foo@@bar").unwrap();
        assert_eq!(s, "foo@bar");
    }

    #[test]
    fn substitute_stray_at_is_an_error() {
        let err = substitute_in_string("@!").unwrap_err();
        assert!(format!("{err}").contains("stray"));
    }

    #[test]
    fn substitute_passes_through_text_without_constants() {
        let s = substitute_in_string(r"^([^/]+\.h)$").unwrap();
        assert_eq!(s, r"^([^/]+\.h)$");
    }

    #[test]
    fn cpp_version_inheritance_is_cumulative() {
        let v11 = match lookup("std.cpp11.system_headers").unwrap() {
            Value::List(l) => l,
            _ => panic!(),
        };
        let v98 = match lookup("std.cpp98.system_headers").unwrap() {
            Value::List(l) => l,
            _ => panic!(),
        };
        // Every C++98 header must still appear in C++11.
        for h in &v98 {
            assert!(v11.contains(h), "C++11 should include {h}");
        }
        // C++11 must add something.
        assert!(v11.contains(&"thread"));
        assert!(!v98.contains(&"thread"));
    }
}
