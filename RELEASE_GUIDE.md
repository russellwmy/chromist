# Release Guide — chromist

How to cut a release and publish to crates.io. The workspace has four publishable crates with a strict dependency order that must be followed.

---

## Publish order

Dependencies only flow downward, so crates must be published bottom-up:

```
1. chromist-types   (no workspace dependencies)
2. chromist-pdl     (depends on chromist-types)
3. chromist-cdp     (depends on chromist-types, chromist-pdl)
4. chromist         (depends on all three)
```

`xtask` has `publish = false` and is never published.

Publishing out of order will fail — `crates.io` will reject a crate whose dependencies are not yet available at the specified version.

---

## Pre-release checklist

Run through every item before bumping versions.

### Code quality
- [ ] `cargo test --workspace` passes with zero failures
- [ ] `cargo test -p chromist-cdp --test generate generated_code_is_fresh` passes
- [ ] `cargo check --workspace` produces no warnings (or all warnings are intentional and documented)
- [ ] `cargo clippy --workspace` produces no new warnings

### API hygiene
- [ ] No `pub` items accidentally exposed — review `pub` vs `pub(crate)` in each crate
- [ ] All deprecated items that were promised for removal in this version are removed
- [ ] `#[non_exhaustive]` is present on all public enums in `chromist-cdp` (generated and handwritten)

### Documentation
- [ ] `PROTOCOL_REVISION` in `chromist-cdp/src/lib.rs` matches the vendored PDL files
- [ ] `README.md` feature table is accurate
- [ ] All four `Cargo.toml` files have matching version strings

### Examples
- [ ] `cargo run --example screenshot` works against a real browser
- [ ] `cargo test -p chromist --test integration --features integration-tests` passes

---

## Versioning

All four crates share the same version number. A single CDP API change in `chromist-cdp` ripples through to `chromist`, so keeping them in sync avoids dependency confusion.

### What requires a major bump (x.0.0)

- Any field rename in generated types (camelCase → new name in PDL)
- Any mandatory field added to an existing struct
- Any removal of a public type, method, or enum variant
- Any change to trait method signatures in `chromist-types`

### What requires a minor bump (0.x.0)

- New commands, events, or domains added (additive)
- New optional fields on existing structs
- New enum variants (safe with `#[non_exhaustive]`)
- New `pub` methods on existing types
- PDL update that is purely additive

### What requires a patch bump (0.0.x)

- Bug fixes with no API change
- Documentation improvements
- Internal refactors with identical public API

---

## Version bump procedure

### 1. Update all four `Cargo.toml` files

```sh
# Edit version in each:
crates/chromist-types/Cargo.toml
crates/chromist-pdl/Cargo.toml
crates/chromist-cdp/Cargo.toml
crates/chromist/Cargo.toml
```

Also update the inter-crate dependency versions. For example, in `crates/chromist/Cargo.toml`:
```toml
[dependencies]
chromist-types = { path = "../chromist-types", version = "0.2.0" }
chromist-cdp   = { path = "../chromist-cdp",   version = "0.2.0" }
```

### 2. Verify the workspace still builds

```sh
cargo check --workspace
cargo test --workspace
```

### 3. Dry-run publish (catches packaging errors without uploading)

```sh
cargo publish --dry-run -p chromist-types
cargo publish --dry-run -p chromist-pdl
cargo publish --dry-run -p chromist-cdp
cargo publish --dry-run -p chromist
```

Common dry-run failures:
- `license` or `description` missing in `Cargo.toml`
- Files excluded by `.gitignore` that are needed by the crate (add `include = [...]` to `Cargo.toml`)
- `cdp.rs` not included — verify `crates/chromist-cdp/Cargo.toml` does not accidentally exclude `src/cdp.rs`

---

## Publishing

```sh
cargo publish -p chromist-types
# Wait for crates.io to index (usually < 60 s) before publishing dependents

cargo publish -p chromist-pdl
cargo publish -p chromist-cdp
cargo publish -p chromist
```

Publish each crate and wait for `crates.io` to index it before publishing the next. Running all four in parallel will fail because the dependency is not yet available on the registry.

---

## Post-release

```sh
# Tag the release
git tag v0.2.0
git push origin v0.2.0
```

Tag after all four crates are live on `crates.io`, not before — if a publish fails partway through, you can re-attempt without a tag conflict.

---

## Semver for generated bindings

`chromist-cdp` exposes ~50 domain modules auto-generated from PDL. The API surface is large and changes with each Chromium release. When a PDL update lands:

1. Check the diff of `cdp.rs` for renamed or removed items (see `PDL_UPDATE_GUIDE.md §3`).
2. Any rename or removal is a **major** version bump for `chromist-cdp` and `chromist`.
3. A purely additive update (new commands/events only) is a **minor** bump.
4. The `PROTOCOL_REVISION` constant in `chromist-cdp/src/lib.rs` records which Chromium commit the bindings reflect — update it with every PDL change, regardless of the semver level.
