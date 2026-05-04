# Phase 2a Images Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add block-level `<img>` rendering with PNG and JPEG decoding from `data:` URLs, including HTML `width`/`height` attrs and CSS `width`/`height` longhands, with paint-as-unit pagination and alt-text fallback.

**Architecture:** A new `parse::Block` enum splits the paragraph stream into `TextBlock` (existing semantics) and `ImageBlock` variants. A new `image.rs` module parses `data:` URLs and base64-decodes their payload (no PNG/JPEG header parsing of our own — krilla owns that). The cascade gains `width_em`/`height_em` longhands. `lib.rs::plan_pages_styled` adds an image branch that emits a new `PlacedImage` paint primitive alongside the existing `PlacedLine`/`PlacedBox`, with proportional shrink for oversize images.

**Tech Stack:** Rust 2021. krilla 0.7 (PDF emission, `Image::from_png`/`from_jpeg`, `Surface::draw_image` — `raster-images` is in default features, krilla already pulls `png`, `zune-jpeg`, and `base64 0.22.1` as direct deps). base64 0.22.1 added to our `[workspace.dependencies]` (pinned to match krilla's transitive). scraper/html5ever (DOM, unchanged). pyo3 0.23 (Python bindings, unchanged). The `image` crate referenced in the spec is **not** needed — krilla's image API already validates and decodes PNG/JPEG.

**Spec deviation noted:** The spec at `docs/superpowers/specs/2026-05-04-phase-2a-images-design.md` proposed an `image = "0.25"` dependency for header parsing. While writing this plan I confirmed krilla 0.7 does this work itself (`png_metadata`/`jpeg_metadata` internally; `Image::size()` exposes dimensions). Removing the `image` dep simplifies Slice A and shrinks wheel size. Behavior in the spec is preserved end-to-end.

---

## File Structure

| File | Role | Owning slice |
| --- | --- | --- |
| `Cargo.toml` (workspace) | Add `base64 = "=0.22.1"` to `[workspace.dependencies]` | Setup |
| `crates/quickpdf-core/Cargo.toml` | Reference workspace `base64` from `[dependencies]` | Setup |
| `crates/quickpdf-core/src/image.rs` *(new)* | `ImageKind` enum, `parse_data_url` | Slice A |
| `crates/quickpdf-core/src/lib.rs` | `pub mod image;` add; image-branch in `plan_pages_styled`; `PlacedImage`; krilla `draw_image` integration; cross-file fixups for the parser rename | Setup (skel), Integrator |
| `crates/quickpdf-core/src/parse.rs` | `Block`/`TextBlock`/`ImageBlock` enum; `is_block` adds `img`; `Document::blocks()` rename + ImageBlock construction; tests | Slice B |
| `crates/quickpdf-core/src/style/mod.rs` | `BlockStyle.width_em`/`height_em` fields | Slice C |
| `crates/quickpdf-core/src/style/cascade.rs` | `parse_value` arms for `width`/`height` (rejecting `%`); `apply_declarations` arms; tests | Slice C |
| `tests/test_render.py` | Python integration tests for images | Integrator |
| `CLAUDE.md` | Roadmap table marks Phase 2a ✓ | Integrator |

Slice A, Slice B, and Slice C are intentionally non-overlapping. A subagent-driven executor MAY run them in parallel after Setup. Slice B's rename of `Document::paragraphs()` → `Document::blocks()` will break `lib.rs`'s compilation; the integrator phase reconciles that.

---

## Phase 0 — Setup

### Task 1: Add `base64` to workspace + crate Cargo.toml

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/quickpdf-core/Cargo.toml`

- [ ] **Step 1: Add base64 to workspace dependencies**

Edit `Cargo.toml`. After the `ego-tree = "0.11"` line in `[workspace.dependencies]`, add:

```toml
# Phase 2a: data-URL decoding for <img src="data:image/png;base64,...">.
# Pinned to "=0.22.1" because krilla 0.7 declares base64 = "0.22.1" directly;
# pinning prevents a duplicate base64 tree in the wheel.
base64 = "=0.22.1"
```

- [ ] **Step 2: Reference workspace base64 from quickpdf-core**

Edit `crates/quickpdf-core/Cargo.toml`. After the `ego-tree = { workspace = true }` line in `[dependencies]`, add:

```toml
base64 = { workspace = true }
```

- [ ] **Step 3: Verify the dep resolves**

Run: `cargo check -p quickpdf-core`
Expected: builds clean, no warnings about duplicate `base64`.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/quickpdf-core/Cargo.toml Cargo.lock
git commit -m "Phase 2a setup: add base64 0.22.1 dep for data-URL decoding"
```

---

## Slice A — `image.rs` data URL decoder

Constraint: Slice A only edits `crates/quickpdf-core/src/image.rs` *(new file)* and adds `pub mod image;` to `lib.rs`. **Do not** touch `parse.rs`, `style/`, or any rendering code in `lib.rs`. Green-bar gate: `cargo check -p quickpdf-core` (skips `#[cfg(test)]` bodies, immune to integrator-only fixups).

### Task 2: Create `image.rs` skeleton

**Files:**
- Create: `crates/quickpdf-core/src/image.rs`
- Modify: `crates/quickpdf-core/src/lib.rs:6-9` (module declarations block)

- [ ] **Step 1: Create `crates/quickpdf-core/src/image.rs` with this content:**

```rust
//! Phase 2a: `<img src="data:image/...">` data-URL parser.
//!
//! Slice A scope: take a raw `data:` URL string, validate the prefix and
//! MIME, base64-decode the payload, and hand back `(kind, bytes)`. We do
//! **not** parse PNG/JPEG headers ourselves — krilla 0.7's `Image::from_png`
//! / `from_jpeg` validates and decodes at emit time, returning `Err` on
//! malformed input. The integrator treats that `Err` exactly like a missing
//! src (alt-text fallback).

use base64::Engine;

/// Which raster format a data URL declared. `parse_data_url` only accepts
/// these two; `image/gif`, `image/webp`, etc. return `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
}

/// Parse a `data:image/png;base64,...` or `data:image/jpeg;base64,...` URL
/// into its kind and raw byte payload. Returns `None` for any failure:
/// missing `data:` prefix, unsupported MIME, missing `;base64,` separator,
/// malformed base64. Returns `Some((kind, bytes))` on success — krilla
/// validates the bytes themselves at emit time.
pub fn parse_data_url(src: &str) -> Option<(ImageKind, Vec<u8>)> {
    let _ = src;
    let _ = base64::engine::general_purpose::STANDARD.decode("");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_none() {
        assert!(parse_data_url("").is_none());
    }
}
```

- [ ] **Step 2: Wire the module into the crate**

Edit `crates/quickpdf-core/src/lib.rs`. Locate the module declarations near the top (currently lines 6-9):

```rust
pub mod font;
pub mod parse;
pub mod style;
pub mod text;
```

Replace with:

```rust
pub mod font;
pub mod image;
pub mod parse;
pub mod style;
pub mod text;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p quickpdf-core`
Expected: builds clean.

- [ ] **Step 4: Run the skeleton test**

Run: `cargo test -p quickpdf-core --lib image::tests::empty_input_returns_none`
Expected: PASS (the stub `parse_data_url` returns `None` for everything).

- [ ] **Step 5: Commit the skeleton**

```bash
git add crates/quickpdf-core/src/image.rs crates/quickpdf-core/src/lib.rs
git commit -m "Phase 2a Slice A: image.rs skeleton with parse_data_url stub"
```

### Task 3: Implement and test `parse_data_url` for the happy paths

**Files:**
- Modify: `crates/quickpdf-core/src/image.rs`

- [ ] **Step 1: Write happy-path tests (still inside `image.rs::tests`)**

Replace the `tests` module body with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// Tiny known-good PNG (1x1 red pixel). Verified valid by hand.
    const TINY_PNG_BYTES: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00,
        0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89,
        0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63,
        0xfc, 0xcf, 0xc0, 0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a,
        0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
        0x42, 0x60, 0x82,
    ];

    /// Tiny known-good JPEG (1x1 white pixel, baseline DCT, no EXIF).
    const TINY_JPEG_BYTES: &[u8] = &[
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00,
        0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xff, 0xdb,
        0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06, 0x05, 0x08, 0x07,
        0x07, 0x07, 0x09, 0x09, 0x08, 0x0a, 0x0c, 0x14, 0x0d, 0x0c, 0x0b,
        0x0b, 0x0c, 0x19, 0x12, 0x13, 0x0f, 0x14, 0x1d, 0x1a, 0x1f, 0x1e,
        0x1d, 0x1a, 0x1c, 0x1c, 0x20, 0x24, 0x2e, 0x27, 0x20, 0x22, 0x2c,
        0x23, 0x1c, 0x1c, 0x28, 0x37, 0x29, 0x2c, 0x30, 0x31, 0x34, 0x34,
        0x34, 0x1f, 0x27, 0x39, 0x3d, 0x38, 0x32, 0x3c, 0x2e, 0x33, 0x34,
        0x32, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01,
        0x01, 0x11, 0x00, 0xff, 0xc4, 0x00, 0x14, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0xff, 0xc4, 0x00, 0x14, 0x10, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00,
        0x3f, 0x00, 0x37, 0xff, 0xd9,
    ];

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn empty_input_returns_none() {
        assert!(parse_data_url("").is_none());
    }

    #[test]
    fn non_data_url_returns_none() {
        assert!(parse_data_url("https://example.com/x.png").is_none());
        assert!(parse_data_url("file:///x.png").is_none());
        assert!(parse_data_url("/relative/x.png").is_none());
    }

    #[test]
    fn valid_png_data_url_parses() {
        let url = format!("data:image/png;base64,{}", b64(TINY_PNG_BYTES));
        let (kind, bytes) = parse_data_url(&url).expect("png parses");
        assert_eq!(kind, ImageKind::Png);
        assert_eq!(bytes, TINY_PNG_BYTES);
    }

    #[test]
    fn valid_jpeg_data_url_parses() {
        let url = format!("data:image/jpeg;base64,{}", b64(TINY_JPEG_BYTES));
        let (kind, bytes) = parse_data_url(&url).expect("jpeg parses");
        assert_eq!(kind, ImageKind::Jpeg);
        assert_eq!(bytes, TINY_JPEG_BYTES);
    }

    #[test]
    fn jpg_alias_also_parses() {
        // Browsers accept both `image/jpeg` and `image/jpg`.
        let url = format!("data:image/jpg;base64,{}", b64(TINY_JPEG_BYTES));
        let (kind, _) = parse_data_url(&url).expect("jpg alias parses");
        assert_eq!(kind, ImageKind::Jpeg);
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail (stub still returns None)**

Run: `cargo test -p quickpdf-core --lib image::`
Expected: 4 of 5 tests FAIL (`valid_png_data_url_parses`, `valid_jpeg_data_url_parses`, `jpg_alias_also_parses`; the `non_data_url_returns_none` and `empty_input_returns_none` PASS because the stub returns `None`).

- [ ] **Step 3: Replace the stub with a real implementation**

Replace the body of `parse_data_url` with:

```rust
pub fn parse_data_url(src: &str) -> Option<(ImageKind, Vec<u8>)> {
    // Strip the `data:` prefix.
    let after_data = src.strip_prefix("data:")?;
    // Split on the first `,` — left = MIME + params, right = payload.
    let (params, payload) = after_data.split_once(',')?;
    // We only accept base64-encoded data URLs. Plain (URL-encoded) data URLs
    // are rare in HTML and out of scope for Phase 2a.
    let mime = params.strip_suffix(";base64")?;
    let kind = match mime {
        "image/png" => ImageKind::Png,
        "image/jpeg" | "image/jpg" => ImageKind::Jpeg,
        _ => return None,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.as_bytes())
        .ok()?;
    Some((kind, bytes))
}
```

- [ ] **Step 4: Run tests to confirm they pass**

Run: `cargo test -p quickpdf-core --lib image::`
Expected: all 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/quickpdf-core/src/image.rs
git commit -m "Phase 2a Slice A: parse_data_url for PNG and JPEG"
```

### Task 4: Edge-case tests for `parse_data_url`

**Files:**
- Modify: `crates/quickpdf-core/src/image.rs`

- [ ] **Step 1: Append edge-case tests to the `tests` module**

Add the following inside the `mod tests` block (just before its closing `}`):

```rust
    #[test]
    fn unsupported_mime_returns_none() {
        let url = format!("data:image/gif;base64,{}", b64(TINY_PNG_BYTES));
        assert!(parse_data_url(&url).is_none());
        let url = format!("data:image/webp;base64,{}", b64(TINY_PNG_BYTES));
        assert!(parse_data_url(&url).is_none());
        let url = format!("data:text/plain;base64,{}", b64(b"hi"));
        assert!(parse_data_url(&url).is_none());
    }

    #[test]
    fn missing_base64_marker_returns_none() {
        // Plain (URL-encoded) data URL — out of scope.
        let url = format!("data:image/png,{}", b64(TINY_PNG_BYTES));
        assert!(parse_data_url(&url).is_none());
    }

    #[test]
    fn missing_comma_returns_none() {
        // No payload separator at all.
        assert!(parse_data_url("data:image/png;base64").is_none());
    }

    #[test]
    fn malformed_base64_returns_none() {
        // Trailing `!` is not a valid base64 character.
        assert!(parse_data_url("data:image/png;base64,abcd!").is_none());
        // Garbled mid-payload.
        assert!(parse_data_url("data:image/png;base64,not===base64").is_none());
    }

    #[test]
    fn empty_payload_returns_some_with_empty_bytes() {
        // `data:image/png;base64,` is technically valid; krilla will reject
        // empty bytes at emit time, which is the integrator's job to handle.
        let (kind, bytes) = parse_data_url("data:image/png;base64,")
            .expect("empty payload still parses to Some");
        assert_eq!(kind, ImageKind::Png);
        assert!(bytes.is_empty());
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p quickpdf-core --lib image::`
Expected: all tests PASS. (The implementation from Task 3 already handles every case correctly — the unsupported-MIME branch hits the `_ => return None` fallthrough, malformed base64 hits the `.ok()?`, etc. If any test fails, the implementation needs tightening.)

- [ ] **Step 3: Commit**

```bash
git add crates/quickpdf-core/src/image.rs
git commit -m "Phase 2a Slice A: edge-case tests for parse_data_url"
```

---

## Slice B — `parse.rs` `Block` enum

Constraint: Slice B only edits `crates/quickpdf-core/src/parse.rs`. **Do not** touch `image.rs`, `style/`, or `lib.rs`. Slice B's rename of `Document::paragraphs()` → `Document::blocks()` and `Paragraph` → `Block` will break `lib.rs`'s compilation — that is **intentional** and reconciled by the Integrator. Green-bar gate: `cargo check -p quickpdf-core` will fail at the *integrator's* call sites, not at parse.rs's own tests; **for Slice B's gate, run `cargo test -p quickpdf-core --lib parse::` directly** and accept that `cargo check` of the whole crate will fail until integration.

### Task 5: Introduce `Block` / `TextBlock` / `ImageBlock` types

**Files:**
- Modify: `crates/quickpdf-core/src/parse.rs:113-119` (the `Paragraph` struct)
- Modify: `crates/quickpdf-core/src/parse.rs:74-79` (`Document::paragraphs`)
- Modify: `crates/quickpdf-core/src/parse.rs:62-64` (`Document::block_texts` — internal caller of `paragraphs`)
- Modify: `crates/quickpdf-core/src/parse.rs:135-175` (`is_block` set)
- Modify: `crates/quickpdf-core/src/parse.rs:177-277` (`collect_paragraphs` walker)

- [ ] **Step 1: Replace the `Paragraph` struct with the `Block` enum**

Locate the `Paragraph` struct (around lines 109-119) and replace it with:

```rust
/// One block-level unit in document order. Either text content (the old
/// `Paragraph`) or an image. The renderer matches on this enum to drive
/// either the text-flow path or the image-paint path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Text(TextBlock),
    Image(ImageBlock),
}

impl Block {
    /// The DOM element this block was emitted for. Anonymous text blocks
    /// reuse the parent block container's element id.
    pub fn element_id(&self) -> ego_tree::NodeId {
        match self {
            Block::Text(t) => t.element_id,
            Block::Image(i) => i.element_id,
        }
    }
}

/// One paragraph's worth of inline text plus the block-level tag that
/// produced it. The tag drives UA-default styles; the `element_id` lets
/// the cascade match author selectors against the original DOM. Equivalent
/// to the pre-2a `Paragraph` struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextBlock {
    pub tag: String,
    pub text: String,
    pub element_id: ego_tree::NodeId,
}

/// A `<img>` lifted into the block stream. `src` is the raw attribute value
/// (caller decodes via `crate::image::parse_data_url`). `width_attr` and
/// `height_attr` capture the HTML `width` / `height` attributes if they
/// parsed as plain numbers (CSS pixels — same as PDF points at our 1:1
/// conversion baseline). `alt` is recorded for the integrator's fallback
/// path when decoding fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBlock {
    pub element_id: ego_tree::NodeId,
    pub src: String,
    pub width_attr: Option<f32>,
    pub height_attr: Option<f32>,
    pub alt: Option<String>,
}

// Backwards alias retained only for transition tests inside this file.
// Removed in Slice B step 4.
pub type Paragraph = TextBlock;
```

Note: the `pub type Paragraph = TextBlock;` alias is a **temporary** transition aid — it lets the old `Paragraph::tag`/`text`/`element_id` field accesses continue compiling. It is removed in Step 4 below.

- [ ] **Step 2: Add `img` to `is_block`**

Locate `fn is_block` (around line 135) and edit the `matches!` body. Insert `"img"` alphabetically between `"hr"` and `"li"`. Final fragment:

```rust
            | "hr"
            | "img"
            | "li"
```

- [ ] **Step 3: Replace `paragraphs` and `collect_paragraphs` with `blocks`/`collect_blocks`**

Replace the `Document::paragraphs` method (around lines 74-79) with:

```rust
    /// Document content grouped into block-level units in document order.
    /// Each entry is either a `TextBlock` (one paragraph of inline text)
    /// or an `ImageBlock` (one `<img>` lifted into the stream). Anonymous
    /// text blocks wrap orphan inline content inside mixed-content
    /// containers; they reuse the parent's `element_id`.
    pub fn blocks(&self) -> Vec<Block> {
        let mut out: Vec<Block> = Vec::new();
        let root = self.html.root_element();
        collect_blocks(root, &mut out);
        out
    }
```

Locate `Document::block_texts` (around lines 62-64) and replace its body with:

```rust
    pub fn block_texts(&self) -> Vec<String> {
        self.blocks()
            .into_iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some(t.text),
                Block::Image(_) => None,
            })
            .collect()
    }
```

Then replace the `collect_paragraphs` walker function (around lines 199-277). The full replacement:

```rust
/// Walk the tree and emit one `Block` per block-level element. For text
/// containers this matches the pre-2a `collect_paragraphs` behavior — leaf
/// blocks emit `Block::Text`, mixed-content containers emit anonymous
/// `Block::Text` runs around their block children. For `<img>` we emit a
/// `Block::Image` carrying `src`, optional `width`/`height` attrs, and
/// optional `alt`.
fn collect_blocks(elem: ElementRef<'_>, out: &mut Vec<Block>) {
    let name = elem.value().name();
    if is_skipped(name) {
        return;
    }

    // <img> is an empty (void) block element. Don't recurse into children.
    if name == "img" {
        let attr_f32 = |key: &str| -> Option<f32> {
            elem.value().attr(key).and_then(|v| v.trim().parse::<f32>().ok())
        };
        out.push(Block::Image(ImageBlock {
            element_id: elem.id(),
            src: elem.value().attr("src").unwrap_or("").to_string(),
            width_attr: attr_f32("width"),
            height_attr: attr_f32("height"),
            alt: elem.value().attr("alt").map(|s| s.to_string()),
        }));
        return;
    }

    let has_block_child = elem.children().any(|c| {
        if let Node::Element(e) = c.value() {
            !is_skipped(e.name()) && is_block(e.name())
        } else {
            false
        }
    });
    if is_block(name) && has_block_child {
        // Mixed-content block: walk children, accumulate inline text/element
        // runs into an anonymous buffer. Block element children flush the
        // buffer and recurse normally.
        let parent_id = elem.id();
        let mut buffer = String::new();
        let flush =
            |buffer: &mut String, out: &mut Vec<Block>| {
                let collapsed = collapse_whitespace(buffer.as_str());
                if !collapsed.is_empty() {
                    out.push(Block::Text(TextBlock {
                        tag: ANONYMOUS_TAG.to_string(),
                        text: collapsed,
                        element_id: parent_id,
                    }));
                }
                buffer.clear();
            };
        for child in elem.children() {
            match child.value() {
                Node::Text(t) => buffer.push_str(&t.text),
                Node::Element(e) => {
                    let child_name = e.name();
                    if is_skipped(child_name) {
                        continue;
                    }
                    if let Some(child_elem) = ElementRef::wrap(child) {
                        if is_block(child_name) {
                            flush(&mut buffer, out);
                            collect_blocks(child_elem, out);
                        } else {
                            collect_text(child_elem, &mut buffer);
                        }
                    }
                }
                _ => {}
            }
        }
        flush(&mut buffer, out);
        return;
    }
    if !is_block(name) {
        for child in elem.children() {
            if let Some(child_elem) = ElementRef::wrap(child) {
                collect_blocks(child_elem, out);
            }
        }
        return;
    }
    // Leaf block: gather inline text content.
    let mut text = String::new();
    collect_text(elem, &mut text);
    let collapsed = collapse_whitespace(&text);
    if !collapsed.is_empty() {
        out.push(Block::Text(TextBlock {
            tag: name.to_string(),
            text: collapsed,
            element_id: elem.id(),
        }));
    }
}
```

- [ ] **Step 4: Update `Document::element_for` to take a `&Block`**

Locate `Document::element_for` (around lines 92-95) and replace the body with:

```rust
    pub fn element_for(&self, b: &Block) -> Option<ElementRef<'_>> {
        let node = self.html.tree.get(b.element_id())?;
        ElementRef::wrap(node)
    }
```

- [ ] **Step 5: Verify parse.rs internals compile**

Run: `cargo check -p quickpdf-core --tests --lib 2>&1 | head -40`
Expected: errors are confined to `lib.rs` (calls to `paragraphs()`, references to `Paragraph`); no errors inside `parse.rs` itself.

- [ ] **Step 6: Commit Slice B's structural change**

```bash
git add crates/quickpdf-core/src/parse.rs
git commit -m "Phase 2a Slice B: introduce Block enum (TextBlock + ImageBlock)"
```

(Note: lib.rs is broken at this commit. The integrator restores green at Task 11.)

### Task 6: Update parse.rs's existing tests for the rename

**Files:**
- Modify: `crates/quickpdf-core/src/parse.rs` (the `#[cfg(test)] mod tests` block, lines ~324-685)

- [ ] **Step 1: Replace `.paragraphs()` calls and `Paragraph` accessors**

Inside the `mod tests` block, every test that calls `d.paragraphs()` and then accesses `.tag` / `.text` / `.element_id` on the elements must be rewritten to handle the `Block` enum. The pattern conversion:

```rust
// BEFORE (Phase 1.7c):
let ps = d.paragraphs();
let tagged: Vec<(&str, &str)> = ps
    .iter()
    .map(|p| (p.tag.as_str(), p.text.as_str()))
    .collect();

// AFTER (Phase 2a Slice B):
let ps = d.blocks();
let tagged: Vec<(&str, &str)> = ps
    .iter()
    .filter_map(|b| match b {
        Block::Text(t) => Some((t.tag.as_str(), t.text.as_str())),
        Block::Image(_) => None,
    })
    .collect();
```

For tests that check `ps.len() == N` and then index `ps[i].tag`, replace with the same `match`-pattern. For the `inline_styles_node_id_resolves_via_element_for` test (which constructs a synthetic `Paragraph`), wrap it in `Block::Text(...)`:

```rust
// BEFORE:
let synthetic = Paragraph {
    tag: "p".to_string(),
    text: "x".to_string(),
    element_id: node_id,
};
let resolved = d.element_for(&synthetic).expect("node id resolves");

// AFTER:
let synthetic = Block::Text(TextBlock {
    tag: "p".to_string(),
    text: "x".to_string(),
    element_id: node_id,
});
let resolved = d.element_for(&synthetic).expect("node id resolves");
```

Apply this pattern to **every** existing test in the file that touches `paragraphs()` or `Paragraph`. There are approximately 16 affected tests (the ones from `block_texts_splits_on_block_boundaries` through `inline_styles_drops_unparseable_into_zero_decls`).

- [ ] **Step 2: Remove the temporary `Paragraph` alias**

Delete the line `pub type Paragraph = TextBlock;` from `parse.rs`. (It was only there to bridge the rename for the rest of this slice.)

- [ ] **Step 3: Run parse.rs's own test suite**

Run: `cargo test -p quickpdf-core --lib parse::`
Expected: all parse.rs tests PASS. (The whole-crate `cargo check` will still fail because lib.rs hasn't been fixed yet — that's the integrator's job.)

- [ ] **Step 4: Commit**

```bash
git add crates/quickpdf-core/src/parse.rs
git commit -m "Phase 2a Slice B: migrate parse.rs tests to Block enum"
```

### Task 7: Add Slice B's new tests for `<img>` semantics

**Files:**
- Modify: `crates/quickpdf-core/src/parse.rs` (append to `mod tests`)

- [ ] **Step 1: Append four new tests inside `mod tests`, just before its closing `}`:**

```rust
    // ---- Phase 2a Slice B: <img> as a block-level element. ----

    #[test]
    fn img_with_src_emits_one_image_block() {
        let d = Document::parse(r#"<img src="data:image/png;base64,xyz">"#);
        let bs = d.blocks();
        assert_eq!(bs.len(), 1, "expected exactly one block, got {bs:?}");
        match &bs[0] {
            Block::Image(img) => {
                assert_eq!(img.src, "data:image/png;base64,xyz");
                assert!(img.width_attr.is_none());
                assert!(img.height_attr.is_none());
                assert!(img.alt.is_none());
            }
            other => panic!("expected ImageBlock, got {other:?}"),
        }
    }

    #[test]
    fn img_captures_width_height_alt_attrs() {
        let d = Document::parse(
            r#"<img src="data:image/png;base64,x" width="120" height="80" alt="logo">"#,
        );
        let bs = d.blocks();
        assert_eq!(bs.len(), 1);
        match &bs[0] {
            Block::Image(img) => {
                assert_eq!(img.width_attr, Some(120.0));
                assert_eq!(img.height_attr, Some(80.0));
                assert_eq!(img.alt.as_deref(), Some("logo"));
            }
            other => panic!("expected ImageBlock, got {other:?}"),
        }
    }

    #[test]
    fn img_inside_p_splits_paragraph_into_three_blocks() {
        // Phase 2a always treats <img> as block-level. The 1.6c
        // anonymous-block walker splits the surrounding inline text.
        let d = Document::parse(
            r#"<p>before <img src="data:image/png;base64,x"> after</p>"#,
        );
        let bs = d.blocks();
        assert_eq!(bs.len(), 3, "expected 3 blocks, got {bs:?}");
        match &bs[0] {
            Block::Text(t) => {
                assert_eq!(t.tag, ANONYMOUS_TAG);
                assert_eq!(t.text, "before");
            }
            other => panic!("[0] should be anon text, got {other:?}"),
        }
        match &bs[1] {
            Block::Image(img) => assert_eq!(img.src, "data:image/png;base64,x"),
            other => panic!("[1] should be image, got {other:?}"),
        }
        match &bs[2] {
            Block::Text(t) => {
                assert_eq!(t.tag, ANONYMOUS_TAG);
                assert_eq!(t.text, "after");
            }
            other => panic!("[2] should be anon text, got {other:?}"),
        }
    }

    #[test]
    fn img_with_unparseable_size_attrs_falls_through_to_none() {
        // `width="auto"` and `height="50%"` aren't plain f32, so the
        // parser stores `None` and the integrator falls back to CSS or
        // intrinsic sizing.
        let d = Document::parse(
            r#"<img src="data:image/png;base64,x" width="auto" height="50%">"#,
        );
        let bs = d.blocks();
        assert_eq!(bs.len(), 1);
        match &bs[0] {
            Block::Image(img) => {
                assert!(img.width_attr.is_none());
                assert!(img.height_attr.is_none());
            }
            other => panic!("expected ImageBlock, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run parse.rs's tests**

Run: `cargo test -p quickpdf-core --lib parse::`
Expected: all tests PASS, including the four new ones.

- [ ] **Step 3: Commit**

```bash
git add crates/quickpdf-core/src/parse.rs
git commit -m "Phase 2a Slice B: tests for <img> as block element"
```

---

## Slice C — `style` cascade for `width` and `height`

Constraint: Slice C only edits `crates/quickpdf-core/src/style/mod.rs` and `crates/quickpdf-core/src/style/cascade.rs`. **Do not** touch `parse.rs`, `image.rs`, or `lib.rs`. Green-bar gate: `cargo test -p quickpdf-core --lib style::` (independent of integrator state).

### Task 8: Add `width_em` and `height_em` to `BlockStyle` and the builder

**Files:**
- Modify: `crates/quickpdf-core/src/style/mod.rs:124-157` (the `BlockStyle` struct + `DEFAULT`)
- Modify: `crates/quickpdf-core/src/style/cascade.rs:344-359` (the `BlockStyleBuilder` struct)
- Modify: `crates/quickpdf-core/src/style/cascade.rs:386-411` (the `build` method)
- Modify: `crates/quickpdf-core/src/style/cascade.rs:414-448` (the `inherit` helper)

- [ ] **Step 1: Add fields to `BlockStyle`**

Edit `style/mod.rs`. Find the `BlockStyle` struct (around lines 124-157). After the `border_color: Color,` line, add:

```rust
    /// Author-set width in em (relative to the block's resolved font
    /// size). `None` means "no explicit width" — layout falls back to
    /// HTML attrs or intrinsic dimensions. Phase 2a only sets this for
    /// `<img>`; in future phases other block types may consume it.
    pub width_em: Option<f32>,
    /// Author-set height in em. Same semantics as `width_em`.
    pub height_em: Option<f32>,
```

Then update the `DEFAULT` const (around lines 160-176). After the `border_color: Color::BLACK,` line, add:

```rust
        width_em: None,
        height_em: None,
```

- [ ] **Step 2: Add fields to `BlockStyleBuilder`**

Edit `style/cascade.rs`. Find the `BlockStyleBuilder` struct (around lines 344-359). After the `pub border_color: Option<Color>,` line, add:

```rust
    pub width_em: Option<Option<f32>>,
    pub height_em: Option<Option<f32>>,
```

(The `Option<Option<f32>>` pattern matches `background_color: Option<Option<Color>>` already in this struct: outer `Option` says "did the cascade set it"; inner `Option` says "is the value `None`-meaning-unset".)

- [ ] **Step 3: Update `BlockStyleBuilder::from_block` and `build`**

In the `from_block` method (around lines 365-383), add to the trailing field initializers:

```rust
            width_em: Some(style.width_em),
            height_em: Some(style.height_em),
```

In the `build` method (around lines 388-411), add to the `BlockStyle { ... }` initializer (just before the closing `}`):

```rust
            width_em: self.width_em.unwrap_or(def.width_em),
            height_em: self.height_em.unwrap_or(def.height_em),
```

- [ ] **Step 4: Update the `inherit` helper**

In the `inherit` function (around lines 417-448), add to the `BlockStyle { ... }` initializer (just before the closing `}`):

```rust
        // Width/height are not inherited per CSS — pass child's through.
        width_em: child.width_em,
        height_em: child.height_em,
```

- [ ] **Step 5: Verify Slice C compiles in isolation**

Run: `cargo check -p quickpdf-core --lib`
Expected: errors are confined to `lib.rs` (where `BlockStyle` is constructed by literal struct expressions — Slice C's new fields trigger them); no errors inside `style/`.

- [ ] **Step 6: Commit**

```bash
git add crates/quickpdf-core/src/style/mod.rs crates/quickpdf-core/src/style/cascade.rs
git commit -m "Phase 2a Slice C: add width_em/height_em to BlockStyle"
```

### Task 9: Cascade plumbing for `width` and `height` longhands

**Files:**
- Modify: `crates/quickpdf-core/src/style/cascade.rs:128-148` (`parse_value`)
- Modify: `crates/quickpdf-core/src/style/cascade.rs:92-124` (`apply_declarations`)

- [ ] **Step 1: Write failing tests for the new behavior**

Inside `cascade.rs`'s `mod tests`, just before the closing `}`, append:

```rust
    // ---- Phase 2a Slice C: width / height longhands. ----

    #[test]
    fn parse_value_accepts_width_and_height_lengths() {
        // Same px / pt / em / rem path as padding-* and border-width.
        assert_eq!(
            parse_value("width", "120px"),
            Some(ParsedValue::LengthEm(120.0 / 12.0))
        );
        assert_eq!(
            parse_value("height", "5em"),
            Some(ParsedValue::LengthEm(5.0))
        );
        assert_eq!(
            parse_value("width", "24pt"),
            Some(ParsedValue::LengthEm(24.0 / 12.0))
        );
        assert_eq!(
            parse_value("height", "2rem"),
            Some(ParsedValue::LengthEm(2.0))
        );
    }

    #[test]
    fn parse_value_rejects_percent_for_width_and_height() {
        // CSS `width: 50%` resolves against the containing block, not the
        // font-size. Our cascade can't preserve that without a richer Length
        // type, so Phase 2a explicitly drops `%` for these properties.
        assert_eq!(parse_value("width", "50%"), None);
        assert_eq!(parse_value("width", "100%"), None);
        assert_eq!(parse_value("height", "50%"), None);
        // Sanity: `%` still works for font-size where it makes sense.
        assert_eq!(
            parse_value("font-size", "150%"),
            Some(ParsedValue::LengthEm(1.5))
        );
    }

    #[test]
    fn apply_declarations_sets_width_and_height_em() {
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[d("width", "120px"), d("height", "60px")],
        );
        assert_eq!(out.width_em, Some(120.0 / 12.0));
        assert_eq!(out.height_em, Some(60.0 / 12.0));
    }

    #[test]
    fn apply_declarations_ignores_unparseable_width() {
        // `width: auto` is a CSS keyword we don't honor in Phase 2a.
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[d("width", "auto"), d("height", "200px")],
        );
        assert!(out.width_em.is_none(), "width:auto must leave width_em as None");
        assert_eq!(out.height_em, Some(200.0 / 12.0));
    }

    #[test]
    fn width_height_not_inherited() {
        let parent = BlockStyle {
            width_em: Some(10.0),
            height_em: Some(5.0),
            ..BlockStyle::DEFAULT
        };
        let child = BlockStyle::DEFAULT;
        let s = inherit(&parent, child);
        assert!(s.width_em.is_none());
        assert!(s.height_em.is_none());
    }
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p quickpdf-core --lib style::cascade::tests::parse_value_accepts_width_and_height_lengths`
Expected: FAIL — `parse_value` doesn't handle `"width"` / `"height"`.

- [ ] **Step 3: Add the new arms to `parse_value`**

In `cascade.rs::parse_value` (around line 128-148), find the existing match arm:

```rust
        "font-size"
        | "margin-top"
        | "margin-bottom"
        | "padding-top"
        | "padding-right"
        | "padding-bottom"
        | "padding-left"
        | "border-width" => parse_length_em(value).map(ParsedValue::LengthEm),
```

Add a new arm immediately after it:

```rust
        "width" | "height" => {
            // Phase 2a: reject `%` because our cascade can't preserve the
            // CSS percentage-of-containing-block semantic. See spec §3.
            if value.trim_end().ends_with('%') {
                return None;
            }
            parse_length_em(value).map(ParsedValue::LengthEm)
        }
```

- [ ] **Step 4: Add the new arms to `apply_declarations`**

In `cascade.rs::apply_declarations` (around lines 98-122), inside the `match (decl.name.as_str(), parsed)` block, add two new arms (place them anywhere — after `border-color` is fine):

```rust
            ("width", ParsedValue::LengthEm(x)) => out.width_em = Some(x),
            ("height", ParsedValue::LengthEm(x)) => out.height_em = Some(x),
```

- [ ] **Step 5: Verify all Slice C tests pass**

Run: `cargo test -p quickpdf-core --lib style::`
Expected: all style tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/quickpdf-core/src/style/cascade.rs
git commit -m "Phase 2a Slice C: parse and apply width/height longhands"
```

---

## Phase 4 — Integrator: render images in `lib.rs`

The integrator phase brings everything together. **Run after Slices A, B, and C are merged.** The integrator's job is:

1. Restore `cargo check -p quickpdf-core` to green by reconciling Slice B's `Document::paragraphs()` → `Document::blocks()` rename in `lib.rs`.
2. Add the image branch to `plan_pages_styled` and the new `PlacedImage` paint primitive.
3. Wire krilla's `Image::from_png` / `Image::from_jpeg` and `Surface::draw_image`.
4. Add Rust integration tests for the image path.
5. Add Python integration tests.
6. Mark the roadmap done.

### Task 10: Reconcile Slice B's rename in `lib.rs`

**Files:**
- Modify: `crates/quickpdf-core/src/lib.rs:127` (call to `paragraphs()`)
- Modify: `crates/quickpdf-core/src/lib.rs:262` and surrounding (signature + body of `plan_pages_styled`)
- Modify: `crates/quickpdf-core/src/lib.rs:419-429` (test helpers `plan` and `plan_full`)

- [ ] **Step 1: Update `html_to_pdf` to consume `Document::blocks()`**

Edit `lib.rs:127`. Replace:

```rust
    let paragraphs = parsed.paragraphs();
```

with:

```rust
    let blocks = parsed.blocks();
```

Then in the call to `plan_pages_styled` (around line 146), replace `&paragraphs` with `&blocks`.

- [ ] **Step 2: Update `plan_pages_styled`'s signature and body**

Find `fn plan_pages_styled` (around line 270). Update the parameter list:

```rust
fn plan_pages_styled(
    doc: &parse::Document,
    blocks: &[parse::Block],
    user_rules: &[style::sheet::Rule],
    inline: &style::InlineStyles<'_>,
    content_width: f32,
    left_margin: f32,
    bottom_limit: f32,
) -> Result<Vec<PagePlan>, Error> {
```

Inside the body, replace the `for para in paragraphs {` line with:

```rust
    for block in blocks {
        // Phase 2a: image branch is added in Task 12. For now, skip non-text
        // blocks so the existing text-rendering tests stay green.
        let para = match block {
            parse::Block::Text(t) => t,
            parse::Block::Image(_) => continue,
        };
```

Inside the `for` loop body, the original code referenced `para.tag`, `para.text`, and `doc.element_for(para)`. Each of those still works because `para` is now a `&TextBlock`. The only fix needed is at the `doc.element_for(para)` call — change the argument:

```rust
        let style = match doc.element_for(block) {
            Some(elem) => style::resolve(elem, user_rules, inline),
            None => style::ua_style(&para.tag),
        };
```

(Note: `element_for` now takes `&Block`, not `&TextBlock`. The `block` variable is the iterator item from `for block in blocks`. The `match block { ... }` pattern earlier in the loop has already given us `para: &TextBlock`, but `element_for` wants the parent `&Block`, so we pass `block` itself.)

- [ ] **Step 3: Update test helpers `plan` and `plan_full`**

Find the test helpers (around lines 413-429). Replace:

```rust
    fn plan_full(html: &str) -> Vec<PagePlan> {
        let doc = parse::Document::parse(html);
        let paragraphs = doc.paragraphs();
        let rules = doc.user_stylesheet();
        let inline_owned = doc.inline_styles();
        let inline_map: style::InlineStyles<'_> = inline_owned
            .iter()
            .map(|(id, decls)| (*id, decls.as_slice()))
            .collect();
        plan_pages_styled(&doc, &paragraphs, &rules, &inline_map, 500.0, 36.0, 800.0).unwrap()
    }
```

with:

```rust
    fn plan_full(html: &str) -> Vec<PagePlan> {
        let doc = parse::Document::parse(html);
        let blocks = doc.blocks();
        let rules = doc.user_stylesheet();
        let inline_owned = doc.inline_styles();
        let inline_map: style::InlineStyles<'_> = inline_owned
            .iter()
            .map(|(id, decls)| (*id, decls.as_slice()))
            .collect();
        plan_pages_styled(&doc, &blocks, &rules, &inline_map, 500.0, 36.0, 800.0).unwrap()
    }
```

- [ ] **Step 4: Restore green for the whole crate**

Run: `cargo test -p quickpdf-core --lib`
Expected: ALL existing tests PASS (Slices A, B, C, plus all Phase 1.x tests). The `Block::Image` branch currently `continue`s, so image HTML inputs render as if the `<img>` were absent — that's the bridge state until Task 12.

- [ ] **Step 5: Commit the bridge state**

```bash
git add crates/quickpdf-core/src/lib.rs
git commit -m "Phase 2a integrator: reconcile Block enum rename in lib.rs"
```

### Task 11: Add `PlacedImage` primitive and `PagePlan.images`

**Files:**
- Modify: `crates/quickpdf-core/src/lib.rs:80-117` (paint-primitive structs and `PagePlan`)

- [ ] **Step 1: Add the `PlacedImage` struct**

In `lib.rs`, immediately after the `PlacedBox` struct (around line 103), add:

```rust
/// One placed image: a krilla `Image` plus the target box rectangle to
/// paint it into. The image was already validated by
/// `krilla::image::Image::from_png` / `from_jpeg`, so emit-time decoding
/// always succeeds. Width and height are in PDF points.
#[derive(Debug, Clone)]
struct PlacedImage {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    image: krilla::image::Image,
}
```

- [ ] **Step 2: Add `images` to `PagePlan` and update `is_empty`**

In `PagePlan` (around lines 107-111), replace:

```rust
#[derive(Debug, Clone, Default)]
struct PagePlan {
    boxes: Vec<PlacedBox>,
    lines: Vec<PlacedLine>,
}

impl PagePlan {
    fn is_empty(&self) -> bool {
        self.boxes.is_empty() && self.lines.is_empty()
    }
}
```

with:

```rust
#[derive(Debug, Clone, Default)]
struct PagePlan {
    boxes: Vec<PlacedBox>,
    images: Vec<PlacedImage>,
    lines: Vec<PlacedLine>,
}

impl PagePlan {
    fn is_empty(&self) -> bool {
        self.boxes.is_empty() && self.images.is_empty() && self.lines.is_empty()
    }
}
```

- [ ] **Step 3: Verify the bridge still compiles**

Run: `cargo check -p quickpdf-core`
Expected: builds clean. (No call sites for `PlacedImage` yet.)

- [ ] **Step 4: Commit**

```bash
git add crates/quickpdf-core/src/lib.rs
git commit -m "Phase 2a integrator: add PlacedImage paint primitive"
```

### Task 12: Implement the `Block::Image` branch and wire `Surface::draw_image`

**Files:**
- Modify: `crates/quickpdf-core/src/lib.rs` (use statements; `plan_pages_styled` body; surface paint loop)

- [ ] **Step 1: Add necessary imports**

At the top of `lib.rs`, augment the existing imports. Find the current krilla imports (around lines 13-20):

```rust
use krilla::Document;
use krilla::SerializeSettings;
use krilla::color::rgb as krilla_rgb;
use krilla::geom::{PathBuilder, Point, Rect, Size};
use krilla::paint::{Fill, Stroke};
use krilla::page::PageSettings;
use krilla::surface::Surface;
use krilla::text::{Font, TextDirection};
```

Add immediately below them:

```rust
use krilla::geom::Transform;
use krilla::image::Image as KrillaImage;
```

- [ ] **Step 2: Write a failing integration test in `lib.rs`'s `mod tests`**

At the end of `lib.rs`'s `mod tests` block, just before the closing `}`, append:

```rust
    // ---- Phase 2a integrator: image rendering. ----

    /// Tiny known-good PNG (1x1 red pixel). Same constant as image.rs
    /// uses — duplicated here so the test is self-contained.
    const TINY_PNG_BYTES: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00,
        0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89,
        0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63,
        0xfc, 0xcf, 0xc0, 0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a,
        0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
        0x42, 0x60, 0x82,
    ];

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn img_with_data_url_emits_placed_image() {
        let html = format!(
            r#"<img src="data:image/png;base64,{}" width="100" height="50">"#,
            b64(TINY_PNG_BYTES),
        );
        let pages = plan_full(&html);
        assert_eq!(pages.len(), 1, "expected one page");
        assert_eq!(pages[0].images.len(), 1, "expected one PlacedImage");
        let pi = &pages[0].images[0];
        assert!((pi.w - 100.0).abs() < 0.5, "expected w≈100pt, got {}", pi.w);
        assert!((pi.h - 50.0).abs() < 0.5, "expected h≈50pt, got {}", pi.h);
    }

    #[test]
    fn img_with_no_size_uses_intrinsic_capped_to_content_width() {
        // Tiny PNG is 1×1 px → intrinsic 1×1 pt. No shrink needed.
        let html = format!(
            r#"<img src="data:image/png;base64,{}">"#,
            b64(TINY_PNG_BYTES),
        );
        let pages = plan_full(&html);
        assert_eq!(pages[0].images.len(), 1);
        let pi = &pages[0].images[0];
        assert!((pi.w - 1.0).abs() < 0.5);
        assert!((pi.h - 1.0).abs() < 0.5);
    }

    #[test]
    fn img_height_only_attr_derives_width_from_aspect() {
        // 1×1 intrinsic, height set to 200 → width should be 200.
        let html = format!(
            r#"<img src="data:image/png;base64,{}" height="200">"#,
            b64(TINY_PNG_BYTES),
        );
        let pages = plan_full(&html);
        let pi = &pages[0].images[0];
        assert!((pi.h - 200.0).abs() < 0.5);
        assert!((pi.w - 200.0).abs() < 0.5);
    }

    #[test]
    fn img_css_width_overrides_html_attr() {
        let html = format!(
            r#"<style>img {{ width: 60px; }}</style>
<img src="data:image/png;base64,{}" width="120" height="120">"#,
            b64(TINY_PNG_BYTES),
        );
        let pages = plan_full(&html);
        let pi = &pages[0].images[0];
        // CSS width 60px → 60pt; aspect-preserved height drops to 60pt.
        assert!((pi.w - 60.0).abs() < 0.5, "expected CSS to override attr, got w={}", pi.w);
    }

    #[test]
    fn img_with_broken_src_falls_through_to_alt_text() {
        // No data URL — the integrator emits a synthetic text block carrying
        // the alt attribute, which the text path then renders.
        let pages = plan(r#"<img src="https://example.com/nope.png" alt="missing image">"#);
        let texts: Vec<&str> = pages[0].iter().map(|l| l.text.as_str()).collect();
        assert!(
            texts.contains(&"missing image"),
            "expected alt text 'missing image' in {texts:?}"
        );
    }

    #[test]
    fn img_with_broken_src_and_no_alt_drops_silently() {
        let pages = plan_full(r#"<img src="garbage">"#);
        assert!(
            pages.is_empty() || pages[0].is_empty(),
            "expected no output for broken-src no-alt, got {pages:?}"
        );
    }

    #[test]
    fn img_too_tall_proportionally_shrinks_to_page_height() {
        // 1×1 intrinsic, but force a 9999pt-tall layout via height attr.
        // page_content_height in plan_full is 800 - 36 = 764.
        let html = format!(
            r#"<img src="data:image/png;base64,{}" width="9999" height="9999">"#,
            b64(TINY_PNG_BYTES),
        );
        let pages = plan_full(&html);
        let pi = &pages[0].images[0];
        // Aspect 1:1, capped at content_height; pad/border are 0 here.
        assert!(pi.h <= 764.0 + 0.01, "expected h ≤ 764pt, got {}", pi.h);
        assert!((pi.w - pi.h).abs() < 0.5, "aspect must be preserved");
    }
```

- [ ] **Step 3: Run the tests to confirm they fail**

Run: `cargo test -p quickpdf-core --lib tests::img_with_data_url_emits_placed_image`
Expected: FAIL — Block::Image is still `continue`d.

- [ ] **Step 4: Implement the Block::Image branch**

In `plan_pages_styled` (lib.rs), find the `match block { ... }` that Task 10 introduced at the top of the `for block in blocks` loop. The current shape is:

```rust
    for block in blocks {
        let para = match block {
            parse::Block::Text(t) => t,
            parse::Block::Image(_) => continue,
        };
        // ... existing text-rendering body uses `para` and `block` ...
    }
```

Replace **only** the inner `match block { ... };` discriminator. The text-rendering body is unchanged. The new discriminator:

```rust
        let para = match block {
            parse::Block::Text(t) => t,
            parse::Block::Image(img_block) => {
                place_image_block(
                    doc, block, img_block, user_rules, inline,
                    left_margin, content_width, bottom_limit, page_content_height,
                    &mut pages, &mut current, &mut cursor_y,
                )?;
                continue;
            }
        };
```

The image branch delegates to a new `place_image_block` helper (added in Step 5). The text-rendering body that follows the match is untouched.

- [ ] **Step 5: Implement `place_image_block`**

Add a new helper function in `lib.rs`, immediately after `plan_pages_styled` (around line 405):

```rust
/// Phase 2a image-block layout. Decodes the data URL, computes the target
/// box, applies paint-as-unit pagination, and emits a `PlacedImage` plus
/// optional `PlacedBox` for any background/border.
///
/// On decode failure, returns `Ok(())` after either emitting a synthetic
/// text block carrying the `alt` attribute (which the caller did NOT plan
/// because the text path is single-pass) or dropping silently. Phase 2a
/// keeps the alt-fallback behavior simple by letting the caller restart
/// the loop with an injected synthetic text block — but to avoid a global
/// refactor, this helper instead emits the alt directly into the current
/// page using the same line-placement primitives the text path uses.
#[allow(clippy::too_many_arguments)]
fn place_image_block(
    doc: &parse::Document,
    block: &parse::Block,
    img_block: &parse::ImageBlock,
    user_rules: &[style::sheet::Rule],
    inline: &style::InlineStyles<'_>,
    left_margin: f32,
    content_width: f32,
    bottom_limit: f32,
    page_content_height: f32,
    pages: &mut Vec<PagePlan>,
    current: &mut PagePlan,
    cursor_y: &mut Option<f32>,
) -> Result<(), Error> {
    let style = match doc.element_for(block) {
        Some(elem) => style::resolve(elem, user_rules, inline),
        None => style::ua_style("img"),
    };
    let font_size = DEFAULT_FONT_SIZE_PT * style.font_size_em;
    let line_height = font_size * DEFAULT_LINE_HEIGHT;
    let pad_top = font_size * style.padding_top_em;
    let pad_right = font_size * style.padding_right_em;
    let pad_bot = font_size * style.padding_bottom_em;
    let pad_left = font_size * style.padding_left_em;
    let border_w = font_size * style.border_width_em;

    // Decode the data URL. Anything other than Some(_) → alt fallback.
    let decoded = crate::image::parse_data_url(&img_block.src);
    let krilla_img = decoded.and_then(|(kind, bytes)| {
        let data: krilla::Data = bytes.into();
        match kind {
            crate::image::ImageKind::Png => KrillaImage::from_png(data, false).ok(),
            crate::image::ImageKind::Jpeg => KrillaImage::from_jpeg(data, false).ok(),
        }
    });
    let krilla_img = match krilla_img {
        Some(i) => i,
        None => {
            // Alt fallback. Emit alt text as a synthetic line at the current
            // cursor position using the same text-flow logic the text path
            // would use for an anonymous paragraph at default style.
            if let Some(alt) = img_block.alt.as_deref().filter(|s| !s.is_empty()) {
                let metrics = text::TextMetrics::new(font::FALLBACK_TTF, font_size)
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
                    });
                    *cursor_y = Some(final_y + line_height);
                }
            }
            return Ok(());
        }
    };

    // Compute target box dimensions. Resolution order matches the spec.
    let (w_px, h_px) = krilla_img.size();
    let intrinsic_w = w_px as f32;
    let intrinsic_h = h_px as f32;
    let aspect = if intrinsic_h > 0.0 { intrinsic_w / intrinsic_h } else { 1.0 };

    let css_w = style.width_em.map(|e| font_size * e);
    let css_h = style.height_em.map(|e| font_size * e);
    let attr_w = img_block.width_attr;
    let attr_h = img_block.height_attr;

    let chosen_w = css_w.or(attr_w);
    let chosen_h = css_h.or(attr_h);

    let (mut target_w, mut target_h) = match (chosen_w, chosen_h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, if aspect > 0.0 { w / aspect } else { intrinsic_h }),
        (None, Some(h)) => (h * aspect, h),
        (None, None) => {
            // Intrinsic, capped at content_width.
            let inner_w = (content_width - 2.0 * border_w - pad_left - pad_right).max(1.0);
            if intrinsic_w > inner_w {
                let scale = inner_w / intrinsic_w;
                (intrinsic_w * scale, intrinsic_h * scale)
            } else {
                (intrinsic_w, intrinsic_h)
            }
        }
    };

    // Paint-as-unit pagination + oversize shrink.
    let block_height_total =
        |h: f32| pad_top + h + pad_bot + 2.0 * border_w;

    if cursor_y.is_none() {
        *cursor_y = Some(MARGIN_PT + line_height);
    }
    let top_margin_pt = font_size * style.margin_top_em;
    let pre_top = cursor_y.unwrap() + top_margin_pt + line_height * PARAGRAPH_GAP_LINES;
    let candidate_top = pre_top - line_height;

    let total_h = block_height_total(target_h);
    let box_top = if candidate_top + total_h <= bottom_limit {
        candidate_top
    } else if total_h <= page_content_height {
        pages.push(std::mem::take(current));
        *cursor_y = Some(MARGIN_PT + line_height);
        MARGIN_PT
    } else {
        // Oversize: proportionally shrink so block_height_total fits.
        let max_image_h = (page_content_height - pad_top - pad_bot - 2.0 * border_w).max(1.0);
        let scale = max_image_h / target_h;
        target_h = max_image_h;
        target_w *= scale;
        pages.push(std::mem::take(current));
        *cursor_y = Some(MARGIN_PT + line_height);
        MARGIN_PT
    };

    // Optional decoration box.
    let has_decoration = style.background_color.is_some() || border_w > 0.0;
    let box_width = (content_width).max(1.0);
    if has_decoration {
        let stroke = if border_w > 0.0 {
            Some((style.border_color, border_w))
        } else {
            None
        };
        current.boxes.push(PlacedBox {
            x: left_margin,
            y: box_top,
            w: box_width,
            h: total_h,
            fill: style.background_color,
            stroke,
        });
    }

    let img_x = left_margin + border_w + pad_left;
    let img_y = box_top + border_w + pad_top;
    current.images.push(PlacedImage {
        x: img_x,
        y: img_y,
        w: target_w,
        h: target_h,
        image: krilla_img,
    });

    *cursor_y = Some(box_top + total_h);
    if let Some(y) = cursor_y.as_mut() {
        *y += font_size * style.margin_bottom_em;
    }
    Ok(())
}
```

- [ ] **Step 6: Wire `Surface::draw_image` into the page-emit loop**

In `lib.rs::html_to_pdf` (the page-emit loop, around line 161), find the surface-paint sequence:

```rust
            // Paint background boxes first so backgrounds sit behind text.
            for b in &page_plan.boxes {
                paint_box(&mut surface, b);
            }
```

Add immediately after that loop:

```rust
            // Paint images between boxes and text — backgrounds behind, text
            // potentially overlapping in front.
            for img in &page_plan.images {
                let Some(target) = krilla::geom::Size::from_wh(img.w, img.h) else {
                    continue;
                };
                surface.push_transform(&Transform::from_translate(img.x, img.y));
                surface.draw_image(img.image.clone(), target);
                surface.pop();
            }
```

- [ ] **Step 7: Run the integration tests**

Run: `cargo test -p quickpdf-core --lib`
Expected: all tests PASS — including the seven new image-block tests.

- [ ] **Step 8: Commit**

```bash
git add crates/quickpdf-core/src/lib.rs
git commit -m "Phase 2a integrator: render Block::Image with sizing + pagination + alt fallback"
```

### Task 13: Test that decorated `<img>` paints a `PlacedBox` surround

**Files:**
- Modify: `crates/quickpdf-core/src/lib.rs` (append to `mod tests`)

- [ ] **Step 1: Append a decoration test inside `mod tests`, just before its closing `}`:**

```rust
    #[test]
    fn img_with_background_color_emits_box_around_image() {
        let html = format!(
            r#"<style>img {{ background-color: yellow; padding: 6px; }}</style>
<img src="data:image/png;base64,{}" width="40" height="20">"#,
            b64(TINY_PNG_BYTES),
        );
        let pages = plan_full(&html);
        assert_eq!(pages[0].images.len(), 1);
        assert_eq!(pages[0].boxes.len(), 1, "expected one decoration box");
        let b = &pages[0].boxes[0];
        assert_eq!(b.fill, Some(Color::rgb(255, 255, 0)));
        // Padding pushes the image inward by 6pt on each axis.
        let img = &pages[0].images[0];
        assert!((img.x - (b.x + 6.0)).abs() < 0.5);
        assert!((img.w - 40.0).abs() < 0.5);
    }
```

- [ ] **Step 2: Run**

Run: `cargo test -p quickpdf-core --lib tests::img_with_background_color_emits_box_around_image`
Expected: PASS (the implementation in Task 12 already supports this).

- [ ] **Step 3: Commit**

```bash
git add crates/quickpdf-core/src/lib.rs
git commit -m "Phase 2a integrator: test decorated <img> emits surround box"
```

---

## Phase 5 — Python integration tests

### Task 14: Python integration tests for image rendering

**Files:**
- Modify: `tests/test_render.py` (append a new section at the end)

- [ ] **Step 1: Build the native module against the new code**

Run: `.venv/Scripts/maturin.exe develop --release`
Expected: builds clean.

- [ ] **Step 2: Append Phase 2a tests to `tests/test_render.py`**

At the end of `tests/test_render.py`, append:

```python
# --- Phase 2a: block-level images via data: URLs --------------------------

# Tiny known-good PNG (1x1 red pixel). Same constant the Rust tests use.
_TINY_PNG = bytes([
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00,
    0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
    0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89,
    0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63,
    0xfc, 0xcf, 0xc0, 0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a,
    0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
])


def _png_data_url() -> str:
    import base64
    return "data:image/png;base64," + base64.b64encode(_TINY_PNG).decode("ascii")


def test_pdf_with_data_url_image_contains_image_xobject():
    # An /XObject of /Subtype /Image must appear in the PDF when the
    # input HTML carries a valid data: URL.
    html = f'<img src="{_png_data_url()}" width="100" height="50">'
    pdf = quickpdf.html_to_pdf(html)
    assert pdf[:5] == b"%PDF-"
    # The image XObject is present in the raw PDF body (krilla emits
    # `/Subtype /Image` in the dict header).
    assert b"/Subtype /Image" in pdf, (
        "expected /Subtype /Image XObject in PDF for data: URL"
    )


def test_pdf_with_broken_src_renders_alt_text():
    # No data: URL — the renderer falls back to the alt attribute as text.
    html = '<img src="https://example.com/nope.png" alt="missing image">'
    pdf = quickpdf.html_to_pdf(html)
    text = _pdf_text(pdf)
    assert "missing image" in text, (
        f"expected alt-text fallback, got {text!r}"
    )
    # And no image XObject was emitted.
    assert b"/Subtype /Image" not in pdf, (
        "broken src must not emit an Image XObject"
    )


def test_pdf_with_broken_src_and_no_alt_renders_blank():
    pdf = quickpdf.html_to_pdf('<img src="garbage">')
    assert pdf[:5] == b"%PDF-"
    text = _pdf_text(pdf).strip()
    # The page is otherwise empty — no image, no alt.
    assert text == "", f"expected empty render, got {text!r}"
    assert b"/Subtype /Image" not in pdf


def test_pdf_image_inside_paragraph_splits_into_three_blocks():
    # <p>before <img> after</p> renders as: "before" text → image → "after" text.
    html = (
        f'<p>before <img src="{_png_data_url()}" width="50" height="50"> after</p>'
    )
    pdf = quickpdf.html_to_pdf(html)
    text = _pdf_text(pdf)
    assert "before" in text
    assert "after" in text
    assert b"/Subtype /Image" in pdf


def test_pdf_image_with_css_width_renders():
    # CSS sizing path: width: 60px overrides the HTML width="120" attr.
    html = (
        '<style>img { width: 60px; }</style>'
        f'<img src="{_png_data_url()}" width="120" height="120">'
    )
    pdf = quickpdf.html_to_pdf(html)
    assert pdf[:5] == b"%PDF-"
    assert b"/Subtype /Image" in pdf


def test_pdf_image_with_decoration_emits_fill_and_image():
    # padding + background → both a fill rect AND an image XObject.
    html = (
        '<style>img { background-color: yellow; padding: 6px; }</style>'
        f'<img src="{_png_data_url()}" width="40" height="20">'
    )
    pdf = quickpdf.html_to_pdf(html)
    streams = _pdf_content_streams(pdf)
    assert "1 1 0 rg" in streams, "expected yellow background fill"
    assert b"/Subtype /Image" in pdf
```

- [ ] **Step 3: Run pytest**

Run: `.venv/Scripts/python.exe -m pytest tests/ -q`
Expected: all tests PASS — the existing 44 plus 6 new ones.

- [ ] **Step 4: Commit**

```bash
git add tests/test_render.py
git commit -m "Phase 2a integrator: Python integration tests for <img> rendering"
```

---

## Phase 6 — Wrap-up

### Task 15: Update CLAUDE.md roadmap

**Files:**
- Modify: `CLAUDE.md` (the roadmap table + "Next session" prose)

- [ ] **Step 1: Update the roadmap table**

In `CLAUDE.md`, find the roadmap table. Locate the row for Phase 2 (currently `→`). Replace it with two rows:

```markdown
|  2a  |   ✓    | Block-level `<img>` (PNG/JPEG via `data:` URL, HTML+CSS sizing, alt fallback) |
|  2b  |   →    | **NEXT.** Web fonts via `@font-face`                                                |
|  2c  |        | Tables (`<table>`/`<tr>`/`<td>`) — proper 2D layout                                  |
```

(Adjust the existing Phase 2 row by replacing it with these three rows; the original `| 2 | → | tables, images, web fonts → renders email-style HTML |` line goes away.)

Also update the "Test posture today" prose under the table to reflect the new totals:

> **Test posture today:** ~190 Rust unit tests + ~50 Python integration tests, all green in ~0.5 s combined.

- [ ] **Step 2: Update the "Next session" section**

In `CLAUDE.md`, find the "Next session: Phase 2" section. Replace its body with:

```markdown
## Next session: Phase 2b — web fonts

Phase 2a is complete. Phase 2b adds `@font-face` parsing in
`sheet.rs::skip_at_rule` (currently dropped) and threads custom
`Font` instances through `font.rs` and the planner. Slice plan to
follow once brainstorming nails down scope (system-font fallback?
Embed only Latin subset of `@font-face` payloads? Rejection on
unsupported font formats?).

Cross-cutting Phase 2a artefacts to keep in mind for future phases:

- `parse::Block` enum is the canonical block-level stream now. New
  block-level features (videos, embeds, eventually tables) add
  variants to `Block` rather than back-doors through `Paragraph`.
- `style::BlockStyle::width_em` / `height_em` are present but only
  consumed by `<img>` today. Tables (Phase 2c) and any future
  fixed-width container will read the same fields — no cascade
  changes needed.
- Image data URLs are decoded via krilla's own decoders
  (`Image::from_png`, `Image::from_jpeg`); no `image` crate
  dependency. WebP/GIF would only need `from_webp`/`from_gif`
  arms in `place_image_block` and corresponding MIME entries in
  `image::parse_data_url`.
- Percentage (`%`) widths/heights on `<img>` are explicitly dropped
  by the cascade. A future `enum Length { Em, Percent }` upgrade
  unblocks them globally.
```

- [ ] **Step 3: Commit the docs update**

```bash
git add CLAUDE.md
git commit -m "Phase 2a: mark roadmap done, point next session at Phase 2b"
```

### Task 16: Final test sweep + clean check

**Files:** none modified — purely verification.

- [ ] **Step 1: Run the full Rust suite**

Run: `cargo test -p quickpdf-core --lib`
Expected: all tests PASS — approximately 190 total. Note the count and add to the `CLAUDE.md` test posture line if it differs from the rough estimate above.

- [ ] **Step 2: Type-check with no warnings**

Run: `cargo check -p quickpdf-core`
Expected: builds clean, zero warnings.

- [ ] **Step 3: Rebuild the wheel and run pytest**

```bash
.venv/Scripts/maturin.exe develop --release
.venv/Scripts/python.exe -m pytest tests/ -q
```

Expected: all tests PASS — approximately 50 total.

- [ ] **Step 4: If any test count is off, update CLAUDE.md**

If the Rust test count differs from "~190" or the Python count differs from "~50", edit `CLAUDE.md`'s test posture line with the actual numbers and amend Task 15's commit:

```bash
git add CLAUDE.md
git commit --amend --no-edit
```

(If the previous commit is already pushed, create a new commit instead with `git commit -m "Phase 2a: update test counts in CLAUDE.md"`.)

- [ ] **Step 5: Confirm the working tree is clean**

Run: `git status`
Expected: clean tree, all Phase 2a commits on `main`.

---

## Self-review notes

After writing the plan I scanned for the writing-plans skill's red flags:

1. **Spec coverage:** Every section of the spec has a task — data URL parser (Task 3-4), Block enum (Task 5-7), cascade (Task 8-9), Block::Image branch (Task 12), pagination + shrink (Task 12), decorated boxes (Task 13), error matrix (Task 12 "broken src" tests + Task 14 Python tests), CLAUDE.md update (Task 15). The only spec item the plan deviates on is the `image` crate dependency — replaced with krilla's native PNG/JPEG support, documented in the plan header.

2. **Placeholder scan:** No `TBD`/`TODO`. Every code block is complete and runnable. Each step has the actual command and expected output.

3. **Type consistency:** `Block`/`TextBlock`/`ImageBlock` shapes match between Slice B's struct definitions, Slice C's cascade arms, and the integrator's `place_image_block`. `width_em: Option<f32>` consistent. `parse_data_url` signature consistent across image.rs and the integrator's caller. `KrillaImage::from_png(data: Data, false)` matches krilla 0.7 source.

4. **Ambiguity:** The integrator's "alt fallback" is a known nuance — Task 12's helper inlines text directly into the current page rather than synthesizing a `Block::Text` and re-entering the loop. The simpler inline path was chosen to avoid a global refactor of the loop's control flow. Documented in the helper's comment.

5. **Slice ordering:** Slices A, B, C can run in parallel after Setup (Task 1). Setup is the only sequential prerequisite. The integrator phase MUST run last and serially — its first task (Task 10) restores green by reconciling Slice B's rename.
