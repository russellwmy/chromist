# PDL Update Guide — chromist

How to update the vendored Chrome DevTools Protocol definitions when Chromium ships a new protocol revision. The PDL files are the source of truth for `chromist-cdp`; this guide is the only place the update workflow is documented.

---

## When to update

- A new Chromium stable release changes behaviour you depend on
- A CDP command you need exists only in a newer protocol revision
- `CdpError::InvalidMessage` appears with a field or event not in the current bindings

The current pinned revision is stored in `crates/chromist-cdp/src/lib.rs`:
```rust
pub const PROTOCOL_REVISION: &str = "1619965";
```

Map this to a browser version at https://chromiumdash.appspot.com/commits.

---

## Where the PDL files come from

The vendored PDL files under `crates/chromist-cdp/pdl/` come from the `devtools-protocol` repository:
https://github.com/ChromeDevTools/devtools-protocol

The two files to update:
- `crates/chromist-cdp/pdl/browser_protocol.pdl`
- `crates/chromist-cdp/pdl/js_protocol.pdl`

---

## Step-by-step update

### 1. Obtain the new PDL files

```sh
# Option A: download directly from a specific Chromium commit
curl -o crates/chromist-cdp/pdl/browser_protocol.pdl \
  "https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/<COMMIT>/json/browser_protocol.pdl"

curl -o crates/chromist-cdp/pdl/js_protocol.pdl \
  "https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/<COMMIT>/json/js_protocol.pdl"

# Option B: clone the repo and copy
git clone https://github.com/ChromeDevTools/devtools-protocol /tmp/devtools-protocol
cp /tmp/devtools-protocol/json/browser_protocol.pdl crates/chromist-cdp/pdl/
cp /tmp/devtools-protocol/json/js_protocol.pdl crates/chromist-cdp/pdl/
```

### 2. Regenerate the bindings

```sh
cargo xtask codegen
```

This writes a new `crates/chromist-cdp/src/cdp.rs`. If codegen fails, the error is a `GeneratorError` — usually a parse error in the new PDL (malformed syntax) or an `Other` error (e.g. conflicting type names). Fix in `chromist-pdl` if it's a generator bug, or report upstream if it's a bad PDL file.

### 3. Review the diff

```sh
git diff crates/chromist-cdp/src/cdp.rs | less
```

**What to look for:**

| Change type | What it means | Action |
|---|---|---|
| New domain module | New CDP domain added | None — auto-generated |
| New command/event in existing domain | Chrome added new capability | None — auto-generated |
| Renamed field (`-old_name` / `+new_name`) | **Breaking change** for callers | Audit all uses in `chromist/src/` |
| Field changed from optional to mandatory | **Breaking change** — default constructors may no longer compile | Fix `Default` impls; check builder `build()` calls |
| Removed variant from an enum | Rare; breaking if used | Search codebase for the variant name |
| New enum variant | Non-breaking with `#[non_exhaustive]` | Verify `#[non_exhaustive]` is on the enum (it should be once added per `CODING_GUIDE.md §7`) |
| Changed type (e.g. `i64` → `f64`) | Breaking | Find all callers |

A typical minor update adds lines (new commands/events) and changes nothing existing — the diff will be additive.

### 4. Fix compilation errors

```sh
cargo check --workspace
```

Most errors after a PDL update are in `crates/chromist/src/` — the high-level API wraps generated types and may reference fields that were renamed. Fix each error site.

### 5. Run the tests

```sh
cargo test --workspace
```

The freshness test (`generated_code_is_fresh`) will pass automatically since you just regenerated. The serde round-trip tests in `chromist-cdp/tests/serde_roundtrip.rs` will catch field renames that break deserialization of real Chrome JSON. If a round-trip test fails, update the JSON fixture in the test to match the new field names.

### 6. Update `PROTOCOL_REVISION`

Edit `crates/chromist-cdp/src/lib.rs` and update the constant to the Chromium commit position of the new PDL files:

```rust
pub const PROTOCOL_REVISION: &str = "<NEW_REVISION>";
```

The commit position is a monotonically increasing integer visible in the commit URL on `chromiumdash.appspot.com`.

### 7. Commit

Commit the three changed artefacts together:
- `crates/chromist-cdp/pdl/browser_protocol.pdl`
- `crates/chromist-cdp/pdl/js_protocol.pdl`
- `crates/chromist-cdp/src/cdp.rs`
- `crates/chromist-cdp/src/lib.rs` (`PROTOCOL_REVISION`)

Plus any fixes to `crates/chromist/src/` from step 4.

---

## Semver impact

| Change in diff | Semver impact on `chromist-cdp` | Semver impact on `chromist` |
|---|---|---|
| New domain / command / event | Minor (additive) | Minor |
| New mandatory field on existing struct | **Major** | **Major** |
| Renamed field | **Major** | **Major** |
| Removed field or enum variant | **Major** | **Major** |
| New optional field | Minor | Minor |
| New enum variant (with `#[non_exhaustive]`) | Minor | Minor |

A Chromium update that only adds new commands is a minor version bump. Any field rename or mandatory field addition requires a major bump. In practice, the Chrome team rarely makes breaking changes to existing domains — most updates are additive.

---

## Troubleshooting

**`GeneratorError::Parse` during codegen**
The new PDL uses syntax not handled by `chromist-pdl`'s parser. Open the failing PDL line (the error includes a line number), compare to the parser in `crates/chromist-pdl/src/pdl/parser.rs`, and extend the parser if needed.

**`cargo check` produces hundreds of errors after regeneration**
The new bindings renamed or removed something widely used. Use `git diff --stat crates/chromist-cdp/src/cdp.rs` to count changed lines — if > 1000, you may be on a much newer PDL than expected. Diff the old and new PDL files directly to understand the scope before fixing.

**Serde round-trip tests fail with "missing field"**
A field was renamed in the PDL. The test's JSON fixture uses the old camelCase name. Update the fixture string to match the new field name from the PDL.
