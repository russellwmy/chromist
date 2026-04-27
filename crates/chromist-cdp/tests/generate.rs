//! Staleness guard for `cdp.rs`.
//!
//! This test checks that the committed `src/cdp.rs` matches what the generator
//! would produce today.  It does **not** rewrite any source files — run
//! `cargo xtask codegen` to regenerate.

use chromist_pdl::build::Generator;
use std::path::Path;

#[test]
fn generated_code_is_fresh() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let browser_pdl = dir.join("pdl/browser_protocol.pdl");
    let js_pdl = dir.join("pdl/js_protocol.pdl");

    let expected =
        Generator::default().compile_pdls(&[browser_pdl, js_pdl]).expect("code generation failed");

    let actual = std::fs::read_to_string(dir.join("src/cdp.rs"))
        .expect("src/cdp.rs missing — run `cargo xtask codegen` to generate it");

    assert_eq!(actual, expected, "cdp.rs is stale — run `cargo xtask codegen` and commit the diff");
}
