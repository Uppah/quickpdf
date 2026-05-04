# Phase 2b Web Fonts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Honor `@font-face` rules from author CSS so HTML emitting brand fonts (sourced exclusively from inline `data:font/ttf|otf;base64,...` URLs) renders in those fonts instead of the bundled Inter fallback.

**Architecture:** A new `Stylesheet` aggregate (`{ rules, font_faces }`) replaces the rules-only output of `parse_stylesheet_full`, with a back-compat `parse_stylesheet -> Vec<Rule>` wrapper preserving the existing test surface. A new `FontRegistry` in `font.rs` decodes each `@font-face`'s `src` (permissive MIME accept + magic-byte sniff for TTF/OTF), pre-registers Inter at handle 0, and answers `lookup(family_chain)` calls. The cascade gains a `font_family: Option<Vec<String>>` longhand, inherited per CSS spec. `lib.rs::plan_pages_styled` resolves each block's family chain to a `FontHandle` stamped on every `PlacedLine`; the emit loop swaps fonts on handle change (mirroring the existing color-switch optimisation).

**Tech Stack:** Rust 2021. krilla 0.7 (`Font::new`, `Surface::draw_text` — already used). skrifa 0.37 (glyph metrics via `text::TextMetrics`, already used). base64 0.22.1 (already in `[workspace.dependencies]` from Phase 2a — no new dep). scraper/html5ever (DOM, unchanged). pyo3 0.23 (Python bindings, unchanged).

**Spec deviation:** None. The spec at `docs/superpowers/specs/2026-05-04-phase-2b-fonts-design.md` was tightened during writing to (a) preserve `parse_stylesheet -> Vec<Rule>` as a back-compat wrapper instead of changing its signature (~30 existing test sites touched otherwise), and (b) make the `cascade::inherit` arm for `Option<Vec<String>>` use `child.or_else(|| parent.clone())` instead of the sentinel-compare pattern used for `f32`/`Color`. Both refinements are reflected here.

---

## File Structure

| File | Role | Owning slice |
| --- | --- | --- |
| `crates/quickpdf-core/src/style/sheet.rs` | `FontFace` struct, `Stylesheet` aggregate, `parse_stylesheet_full`, `@font-face` capture, back-compat `parse_stylesheet` wrapper | Slice A |
| `crates/quickpdf-core/src/parse.rs` | New `Document::stylesheet() -> Stylesheet` accessor; preserved `user_stylesheet() -> Vec<Rule>` for back-compat | Slice A |
| `crates/quickpdf-core/src/font.rs` | `FontHandle` type alias, `RegisteredFont` + `FontRegistry` types, `FontRegistry::build`/`lookup`, src-list tokenizer, MIME accept list, magic-byte sniff | Slice B |
| `crates/quickpdf-core/src/style/mod.rs` | `BlockStyle.font_family: Option<Vec<String>>` field + `DEFAULT` initialiser | Slice C |
| `crates/quickpdf-core/src/style/cascade.rs` | `font-family` value parser (comma list, quote stripping, generic-keyword drop), `apply_declaration` arm, `inherit` arm | Slice C |
| `crates/quickpdf-core/src/lib.rs` | `PlacedLine.font_handle` field; `FontRegistry` build at top of `html_to_pdf`; planner + emitter font routing; alt-text path stamps font handle | Integrator |
| `tests/test_render.py` | Python integration tests for `@font-face` rendering | Integrator |
| `CLAUDE.md` | Roadmap table marks Phase 2b ✓; "Next session" prose points at Phase 2c | Integrator |

Slice A, Slice B, and Slice C are intentionally non-overlapping. A subagent-driven executor MAY run them in parallel after Phase 0. Slice A's signature changes are wrapped in back-compat shims so `lib.rs` keeps compiling; the integrator phase opts into the new `Document::stylesheet()` aggregate.

---

## Phase 0 — Setup

### Task 1: Verify the build is green from Phase 2a

**Files:** none modified — purely verification.

- [ ] **Step 1: Confirm clean working tree**

Run: `git status`
Expected: `nothing to commit, working tree clean` (or only the design spec file present).

- [ ] **Step 2: Confirm the workspace builds and tests pass**

Run: `cargo test -p quickpdf-core --lib`
Expected: 189 tests PASS (the Phase 2a baseline).

- [ ] **Step 3: Confirm `base64` is already in workspace deps**

Read `Cargo.toml` (workspace root). Confirm the `[workspace.dependencies]` table contains `base64 = "=0.22.1"`. No new dependency is needed for Phase 2b.

If absent (would be a Phase 2a regression): STOP and surface to the user. Do not proceed.

- [ ] **Step 4: Confirm `skrifa` and `krilla` versions match the spec**

Run: `cargo tree -p quickpdf-core -i krilla --depth 0`
Expected: krilla 0.7.x. The Phase 2b code calls `krilla::text::Font::new(bytes, 0) -> Option<Font>` — same signature already used in `lib.rs::html_to_pdf`.

No commit at this step (no file changes).

---

## Phase 1 — Slice A: `Stylesheet` aggregate + `@font-face` capture

Constraint: Slice A only edits `crates/quickpdf-core/src/style/sheet.rs` and `crates/quickpdf-core/src/parse.rs`. **Do not** touch `font.rs`, `style/mod.rs`, `style/cascade.rs`, or `lib.rs`. Green-bar gate: `cargo check -p quickpdf-core` (skips `#[cfg(test)]` bodies, immune to integrator-only fixups). The new symbols are additive — every existing call site must continue to compile via the back-compat wrappers introduced below.

### Task 2: Add `FontFace` and `Stylesheet` types

**Files:**
- Modify: `crates/quickpdf-core/src/style/sheet.rs` (add types near the existing `Rule` definition, around lines 14-37)

- [ ] **Step 1: Read the existing module header**

Run: `cargo check -p quickpdf-core` to confirm the baseline still builds.

- [ ] **Step 2: Add the new types**

In `crates/quickpdf-core/src/style/sheet.rs`, immediately after the `Declaration` struct (around line 37, just before the `parse_inline_declarations` function), insert:

```rust
/// One `@font-face` block captured from author CSS. Phase 2b's font
/// registry walks these to register web fonts; the registry parses
/// the `font-family` and `src` descriptors itself.
///
/// `declarations` includes every declaration inside the block, normalised
/// the same way qualified-rule declarations are: comments stripped,
/// shorthand expanded, `!important` flag honoured. Non-`font-family` /
/// non-`src` descriptors (e.g. `font-weight`, `unicode-range`) are
/// preserved for forward compatibility but ignored by Phase 2b.
#[derive(Debug, Clone)]
pub struct FontFace {
    pub declarations: Vec<Declaration>,
    /// Source order across the full stylesheet (shared numbering with
    /// `Rule.source_order`). Used by the registry for last-wins
    /// disambiguation when two `@font-face` blocks declare the same
    /// family name.
    pub source_order: usize,
}

/// Parsed stylesheet: qualified rules and `@font-face` blocks. Other
/// at-rules (`@media`, `@import`, `@keyframes`, …) are still dropped
/// silently by `skip_at_rule`.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    pub font_faces: Vec<FontFace>,
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p quickpdf-core`
Expected: builds clean, no warnings about unused types (the types are `pub`, so they're part of the public API).

- [ ] **Step 4: Commit**

```bash
git add crates/quickpdf-core/src/style/sheet.rs
git commit -m "Phase 2b Slice A: add FontFace and Stylesheet types"
```

### Task 3: Refactor `parse_stylesheet` to delegate to a `_full` variant

**Files:**
- Modify: `crates/quickpdf-core/src/style/sheet.rs` (the `parse_stylesheet` function, lines 49-97)

This task introduces `parse_stylesheet_full` returning the new `Stylesheet` aggregate, and re-shapes the existing `parse_stylesheet` to delegate. Behavior is byte-identical to today; the new function returns an empty `font_faces` Vec for now. Task 5 lights up the `@font-face` capture.

- [ ] **Step 1: Write a failing test for the empty-input case**

Append to the existing `#[cfg(test)] mod tests` block in `sheet.rs` (after the `important_*` and `padding_*` helpers, just before the closing `}`):

```rust
    // ---- Phase 2b Slice A: Stylesheet aggregate. ----

    #[test]
    fn parse_stylesheet_full_empty_returns_empty_aggregate() {
        let sheet = parse_stylesheet_full("");
        assert!(sheet.rules.is_empty());
        assert!(sheet.font_faces.is_empty());
    }

    #[test]
    fn parse_stylesheet_full_rules_match_legacy_parse_stylesheet() {
        let src = "h1 { font-size: 24px; } p { font-size: 12px; }";
        let aggregate = parse_stylesheet_full(src);
        let legacy = parse_stylesheet(src);
        assert_eq!(aggregate.rules.len(), legacy.len());
        for (a, l) in aggregate.rules.iter().zip(legacy.iter()) {
            assert_eq!(a.selector_text, l.selector_text);
            assert_eq!(a.source_order, l.source_order);
        }
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test -p quickpdf-core --lib parse_stylesheet_full`
Expected: FAIL with "cannot find function `parse_stylesheet_full`".

- [ ] **Step 3: Implement `parse_stylesheet_full` and refactor `parse_stylesheet`**

In `crates/quickpdf-core/src/style/sheet.rs`, replace the existing `parse_stylesheet` function (lines 49-97) with:

```rust
/// Parse a stylesheet source string into a `Stylesheet` aggregate
/// containing both qualified rules and `@font-face` blocks. Always
/// returns — malformed rules are silently skipped (browsers do the
/// same). Phase 2b's font registry consumes the `font_faces` field;
/// the cascade consumes `rules` exactly as before.
pub fn parse_stylesheet_full(source: &str) -> Stylesheet {
    let bytes = source.as_bytes();
    let mut pos = 0;
    let mut rules: Vec<Rule> = Vec::new();
    let mut font_faces: Vec<FontFace> = Vec::new();
    let mut order: usize = 0;

    while pos < bytes.len() {
        pos = skip_ws_and_comments(bytes, pos);
        if pos >= bytes.len() {
            break;
        }

        // At-rule? `@font-face` is captured; everything else is dropped.
        if bytes[pos] == b'@' {
            match try_capture_font_face(source, bytes, pos, order) {
                Some((face, next_pos)) => {
                    font_faces.push(face);
                    pos = next_pos;
                    order += 1;
                }
                None => {
                    pos = skip_at_rule(bytes, pos);
                }
            }
            continue;
        }

        // Stray `}` at top level — skip it and keep going.
        if bytes[pos] == b'}' {
            pos += 1;
            continue;
        }

        // Otherwise: read a qualified rule (selector { decls }).
        match read_qualified_rule(source, bytes, pos) {
            Some((rule_opt, next_pos)) => {
                if let Some((selector_text, declarations)) = rule_opt {
                    rules.push(Rule {
                        selector_text,
                        declarations,
                        source_order: order,
                    });
                    order += 1;
                }
                pos = next_pos;
            }
            None => {
                // Couldn't find a `{` matching the prelude — input ran out
                // without a block opener. Treat the rest of the stream as
                // malformed and stop.
                break;
            }
        }
    }

    Stylesheet { rules, font_faces }
}

/// Back-compat wrapper. Existing callers and ~30 unit tests in this
/// file consume `Vec<Rule>` directly; this preserves their contract.
/// New callers should prefer `parse_stylesheet_full` for the aggregate.
pub fn parse_stylesheet(source: &str) -> Vec<Rule> {
    parse_stylesheet_full(source).rules
}
```

Add this stub immediately after `parse_stylesheet`. The real recogniser comes in Task 5; for now it always returns `None` so behavior matches today (every `@`-rule falls through to `skip_at_rule`):

```rust
/// Try to recognise an `@font-face` rule starting at `bytes[pos] == b'@'`.
/// Returns `Some((face, next_pos))` on a successful capture, or `None`
/// if this isn't an `@font-face` rule (or it's malformed) — in which
/// case the caller falls back to `skip_at_rule` to consume it.
///
/// Phase 2b Task 5 wires up the real recogniser. Until then this stub
/// always returns `None` so existing behavior is unchanged.
fn try_capture_font_face(
    _source: &str,
    _bytes: &[u8],
    _pos: usize,
    _source_order: usize,
) -> Option<(FontFace, usize)> {
    None
}
```

- [ ] **Step 4: Run the new tests + the full module tests**

Run: `cargo test -p quickpdf-core --lib sheet::`
Expected: all tests PASS, including the two new `parse_stylesheet_full_*` tests and every pre-existing `parse_stylesheet` test (back-compat wrapper preserves behavior).

- [ ] **Step 5: Commit**

```bash
git add crates/quickpdf-core/src/style/sheet.rs
git commit -m "Phase 2b Slice A: add parse_stylesheet_full + back-compat shim"
```

### Task 4: Add `Document::stylesheet()` accessor

**Files:**
- Modify: `crates/quickpdf-core/src/parse.rs` (add a method alongside `user_stylesheet`, around lines 87-89)

- [ ] **Step 1: Write a failing test for the accessor**

Open `crates/quickpdf-core/src/parse.rs`. Find the existing `#[cfg(test)] mod tests` block. Append (just before the closing `}`):

```rust
    #[test]
    fn document_stylesheet_returns_aggregate_with_rules_and_empty_faces() {
        let doc = Document::parse(
            "<style>p { color: red; }</style><p>x</p>",
        );
        let sheet = doc.stylesheet();
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.rules[0].selector_text, "p");
        // Phase 2b Slice A's @font-face stub returns no faces yet; Task 5
        // lights up real capture and adds another assertion at that layer.
        assert!(sheet.font_faces.is_empty());
    }

    #[test]
    fn document_user_stylesheet_back_compat_still_works() {
        // The existing rules-only accessor must continue to work so
        // pre-2b callers (lib.rs) keep compiling.
        let doc = Document::parse(
            "<style>p { color: red; }</style><p>x</p>",
        );
        let rules = doc.user_stylesheet();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selector_text, "p");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p quickpdf-core --lib document_stylesheet_returns_aggregate`
Expected: FAIL with "no method named `stylesheet` found".

- [ ] **Step 3: Implement the accessor**

In `crates/quickpdf-core/src/parse.rs`, find the existing `user_stylesheet` method (around lines 87-89). Add a new method immediately after it:

```rust
    /// Phase 2b: parse the document's `<style>` blocks into the aggregate
    /// stylesheet (qualified rules + `@font-face` blocks). New code
    /// should prefer this over `user_stylesheet` because it surfaces
    /// font-face data needed for the registry.
    pub fn stylesheet(&self) -> sheet::Stylesheet {
        sheet::parse_stylesheet_full(&sheet::collect_style_blocks(self))
    }
```

(`user_stylesheet` is preserved unchanged; lib.rs continues to use it until the integrator phase.)

- [ ] **Step 4: Run the new tests**

Run: `cargo test -p quickpdf-core --lib document_stylesheet`
Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/quickpdf-core/src/parse.rs
git commit -m "Phase 2b Slice A: add Document::stylesheet() aggregate accessor"
```

### Task 5: Implement `@font-face` capture

**Files:**
- Modify: `crates/quickpdf-core/src/style/sheet.rs` (the `try_capture_font_face` stub from Task 3)

- [ ] **Step 1: Write failing tests for the recogniser**

Append to the existing `#[cfg(test)] mod tests` block in `sheet.rs`:

```rust
    #[test]
    fn font_face_basic_block_captured() {
        let src = "@font-face { font-family: Acme; src: url(data:font/ttf;base64,AAA); }";
        let sheet = parse_stylesheet_full(src);
        assert_eq!(sheet.font_faces.len(), 1);
        assert_eq!(sheet.rules.len(), 0);
        let face = &sheet.font_faces[0];
        assert_eq!(face.source_order, 0);
        // Both descriptors land in the declaration list, normalised by
        // parse_declaration_block.
        let names: Vec<&str> = face.declarations.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"font-family"));
        assert!(names.contains(&"src"));
    }

    #[test]
    fn font_face_keyword_is_case_insensitive() {
        for variant in ["@font-face", "@Font-Face", "@FONT-FACE", "@fOnT-fAcE"] {
            let src = format!("{variant} {{ font-family: A; src: url(x); }}");
            let sheet = parse_stylesheet_full(&src);
            assert_eq!(sheet.font_faces.len(), 1, "variant {variant} not recognised");
        }
    }

    #[test]
    fn font_face_tolerates_whitespace_before_brace() {
        let src = "@font-face\n  \t{ font-family: Acme; src: url(x); }";
        let sheet = parse_stylesheet_full(src);
        assert_eq!(sheet.font_faces.len(), 1);
    }

    #[test]
    fn font_face_source_order_is_shared_with_rules() {
        // Three top-level items: rule, font-face, rule. Source order
        // assignments must be 0, 1, 2 in that interleaved order.
        let src = "p { color: red; } \
                   @font-face { font-family: A; src: url(x); } \
                   h1 { color: blue; }";
        let sheet = parse_stylesheet_full(src);
        assert_eq!(sheet.rules.len(), 2);
        assert_eq!(sheet.font_faces.len(), 1);
        assert_eq!(sheet.rules[0].source_order, 0);   // p
        assert_eq!(sheet.font_faces[0].source_order, 1); // @font-face
        assert_eq!(sheet.rules[1].source_order, 2);   // h1
    }

    #[test]
    fn font_face_unterminated_block_dropped_silently() {
        // No closing brace — must not panic, must not corrupt subsequent
        // parsing. Phase 2b's policy: drop the malformed @font-face and
        // any tail content inside it (the caller's `skip_at_rule`
        // fallback consumes through the next balanced } or EOF).
        let src = "@font-face { font-family: A; src: url(x); /* no closer */ ";
        let sheet = parse_stylesheet_full(src);
        // No crash; no faces captured.
        assert_eq!(sheet.font_faces.len(), 0);
    }

    #[test]
    fn font_face_other_at_rules_still_dropped() {
        // @media, @import, etc. continue to be silently skipped — only
        // @font-face is carved out.
        let src = "@media print { p { x: y; } } \
                   @font-face { font-family: A; src: url(x); } \
                   @import url('foo.css');";
        let sheet = parse_stylesheet_full(src);
        assert_eq!(sheet.font_faces.len(), 1);
        assert_eq!(sheet.rules.len(), 0);
    }

    #[test]
    fn font_face_empty_block_yields_empty_declarations() {
        // `@font-face { }` is technically valid CSS but useless. The
        // registry will drop it (no font-family). At the parser layer
        // we capture it with an empty declaration list.
        let sheet = parse_stylesheet_full("@font-face { }");
        assert_eq!(sheet.font_faces.len(), 1);
        assert_eq!(sheet.font_faces[0].declarations.len(), 0);
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test -p quickpdf-core --lib font_face_`
Expected: every `font_face_*` test FAILS — most because they expect `font_faces.len() == 1` while the stub returns `None` (so the parser falls through to `skip_at_rule`).

- [ ] **Step 3: Replace the stub with a real recogniser**

In `crates/quickpdf-core/src/style/sheet.rs`, replace the `try_capture_font_face` stub from Task 3 with this implementation:

```rust
/// Try to recognise an `@font-face` rule starting at `bytes[pos] == b'@'`.
/// Returns `Some((face, next_pos))` on a successful capture, or `None`
/// if this isn't an `@font-face` rule — in which case the caller falls
/// back to `skip_at_rule` to consume it.
///
/// Recognition: the byte after `@` plus the next 9 ASCII bytes are
/// matched case-insensitively against `"font-face"`. Whitespace and
/// comments may follow before the `{`. If the keyword matches but no
/// `{` follows (truncated input, syntax error), we still return None
/// so the caller's `skip_at_rule` consumes the malformed remainder.
fn try_capture_font_face(
    source: &str,
    bytes: &[u8],
    pos: usize,
    source_order: usize,
) -> Option<(FontFace, usize)> {
    debug_assert_eq!(bytes.get(pos), Some(&b'@'));

    // 1. Match the keyword (case-insensitive) at bytes[pos+1..pos+10].
    const KEYWORD: &[u8] = b"font-face";
    if pos + 1 + KEYWORD.len() > bytes.len() {
        return None;
    }
    for (i, want) in KEYWORD.iter().enumerate() {
        let got = bytes[pos + 1 + i];
        if !got.eq_ignore_ascii_case(want) {
            return None;
        }
    }

    // 2. After the keyword, the next char must be whitespace, `{`, or
    //    comment-start. Anything else (e.g. `@font-face-extra`) means
    //    we matched a prefix of a different at-rule — bail out.
    let after_kw = pos + 1 + KEYWORD.len();
    if after_kw < bytes.len() {
        let c = bytes[after_kw];
        let ok = is_css_ws(c) || c == b'{' || (c == b'/' && bytes.get(after_kw + 1) == Some(&b'*'));
        if !ok {
            return None;
        }
    }

    // 3. Skip whitespace + comments to find the `{`.
    let mut p = skip_ws_and_comments(bytes, after_kw);
    if p >= bytes.len() || bytes[p] != b'{' {
        // Keyword matched but no opening brace before EOF/garbage. Caller's
        // `skip_at_rule` fallback handles the cleanup.
        return None;
    }

    // 4. Walk the balanced block; capture the body for declaration parsing.
    let block_open = p;
    let block_close_plus_one = skip_balanced_block(bytes, block_open);
    // Reuse the same closer-validation that read_qualified_rule uses: if
    // the block never closed, we treat the whole stream as truncated and
    // bail (caller's skip_at_rule will eat to EOF too).
    if block_close_plus_one == bytes.len() && !ends_with_close_brace(bytes, block_close_plus_one) {
        return None;
    }
    p = block_close_plus_one;

    let body_start = block_open + 1;
    let body_end = block_close_plus_one.saturating_sub(1).max(body_start);
    let body = &source[body_start..body_end];
    let declarations = parse_declaration_block(body);

    Some((
        FontFace {
            declarations,
            source_order,
        },
        p,
    ))
}
```

- [ ] **Step 4: Run all the `font_face_*` tests**

Run: `cargo test -p quickpdf-core --lib font_face_`
Expected: all 7 new `font_face_*` tests PASS.

- [ ] **Step 5: Run the full sheet.rs test set to verify nothing regressed**

Run: `cargo test -p quickpdf-core --lib sheet::`
Expected: all sheet.rs tests PASS — the original ones (parse_stylesheet, important, padding/margin/border shorthands, …) plus the new ones from Tasks 3 and 5.

- [ ] **Step 6: Commit**

```bash
git add crates/quickpdf-core/src/style/sheet.rs
git commit -m "Phase 2b Slice A: capture @font-face blocks in parse_stylesheet_full"
```

### Task 6: Slice A green-bar gate + finalisation

**Files:** none modified — purely verification.

- [ ] **Step 1: Run the full Rust test suite**

Run: `cargo test -p quickpdf-core --lib`
Expected: all tests PASS — 189 baseline + ~9 new from Slice A = ~198.

- [ ] **Step 2: Type-check**

Run: `cargo check -p quickpdf-core`
Expected: builds clean, no warnings.

No commit at this step — Slice A is complete. Slice B and Slice C may now run in parallel; the Integrator phase runs serially after both.

---

## Phase 2 — Slice B: `FontRegistry`

Constraint: Slice B only edits `crates/quickpdf-core/src/font.rs`. **Do not** touch `parse.rs`, `style/`, or `lib.rs`. Slice B depends on Slice A's `FontFace` struct shape (`{ declarations: Vec<Declaration>, source_order: usize }`); that shape is locked by Task 2. Slice B may run before or in parallel with Slice C. Green-bar gate: `cargo check -p quickpdf-core`.

### Task 7: Define registry types

**Files:**
- Modify: `crates/quickpdf-core/src/font.rs` (currently a 38-line file with `FALLBACK_TTF` and `FALLBACK_FAMILY`; add the registry types after the constants)

- [ ] **Step 1: Add the type definitions**

In `crates/quickpdf-core/src/font.rs`, after the existing `pub const FALLBACK_FAMILY: &str = "Inter";` line (around line 19), append:

```rust

// ---------------------------------------------------------------------------
// Phase 2b: font registry. Builds a per-document map from family name to
// concrete font (krilla `Font` instance + raw bytes for skrifa-side glyph
// metrics). The bundled Inter is always pre-registered at handle 0 and
// serves as the silent fallback when a `font-family` cascade chain has
// no registered hit.
// ---------------------------------------------------------------------------

use crate::style::sheet::{Declaration, FontFace};
use base64::Engine;
use std::collections::HashMap;
use std::sync::Arc;

/// Opaque handle into `FontRegistry::fonts`. Index 0 is always the
/// bundled Inter fallback; any unsuccessful lookup returns 0.
pub type FontHandle = usize;

/// One registered font: raw bytes (kept for skrifa glyph-advance
/// measurement) plus the krilla `Font` instance used at PDF emit time.
#[derive(Clone)]
pub struct RegisteredFont {
    /// Owned via `Arc<[u8]>` so multiple call sites (planner's
    /// `TextMetrics::new`, krilla's `Font::new`) can hold cheap
    /// references without copying.
    pub bytes: Arc<[u8]>,
    pub krilla_font: krilla::text::Font,
}

/// A document-scoped registry of known font faces. Built once per
/// `html_to_pdf` call and consumed by the planner and emitter.
pub struct FontRegistry {
    /// Index 0 is always Inter (the bundled fallback).
    pub fonts: Vec<RegisteredFont>,
    /// Lowercased family name → handle. Inter is registered as "inter"
    /// so authors can name it explicitly via `font-family: Inter`.
    pub by_family: HashMap<String, FontHandle>,
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p quickpdf-core`
Expected: builds clean. (`crate::style::sheet::{Declaration, FontFace}` resolves because Slice A's Task 2 made them public; if Slice B is running in parallel and Slice A hasn't merged yet, this Step depends on the contract document — see the file structure section.)

- [ ] **Step 3: Commit**

```bash
git add crates/quickpdf-core/src/font.rs
git commit -m "Phase 2b Slice B: add FontHandle, RegisteredFont, FontRegistry types"
```

### Task 8: Implement `FontRegistry::build` for the empty case (Inter only)

**Files:**
- Modify: `crates/quickpdf-core/src/font.rs`

- [ ] **Step 1: Write failing tests**

In `crates/quickpdf-core/src/font.rs`, find the existing `#[cfg(test)] mod tests` block and append:

```rust

    // ---- Phase 2b: FontRegistry. ----

    #[test]
    fn registry_with_no_faces_has_inter_at_handle_zero() {
        let registry = FontRegistry::build(&[]);
        assert_eq!(registry.fonts.len(), 1);
        assert_eq!(registry.by_family.get("inter").copied(), Some(0));
    }

    #[test]
    fn registry_lookup_empty_chain_returns_zero() {
        let registry = FontRegistry::build(&[]);
        assert_eq!(registry.lookup(&[]), 0);
    }

    #[test]
    fn registry_lookup_unknown_family_returns_zero() {
        let registry = FontRegistry::build(&[]);
        assert_eq!(registry.lookup(&["notregistered".to_string()]), 0);
    }

    #[test]
    fn registry_lookup_inter_by_name_returns_zero() {
        let registry = FontRegistry::build(&[]);
        // Authors can spell out the bundled font's name; it resolves to
        // handle 0, same as the silent fallback.
        assert_eq!(registry.lookup(&["inter".to_string()]), 0);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p quickpdf-core --lib registry_`
Expected: FAIL with "no function or associated item named `build` found" (and similarly for `lookup`).

- [ ] **Step 3: Implement `build` and `lookup` for the empty case**

In `crates/quickpdf-core/src/font.rs`, append after the `FontRegistry` struct definition:

```rust

impl FontRegistry {
    /// Build a registry from the parsed `@font-face` rules. The bundled
    /// Inter is always pre-registered at handle 0 (under the lowercased
    /// key `"inter"`). Faces are processed in `source_order`; duplicate
    /// family names overwrite earlier handles (last-wins, mirroring
    /// the rest of the cascade). Faces with no decodable `src:` are
    /// silently dropped.
    pub fn build(font_faces: &[FontFace]) -> Self {
        let mut fonts: Vec<RegisteredFont> = Vec::with_capacity(1 + font_faces.len());
        let mut by_family: HashMap<String, FontHandle> = HashMap::new();

        // Pre-register Inter at handle 0.
        let inter_bytes: Arc<[u8]> = Arc::from(FALLBACK_TTF);
        if let Some(font) = krilla::text::Font::new(inter_bytes.to_vec().into(), 0) {
            fonts.push(RegisteredFont {
                bytes: inter_bytes,
                krilla_font: font,
            });
            by_family.insert(FALLBACK_FAMILY.to_ascii_lowercase(), 0);
        } else {
            // Should never happen: FALLBACK_TTF is verified by an
            // existing unit test. If it does happen, downstream code
            // expects fonts[0] to exist; panic loudly so we notice.
            panic!("bundled Inter failed to load — corrupt FALLBACK_TTF?");
        }

        // Phase 2b Task 9 will iterate font_faces here and register
        // each one. For now the registry only contains Inter.
        let _ = font_faces; // silence unused-warnings until Task 9.

        FontRegistry { fonts, by_family }
    }

    /// Walk a resolved family chain (lowercased, quote-stripped) and
    /// return the first registered handle. Returns 0 (Inter) if the
    /// chain is empty or no name matches.
    pub fn lookup(&self, family_chain: &[String]) -> FontHandle {
        for name in family_chain {
            if let Some(&h) = self.by_family.get(name) {
                return h;
            }
        }
        0
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p quickpdf-core --lib registry_`
Expected: all 4 new `registry_*` tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/quickpdf-core/src/font.rs
git commit -m "Phase 2b Slice B: implement FontRegistry::build (Inter-only) + lookup"
```

### Task 9: Implement `@font-face` registration

**Files:**
- Modify: `crates/quickpdf-core/src/font.rs`

This task adds the actual `@font-face` → registry pipeline: descriptor extraction, src-list tokenisation, MIME accept list, base64 decode, magic-byte sniff, last-wins on dup names. Each behaviour gets a failing test, then a small implementation slice, then verification.

- [ ] **Step 1: Add a small builder helper for FontFace test fixtures**

We need to construct synthetic `FontFace` values in tests. Add this `cfg(test)`-gated helper at the bottom of the existing `tests` module in `font.rs`:

```rust

    /// Build a `FontFace` from a set of (name, value) declaration pairs
    /// — bypasses the full stylesheet parser so each test can pin one
    /// behaviour at a time. `important` is always false (irrelevant for
    /// font-face descriptors per CSS spec).
    fn fixture_face(decls: &[(&str, &str)], source_order: usize) -> FontFace {
        FontFace {
            declarations: decls
                .iter()
                .map(|(n, v)| Declaration {
                    name: n.to_string(),
                    value: v.to_string(),
                    important: false,
                })
                .collect(),
            source_order,
        }
    }

    /// Base64-encode the bundled Inter bytes so tests can build valid
    /// `data:font/ttf;base64,...` URLs without external fixtures. Inter
    /// is a real TTF and passes the magic-byte sniff.
    fn inter_data_url() -> String {
        let b64 = base64::engine::general_purpose::STANDARD.encode(FALLBACK_TTF);
        format!("data:font/ttf;base64,{b64}")
    }
```

- [ ] **Step 2: Write a failing test for single-face registration**

Append to the same `tests` module:

```rust

    #[test]
    fn registry_registers_one_valid_face() {
        let url = inter_data_url();
        let face = fixture_face(
            &[
                ("font-family", "Acme"),
                ("src", &format!("url({url})")),
            ],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        // Inter at 0, Acme at 1.
        assert_eq!(registry.fonts.len(), 2);
        assert_eq!(registry.by_family.get("acme").copied(), Some(1));
        assert_eq!(registry.lookup(&["acme".to_string()]), 1);
    }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p quickpdf-core --lib registry_registers_one_valid_face`
Expected: FAIL — `fonts.len()` is 1 (only Inter), the Acme entry was never added.

- [ ] **Step 4: Implement descriptor extraction + src list parsing + face registration**

Add this implementation block at the bottom of `crates/quickpdf-core/src/font.rs` (after the existing `impl FontRegistry`):

```rust

// ---------------------------------------------------------------------------
// Phase 2b: @font-face descriptor extraction and src-list parsing.
// ---------------------------------------------------------------------------

/// Internal: per-face data extracted from the declaration list.
struct ParsedDescriptors<'a> {
    family: String,            // lowercased, quote-stripped, trimmed
    src_value: &'a str,        // raw value of the src descriptor
}

/// Pull out the first `font-family` and `src` declarations (last-wins
/// among duplicates within a single block). Returns `None` if either
/// descriptor is missing or the family normalises to an empty string.
fn extract_descriptors(face: &FontFace) -> Option<ParsedDescriptors<'_>> {
    let mut family_raw: Option<&str> = None;
    let mut src_value: Option<&str> = None;
    for d in &face.declarations {
        match d.name.as_str() {
            "font-family" => family_raw = Some(d.value.as_str()),
            "src" => src_value = Some(d.value.as_str()),
            _ => {}
        }
    }
    let raw = family_raw?;
    let src_value = src_value?;

    // The font-family DESCRIPTOR is technically a single name, but real
    // authoring tools sometimes emit a comma list ("Acme", fallback).
    // We honour only the first comma-separated entry.
    let first = raw.split(',').next()?.trim();
    let stripped = strip_outer_quotes(first).trim();
    if stripped.is_empty() {
        return None;
    }
    Some(ParsedDescriptors {
        family: stripped.to_ascii_lowercase(),
        src_value,
    })
}

/// Strip a single pair of matching surrounding `"..."` or `'...'` quotes.
/// No-op if the input isn't quoted.
fn strip_outer_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// One entry in a `src:` list. Phase 2b only acts on `Url`; `Local` is
/// captured for symmetry but always skipped (per spec — no system probe).
#[derive(Debug, Clone, PartialEq, Eq)]
enum SrcEntry {
    Url(String),
    Local(String),
    /// Anything we couldn't classify. Caller skips.
    Unknown,
}

/// Tokenise a `src:` descriptor value into entries. The grammar is
/// comma-separated; each entry is one of:
///   - `url(<url>)` optionally followed by `format(<hint>)`
///   - `local(<name>)`
///
/// Whitespace between tokens is tolerated. `format(...)` hints are
/// captured but discarded — the registry sniffs bytes itself. Quotes
/// inside `url(...)` / `local(...)` are stripped.
fn parse_src_list(value: &str) -> Vec<SrcEntry> {
    let mut out: Vec<SrcEntry> = Vec::new();
    for piece in split_top_level_commas(value) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        // Strip an optional trailing `format(...)` so the head is purely
        // url(...) or local(...).
        let head = match split_off_format_hint(piece) {
            Some((h, _hint)) => h.trim(),
            None => piece,
        };
        if let Some(inner) = head.strip_prefix("url(").and_then(|s| s.strip_suffix(')')) {
            let url = strip_outer_quotes(inner.trim()).trim();
            out.push(SrcEntry::Url(url.to_string()));
        } else if let Some(inner) = head.strip_prefix("local(").and_then(|s| s.strip_suffix(')')) {
            let name = strip_outer_quotes(inner.trim()).trim();
            out.push(SrcEntry::Local(name.to_string()));
        } else {
            out.push(SrcEntry::Unknown);
        }
    }
    out
}

/// Split a string on top-level commas, respecting parens and quotes.
/// Used to break a `src:` descriptor into entries.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out: Vec<&str> = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0;
    let mut pos = 0;
    let mut in_quote: Option<u8> = None;
    while pos < bytes.len() {
        let b = bytes[pos];
        match in_quote {
            Some(q) => {
                if b == b'\\' && pos + 1 < bytes.len() {
                    pos += 2;
                    continue;
                }
                if b == q {
                    in_quote = None;
                }
                pos += 1;
            }
            None => {
                if b == b'"' || b == b'\'' {
                    in_quote = Some(b);
                    pos += 1;
                } else if b == b'(' {
                    depth += 1;
                    pos += 1;
                } else if b == b')' {
                    if depth > 0 {
                        depth -= 1;
                    }
                    pos += 1;
                } else if b == b',' && depth == 0 {
                    out.push(&s[start..pos]);
                    pos += 1;
                    start = pos;
                } else {
                    pos += 1;
                }
            }
        }
    }
    if start < bytes.len() {
        out.push(&s[start..]);
    }
    out
}

/// If `entry` ends with a top-level `format(...)` clause, split it off
/// and return `(head, hint_inner)`. Otherwise return `None`.
fn split_off_format_hint(entry: &str) -> Option<(&str, &str)> {
    // Search for the last top-level `format(` in the trimmed entry.
    // We only support the trailing-hint shape, which is the only one
    // CSS Fonts 4 actually defines.
    let trimmed = entry.trim_end();
    let bytes = trimmed.as_bytes();
    if !trimmed.ends_with(')') {
        return None;
    }
    // Walk backward: find the matching `format(` for the trailing `)`.
    let mut depth = 0i32;
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                depth -= 1;
                if depth == 0 {
                    // Check the preceding 6 bytes are "format" (case-insensitive).
                    if i >= 6 {
                        let kw = &trimmed[i - 6..i];
                        if kw.eq_ignore_ascii_case("format") {
                            let head = trimmed[..i - 6].trim();
                            let hint = &trimmed[i + 1..bytes.len() - 1];
                            return Some((head, hint));
                        }
                    }
                    return None;
                }
            }
            _ => {}
        }
    }
    None
}

/// Decode a single `url(data:...)` payload into raw font bytes if the
/// MIME and magic bytes both clear the gate. Returns `None` for any
/// failure mode (unsupported scheme, non-data URL, unaccepted MIME,
/// missing `;base64,` segment, malformed base64, wrong magic).
fn decode_data_url(url: &str) -> Option<Vec<u8>> {
    // 1. Must start with "data:" (case-insensitive).
    if url.len() < 5 || !url[..5].eq_ignore_ascii_case("data:") {
        return None;
    }
    let after = &url[5..];

    // 2. Find the first ';' which separates the MIME from the encoding.
    //    No ';' → malformed for our purposes.
    let semi = after.find(';')?;
    let mime = after[..semi].trim().to_ascii_lowercase();

    // 3. MIME accept list. Permissive: real authoring tools emit several
    //    variants for the same payload format.
    let mime_ok = matches!(
        mime.as_str(),
        "font/ttf"
            | "font/otf"
            | "application/font-sfnt"
            | "application/x-font-ttf"
            | "application/x-font-otf"
            | "application/octet-stream"
    );
    if !mime_ok {
        return None;
    }

    // 4. Require ";base64," (we don't support percent-encoded data URLs
    //    for binary payloads — same posture as Phase 2a's image parser).
    let rest = &after[semi..];
    let payload = rest.strip_prefix(";base64,")?;

    // 5. Decode.
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.as_bytes())
        .ok()?;

    // 6. Magic-byte sniff: TrueType (0x00010000) or OpenType/CFF ("OTTO").
    if bytes.len() < 4 {
        return None;
    }
    let magic_ok = bytes[..4] == [0x00, 0x01, 0x00, 0x00] || &bytes[..4] == b"OTTO";
    if !magic_ok {
        return None;
    }

    Some(bytes)
}

/// Walk a face's src list and load the first decodable url() entry.
/// Returns `None` if every entry was unacceptable (skipped or failed).
fn load_first_decodable_src(src_value: &str) -> Option<Vec<u8>> {
    for entry in parse_src_list(src_value) {
        if let SrcEntry::Url(url) = entry {
            if let Some(bytes) = decode_data_url(&url) {
                return Some(bytes);
            }
        }
        // SrcEntry::Local and SrcEntry::Unknown: skip, try next.
    }
    None
}
```

Now wire the per-face loop into `FontRegistry::build`. Replace the placeholder line `let _ = font_faces; // silence unused-warnings until Task 9.` with:

```rust
        // Walk faces in source order. Each one may add a new font (or
        // overwrite an existing handle, last-wins). Faces with no
        // decodable src are silently dropped.
        for face in font_faces {
            let descriptors = match extract_descriptors(face) {
                Some(d) => d,
                None => continue,
            };
            let bytes = match load_first_decodable_src(descriptors.src_value) {
                Some(b) => b,
                None => continue,
            };
            let arc_bytes: Arc<[u8]> = Arc::from(bytes);
            let krilla_font = match krilla::text::Font::new(arc_bytes.to_vec().into(), 0) {
                Some(f) => f,
                None => continue,
            };
            // Last-wins on duplicate family names: overwrite the handle.
            let new_handle = fonts.len();
            fonts.push(RegisteredFont {
                bytes: arc_bytes,
                krilla_font,
            });
            by_family.insert(descriptors.family, new_handle);
        }
```

- [ ] **Step 5: Run the single-face test**

Run: `cargo test -p quickpdf-core --lib registry_registers_one_valid_face`
Expected: PASS.

- [ ] **Step 6: Commit the registration baseline**

```bash
git add crates/quickpdf-core/src/font.rs
git commit -m "Phase 2b Slice B: register decodable @font-face entries"
```

- [ ] **Step 7: Add the full error-matrix tests**

Append to the same `tests` module in `font.rs`:

```rust

    #[test]
    fn registry_drops_face_with_no_font_family() {
        let face = fixture_face(&[("src", &format!("url({})", inter_data_url()))], 0);
        let registry = FontRegistry::build(&[face]);
        // Only Inter remains; the orphaned src is dropped.
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_drops_face_with_empty_font_family() {
        let face = fixture_face(
            &[
                ("font-family", "\"\""),
                ("src", &format!("url({})", inter_data_url())),
            ],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_drops_face_with_no_src() {
        let face = fixture_face(&[("font-family", "Acme")], 0);
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
        assert!(registry.by_family.get("acme").is_none());
    }

    #[test]
    fn registry_drops_face_with_only_local_src() {
        let face = fixture_face(
            &[("font-family", "Acme"), ("src", "local(\"Arial\")")],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
        assert!(registry.by_family.get("acme").is_none());
    }

    #[test]
    fn registry_drops_face_with_only_http_src() {
        let face = fixture_face(
            &[
                ("font-family", "Acme"),
                ("src", "url(https://example.com/Acme.woff2)"),
            ],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_drops_face_with_only_woff2_data_url() {
        // data:font/woff2 is not in the accept list. Even if it were,
        // the magic-byte sniff would reject WOFF2's "wOF2" header.
        let url = "data:font/woff2;base64,d09GMgABAAAAAAAA";
        let face = fixture_face(
            &[("font-family", "Acme"), ("src", &format!("url({url})"))],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_walks_multi_src_to_first_acceptable() {
        // First entry: woff2 (rejected). Second: ttf (accepted). The
        // registry should pick the second.
        let url_ttf = inter_data_url();
        let value = format!(
            "url(data:font/woff2;base64,d09GMg==) format(\"woff2\"), \
             url({url_ttf}) format(\"truetype\")"
        );
        let face = fixture_face(&[("font-family", "Acme"), ("src", &value)], 0);
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 2);
        assert_eq!(registry.by_family.get("acme").copied(), Some(1));
    }

    #[test]
    fn registry_drops_face_with_base64_garbage() {
        let face = fixture_face(
            &[
                ("font-family", "Acme"),
                ("src", "url(data:font/ttf;base64,!!!notbase64!!!)"),
            ],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_drops_face_with_wrong_magic_in_ttf_mime() {
        // Valid base64 of bytes that don't pass the magic sniff (random
        // 4-byte payload). MIME claims TTF but bytes aren't TrueType.
        let payload = base64::engine::general_purpose::STANDARD.encode([0xDE, 0xAD, 0xBE, 0xEF]);
        let url = format!("data:font/ttf;base64,{payload}");
        let face = fixture_face(
            &[("font-family", "Acme"), ("src", &format!("url({url})"))],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_accepts_octet_stream_with_ttf_magic() {
        // application/octet-stream is accepted only when the magic
        // bytes confirm the payload is TTF/OTF.
        let b64 = base64::engine::general_purpose::STANDARD.encode(FALLBACK_TTF);
        let url = format!("data:application/octet-stream;base64,{b64}");
        let face = fixture_face(
            &[("font-family", "Acme"), ("src", &format!("url({url})"))],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 2);
        assert_eq!(registry.by_family.get("acme").copied(), Some(1));
    }

    #[test]
    fn registry_duplicate_family_last_wins() {
        // Two @font-face blocks both declare "Acme"; the second registers
        // a new handle and overwrites the first in by_family.
        let url = inter_data_url();
        let f1 = fixture_face(
            &[("font-family", "Acme"), ("src", &format!("url({url})"))],
            0,
        );
        let f2 = fixture_face(
            &[("font-family", "Acme"), ("src", &format!("url({url})"))],
            1,
        );
        let registry = FontRegistry::build(&[f1, f2]);
        // Both faces register; by_family points at the second (handle 2).
        assert_eq!(registry.fonts.len(), 3);
        assert_eq!(registry.by_family.get("acme").copied(), Some(2));
    }

    #[test]
    fn registry_lookup_walks_chain_left_to_right() {
        // Build a registry with two registered families; lookup picks
        // whichever appears first in the chain.
        let url = inter_data_url();
        let f1 = fixture_face(
            &[("font-family", "Alpha"), ("src", &format!("url({url})"))],
            0,
        );
        let f2 = fixture_face(
            &[("font-family", "Beta"), ("src", &format!("url({url})"))],
            1,
        );
        let registry = FontRegistry::build(&[f1, f2]);
        assert_eq!(
            registry.lookup(&["alpha".to_string(), "beta".to_string()]),
            registry.by_family.get("alpha").copied().unwrap()
        );
        assert_eq!(
            registry.lookup(&["unknown".to_string(), "beta".to_string()]),
            registry.by_family.get("beta").copied().unwrap()
        );
        assert_eq!(
            registry.lookup(&["unknown1".to_string(), "unknown2".to_string()]),
            0
        );
    }
```

- [ ] **Step 8: Run all the new tests**

Run: `cargo test -p quickpdf-core --lib registry_`
Expected: all 14 `registry_*` tests PASS (the 4 from Task 8 + the 10 new ones above).

- [ ] **Step 9: Run the full font.rs test set**

Run: `cargo test -p quickpdf-core --lib font::`
Expected: the original `fallback_font_is_a_real_font` test still PASSes; all new `registry_*` tests PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/quickpdf-core/src/font.rs
git commit -m "Phase 2b Slice B: error-matrix tests for FontRegistry::build"
```

### Task 10: Slice B green-bar gate + finalisation

**Files:** none modified — purely verification.

- [ ] **Step 1: Type-check**

Run: `cargo check -p quickpdf-core`
Expected: builds clean, no warnings.

- [ ] **Step 2: Run the full Rust suite**

Run: `cargo test -p quickpdf-core --lib`
Expected: all tests PASS — Slice A's ~198 + Slice B's ~14 new = ~212.

No commit at this step. Slice B is complete. The Integrator phase consumes `FontRegistry` from `lib.rs`.

---

## Phase 3 — Slice C: `font-family` cascade

Constraint: Slice C only edits `crates/quickpdf-core/src/style/mod.rs` and `crates/quickpdf-core/src/style/cascade.rs`. **Do not** touch `parse.rs`, `font.rs`, `sheet.rs`, or `lib.rs`. Slice C is independent of Slices A and B and may run in parallel with either. Green-bar gate: `cargo check -p quickpdf-core`.

### Task 11: Add `font_family` to `BlockStyle`

**Files:**
- Modify: `crates/quickpdf-core/src/style/mod.rs` (the `BlockStyle` struct definition + `DEFAULT` literal, lines ~123-185)

- [ ] **Step 1: Write a failing test for the default**

Append to the existing `#[cfg(test)] mod tests` block in `crates/quickpdf-core/src/style/mod.rs`:

```rust

    // ---- Phase 2b Slice C: font-family cascade. ----

    #[test]
    fn default_block_style_has_no_font_family() {
        assert!(BlockStyle::DEFAULT.font_family.is_none());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p quickpdf-core --lib default_block_style_has_no_font_family`
Expected: FAIL with "no field `font_family` on `BlockStyle`".

- [ ] **Step 3: Add the field**

In `crates/quickpdf-core/src/style/mod.rs`, find the `BlockStyle` struct definition (around lines 123-164). Add a new field at the bottom, just before the closing `}`:

```rust
    /// Resolved `font-family` fallback list. `None` means "no
    /// author-set value"; the planner uses bundled Inter at registry
    /// index 0. Items are lowercased, quote-stripped, with generic
    /// keywords (sans-serif/serif/monospace/...) dropped at parse
    /// time. Inherited per CSS spec.
    pub font_family: Option<Vec<String>>,
```

- [ ] **Step 4: Update `BlockStyle::DEFAULT`**

Find the `DEFAULT` const (around lines 167-184). Add the new field at the bottom of the struct literal:

```rust
        font_family: None,
```

- [ ] **Step 5: Verify it compiles and the new test passes**

Run: `cargo test -p quickpdf-core --lib default_block_style_has_no_font_family`
Expected: PASS.

Run: `cargo check -p quickpdf-core`
Expected: clean. (Existing tests that build `BlockStyle` literals via `..BlockStyle::DEFAULT` continue working because the new field is in DEFAULT.)

- [ ] **Step 6: Commit**

```bash
git add crates/quickpdf-core/src/style/mod.rs
git commit -m "Phase 2b Slice C: add font_family field to BlockStyle"
```

### Task 12: Implement `font-family` value parser

**Files:**
- Modify: `crates/quickpdf-core/src/style/cascade.rs` (add a parser + apply arm)

- [ ] **Step 1: Write failing tests for the value parser**

Append to the existing `#[cfg(test)] mod tests` block in `crates/quickpdf-core/src/style/cascade.rs`:

```rust

    // ---- Phase 2b Slice C: font-family parsing. ----

    #[test]
    fn font_family_single_unquoted_name() {
        assert_eq!(
            parse_font_family("Acme"),
            Some(vec!["acme".to_string()])
        );
    }

    #[test]
    fn font_family_quoted_name_strips_quotes() {
        assert_eq!(
            parse_font_family("\"Acme Sans\""),
            Some(vec!["acme sans".to_string()])
        );
        assert_eq!(
            parse_font_family("'Acme Sans'"),
            Some(vec!["acme sans".to_string()])
        );
    }

    #[test]
    fn font_family_comma_list_preserves_order() {
        assert_eq!(
            parse_font_family("\"Acme\", Helvetica, Arial"),
            Some(vec![
                "acme".to_string(),
                "helvetica".to_string(),
                "arial".to_string(),
            ])
        );
    }

    #[test]
    fn font_family_drops_generic_keywords() {
        // sans-serif / serif / monospace / etc. are dropped (we have no
        // concrete font for any of them in 2b).
        assert_eq!(
            parse_font_family("Acme, sans-serif"),
            Some(vec!["acme".to_string()])
        );
        // All-generic chain → None (empty list after drops).
        assert_eq!(parse_font_family("sans-serif, serif"), None);
    }

    #[test]
    fn font_family_empty_value_returns_none() {
        assert_eq!(parse_font_family(""), None);
        assert_eq!(parse_font_family("  "), None);
        assert_eq!(parse_font_family(","), None);
    }

    #[test]
    fn font_family_lowercases_for_lookup() {
        assert_eq!(
            parse_font_family("ACME, Helvetica"),
            Some(vec!["acme".to_string(), "helvetica".to_string()])
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p quickpdf-core --lib font_family_`
Expected: FAIL with "cannot find function `parse_font_family`".

- [ ] **Step 3: Implement the parser**

In `crates/quickpdf-core/src/style/cascade.rs`, add this function near the top of the module (after the existing `Color`/`TextAlign`/`BorderStyle` definitions, before `parse_value`). If the file already has a logical "value parsers" section, place it there:

```rust

/// Phase 2b: parse a CSS `font-family` value into a normalised fallback
/// list. Tokenises on top-level commas (respecting `"..."` and
/// `'...'`), strips surrounding quotes, lowercases for symmetric
/// lookup against `FontRegistry::by_family`, and drops generic family
/// keywords (`serif`, `sans-serif`, `monospace`, …) for which Phase 2b
/// has no concrete font. Returns `None` if the resulting list is
/// empty (so `cascade::inherit` does not overwrite an inherited value).
pub fn parse_font_family(value: &str) -> Option<Vec<String>> {
    const GENERIC_KEYWORDS: &[&str] = &[
        "serif",
        "sans-serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
        "ui-serif",
        "ui-sans-serif",
        "ui-monospace",
        "ui-rounded",
        "emoji",
        "math",
        "fangsong",
    ];

    let mut out: Vec<String> = Vec::new();
    for piece in split_top_level_commas(value) {
        let trimmed = piece.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Strip a single matching pair of surrounding quotes.
        let stripped = strip_outer_quotes(trimmed).trim();
        if stripped.is_empty() {
            continue;
        }
        let lower = stripped.to_ascii_lowercase();
        if GENERIC_KEYWORDS.contains(&lower.as_str()) {
            continue;
        }
        out.push(lower);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Strip a single pair of matching surrounding `"..."` or `'...'`
/// quotes. No-op if the input isn't quoted.
fn strip_outer_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Split a comma-separated CSS value at top-level commas, respecting
/// quotes. Used by `parse_font_family`.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out: Vec<&str> = Vec::new();
    let mut start = 0;
    let mut pos = 0;
    let mut in_quote: Option<u8> = None;
    while pos < bytes.len() {
        let b = bytes[pos];
        match in_quote {
            Some(q) => {
                if b == b'\\' && pos + 1 < bytes.len() {
                    pos += 2;
                    continue;
                }
                if b == q {
                    in_quote = None;
                }
                pos += 1;
            }
            None => {
                if b == b'"' || b == b'\'' {
                    in_quote = Some(b);
                    pos += 1;
                } else if b == b',' {
                    out.push(&s[start..pos]);
                    pos += 1;
                    start = pos;
                } else {
                    pos += 1;
                }
            }
        }
    }
    if start < bytes.len() {
        out.push(&s[start..]);
    }
    out
}
```

- [ ] **Step 4: Run the parser tests**

Run: `cargo test -p quickpdf-core --lib font_family_`
Expected: all 6 `font_family_*` tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/quickpdf-core/src/style/cascade.rs
git commit -m "Phase 2b Slice C: implement parse_font_family value parser"
```

### Task 13: Wire `font-family` into `apply_declarations` and `inherit`

**Files:**
- Modify: `crates/quickpdf-core/src/style/cascade.rs` (the `apply_declaration` arm + `inherit` body)

- [ ] **Step 1: Write failing tests for cascade integration**

Append to the same `tests` module in `cascade.rs`:

```rust

    /// Helper: apply a single declaration to BlockStyle::DEFAULT and
    /// return the resulting style. (Mirrors the helpers used by the
    /// existing `*_value_parsers_apply` tests.)
    fn apply_one(name: &str, value: &str) -> BlockStyle {
        let decl = crate::style::sheet::Declaration {
            name: name.to_string(),
            value: value.to_string(),
            important: false,
        };
        apply_declarations(BlockStyle::DEFAULT, &[decl])
    }

    #[test]
    fn font_family_apply_sets_block_style() {
        let style = apply_one("font-family", "\"Acme\", Helvetica");
        assert_eq!(
            style.font_family,
            Some(vec!["acme".to_string(), "helvetica".to_string()])
        );
    }

    #[test]
    fn font_family_apply_empty_value_keeps_none() {
        let style = apply_one("font-family", "");
        assert!(style.font_family.is_none());
    }

    #[test]
    fn font_family_inherits_when_child_has_none() {
        let mut parent = BlockStyle::DEFAULT;
        parent.font_family = Some(vec!["acme".to_string()]);
        let child = BlockStyle::DEFAULT;
        let resolved = inherit(&parent, child);
        assert_eq!(resolved.font_family, Some(vec!["acme".to_string()]));
    }

    #[test]
    fn font_family_child_value_wins_over_parent() {
        let mut parent = BlockStyle::DEFAULT;
        parent.font_family = Some(vec!["acme".to_string()]);
        let mut child = BlockStyle::DEFAULT;
        child.font_family = Some(vec!["beta".to_string()]);
        let resolved = inherit(&parent, child);
        assert_eq!(resolved.font_family, Some(vec!["beta".to_string()]));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p quickpdf-core --lib font_family_apply font_family_inherits font_family_child`
Expected: FAIL — `font_family_apply_sets_block_style` will see `None` (the apply path doesn't recognise `font-family` yet); the inherit tests will see whichever sentinel-comparison logic the existing `inherit` arm uses for `font_family` (likely `None` always).

- [ ] **Step 3: Add the `apply_declaration` arm**

In `crates/quickpdf-core/src/style/cascade.rs`, find `apply_declarations`. It dispatches to per-property arms based on `decl.name`. Locate the existing arm chain (look for lines like `"color" => { ... }`, `"background-color" => { ... }`, etc.). Add a new arm for `"font-family"`. The exact insertion site depends on the file's current structure, but the arm body is:

```rust
            "font-family" => {
                if let Some(chain) = parse_font_family(&decl.value) {
                    style.font_family = Some(chain);
                }
                // Empty/all-generic value: leave inherited value intact
                // by NOT writing None.
            }
```

If `apply_declarations` uses an indirect dispatch via `parse_value` + a `ParsedValue` enum (like the existing `LengthEm`/`Color`/`Weight` arms do), instead extend `parse_value` to recognise `font-family` and return a new `ParsedValue::FontFamily(Vec<String>)` variant, then handle that variant in `apply_declarations`. The choice between direct arm and parsed-value indirection is structural — match what the rest of the file does for declarations whose value isn't a single typed primitive.

In either shape, the rule is: parse the value via `parse_font_family`; on `Some(chain)` set `style.font_family = Some(chain)`; on `None` do nothing.

- [ ] **Step 4: Update `inherit` to propagate `font_family`**

Find the `inherit` function in `crates/quickpdf-core/src/style/cascade.rs` (around lines 433-470). Inside the `BlockStyle { ... }` literal, add a new field assignment before the closing `}`:

```rust
        // font-family is inherited per CSS spec. For Option, the rule is
        // "child value wins; fall back to parent if child has none."
        // This differs from f32/Color arms above (which sentinel-compare
        // against DEFAULT) — for an Option, None is the natural sentinel.
        font_family: child
            .font_family
            .clone()
            .or_else(|| parent.font_family.clone()),
```

Note: this assumes `child` is owned (the `inherit` signature is `fn inherit(parent: &BlockStyle, child: BlockStyle) -> BlockStyle` and the function moves `child`). The `.clone()` on `child.font_family` is needed because the field is read elsewhere in the struct literal (or to keep the move semantics simple); review the surrounding fields for whether `child` is consumed wholesale or partially-moved. If `child.font_family` can be moved here without breaking neighbors, drop the `.clone()`.

- [ ] **Step 5: Run all the new tests**

Run: `cargo test -p quickpdf-core --lib font_family_`
Expected: all 10 `font_family_*` tests PASS (the 6 parser tests from Task 12 + the 4 cascade tests above).

- [ ] **Step 6: Run the full cascade.rs test set + style/mod.rs**

Run: `cargo test -p quickpdf-core --lib cascade:: style::`
Expected: every cascade and style/mod test PASSes.

- [ ] **Step 7: Commit**

```bash
git add crates/quickpdf-core/src/style/cascade.rs
git commit -m "Phase 2b Slice C: wire font-family into apply_declarations + inherit"
```

### Task 14: Slice C green-bar gate + finalisation

**Files:** none modified — purely verification.

- [ ] **Step 1: Type-check**

Run: `cargo check -p quickpdf-core`
Expected: clean.

- [ ] **Step 2: Full Rust suite**

Run: `cargo test -p quickpdf-core --lib`
Expected: all tests PASS — Slice A's ~198 + Slice B's ~14 + Slice C's ~10 = ~222.

No commit at this step. Slice C is complete. The Integrator phase consumes the new `font_family` field from `lib.rs`.

---

## Phase 4 — Integrator: `lib.rs` plumbing

The integrator runs after Slices A, B, and C have all merged. It threads the `FontRegistry` through `html_to_pdf` and `plan_pages_styled`, stamps a `font_handle` on every `PlacedLine`, swaps fonts at emit time, and adds the integration-level Rust tests.

### Task 15: Add `font_handle` to `PlacedLine`

**Files:**
- Modify: `crates/quickpdf-core/src/lib.rs` (the `PlacedLine` struct, around lines 86-92)

- [ ] **Step 1: Add the field**

In `crates/quickpdf-core/src/lib.rs`, find the `PlacedLine` struct (around lines 86-92). Add a new field:

```rust
#[derive(Debug, Clone)]
struct PlacedLine {
    y: f32,
    x: f32,
    font_size_pt: f32,
    text: String,
    color: Color,
    /// Phase 2b: which registered font to use at emit time. Always
    /// `0` for the bundled Inter fallback; non-zero for any
    /// `@font-face`-resolved family.
    font_handle: crate::font::FontHandle,
}
```

- [ ] **Step 2: Confirm it doesn't compile yet (planner doesn't set the field)**

Run: `cargo check -p quickpdf-core`
Expected: errors about `PlacedLine { ... }` literals missing `font_handle`. The errors point at every site in `plan_pages_styled` and `place_image_block` that constructs a `PlacedLine`. Tasks 16 and 17 fix them.

No commit at this step (broken intermediate state).

### Task 16: Build the registry and thread it through `plan_pages_styled`

**Files:**
- Modify: `crates/quickpdf-core/src/lib.rs` (the `html_to_pdf` body around lines 142-230, the `plan_pages_styled` signature + body around lines 298-443)

- [ ] **Step 1: Rewire `html_to_pdf` to use `Document::stylesheet()` and build the registry**

In `crates/quickpdf-core/src/lib.rs`, find `html_to_pdf` (around line 142). Locate the existing parse + setup block that currently looks roughly like:

```rust
    let parsed = parse::Document::parse(html);
    let blocks = parsed.blocks();
    let user_rules = parsed.user_stylesheet();
    let inline_owned = parsed.inline_styles();
    let inline_map: style::InlineStyles<'_> = inline_owned
        .iter()
        .map(|(id, decls)| (*id, decls.as_slice()))
        .collect();

    let font = Font::new(font::FALLBACK_TTF.to_vec().into(), 0)
        .ok_or_else(|| Error::Pdf("could not load embedded fallback font".into()))?;
```

Replace with:

```rust
    let parsed = parse::Document::parse(html);
    let blocks = parsed.blocks();
    let stylesheet = parsed.stylesheet();
    let inline_owned = parsed.inline_styles();
    let inline_map: style::InlineStyles<'_> = inline_owned
        .iter()
        .map(|(id, decls)| (*id, decls.as_slice()))
        .collect();

    // Phase 2b: build the font registry once per render. Inter is
    // pre-registered at handle 0; @font-face blocks register additional
    // faces under their lowercased family names.
    let registry = font::FontRegistry::build(&stylesheet.font_faces);
```

(The standalone `let font = Font::new(...)` is removed. The emit loop now reads from `registry.fonts`.)

- [ ] **Step 2: Update the call to `plan_pages_styled`**

In the same function, find the existing call:

```rust
    let pages = plan_pages_styled(
        &parsed,
        &blocks,
        &user_rules,
        &inline_map,
        content_width,
        MARGIN_PT,
        bottom_limit,
    )?;
```

Replace with:

```rust
    let pages = plan_pages_styled(
        &parsed,
        &blocks,
        &stylesheet.rules,
        &inline_map,
        &registry,
        content_width,
        MARGIN_PT,
        bottom_limit,
    )?;
```

- [ ] **Step 3: Update the `plan_pages_styled` signature**

Find `plan_pages_styled` (around line 298). Add a `registry: &font::FontRegistry` parameter between `inline` and `content_width`:

```rust
fn plan_pages_styled(
    doc: &parse::Document,
    blocks: &[parse::Block],
    user_rules: &[style::sheet::Rule],
    inline: &style::InlineStyles<'_>,
    registry: &font::FontRegistry,
    content_width: f32,
    left_margin: f32,
    bottom_limit: f32,
) -> Result<Vec<PagePlan>, Error> {
```

- [ ] **Step 4: Resolve the font handle and stamp it on each PlacedLine in the text-block path**

In `plan_pages_styled`'s `Block::Text(t)` path (the bulk of the function body), after the `let style = match doc.element_for_block(block) { ... }` line and before the `let metrics = text::TextMetrics::new(...)` line, insert:

```rust
        let font_handle = registry.lookup(
            style.font_family.as_deref().unwrap_or(&[]),
        );
        let font_bytes: &[u8] = &registry.fonts[font_handle].bytes;
```

Then change the `text::TextMetrics::new(font::FALLBACK_TTF, font_size)` call to use `font_bytes`:

```rust
        let metrics = text::TextMetrics::new(font_bytes, font_size)
            .ok_or_else(|| Error::Pdf("could not measure font at requested size".into()))?;
```

Find every `PlacedLine { ... }` constructor in the text path (there are typically two: one inside the paint-as-unit branch and one inside the streaming branch). Add `font_handle,` to each literal:

```rust
                current.lines.push(PlacedLine {
                    y: ...,
                    x: ...,
                    font_size_pt: font_size,
                    text: line,
                    color: style.color,
                    font_handle, // NEW
                });
```

- [ ] **Step 5: Update `place_image_block`'s alt-text path**

In `crates/quickpdf-core/src/lib.rs`, find `place_image_block` (around line 457). Add `registry: &font::FontRegistry` to its signature, propagating from the call site in `plan_pages_styled`. The signature becomes:

```rust
#[allow(clippy::too_many_arguments)]
fn place_image_block(
    doc: &parse::Document,
    block: &parse::Block,
    img_block: &parse::ImageBlock,
    user_rules: &[style::sheet::Rule],
    inline: &style::InlineStyles<'_>,
    registry: &font::FontRegistry,
    left_margin: f32,
    content_width: f32,
    bottom_limit: f32,
    page_content_height: f32,
    pages: &mut Vec<PagePlan>,
    current: &mut PagePlan,
    cursor_y: &mut Option<f32>,
) -> Result<(), Error> {
```

Update the call site in `plan_pages_styled`:

```rust
            parse::Block::Image(img_block) => {
                place_image_block(
                    doc, block, img_block, user_rules, inline, &registry,
                    left_margin, content_width, bottom_limit, page_content_height,
                    &mut pages, &mut current, &mut cursor_y,
                )?;
                continue;
            }
```

Inside `place_image_block`'s alt-text fallback branch (the `match krilla_img { Some(i) => i, None => { ... } }` arm), resolve the font handle from the cascade and stamp it on the synthetic PlacedLines:

```rust
        None => {
            // Alt fallback. Emit alt text as a synthetic line at the current
            // cursor position using the same text-flow logic the text path
            // would use for an anonymous paragraph at default style.
            if let Some(alt) = img_block.alt.as_deref().filter(|s| !s.is_empty()) {
                let font_handle = registry.lookup(
                    style.font_family.as_deref().unwrap_or(&[]),
                );
                let font_bytes: &[u8] = &registry.fonts[font_handle].bytes;
                let metrics = text::TextMetrics::new(font_bytes, font_size)
                    .ok_or_else(|| Error::Pdf("fallback metrics".into()))?;
                let lines = text::wrap_lines(&metrics, alt, content_width);
                if lines.is_empty() {
                    return Ok(());
                }
                if cursor_y.is_none() {
                    *cursor_y = Some(MARGIN_PT + line_height);
                }
                for line in lines {
                    let y = cursor_y.unwrap();
                    if y > bottom_limit {
                        pages.push(std::mem::take(current));
                        *cursor_y = Some(MARGIN_PT + line_height);
                    }
                    let final_y = cursor_y.unwrap();
                    current.lines.push(PlacedLine {
                        y: final_y,
                        x: left_margin,
                        font_size_pt: font_size,
                        text: line,
                        color: style.color,
                        font_handle, // NEW
                    });
                    *cursor_y = Some(final_y + line_height);
                }
            }
            return Ok(());
        }
```

- [ ] **Step 6: Update the emit loop to switch fonts on handle change**

Back in `html_to_pdf`, find the per-page emit loop (around lines 173-220). The current text-emit block uses a single `font` and only switches color:

```rust
            let mut current_color: Option<Color> = None;
            for line in &page_plan.lines {
                if current_color != Some(line.color) {
                    let fill = Fill { ... };
                    surface.set_fill(Some(fill));
                    current_color = Some(line.color);
                }
                surface.draw_text(
                    Point::from_xy(line.x, line.y),
                    font.clone(),
                    line.font_size_pt,
                    &line.text,
                    false,
                    TextDirection::Auto,
                );
            }
```

Replace with a font-aware version:

```rust
            let mut current_color: Option<Color> = None;
            let mut current_handle: Option<crate::font::FontHandle> = None;
            for line in &page_plan.lines {
                if current_color != Some(line.color) {
                    let fill = Fill {
                        paint: krilla_rgb::Color::new(
                            line.color.r,
                            line.color.g,
                            line.color.b,
                        )
                        .into(),
                        ..Fill::default()
                    };
                    surface.set_fill(Some(fill));
                    current_color = Some(line.color);
                }
                if current_handle != Some(line.font_handle) {
                    current_handle = Some(line.font_handle);
                }
                let font_for_line = registry.fonts[line.font_handle].krilla_font.clone();
                surface.draw_text(
                    Point::from_xy(line.x, line.y),
                    font_for_line,
                    line.font_size_pt,
                    &line.text,
                    false,
                    TextDirection::Auto,
                );
            }
```

(Note: `krilla::text::Font` is a cheap-to-clone handle type backed by an `Arc`, so `.clone()` per draw_text call is fine. The `current_handle` tracking is currently unused beyond the assignment but is left in place as a hook for any future "skip work on no-change" optimisation symmetric with `current_color`.)

- [ ] **Step 7: Verify the integrator's wiring compiles**

Run: `cargo check -p quickpdf-core`
Expected: clean. If errors persist, they're typically: (a) a `PlacedLine` literal somewhere we missed — search `PlacedLine {` and add `font_handle` to every constructor; (b) the `Font` import at the top of lib.rs is now unused — remove it from the `use krilla::text::{Font, TextDirection};` line, leaving just `TextDirection`.

- [ ] **Step 8: Run the full Rust suite**

Run: `cargo test -p quickpdf-core --lib`
Expected: all tests PASS — Slices A+B+C combined plus the integrator's not-yet-added integration tests = currently ~222.

- [ ] **Step 9: Commit**

```bash
git add crates/quickpdf-core/src/lib.rs
git commit -m "Phase 2b integrator: thread FontRegistry through planner + emitter"
```

### Task 17: Add Rust integration tests for the planner

**Files:**
- Modify: `crates/quickpdf-core/src/lib.rs` (the `#[cfg(test)] mod tests` block)

- [ ] **Step 1: Update the existing `plan_full` helper to satisfy the new signature**

Find `plan_full` in `crates/quickpdf-core/src/lib.rs` (around line 641). It currently calls `plan_pages_styled` without a registry. Update the body:

```rust
    fn plan_full(html: &str) -> Vec<PagePlan> {
        let doc = parse::Document::parse(html);
        let blocks = doc.blocks();
        let stylesheet = doc.stylesheet();
        let inline_owned = doc.inline_styles();
        let inline_map: style::InlineStyles<'_> = inline_owned
            .iter()
            .map(|(id, decls)| (*id, decls.as_slice()))
            .collect();
        let registry = font::FontRegistry::build(&stylesheet.font_faces);
        plan_pages_styled(&doc, &blocks, &stylesheet.rules, &inline_map, &registry, 500.0, 36.0, 800.0).unwrap()
    }
```

- [ ] **Step 2: Add the integration tests**

Append to the same `tests` module in `lib.rs`:

```rust

    // ---- Phase 2b integrator: font handle threading. ----

    /// Helper: base64-encode the bundled Inter so a test HTML can carry
    /// a valid `data:font/ttf;base64,...` URL inside an @font-face block.
    fn inter_data_url() -> String {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(font::FALLBACK_TTF);
        format!("data:font/ttf;base64,{b64}")
    }

    #[test]
    fn no_font_face_lines_use_handle_zero() {
        let pages = plan_full("<p>x</p>");
        assert_eq!(pages.len(), 1);
        let line = &pages[0].lines[0];
        assert_eq!(line.font_handle, 0, "default font handle should be 0 (Inter)");
    }

    #[test]
    fn font_family_with_no_face_falls_back_to_handle_zero() {
        // Cascade resolves font-family but no @font-face registered "Acme",
        // so the registry returns 0.
        let pages = plan_full(r#"<p style="font-family: Acme">x</p>"#);
        let line = &pages[0].lines[0];
        assert_eq!(line.font_handle, 0);
    }

    #[test]
    fn font_face_resolves_to_non_zero_handle() {
        let url = inter_data_url();
        let html = format!(
            r#"<style>@font-face {{ font-family: "Acme"; src: url({url}); }}</style>
<p style="font-family: Acme">x</p>"#
        );
        let pages = plan_full(&html);
        let line = &pages[0].lines[0];
        assert_ne!(line.font_handle, 0, "expected Acme to resolve to a non-zero handle");
    }

    #[test]
    fn font_family_inherits_to_descendants() {
        // section sets font-family Acme; the inner p has no font-family,
        // so it inherits and resolves to the same non-zero handle.
        let url = inter_data_url();
        let html = format!(
            r#"<style>@font-face {{ font-family: "Acme"; src: url({url}); }}
                section {{ font-family: Acme; }}</style>
<section><p>nested</p></section>"#
        );
        let pages = plan_full(&html);
        let line = pages[0].lines.iter().find(|l| l.text == "nested").unwrap();
        assert_ne!(line.font_handle, 0);
    }

    #[test]
    fn font_family_chain_picks_first_registered() {
        // Two @font-face blocks (Alpha and Beta). A paragraph with
        // font-family: Beta, Alpha should pick Beta (first in the chain
        // and registered). Switching the chain order should pick Alpha.
        let url = inter_data_url();
        let html = format!(
            r#"<style>
                @font-face {{ font-family: "Alpha"; src: url({url}); }}
                @font-face {{ font-family: "Beta"; src: url({url}); }}
            </style>
            <p style="font-family: Beta, Alpha">first</p>
            <p style="font-family: Alpha, Beta">second</p>"#
        );
        let pages = plan_full(&html);
        let first = pages[0].lines.iter().find(|l| l.text == "first").unwrap();
        let second = pages[0].lines.iter().find(|l| l.text == "second").unwrap();
        // Both handles are non-zero; the two paragraphs use different ones.
        assert_ne!(first.font_handle, 0);
        assert_ne!(second.font_handle, 0);
        assert_ne!(first.font_handle, second.font_handle, "chain order should select different handles");
    }
```

(These tests are appended *inside* the existing `#[cfg(test)] mod tests { ... }` block in `lib.rs` — do not add an extra closing brace.)

- [ ] **Step 3: Run the new tests**

Run: `cargo test -p quickpdf-core --lib font_face_resolves font_family_with font_family_inherits font_family_chain no_font_face`
Expected: all 5 tests PASS.

- [ ] **Step 4: Run the full Rust suite**

Run: `cargo test -p quickpdf-core --lib`
Expected: all tests PASS — ~227 total (Slices' ~222 + integrator's 5).

- [ ] **Step 5: Commit**

```bash
git add crates/quickpdf-core/src/lib.rs
git commit -m "Phase 2b integrator: Rust integration tests for font handle routing"
```

---

## Phase 5 — Python integration tests

The Phase 2a postmortem (commits `cbf5dfc`, `e348777`) documented that Rust unit tests can pass while real PDF rendering is broken because krilla decodes lazily at emit time. Phase 2b's equivalent risk: the registry might validate font bytes at `Font::new` time (or might not), and the actual draw_text call could fail or emit malformed glyph data. End-to-end Python tests catch this.

### Task 18: Add Python tests for `@font-face` rendering

**Files:**
- Modify: `tests/test_render.py`

- [ ] **Step 1: Rebuild the wheel with the integrator's changes**

Run: `.venv/Scripts/maturin.exe develop --release`
Expected: builds clean, produces `python/quickpdf/_native.pyd`.

- [ ] **Step 2: Inspect the existing test file's helpers**

Open `tests/test_render.py`. Locate the existing helpers (`_pdf_text`, `_pdf_content_streams`, etc., introduced in Phase 2a). The new tests reuse these, plus a new helper that emits a base64'd copy of the bundled Inter so Python doesn't need to ship its own font bytes.

- [ ] **Step 3: Add the helper for inlining a font as a data URL**

Append to `tests/test_render.py` (in the existing helpers section, before the test functions):

```python
def _inter_data_url() -> str:
    """Return a `data:font/ttf;base64,...` URL using the bundled Inter
    bytes. quickpdf doesn't expose the font bytes through its Python
    API, so this helper reads the source-tree fixture directly."""
    import base64
    # quickpdf-core embeds Inter via include_bytes! at build time. The
    # wheel doesn't redistribute the .ttf file, so we read it from the
    # source tree (this test must run from a checkout, not from a pure
    # pip install of the wheel — same constraint as the rest of
    # tests/test_render.py).
    src = (
        Path(__file__).resolve().parent.parent
        / "crates" / "quickpdf-core" / "assets" / "fonts" / "Inter-Regular.ttf"
    )
    raw = src.read_bytes()
    return "data:font/ttf;base64," + base64.b64encode(raw).decode("ascii")
```

If `Path` and `Path(__file__)` are not already imported at the top of the file, add `from pathlib import Path` to the existing imports.

- [ ] **Step 4: Add the integration tests**

Append to `tests/test_render.py`:

```python
def test_pdf_font_face_renders_paragraph():
    # Smoke: declaring an @font-face block and using it on a <p> must
    # produce a valid PDF without raising.
    url = _inter_data_url()
    html = (
        f'<style>@font-face {{ font-family: "Acme"; src: url({url}); }}</style>'
        '<p style="font-family: Acme">hello</p>'
    )
    pdf = quickpdf.html_to_pdf(html)
    assert pdf[:5] == b"%PDF-"
    text = _pdf_text(pdf)
    assert "hello" in text


def test_pdf_font_face_with_no_match_falls_back_silently():
    # font-family on a paragraph but no matching @font-face → silent
    # fallback to Inter; rendering still produces a valid PDF.
    html = '<p style="font-family: NotRegistered">hello</p>'
    pdf = quickpdf.html_to_pdf(html)
    assert pdf[:5] == b"%PDF-"
    text = _pdf_text(pdf)
    assert "hello" in text


def test_pdf_font_face_with_broken_payload_falls_back():
    # Garbage base64 inside an otherwise well-formed @font-face block.
    # The face is silently dropped; the paragraph still renders.
    html = (
        '<style>@font-face { font-family: "Acme"; '
        'src: url(data:font/ttf;base64,!!!notbase64!!!); }</style>'
        '<p style="font-family: Acme">resilient</p>'
    )
    pdf = quickpdf.html_to_pdf(html)
    assert pdf[:5] == b"%PDF-"
    text = _pdf_text(pdf)
    assert "resilient" in text


def test_pdf_font_face_with_woff2_src_drops_face():
    # data:font/woff2 is not in the accept list. The face is dropped;
    # font-family: Acme cascades to Inter; PDF renders fine.
    html = (
        '<style>@font-face { font-family: "Acme"; '
        'src: url(data:font/woff2;base64,d09GMg==); }</style>'
        '<p style="font-family: Acme">woff2 dropped</p>'
    )
    pdf = quickpdf.html_to_pdf(html)
    assert pdf[:5] == b"%PDF-"
    text = _pdf_text(pdf)
    assert "woff2 dropped" in text


def test_pdf_multi_paragraph_mixed_families_renders():
    # Two paragraphs, each using a different @font-face. The PDF must
    # render both without crashing on the font swap mid-page.
    url = _inter_data_url()
    html = (
        f'<style>'
        f'@font-face {{ font-family: "Alpha"; src: url({url}); }} '
        f'@font-face {{ font-family: "Beta"; src: url({url}); }}'
        f'</style>'
        '<p style="font-family: Alpha">first</p>'
        '<p style="font-family: Beta">second</p>'
    )
    pdf = quickpdf.html_to_pdf(html)
    assert pdf[:5] == b"%PDF-"
    text = _pdf_text(pdf)
    assert "first" in text
    assert "second" in text
```

- [ ] **Step 5: Run pytest**

Run: `.venv/Scripts/python.exe -m pytest tests/ -q`
Expected: all tests PASS — 50 baseline + 5 new = 55.

- [ ] **Step 6: Commit**

```bash
git add tests/test_render.py
git commit -m "Phase 2b integrator: Python integration tests for @font-face rendering"
```

---

## Phase 6 — Wrap-up

### Task 19: Update CLAUDE.md roadmap

**Files:**
- Modify: `CLAUDE.md` (the roadmap table + "Next session" prose)

- [ ] **Step 1: Update the roadmap table**

In `CLAUDE.md`, find the roadmap table. Locate the row for Phase 2b (currently `→`). Update it and the next-pointer:

```markdown
|  2a   |   ✓    | Block-level `<img>` (PNG/JPEG via `data:` URL, HTML+CSS sizing, alt fallback)         |
|  2b   |   ✓    | Web fonts via `@font-face` (data:font/ttf|otf URLs, permissive MIME + magic sniff)    |
|  2c   |   →    | **NEXT.** Tables (`<table>`/`<tr>`/`<td>`) — proper 2D layout                          |
```

Also update the "Test posture today" prose under the table to reflect the new totals:

> **Test posture today:** ~227 Rust unit tests + ~55 Python integration tests, all green in ~0.5 s combined.

(Update the actual numbers if they differ from the rough estimates after running the full sweep in Task 20.)

- [ ] **Step 2: Update the "Next session" section**

In `CLAUDE.md`, find the "Next session: Phase 2b" section. Replace its body with:

```markdown
## Next session: Phase 2c — tables

Phase 2b is complete. Phase 2c adds `<table>`/`<tr>`/`<td>` block
support. The block stream already gains new variants cleanly via
`parse::Block` (the pattern Phase 2a established), and
`BlockStyle::width_em`/`height_em` already exist for column-width
expression. New work:

- A new `parse::Block::Table(...)` variant carrying row/cell structure.
- A 2D layout pass that resolves column widths (auto, fixed, percentage),
  flows cell content as nested blocks, and paginates rows.
- Cascade for `border-collapse`, `border-spacing`, `vertical-align`.

Cross-cutting Phase 2b artefacts to keep in mind for future phases:

- `style::sheet::Stylesheet` is the canonical stylesheet output (rules
  + font_faces). The legacy `parse_stylesheet -> Vec<Rule>` and
  `Document::user_stylesheet` are kept as back-compat shims; new code
  should use `Document::stylesheet()` and `parse_stylesheet_full`.
- `font::FontRegistry` is built once per `html_to_pdf` call. Phase 3's
  bulk-render path will likely hoist registry construction out of the
  per-call loop and share it across multiple HTMLs in a batch.
- `BlockStyle.font_family: Option<Vec<String>>` is inherited per CSS
  spec via `cascade::inherit`'s `child.or_else(|| parent.clone())` arm
  (different shape from the f32/Color sentinel-compare arms — `Option`
  uses `None` as the natural sentinel).
- Generic family keywords (`sans-serif`, `serif`, …) are dropped at
  cascade time. A future phase that maps them to concrete fonts (via
  system probing or bundled additional families) would re-introduce
  them as registry entries rather than touching the cascade.
- WOFF/WOFF2 srcs are silently skipped. Adding WOFF support would mean
  an extra arm in `font::decode_data_url` plus a zlib decode of the
  WOFF wrapper (krilla owns the resulting sfnt).
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "Phase 2b: mark roadmap done, point next session at Phase 2c"
```

### Task 20: Final test sweep + clean check

**Files:** none modified — purely verification.

- [ ] **Step 1: Run the full Rust suite**

Run: `cargo test -p quickpdf-core --lib`
Expected: all tests PASS — approximately 227 total. Note the actual count.

- [ ] **Step 2: Type-check with no warnings**

Run: `cargo check -p quickpdf-core`
Expected: builds clean, zero warnings.

- [ ] **Step 3: Rebuild the wheel and run pytest**

```bash
.venv/Scripts/maturin.exe develop --release
.venv/Scripts/python.exe -m pytest tests/ -q
```

Expected: all tests PASS — approximately 55 total.

- [ ] **Step 4: If any test count is off, update CLAUDE.md**

If the Rust test count differs from "~227" or the Python count differs from "~55", edit `CLAUDE.md`'s test posture line with the actual numbers and add a follow-up commit:

```bash
git add CLAUDE.md
git commit -m "Phase 2b: update test counts in CLAUDE.md"
```

- [ ] **Step 5: Confirm the working tree is clean**

Run: `git status`
Expected: clean tree, all Phase 2b commits on `main`.

- [ ] **Step 6: Push to origin**

Run: `git push origin main`
Expected: push succeeds; `gh repo view Uppah/quickpdf` shows the latest Phase 2b commit on `main`.

---

## Self-review notes

After writing the plan I scanned for the writing-plans skill's red flags:

1. **Spec coverage:** Every spec section has tasks. At-rule capture (Tasks 2-5), `Stylesheet` aggregate (Tasks 2-4), `FontRegistry` (Tasks 7-9 covering build, lookup, MIME accept, magic sniff, multi-src walk, last-wins, error matrix), `font-family` cascade (Tasks 11-13 covering field, parser, apply arm, inheritance), planner + emitter font routing (Tasks 15-16), alt-text path (Task 16 step 5), Rust integration tests (Task 17), Python integration tests (Task 18), CLAUDE.md (Task 19). The error handling matrix's 15 rows are all covered: missing/empty `font-family`, missing `src`, only-local, only-http, only-woff2, base64 garbage, multi-src walks, present-vs-absent family fallback, duplicate family last-wins, inheritance, inline override, octet-stream-with-magic, wrong-magic-in-ttf-mime — see the test names in Tasks 5, 9, 13, 17.

2. **Placeholder scan:** No `TBD`/`TODO`. Every code block is complete and runnable. Each step has the actual command and expected output. The single deviation is the Task 13 Step 3 note that the apply path's exact insertion shape depends on the existing `apply_declarations` structure; the plan documents both shapes (direct arm vs `parse_value` indirection) and the rule for picking between them. Slightly less mechanical than the rest of the plan but unavoidable until the integrator reads the file.

3. **Type consistency:** `FontFace { declarations, source_order }` consistent across Slice A (Task 2 definition), Slice B (Task 7 import + Task 9 consumption). `FontHandle = usize` consistent across Slice B (Task 7) and Integrator (Task 15 PlacedLine field, Task 17 tests). `Stylesheet { rules, font_faces }` consistent across Slice A (Task 2), `Document::stylesheet()` (Task 4), Integrator (Task 16 `parsed.stylesheet()`). `BlockStyle.font_family: Option<Vec<String>>` consistent across Slice C (Tasks 11, 12, 13) and Integrator (Task 16 `style.font_family.as_deref()`). `parse_font_family(value: &str) -> Option<Vec<String>>` signature consistent across Slice C tasks.

4. **Slice ordering:** Setup (Task 1) is the only sequential prerequisite. Slices A, B, and C can run in parallel after Task 1 — Slice B's only contract dependency on Slice A (the `FontFace` struct shape) is locked at Task 2 Step 2. The Integrator phase (Tasks 15-18) MUST run last and serially.

5. **One known nuance:** Task 17 Step 2's tests assert `assert_ne!(line.font_handle, 0)` for paragraphs whose `font-family` resolves to a registered face. Because the registry assigns handles in source order (Inter at 0, then 1, 2, …), the exact non-zero handle isn't predictable when multiple `@font-face` blocks exist, so tests check `!= 0` and `!=` between two handles rather than equality with a specific number. This is intentional — relying on specific handle indices would couple tests to registry insertion order, which is an implementation detail.

6. **Test fixture decision:** The plan deliberately uses `FALLBACK_TTF` (the bundled Inter bytes) as the payload for *every* `@font-face` test instead of shipping a separate fixture font. Rationale: krilla embeds a font's internal `name` table records into the PDF, so registering Inter under "Acme" still emits a PDF whose font name is "Inter" — we cannot distinguish it at the byte level anyway. The Rust unit tests in Slice B prove the registry maps families to handles correctly; the Python tests prove the end-to-end pipeline (parse → cascade → registry → planner → emitter) does not crash and produces a valid PDF. A future phase could ship a distinct test font (and assert PDF font-name extraction matches) if the Phase 4+ work needs to verify which face krilla picked at PDF level.
