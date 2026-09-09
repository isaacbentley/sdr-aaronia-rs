//! Guards against the two hand-maintained mirrors of this crate's public
//! surface drifting from the code they describe.
//!
//! `include/aaronia.h` is written by hand and nothing generates or diffs
//! it, and `python-aaronia/aaronia.pyi` says in its own header that it is
//! "kept in sync by hand". Both are exactly the kind of file a rename
//! forgets, and neither failure is visible to a compiler: a C consumer of
//! a stale header gets a link error at best, and a missing `.pyi` entry
//! degrades silently to `Any`.
//!
//! These read the sources at runtime rather than with `include_str!`, so
//! that `cargo publish`'s verification build — which excludes workspace
//! members — still compiles when `python-aaronia/` is absent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Everything between the `(` at `open` and its matching `)`.
fn balanced_args(src: &str, open: usize) -> Option<&str> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                // Guard the underflow: an unbalanced source should make
                // this parser give up, never panic in a debug build.
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&src[open + 1..i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Count top-level comma-separated parameters.
///
/// Rust wraps long parameter lists and leaves a trailing comma, which is
/// not a parameter; C never does. Strip it before counting or every
/// multi-line export looks like it takes one argument more than the
/// header declares.
fn arity(args: &str) -> usize {
    let trimmed = args.trim().trim_end_matches(',').trim_end();
    if trimmed.is_empty() || trimmed == "void" {
        return 0;
    }
    let mut depth = 0usize;
    let mut n = 1usize;
    let mut prev = ' ';
    for c in trimmed.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            // `<`/`>` are only brackets when they are not the `->` of a
            // function-pointer return type, which would otherwise
            // unbalance the depth and inflate the count.
            '<' => depth += 1,
            '>' if prev != '-' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => n += 1,
            _ => {}
        }
        prev = c;
    }
    n
}

/// `aaronia_*` functions the Rust side actually exports, with their arity.
fn rust_exports(src: &str) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let mut cursor = 0usize;
    while let Some(rel) = src[cursor..].find("extern \"C\" fn ") {
        let at = cursor + rel + "extern \"C\" fn ".len();
        let rest = &src[at..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.starts_with("aaronia_")
            && let Some(open) = src[at..].find('(')
            && let Some(args) = balanced_args(src, at + open)
        {
            out.insert(name, arity(args));
        }
        cursor = at;
    }
    out
}

/// Strip `//` and `/* */` comments so prose mentioning a symbol is not
/// mistaken for a declaration.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// `aaronia_*` declarations in the C header, with their arity.
///
/// Scans the whole comment-free text rather than line by line: several
/// declarations wrap their argument list across lines, and a line-based
/// parser silently misses exactly those.
fn header_decls(src: &str) -> BTreeMap<String, usize> {
    let text = strip_comments(src);
    let mut out = BTreeMap::new();
    let mut cursor = 0usize;
    while let Some(rel) = text[cursor..].find("aaronia_") {
        let at = cursor + rel;
        let name: String = text[at..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let after = &text[at + name.len()..];
        if after.trim_start().starts_with('(')
            && let Some(open_rel) = after.find('(')
            && let Some(args) = balanced_args(&text, at + name.len() + open_rel)
        {
            out.insert(name.clone(), arity(args));
        }
        cursor = at + name.len().max(1);
    }
    out
}

/// The C header must declare exactly the symbols Rust exports, with the
/// same arity — in both directions.
#[test]
fn c_header_matches_the_exported_symbols() {
    let header_path = repo("include/aaronia.h");
    if !header_path.exists() {
        eprintln!("include/aaronia.h not present (packaged build) — skipping");
        return;
    }
    let rust = std::fs::read_to_string(repo("src/c_api.rs")).expect("src/c_api.rs");
    let header = std::fs::read_to_string(&header_path).expect("include/aaronia.h");

    let exports = rust_exports(&rust);
    let decls = header_decls(&header);

    assert!(
        exports.len() > 40,
        "parsed only {} exports from c_api.rs — the parser, not the code, is probably wrong",
        exports.len()
    );

    let missing: Vec<_> = exports.keys().filter(|k| !decls.contains_key(*k)).collect();
    assert!(
        missing.is_empty(),
        "exported from Rust but absent from include/aaronia.h: {missing:?}\n\
         A C consumer cannot call these."
    );

    let extra: Vec<_> = decls.keys().filter(|k| !exports.contains_key(*k)).collect();
    assert!(
        extra.is_empty(),
        "declared in include/aaronia.h but not exported by Rust: {extra:?}\n\
         A C consumer linking these gets an undefined symbol."
    );

    let mismatched: Vec<_> = exports
        .iter()
        .filter_map(|(name, n)| {
            decls
                .get(name)
                .filter(|m| *m != n)
                .map(|m| format!("{name}: rust takes {n}, header declares {m}"))
        })
        .collect();
    assert!(
        mismatched.is_empty(),
        "argument-count drift between Rust and the header: {mismatched:#?}"
    );
}

/// Does the stub declare `name`, as a declaration rather than as an
/// incidental substring? A bare `contains` passes vacuously for short
/// names that occur in a comment or another symbol.
fn stub_declares(stub: &str, name: &str) -> bool {
    stub.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with(&format!("def {name}("))
            || t.starts_with(&format!("async def {name}("))
            || t.starts_with(&format!("class {name}"))
            || t.starts_with(&format!("{name}:"))
            || t.starts_with(&format!("{name} ="))
    })
}

/// Collect every name PyO3 actually exposes to Python.
///
/// Covers all three shapes, because missing any one makes the test pass
/// vacuously for exactly the names a rename touches: `#[pyclass]` names,
/// `get_`/`set_` pairs (which become properties), plain `#[pymethods]`
/// functions, and `#[pyfunction]` module-level functions.
fn pyo3_exposed(src: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    let mut cursor = 0usize;
    while let Some(rel) = src[cursor..].find("#[pyclass(name = \"") {
        let at = cursor + rel + "#[pyclass(name = \"".len();
        out.push(src[at..].chars().take_while(|c| *c != '"').collect());
        cursor = at;
    }

    // Every `fn` inside a #[pymethods] block, plus #[pyfunction]s. Both
    // are found by scanning forward from the attribute; a `fn` in a plain
    // `impl` block is not exposed and must not be collected.
    let mut collect_fns = |from: usize, until: usize| {
        let mut i = from;
        while let Some(rel) = src[i..until.min(src.len())].find("fn ") {
            let at = i + rel + 3;
            let name: String = src[at..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && !name.starts_with("__") && name != "new" {
                // Only a #[getter]/#[setter] becomes a property; a plain
                // `fn set_center_frequency` stays a method of that name.
                // Stripping the prefix unconditionally invents a property
                // that Python never exposes.
                let preceding = &src[at.saturating_sub(64)..at];
                let is_accessor =
                    preceding.contains("#[getter]") || preceding.contains("#[setter]");
                let exposed = match (
                    is_accessor,
                    name.strip_prefix("get_"),
                    name.strip_prefix("set_"),
                ) {
                    (true, Some(prop), _) | (true, _, Some(prop)) => prop.to_string(),
                    _ => name.clone(),
                };
                out.push(exposed);
            }
            i = at;
        }
    };

    for (attr, span) in [("#[pymethods]", 6000usize), ("#[pyfunction]", 400usize)] {
        let mut cursor = 0usize;
        while let Some(rel) = src[cursor..].find(attr) {
            let at = cursor + rel + attr.len();
            let end = match src[at..].find(attr) {
                Some(next) => at + next,
                None => (at + span).min(src.len()),
            };
            collect_fns(at, end.min(at + span));
            cursor = at;
        }
    }

    out.sort();
    out.dedup();
    out
}

/// Every name PyO3 exposes must be declared in the hand-written stub.
///
/// One-directional on purpose: a stub carrying an extra overload or alias
/// is harmless; a stub missing a real method is not, because it degrades
/// silently to `Any` rather than erroring.
#[test]
fn python_stub_declares_everything_pyo3_exposes() {
    let lib = repo("python-aaronia/src/lib.rs");
    let pyi = repo("python-aaronia/aaronia.pyi");
    if !lib.exists() || !pyi.exists() {
        eprintln!("python-aaronia not present (packaged build) — skipping");
        return;
    }
    let src = std::fs::read_to_string(&lib).expect("python-aaronia/src/lib.rs");
    let stub = std::fs::read_to_string(&pyi).expect("python-aaronia/aaronia.pyi");

    let exposed = pyo3_exposed(&src);
    assert!(
        exposed.len() > 20,
        "collected only {} exposed names — the scanner, not the bindings, is probably wrong",
        exposed.len()
    );

    let missing: Vec<_> = exposed
        .iter()
        .filter(|n| !stub_declares(&stub, n))
        .collect();
    assert!(
        missing.is_empty(),
        "exposed to Python but not declared in aaronia.pyi: {missing:?}\n\
         The stub is maintained by hand and nothing else checks it."
    );
}
