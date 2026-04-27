use std::hint::black_box;
use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, Criterion};

fn pdl_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ parent must exist")
        .join("chromist-cdp/pdl")
}

fn bench_parse_browser_protocol(c: &mut Criterion) {
    let dir = pdl_dir();
    let browser_pdl = dir.join("browser_protocol.pdl");

    // Resolve includes outside the timing loop — only parsing is measured.
    let flat = chromist_pdl::pdl::resolver::read_pdl(&browser_pdl)
        .expect("failed to resolve browser_protocol.pdl includes");

    c.bench_function("parse_browser_protocol", |b| {
        b.iter(|| chromist_pdl::pdl::parse(black_box(&flat)).expect("parse must succeed"));
    });
}

fn bench_parse_js_protocol(c: &mut Criterion) {
    let dir = pdl_dir();
    let js_pdl = dir.join("js_protocol.pdl");

    // js_protocol.pdl has no includes, so read_pdl is equivalent to
    // fs::read_to_string but goes through the same resolver path.
    let flat =
        chromist_pdl::pdl::resolver::read_pdl(&js_pdl).expect("failed to read js_protocol.pdl");

    c.bench_function("parse_js_protocol", |b| {
        b.iter(|| chromist_pdl::pdl::parse(black_box(&flat)).expect("parse must succeed"));
    });
}

fn bench_generate_full(c: &mut Criterion) {
    let dir = pdl_dir();
    let browser_pdl = dir.join("browser_protocol.pdl");
    let js_pdl = dir.join("js_protocol.pdl");

    let files = vec![browser_pdl, js_pdl];

    c.bench_function("generate_full_codegen", |b| {
        b.iter(|| {
            chromist_pdl::build::Generator::default()
                .compile_pdls(black_box(&files))
                .expect("codegen must succeed")
        });
    });
}

criterion_group!(
    benches,
    bench_parse_browser_protocol,
    bench_parse_js_protocol,
    bench_generate_full,
);
criterion_main!(benches);
