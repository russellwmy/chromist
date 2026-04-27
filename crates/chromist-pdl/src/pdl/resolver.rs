//! Resolves PDL `include` directives and pre-processes the file for parsing.
//!
//! The main browser protocol PDL uses `include domains/Foo.pdl` to inline
//! sub-files.  [`resolve_includes`] walks those directives and produces a
//! single flat string that [`super::parse`] can consume.

use std::fs;
use std::path::Path;

use super::parser::ParseError;

/// Pre-process `input` by resolving `include <filename>` directives relative
/// to `base_dir`.
///
/// The license header at the top of each included file (everything up to and
/// including the first blank line) is stripped so version/domain declarations
/// are not duplicated.
///
/// If `base_dir` is `None` (no parent directory context), any `include`
/// directive will cause an error.
///
/// # Example
///
/// ```
/// // A PDL that has no includes passes through unchanged.
/// let input = "version\n  major 1\n  minor 0\n";
/// let out = chromist_pdl::pdl::resolver::resolve_includes(input, None).unwrap();
/// assert_eq!(out, input);
/// ```
pub fn resolve_includes(input: &str, base_dir: Option<&Path>) -> Result<String, ParseError> {
    let mut out = String::with_capacity(input.len() * 2);

    for line in input.lines() {
        if line.starts_with("include ") {
            let name = line
                .split_whitespace()
                .nth(1)
                .ok_or_else(|| ParseError::MissingHeader("malformed include directive".into()))?;

            let dir = base_dir.ok_or_else(|| {
                ParseError::MissingHeader(format!(
                    "include '{name}' found but no base directory provided"
                ))
            })?;

            let path = dir.join(name);
            let content = fs::read_to_string(&path).map_err(|e| {
                ParseError::MissingHeader(format!(
                    "cannot read included file '{}': {e}",
                    path.display()
                ))
            })?;

            // Strip license header — everything up to (and including) the
            // first blank line.
            let body =
                if let Some((_, rest)) = content.split_once("\n\n") { rest } else { &content };

            out.push_str(body);
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    Ok(out)
}

/// Convenience: read a `.pdl` file from `path`, resolve its includes, and
/// return the flat string.
pub fn read_pdl(path: &Path) -> Result<String, ParseError> {
    let content = fs::read_to_string(path)
        .map_err(|e| ParseError::MissingHeader(format!("cannot read '{}': {e}", path.display())))?;
    resolve_includes(&content, path.parent())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_without_includes() {
        let input = "version\n  major 1\n  minor 0\n";
        let out = resolve_includes(input, None).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn include_without_base_dir_is_error() {
        let input = "include subfile.pdl\n";
        let err = resolve_includes(input, None).unwrap_err();
        match err {
            ParseError::MissingHeader(msg) => {
                assert!(msg.contains("no base directory"), "unexpected message: {msg}");
            }
            other => panic!("expected MissingHeader, got {other:?}"),
        }
    }

    #[test]
    fn include_missing_file_is_error() {
        let input = "include nonexistent.pdl\n";
        let base = std::path::Path::new("/tmp");
        let err = resolve_includes(input, Some(base)).unwrap_err();
        match err {
            ParseError::MissingHeader(msg) => {
                assert!(msg.contains("nonexistent.pdl"), "unexpected message: {msg}");
            }
            other => panic!("expected MissingHeader, got {other:?}"),
        }
    }

    #[test]
    fn include_resolves_file_content() {
        use std::io::Write;
        let dir = std::env::temp_dir().join("chromist_pdl_resolver_test");
        fs::create_dir_all(&dir).unwrap();
        let sub_path = dir.join("sub.pdl");
        let mut f = fs::File::create(&sub_path).unwrap();
        // License header (stripped up to the first blank line) + body
        writeln!(f, "// Copyright\n\ndomain Sub").unwrap();
        drop(f);

        let input = "include sub.pdl\n";
        let out = resolve_includes(input, Some(&dir)).unwrap();
        assert!(out.contains("domain Sub"), "got: {out}");

        let _ = fs::remove_file(sub_path);
        let _ = fs::remove_dir(dir);
    }
}
