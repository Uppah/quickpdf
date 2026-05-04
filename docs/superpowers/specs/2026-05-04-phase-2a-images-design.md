# Phase 2a — Block-level images

**Status:** approved 2026-05-04
**Author:** Claude (Opus 4.7) brainstorming session with Uppah
**Predecessor:** Phase 1.7c (commit `e3f9d42`)
**Successors planned:** Phase 2b (web fonts) → Phase 2c (tables)

## Mission

Add block-level `<img>` rendering with PNG and JPEG support, sourced
exclusively from `data:` URLs. The renderer must continue to be a
pure function: no I/O, no network, no system-font dependency. The
sub-phase ships when an HTML document containing inline-base64
images renders to a PDF whose extracted content includes the
correct Image XObjects.

## Scope (locked)

| Decision | Locked answer |
| --- | --- |
| Phase 2 split & order | Phase 2a images → 2b web fonts → 2c tables |
| Image formats | PNG + JPEG |
| Image sources | `data:` URLs only — no HTTP, no `file://`, no relative paths |
| Layout level | Always block-level (`<img>` joins `is_block`) |
| Sizing inputs | HTML `width`/`height` attrs **and** CSS `width`/`height` longhands; CSS overrides attrs |
| Decoder crate | `image = "0.25"` with `default-features = false`, `features = ["png", "jpeg"]` |

## Explicit non-goals (deferred)

- `max-width`, `min-width`, `height: auto`, `object-fit`, `object-position`
- HTTP/HTTPS fetch, `file://`, relative path resolution
- Inline replaced-element layout (image flowing within a line of text — Phase 4 territory)
- Percentage (`%`) widths/heights on `<img>` — needs a richer `Length` type to resolve against the containing block; deferred to a later phase that touches every `*_em` field
- GIF, WebP, AVIF, BMP, TIFF, SVG
- `srcset`, `<picture>`, lazy-load semantics
- Image caching across `html_to_pdf` calls (matters for bulk in Phase 3, not 2a)

## Architecture

### 1. Data model — `parse::Block` enum

`Paragraph` is renamed to `TextBlock` and joined under a `Block`
enum. The new `ImageBlock` variant carries every input the layout
pass needs to render an image without re-walking the DOM.

```rust
pub enum Block {
    Text(TextBlock),
    Image(ImageBlock),
}

pub struct TextBlock {
    pub tag: String,
    pub text: String,
    pub element_id: NodeId,
}

pub struct ImageBlock {
    pub element_id: NodeId,
    pub src: String,
    pub width_attr: Option<f32>,   // HTML `width` attr, in CSS px
    pub height_attr: Option<f32>,  // HTML `height` attr, in CSS px
    pub alt: Option<String>,
}
```

`Document::paragraphs()` is replaced by `Document::blocks() ->
Vec<Block>`. Adding `img` to `parse::is_block` makes the existing
1.6c anonymous-block logic split text around images for free —
`<p>foo <img> bar</p>` becomes three `Block`s in document order.

The `ANONYMOUS_TAG` sentinel and `Document::element_for(&Block)`
behavior are preserved. `element_for` becomes generic over the
block variant: it reads `element_id` regardless of the variant.

### 2. Image decoder — `crates/quickpdf-core/src/image.rs` *(new)*

```rust
pub enum ImageKind { Png, Jpeg }

pub struct ImageData {
    pub kind: ImageKind,
    pub bytes: Vec<u8>,        // raw, undecoded — krilla decodes at emit time
    pub width_px: u32,
    pub height_px: u32,
}

/// Parse a `data:image/png;base64,...` or `data:image/jpeg;base64,...`
/// URL into validated bytes plus intrinsic dimensions. Returns `None`
/// for any failure: missing prefix, unsupported MIME, malformed base64,
/// truncated header, decoder rejection.
pub fn parse_data_url(src: &str) -> Option<ImageData>;
```

The decoder uses the `image` crate's reader to validate the header
and pull width/height; it does **not** fully decode pixel data at
parse time. krilla performs the full decode at emit. This keeps
parse cheap and lets us reject bogus images early.

Base64 decoding uses the `base64` crate (`base64 = "0.22"`). New
deps land in `[workspace.dependencies]` and are referenced from
`crates/quickpdf-core/Cargo.toml`'s `[dependencies]` — same
pattern as the existing `krilla` / `scraper` / `skrifa` /
`ego-tree` entries. If `Cargo.lock` shows that a transitive dep
already pulls a `base64` minor version, Slice A pins to that
version to avoid duplicate trees in the binary (the same
discipline applied to `skrifa` for krilla).

### 3. Cascade plumbing — `style::BlockStyle`

```rust
// New fields, both defaulting to None:
pub width_em:  Option<f32>,
pub height_em: Option<f32>,
```

`BlockStyleBuilder` learns `width` and `height` longhands using the
existing `parse_length_em` helper, but with one targeted exclusion.
Behavior:

- `px`/`pt`/`em`/`rem` units parse via `parse_length_em`, just like
  `padding-*` and `border-width` already do.
- **`%` is rejected for `width` and `height` in Phase 2a.** The
  current `parse_length_em` converts `N%` to `N/100` em — fine for
  `font-size: 150%` (because font-size is itself an em multiplier),
  but wrong for `width: 50%` which CSS defines as 50% of the
  containing block's width. Honoring percentage widths correctly
  requires a richer length type (`enum Length { Em(f32),
  Percent(f32) }`) that layout can resolve against the
  content-area, and that's bigger than the Phase 2a budget.
  Slice C therefore special-cases `width`/`height`: if the value
  ends in `%`, return `None` from `parse_value`. The declaration
  is silently dropped, and layout falls through to HTML attrs or
  intrinsic dimensions.
- Inline `style="..."` and author rules feed through the same path.

### 4. Layout & paint — `lib::plan_pages_styled`

A new `PlacedImage` paint primitive joins `PlacedLine` and
`PlacedBox`:

```rust
struct PlacedImage {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    image: ImageData,
}
```

`PagePlan` gains an `images: Vec<PlacedImage>` field, painted
**between** boxes and lines (so a background paints behind an
image, but the image paints behind any subsequent text).

`plan_pages_styled` matches on the block variant:

- **`Block::Text`** — existing path, no behavioral change.
- **`Block::Image`** — new path:
  1. Resolve cascade against `image_block.element_id` (already
     handled by `style::resolve`).
  2. Decode `src` via `image::parse_data_url`. If `None`:
     - If `alt` is present → emit a synthetic `TextBlock` with
       `tag = ANONYMOUS_TAG`, `text = alt`, the image's
       `element_id`, and re-enter the text path.
     - Else → drop the block silently.
  3. Compute target box dimensions (see below).
  4. Apply paint-as-unit pagination (see below).
  5. Emit `PlacedImage` plus, if the cascade resolved any
     `background-color` / `border-*` / `padding-*`, a wrapping
     `PlacedBox` exactly like text blocks today.

#### Sizing resolution (per dimension, first match wins)

1. CSS `width_em` / `height_em` from `BlockStyle`, multiplied by
   the block's resolved font size.
2. HTML `width_attr` / `height_attr` (CSS px = PDF pt at our
   1:1 conversion baseline).
3. If only one dimension is set, derive the other using the
   intrinsic aspect ratio (`width_px / height_px`).
4. If neither is set, use `width_px` and `height_px` as-is, then
   if `width > content_width` shrink proportionally so width fits.

#### Pagination (paint-as-unit, mirrors 1.7b)

Let `block_height = pad_top + h + pad_bot + 2*border_w`,
`page_content_height = bottom_limit - MARGIN_PT`.

- `block_height ≤ remaining` on current page → place on current page.
- Else if `block_height ≤ page_content_height` → flush current
  page, start fresh page, place at top.
- Else → proportionally shrink the image so
  `pad_top + h_shrunk + pad_bot + 2*border_w == page_content_height`,
  flush, place on fresh page. The original aspect ratio is
  preserved by computing `w_shrunk = w * (h_shrunk / h)`.

This is strictly stronger than the 1.7b text path's "fall back to
streaming without box" — images cannot be split, so they shrink.
Documented as a Phase 2a behavior; full image pagination via
fragmentation is not on the roadmap.

### 5. Krilla integration

`krilla::image::Image` and `Surface::draw_image()` are the public
entrypoints. Decoded images are constructed lazily inside
`plan_pages_styled` to avoid holding undecoded bytes across the
two-pass planner / paint split.

Krilla 0.7's image API at the time of this spec:
- `krilla::image::Image::from_png(&[u8]) -> Option<Image>`
- `krilla::image::Image::from_jpeg(&[u8]) -> Option<Image>`

If the krilla version pinned in `Cargo.lock` exposes a different
API, the integrator updates this section in the next commit
without re-running brainstorming — krilla is a leaf dependency
and any signature delta is a mechanical change.

## Data flow

```
HTML string
  │
  ▼
parse::Document::parse  ──►  html5ever DOM
  │
  ▼
Document::blocks()   ──►  Vec<Block>      (Text + Image variants)
Document::user_stylesheet()      ──►  Vec<Rule>
Document::inline_styles()        ──►  Vec<(NodeId, Vec<Declaration>)>
  │
  ▼
plan_pages_styled
  for each Block:
    ├── style::resolve(elem, rules, inline)  ──►  BlockStyle
    │       (width_em, height_em are new fields)
    │
    ├── Block::Text  ──►  text::wrap_lines  ──►  PlacedLine[s]
    │
    └── Block::Image ──►  image::parse_data_url
                          ├── None + alt   ──►  synthetic TextBlock
                          ├── None         ──►  drop
                          └── Some(data)   ──►  size resolve, paginate,
                                                emit PlacedImage (+ optional PlacedBox)
  │
  ▼
Vec<PagePlan>  (boxes, images, lines per page)
  │
  ▼
krilla emit:
  for each page:
    paint boxes  →  paint images (Surface::draw_image)  →  draw text
```

## Error handling matrix

| Condition | Behavior | Test |
| --- | --- | --- |
| `<img>` missing `src` | Render `alt` if present; else drop | `parse.rs::img_missing_src_drops`, `lib.rs::img_with_alt_renders_alt` |
| `src=""` | Same as missing | `parse.rs::img_empty_src_drops` |
| Non-data-URL src | Same as missing | `lib.rs::img_http_src_falls_through_to_alt` |
| Malformed data URL | `parse_data_url` → `None`; same as missing | `image.rs::malformed_url_returns_none` |
| Unsupported MIME (e.g. `image/gif`) | `parse_data_url` → `None`; same as missing | `image.rs::gif_mime_returns_none` |
| Decoder rejects bytes | `parse_data_url` → `None`; same as missing | `image.rs::decoder_rejection_returns_none` |
| Image too tall to fit any page | Proportional shrink to page content height | `lib.rs::oversize_image_shrinks` |
| Image with `background-color` and `border` | `PlacedBox` paints around the image | `lib.rs::img_with_decoration_paints_box` |

## Testing posture

| Layer | Approx test count delta | Where |
| --- | --- | --- |
| `image.rs` unit | +12 (success cases for PNG/JPEG, all the `None` paths in the matrix) | `image.rs` `#[cfg(test)]` |
| `parse.rs` unit | +8 (`<img>` is its own block, splits paragraphs, alt/width/height capture, anonymous interaction) | `parse.rs` `#[cfg(test)]` |
| `style/cascade.rs` unit | +4 (`width: 200px`, `height: 5em`, `width: 50%`, inline override) | `cascade.rs` `#[cfg(test)]` |
| `lib.rs` integration | +5 (paints PlacedImage, oversize shrinks, CSS beats attrs, alt fallback, decorated img) | `lib.rs` `#[cfg(test)]` |
| Python | +6 (data URL renders, oversized renders, alt-fallback path produces text, multi-image pagination, missing-src drop, decoded XObject roundtrip) | `tests/test_render.py` |

Target totals after merge: **~190 Rust unit + ~50 Python integration**, up from 161 + 44.

A small (~1 KB) PNG test fixture and a small JPEG test fixture
live in `crates/quickpdf-core/assets/test-fixtures/` — added by
Slice A and reused across all layers via base64-encoded constants
in tests.

## Sprint structure

Mirrors the 4-agent parallel-sprint pattern proven in Phases 1.6
and 1.7. Contracts artifact: `.claude-2a-contracts.md` (gitignored).

| Agent | Owns | Hard "don't touch" constraint |
| --- | --- | --- |
| **Plan** | `.claude-2a-contracts.md` | Writes interface contracts for slices A/B/C; does not write implementation. |
| **Slice A** | `crates/quickpdf-core/src/image.rs` *(new)*, `Cargo.toml` (new `image` and `base64` deps), `crates/quickpdf-core/assets/test-fixtures/*.{png,jpg}` *(new)* | No edits to `parse.rs`, `style/`, `lib.rs`, or any Python file. |
| **Slice B** | `crates/quickpdf-core/src/parse.rs` (Block enum, `is_block` adds `img`, `Document::blocks()` rename + ImageBlock construction) | No edits to `image.rs`, `style/`, `lib.rs`, or Python. May break `lib.rs` test compilation — that's the integrator's fixup. |
| **Slice C** | `crates/quickpdf-core/src/style/mod.rs`, `crates/quickpdf-core/src/style/cascade.rs` (BlockStyle width_em/height_em, builder pickup) | No edits to `parse.rs`, `image.rs`, `lib.rs`, or Python. |
| **Integrator** *(main thread)* | `lib.rs` plan/paint, krilla `draw_image` integration, all cross-file fixups, full test sweep, final commit | Reconciles Slice B breakage in `lib.rs` and any other unrelated touches needed to make `cargo test -p quickpdf-core --lib` and `pytest tests/ -q` both green. |

`cargo check -p quickpdf-core` is the green-bar gate for slice
agents (skips `#[cfg(test)]` bodies, immune to integrator-only
fixups). The integrator's gate is the full test command.

## Risks & open questions

1. **Krilla image API drift.** If `krilla 0.7`'s `Image::from_png` /
   `from_jpeg` signatures differ from the spec text, the integrator
   updates the integration without re-brainstorming. Likely a
   2-line change.
2. **Percentage widths are silently dropped.** `width: 50%` on an
   `<img>` falls through to HTML attrs or intrinsic dimensions in
   Phase 2a. The cascade does not preserve the `%` semantic; a
   future phase introducing `enum Length { Em(f32), Percent(f32) }`
   will let layout resolve it against the containing block.
   Documented in the non-goals list and in `parse_value`'s
   width/height special case.
3. **Test fixture provenance.** The PNG/JPEG fixtures must be
   author-licenced (CC0 or public domain) and small (≤ 1 KB
   each). Slice A picks fixtures from existing OFL/CC0 sources or
   generates them with `image` itself.
4. **`base64` crate version.** If a transitive dependency already
   pulls a `base64` version, Slice A pins to the same minor
   version to avoid duplicate trees in the binary (the same
   discipline we apply to `skrifa` for krilla).

## Definition of done

- `cargo test -p quickpdf-core --lib` passes with the new tests.
- `pytest tests/ -q` passes with the new Python integration tests.
- `cargo check -p quickpdf-core` is clean (no warnings).
- A real-world payload (a 600×400 PNG hero + 2 paragraphs of text)
  renders to a single-page PDF whose decoded content includes
  exactly one Image XObject of the correct dimensions.
- CLAUDE.md roadmap table marks Phase 2a as ✓.
- One commit per slice (or one squashed commit for the full
  sprint), title format `Phase 2a: <slice description>`, signed
  off as a single integration commit on `main`.
