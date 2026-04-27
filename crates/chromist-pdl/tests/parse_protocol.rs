#[test]
fn parses_browser_protocol() {
    let pdl_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../chromist-cdp/pdl/browser_protocol.pdl");
    let content = std::fs::read_to_string(pdl_path).unwrap();
    // The browser protocol uses `include` directives — resolve them first
    let base = std::path::Path::new(pdl_path).parent().unwrap();
    let resolved = chromist_pdl::pdl::resolver::resolve_includes(&content, Some(base)).unwrap();
    let protocol = chromist_pdl::pdl::parse(&resolved).unwrap();
    // should have many domains
    assert!(protocol.domains.len() > 10, "expected >10 domains, got {}", protocol.domains.len());
    // find Page domain
    let page = protocol.domains.iter().find(|d| d.name == "Page").expect("Page domain not found");
    assert!(!page.commands.is_empty(), "Page domain should have commands");
    assert!(!page.events.is_empty(), "Page domain should have events");
}

#[test]
fn parses_js_protocol() {
    let pdl_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../chromist-cdp/pdl/js_protocol.pdl");
    let content = std::fs::read_to_string(pdl_path).unwrap();
    let base = std::path::Path::new(pdl_path).parent().unwrap();
    let resolved = chromist_pdl::pdl::resolver::resolve_includes(&content, Some(base)).unwrap();
    let protocol = chromist_pdl::pdl::parse(&resolved).unwrap();
    assert!(protocol.domains.len() >= 3, "expected >=3 JS domains, got {}", protocol.domains.len());
}

#[test]
fn generator_runs_on_both_pdls() {
    use std::path::PathBuf;
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../chromist-cdp/pdl");
    let pdls = vec![base.join("browser_protocol.pdl"), base.join("js_protocol.pdl")];
    let out = chromist_pdl::build::Generator::default()
        .compile_pdls(&pdls)
        .expect("generator should succeed");
    assert!(out.contains("pub mod page"), "expected 'pub mod page' in output, found none");
    assert!(out.contains("pub mod network"), "expected 'pub mod network' in output, found none");
    // sanity: output is non-trivial
    assert!(out.len() > 100_000, "generated output too short: {} bytes", out.len());
}

#[test]
fn parse_rejects_empty_input() {
    let err = chromist_pdl::pdl::parse("").expect_err("empty input must fail");
    assert!(matches!(err, chromist_pdl::pdl::parser::ParseError::MissingHeader(_)));
}

#[test]
fn parse_rejects_whitespace_only_input() {
    let err = chromist_pdl::pdl::parse("   \n\n\t\n").expect_err("whitespace-only input must fail");
    assert!(matches!(err, chromist_pdl::pdl::parser::ParseError::MissingHeader(_)));
}

#[test]
fn parse_rejects_comments_without_version_header() {
    let err = chromist_pdl::pdl::parse("# just a comment\n# another one\n")
        .expect_err("comment-only input must fail");
    assert!(matches!(err, chromist_pdl::pdl::parser::ParseError::MissingHeader(_)));
}

#[test]
fn parse_accepts_minimal_version_only_protocol() {
    let src = "version\n  major 1\n  minor 3\n";
    let proto = chromist_pdl::pdl::parse(src).expect("minimal protocol parses");
    assert_eq!(proto.version.major, "1");
    assert_eq!(proto.version.minor, "3");
    assert!(proto.domains.is_empty());
}

#[test]
fn generator_parse_error_includes_file_path() {
    use chromist_pdl::build::GeneratorError;
    use std::path::PathBuf;

    let tmp = std::env::temp_dir().join("chromist-pdl-bad.pdl");
    std::fs::write(&tmp, "this is not a valid pdl\nno version header here").unwrap();

    let err = chromist_pdl::build::Generator::default()
        .compile_pdls(std::slice::from_ref(&tmp))
        .expect_err("malformed PDL should fail");

    match err {
        GeneratorError::Parse { ref path, .. } => {
            assert!(
                path.contains("chromist-pdl-bad.pdl"),
                "error path should include file name, got: {path}"
            );
        }
        other => panic!("expected Parse variant, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(msg.contains("chromist-pdl-bad.pdl"), "display should include file path: {msg}");

    let _ = std::fs::remove_file(&tmp);
    let _: PathBuf = tmp;
}

/// Verify that the generator emits the expected Rust constructs for a
/// well-known minimal PDL snippet.  This catches codegen regressions that the
/// large real-PDL tests might miss.
#[test]
fn generator_emits_correct_constructs_for_minimal_pdl() {
    let pdl = "\
version
  major 1
  minor 0

domain Greeter

  type Status extends string
    enum
      active
      inactive

  command greet
    parameters
      string name
    returns
      string message

  event greeted
    parameters
      string name
";

    let tmp = std::env::temp_dir().join("chromist_pdl_minimal.pdl");
    std::fs::write(&tmp, pdl).unwrap();

    let out = chromist_pdl::build::Generator::default()
        .compile_pdls(std::slice::from_ref(&tmp))
        .expect("minimal PDL should generate without errors");

    // Domain module present
    assert!(out.contains("pub mod greeter"), "expected 'pub mod greeter', got:\n{out}");

    // Enum type with non_exhaustive
    assert!(out.contains("pub enum Status"), "expected 'pub enum Status'");
    assert!(out.contains("#[non_exhaustive]"), "expected #[non_exhaustive] on enum");
    assert!(out.contains("Active"), "expected 'Active' variant");
    assert!(out.contains("Inactive"), "expected 'Inactive' variant");

    // Command params/response structs
    assert!(out.contains("pub struct GreetParams"), "expected 'pub struct GreetParams'");
    assert!(out.contains("pub struct GreetResponse"), "expected 'pub struct GreetResponse'");
    assert!(out.contains("IDENTIFIER"), "expected IDENTIFIER const on params struct");

    // Event struct
    assert!(out.contains("pub struct GreetedEvent"), "expected 'pub struct GreetedEvent'");

    // Global CdpEvent enum references the event
    assert!(out.contains("GreeterGreeted"), "expected 'GreeterGreeted' variant in CdpEvent");

    let _ = std::fs::remove_file(&tmp);
}
