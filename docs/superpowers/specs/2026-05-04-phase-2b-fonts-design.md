# Phase 2b — Web fonts via `@font-face`

**Status:** approved 2026-05-04
**Author:** Claude (Opus 4.7) brainstorming session with Uppah
**Predecessor:** Phase 2a (commit `79134d0`)
**Successors planned:** Phase 2c (tables) → Phase 3 (BulkSession + wheel)

## Mission

Honor `@font-face` rules in author CSS so HTML emitting brand-fonts
renders in those fonts instead of the bundled Inter fallback. Font
bytes are sourced exclusively from `data:` URLs inside the HTML,
mirroring Phase 2a's posture for `<img>`. The wheel stays
self-contained: no network, no system-font probing, no new C/Rust
deps. The sub-phase ships when an HTML document declaring
`@font-face` with a base64'd TTF and a `font-family: <name>` on a
paragraph renders to a PDF whose embedded font name matches the
declared family.

## Scope (locked)

| Decision | Locked answer |
| --- | --- |
| Font sources | `data:` URLs only — no `http(s)://`, no `file://`, no `local()` |
| Font formats | TTF + OTF only (sniffable as `0x00010000` / `OTTO`); WOFF/WOFF2 srcs skipped |
| MIME acceptance | Permissive: `font/ttf`, `font/otf`, `application/font-sfnt`, `application/x-font-ttf`, `application/x-font-otf`, `application/octet-stream`. Magic-byte sniff is the tiebreaker for `octet-stream` and any otherwise-accepted MIME |
| System-font fallback | None. Unknown family → bundled Inter |
| Public API | HTML-only. No Python kwargs added in 2b |
| `font-family` cascade | New `BlockStyle.font_family: Option<Vec<String>>`, inherited per CSS spec |
| Last-wins on duplicate `@font-face` family | Yes (matches existing cascade discipline) |

## Explicit non-goals (deferred)

- Bold/italic descriptors on `@font-face` (`font-weight`, `font-style`).
  One face per family; later `@font-face` for the same name overrides.
- WOFF / WOFF2 decoding (Brotli + table reordering — defer to a future
  phase if a real use case demands it).
- `local()` srcs and any system font enumeration.
- HTTP / HTTPS / `file://` font fetching.
- `unicode-range`, `font-display`, `size-adjust`, `ascent-override`
  and other modern descriptors.
- Inline `font-family` (`<span style="font-family: Brand">`) — paragraph-
  level only, same posture as every other property today.
- Glyph-level fallback chain (current behavior: missing glyphs render
  as `.notdef` or a tofu box, depending on the chosen font).
- Subsetting at our layer — krilla already subsets by used glyphs at
  PDF emission. We embed the full bytes; krilla decides what ships.
- Caller-supplied default font (i.e. swapping the Inter fallback).
- Font registry shared across `html_to_pdf` calls (Phase 3 territory).
- New Python API surface of any kind.

## Architecture

### 1. At-rule capture — `style/sheet.rs`

Today `skip_at_rule` drops every `@`-prefixed rule indiscriminately.
2b carves out `@font-face` specifically and leaves every other
at-rule (`@media`, `@import`, `@keyframes`, …) on the floor.

A new aggregate type lets callers see both kinds without disturbing
the existing rules-only API surface:

```rust
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    pub font_faces: Vec<FontFace>,
}

pub struct FontFace {
    /// Every declaration inside the @font-face block, normalized via
    /// `parse_declaration_block` (so shorthand expansion and
    /// `!important` stripping run as usual). The font registry layer
    /// fishes out `font-family` and `src` from this list.
    pub declarations: Vec<Declaration>,
    /// Source order across the full stylesheet (shared numbering with
    /// `Rule.source_order`). Used by the registry for last-wins
    /// disambiguation when two `@font-face` blocks declare the same
    /// family name.
    pub source_order: usize,
}
```

API shape (chosen to minimise test churn):

- The existing `parse_stylesheet(source: &str) -> Vec<Rule>` is
  preserved so the ~30 existing test sites continue compiling
  unchanged. Internally it forwards to `parse_stylesheet_full` and
  returns `.rules`.
- New `parse_stylesheet_full(source: &str) -> Stylesheet` is the
  primary parser. It walks the source once and emits both vectors.
- `Document::user_stylesheet() -> Vec<Rule>` is preserved (returns
  `.rules`); a new `Document::stylesheet() -> Stylesheet` returns
  the aggregate. `lib.rs::html_to_pdf` switches to `stylesheet()`.

The `@font-face` recogniser is case-insensitive on the `font-face`
keyword (real authoring tools emit `@Font-Face`, `@FONT-FACE`,
etc.). Whitespace between `@font-face` and `{` is tolerated.
Malformed `@font-face` blocks (truncated, no closer, unparseable
declarations) are dropped silently — same posture as malformed
qualified rules.

`collect_style_blocks` is unchanged (still a flat string concat).

### 2. Font registry — `font.rs` becomes the registry home

```rust
pub type FontHandle = usize;

/// One concrete font face known to the renderer, including its raw
/// bytes (for skrifa-side measurement) and the krilla-side handle
/// (for PDF emission). Held by `FontRegistry`.
pub struct RegisteredFont {
    pub bytes: std::sync::Arc<[u8]>,
    pub krilla_font: krilla::text::Font,
}

pub struct FontRegistry {
    /// Index 0 is always the bundled Inter fallback. Every lookup that
    /// can't find a match returns 0.
    pub fonts: Vec<RegisteredFont>,
    /// Lowercased family name → handle. Inter is registered as both
    /// "inter" and (for spec compliance) the empty default lookup
    /// (handled by `lookup` returning 0 when chain is empty).
    pub by_family: std::collections::HashMap<String, FontHandle>,
}

impl FontRegistry {
    /// Build a registry from the parsed `@font-face` rules. Inter is
    /// always registered first at index 0. Faces are processed in
    /// source order; duplicate family names overwrite earlier handles
    /// (last-wins). Faces with no decodable src are silently dropped.
    pub fn build(font_faces: &[FontFace]) -> Self;

    /// Walk a resolved family chain (lowercased) left-to-right and
    /// return the first registered handle. Returns 0 (Inter) if the
    /// chain is empty or no name matches.
    pub fn lookup(&self, family_chain: &[String]) -> FontHandle;
}
```

`FALLBACK_TTF` and `FALLBACK_FAMILY` stay where they are; the
registry uses them at construction.

#### `font-family` descriptor parsing (registry-side)

A single `@font-face` block is expected to declare one
`font-family`. The descriptor is a single quoted or unquoted name
(unlike the *property* `font-family`, which is a fallback list).
Real authoring tools sometimes emit `font-family: "Acme Sans",
fallback` here — we handle that by taking only the first
comma-separated entry and dropping the rest.

Normalisation: strip surrounding `"` or `'`, trim whitespace,
lowercase. Empty names (`font-family: ""`) → drop the face.

#### `src:` parsing (registry-side)

The descriptor is comma-separated. Each entry is one of:

- `url(<url>)` optionally followed by `format(<hint>)` — the only
  shape we attempt to decode.
- `local(<name>)` — silently skipped.

Tokenisation uses a small hand-roller (consistent with the rest of
`sheet.rs`): walks the string, respects parens/quotes, splits on
top-level commas. Unrecognised entries → skip and continue to the
next entry.

For each `url(...)`:

1. Strip surrounding quotes from the URL.
2. Verify it begins with `data:` (case-insensitive). Otherwise skip.
3. Parse the MIME segment up to `;`. Lowercase. Accepted set:
   - `font/ttf`, `font/otf`
   - `application/font-sfnt`
   - `application/x-font-ttf`, `application/x-font-otf`
   - `application/octet-stream`
   - Anything else → skip.
4. Verify a `;base64,` segment follows. (We don't support the
   percent-encoded form of `data:` URLs in 2b — same posture as
   Phase 2a; real-world inliners always use base64 for binary
   payloads.)
5. Base64-decode the payload via the existing `base64` dep
   (already in Cargo.toml from Phase 2a). On decode error, skip.
6. Magic-byte sniff: accept iff `bytes[0..4]` is `0x00 0x01 0x00
   0x00` (TrueType) or `b"OTTO"` (CFF/OpenType). On mismatch,
   skip. This catches WOFF/WOFF2 payloads that happened to be
   served under an accepted MIME, and rejects truncated or
   garbage data before we hand it to krilla.
7. `Arc<[u8]>` wrap → `krilla::text::Font::new(bytes.clone(), 0)`.
   On `None`, skip. On `Some(font)`, register the face and stop
   walking the src list.

If every src in a single `@font-face` block fails, the face is
silently absent from the registry and any `font-family: <that name>`
falls through to Inter. No error, no warning.

`format(...)` hints are ignored when present — we sniff bytes
ourselves. A future increment can use the hint as a fast-skip for
WOFF/WOFF2 srcs without decoding.

### 3. `font-family` cascade — `style/cascade.rs` + `style/mod.rs`

```rust
// New BlockStyle field:
pub font_family: Option<Vec<String>>,
```

`BlockStyle::DEFAULT` initialises `font_family: None`. `cascade::
inherit` propagates the parent's value when the child has none —
in Rust terms, `child.font_family.or_else(|| parent.font_family
.clone())`. This differs from the existing `inherit` arms for
`f32`/`Color` (which sentinel-compare against `DEFAULT`); for an
`Option`, `None` is the natural "no author-set value" sentinel.

`BlockStyleBuilder` learns a `"font-family"` arm in
`apply_declaration`. Value parsing:

- Tokenise on top-level commas, respecting `"..."` and `'...'`.
- For each token: trim, strip matching surrounding quotes, lowercase.
- Drop generic family keywords: `serif`, `sans-serif`, `monospace`,
  `cursive`, `fantasy`, `system-ui`, `ui-serif`, `ui-sans-serif`,
  `ui-monospace`, `ui-rounded`, `emoji`, `math`, `fangsong`. We
  have no concrete font for any of them in 2b; dropping (rather
  than mapping to Inter explicitly) keeps the chain "real names
  only" so registry lookup stays honest and a future phase can
  add real generic-family mappings without changing the cascade.
- Drop empty tokens (`font-family: ""`, `font-family: ,`).
- If the resulting list is non-empty, set
  `font_family = Some(list)`. If the list is empty after all
  drops, leave `font_family` untouched (don't overwrite an
  inherited value with `None`).

### 4. Planner & emitter — `lib.rs`

`PlacedLine` gains a font handle:

```rust
struct PlacedLine {
    y: f32,
    x: f32,
    font_size_pt: f32,
    text: String,
    color: Color,
    font_handle: FontHandle, // NEW
}
```

`html_to_pdf` builds the registry once at the top, before
`plan_pages_styled`. The existing `parsed.user_stylesheet()` call
is replaced with the new aggregate accessor:

```rust
let stylesheet = parsed.stylesheet();         // Stylesheet { rules, font_faces }
let registry = FontRegistry::build(&stylesheet.font_faces);
// ...later, plan_pages_styled is called with &stylesheet.rules in
// place of the previous &user_rules.
```

`plan_pages_styled` accepts `&FontRegistry` and uses it twice:

1. Per text block: compute `font_handle = registry.lookup(
   style.font_family.as_deref().unwrap_or(&[]))`. Build
   `text::TextMetrics::new(&registry.fonts[font_handle].bytes,
   font_size)` — wrapping uses the right glyph advances. Stamp
   `font_handle` on every `PlacedLine` produced.
2. In `place_image_block`'s alt-text fallback path: same
   resolution, same stamp on the synthetic lines.

Emit loop changes:

- The standalone `let font = Font::new(font::FALLBACK_TTF...)`
  becomes a registry-driven `Vec<&krilla::text::Font>` lookup keyed
  by `line.font_handle`.
- The existing `current_color` switch optimisation extends to a
  parallel `current_font_handle` so we only re-set the font when
  the handle changes between consecutive lines.

### 5. Cargo.toml

No new dependencies. `base64` is already in
`crates/quickpdf-core/Cargo.toml` from Phase 2a. `krilla::text::
Font` and `skrifa::FontRef` are already pulled.

## Data flow

```
HTML string
  │
  ▼
parse::Document::parse  ──►  html5ever DOM
  │
  ▼
Document::stylesheet()  ──►  Stylesheet { rules, font_faces }
Document::blocks()      ──►  Vec<Block>     (Text + Image variants)
Document::inline_styles()
  │
  ▼
FontRegistry::build(&stylesheet.font_faces)
  ├── pre-register Inter at index 0
  └── for each FontFace in source order:
        ├── extract font-family descriptor (single name)
        ├── extract src descriptor (comma list)
        ├── walk src entries left-to-right:
        │     ├── url(data:<accepted-MIME>;base64,<payload>) → decode → sniff → register → stop
        │     └── anything else → skip
        └── on no decodable src → drop the face
  │
  ▼
plan_pages_styled(&parsed, &blocks, &stylesheet.rules, &inline_map, &registry, ...)
  for each Block:
    ├── style::resolve(elem, rules, inline) ──► BlockStyle
    │       (font_family is now resolved via cascade + inheritance)
    ├── font_handle = registry.lookup(style.font_family.as_deref().unwrap_or(&[]))
    ├── text::TextMetrics::new(&registry.fonts[font_handle].bytes, font_size)
    └── PlacedLine { …, font_handle }
  │
  ▼
Vec<PagePlan>  (boxes, images, lines per page; lines carry font_handle)
  │
  ▼
krilla emit:
  for each page:
    paint boxes  →  paint images  →  draw text using registry.fonts[line.font_handle].krilla_font
                                      (re-set font on switch, like color)
```

## Error handling matrix

| Condition | Behavior | Test |
| --- | --- | --- |
| `@font-face` block with no `font-family` descriptor | Drop the face | `font.rs::missing_family_drops` |
| `@font-face` with empty `font-family: ""` | Drop the face | `font.rs::empty_family_drops` |
| `@font-face` with no `src` descriptor | Drop the face | `font.rs::missing_src_drops` |
| `src: local("Arial")` only | Drop the face (no fallback to Inter under that name) | `font.rs::local_only_drops` |
| `src: url(http://...)` only | Drop the face | `font.rs::http_only_drops` |
| `src: url(data:font/woff2;base64,...)` only | Drop the face (sniff fails or MIME unaccepted) | `font.rs::woff2_only_drops` |
| `src: url(data:font/woff2;...) format("woff2"), url(data:font/ttf;...)` | Use the second entry | `font.rs::multi_src_walks_to_ttf` |
| Base64 garbage in payload | Skip that entry; try next; drop face if none works | `font.rs::base64_garbage_skipped` |
| `font-family: "Brand", Helvetica, sans-serif` on a `<p>` with Brand registered | Use Brand | `lib.rs::font_family_picks_brand_when_present` |
| `font-family: "Brand", Helvetica` on a `<p>` with neither registered | Fall back to Inter | `lib.rs::font_family_falls_back_to_inter` |
| Two `@font-face` blocks declare `font-family: Brand` with different srcs | Last wins (source-order) | `font.rs::duplicate_family_last_wins` |
| `font-family: "Brand"` on `<section>`, `<p>` inside has no `font-family` rule | `<p>` inherits Brand | `style/cascade.rs::font_family_inherits` |
| Inline `style="font-family: Brand"` on `<p>` overrides cascade | Inline wins via existing INLINE specificity | `style/cascade.rs::inline_font_family_wins` |
| `application/octet-stream` payload that's valid TTF | Accepted via magic-byte sniff | `font.rs::octet_stream_with_ttf_magic_accepted` |
| `font/ttf` MIME but payload starts with `wOFF` | Sniff fails → drop entry | `font.rs::wrong_magic_in_ttf_mime_dropped` |

## Testing posture

| Layer | Approx test count delta | Where |
| --- | --- | --- |
| `style/sheet.rs` unit | +6 (`@font-face` captured, declarations parse normally, source_order shared with rules, case-insensitive at-rule keyword, malformed @font-face dropped, `Stylesheet` aggregate round-trip) | `sheet.rs` `#[cfg(test)]` |
| `font.rs` unit (registry) | +14 (build with no faces, single face, multi-src walks, all the error matrix rows above, last-wins on dup, Inter always at handle 0, `lookup` empty chain → 0, `lookup` chain hit, sniff-vs-MIME edge cases) | `font.rs` `#[cfg(test)]` |
| `style/cascade.rs` unit | +5 (`font-family` parses single name, comma list, generic keyword drop, quoted name strips, inheritance copies parent chain) | `cascade.rs` `#[cfg(test)]` |
| `lib.rs` integration | +5 (PlacedLine carries font_handle, registered family resolves to non-zero handle, unknown family falls back to 0, inheritance flows through planner, alt-text path picks font from chain) | `lib.rs` `#[cfg(test)]` |
| Python | +5 (HTML with `@font-face` + matching `font-family` produces a PDF whose embedded font name differs from Inter; HTML with no `@font-face` but `font-family: Acme` on a paragraph still embeds only Inter; multi-paragraph HTML with mixed families produces both embedded; broken `@font-face` payload drops silently and the paragraph still renders; `data:font/woff2` src is dropped and Inter is used) | `tests/test_render.py` |

Target totals after merge: **~224 Rust unit + ~55 Python integration**, up from 189 + 50.

A small (~10 KB) TTF test fixture distinct from Inter lives in
`crates/quickpdf-core/assets/test-fixtures/font.ttf` — added by
Slice B (the registry slice), reused by `lib.rs` and Python tests
via base64-encoded inline constants. The fixture must be CC0 / OFL
licensed; a 1-glyph TTF generated from a public-domain source is
acceptable. The accompanying license file lives next to it.

## Sprint structure

Mirrors the 4-agent parallel-sprint pattern proven in Phases 1.6,
1.7, and 2a. Contracts artifact: `.claude-2b-contracts.md`
(gitignored).

| Agent | Owns | Hard "don't touch" constraint |
| --- | --- | --- |
| **Plan** | `.claude-2b-contracts.md` | Writes interface contracts for slices A/B/C; does not write implementation. |
| **Slice A** | `crates/quickpdf-core/src/style/sheet.rs` (FontFace struct, Stylesheet aggregate, `parse_stylesheet_full`, `@font-face` capture, back-compat `parse_stylesheet` wrapper), `crates/quickpdf-core/src/parse.rs` (add `Document::stylesheet()` alongside the preserved `user_stylesheet()`) | No edits to `font.rs`, `style/cascade.rs`, `style/mod.rs`, or `lib.rs`. The back-compat wrappers mean `lib.rs` keeps compiling against the old API; the integrator opts in to the new aggregate type. |
| **Slice B** | `crates/quickpdf-core/src/font.rs` (FontHandle, RegisteredFont, FontRegistry, build, lookup, src tokenizer, MIME accept list, base64 decode, magic-byte sniff), `crates/quickpdf-core/assets/test-fixtures/font.ttf` *(new)* + license file. Consumes `&[FontFace]` (via Slice A's contract) but does not depend on Slice A's implementation. | No edits to `parse.rs`, `style/`, `lib.rs`, or Python. |
| **Slice C** | `crates/quickpdf-core/src/style/mod.rs` (BlockStyle.font_family field, default), `crates/quickpdf-core/src/style/cascade.rs` (font-family parser, generic keyword drop, inheritance) | No edits to `parse.rs`, `font.rs`, `sheet.rs`, `lib.rs`, or Python. |
| **Integrator** *(main thread)* | `lib.rs` plan/paint changes (PlacedLine.font_handle, registry build at top, planner threads handle, emitter switches font), all cross-file fixups, full test sweep, Python integration tests, final commit | Reconciles Slice A's `Stylesheet` rename in `lib.rs` and any other unrelated touches needed to make `cargo test -p quickpdf-core --lib` and `pytest tests/ -q` both green. |

Slice ordering: A and C have no contract dependency between them
(A changes the parse output type; C only touches the cascade layer
which doesn't see the parse type). B depends on A only via the
`FontFace` struct shape, which is frozen in the contracts artifact
— B can proceed in parallel by mocking its own input fixtures.

`cargo check -p quickpdf-core` is the green-bar gate for slice
agents (skips `#[cfg(test)]` bodies, immune to integrator-only
fixups). The integrator's gate is the full test command.

## Risks & open questions

1. **Krilla `Font::new` API drift.** Current usage is
   `Font::new(font::FALLBACK_TTF.to_vec().into(), 0)` which returns
   `Option<Font>`. If krilla's pinned version exposes a different
   constructor or accepts `Arc<[u8]>` directly, the registry update
   is mechanical and does not require re-brainstorming.
2. **Test fixture provenance.** The TTF fixture must be
   author-licenced (CC0 or OFL) and small (≤ 10 KB). Slice B picks
   an existing OFL font (e.g. the existing Inter, but renamed in
   the file's `name` table to make it identifiable as "different")
   or generates a single-glyph TTF. The accompanying license file
   ships alongside.
3. **Embedded-font-name introspection in Python tests.** `pypdf`
   exposes per-page font dictionaries via
   `page["/Resources"]["/Font"]`. Tests assert that at least one
   embedded BaseFont name matches the registered family (krilla
   emits a randomized prefix like `ABCDEF+Inter`, so tests use
   `endswith` or contains-match against the expected family name).
4. **Whitespace and casing in font-family lookup.** "Inter" vs
   "inter" vs " Inter " must all match. Lowercasing and trimming
   happen at both registry build time (key) and lookup time (chain
   tokens). The cascade parser already lowercases; the registry
   parser is the symmetric side and must match.
5. **Two `@font-face` blocks declaring different `font-family` but
   the same `src`.** Both register; both lookups succeed
   independently. Krilla embeds the bytes once if it deduplicates
   internally; otherwise once per `Font::new` call. Phase 2b does
   not promise any size optimisation here — verify the resulting
   PDF size in the Python test is "small enough" rather than
   "byte-for-byte minimal".
6. **`@font-face` inside `@media`.** Unsupported. The `@media`
   block is still skipped wholesale by `skip_at_rule`, so any
   nested `@font-face` is dropped. Documented as a non-goal; not
   tested explicitly.

## Definition of done

- `cargo test -p quickpdf-core --lib` passes with the new tests
  (~224 total).
- `pytest tests/ -q` passes with the new Python integration tests
  (~55 total).
- `cargo check -p quickpdf-core` is clean (no warnings).
- A real-world payload (one HTML page with one `@font-face` block
  declaring a brand font and one paragraph using it) renders to a
  PDF whose embedded font name list contains the brand font's
  postscript name.
- A second payload (same HTML but the `@font-face` `src` payload
  is corrupted) renders successfully and embeds only Inter — proof
  that the silent-fallback path works end-to-end.
- CLAUDE.md roadmap table marks Phase 2b as ✓ and points 2c at
  tables.
- One commit per slice (or one squashed integration commit on
  `main`), title format `Phase 2b: <slice description>`.
