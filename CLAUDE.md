# quickpdf — Claude Code project context

> Read this first. The detailed plan lives at
> `~/.claude/plans/cheerful-riding-castle.md` (full architecture + phasing).
> The README is user-facing; this file is for Claude.

## Mission

Native Rust HTML→PDF library for Python. Distributed as a single self-contained
pip wheel — no Chromium binary, no Node, no system browser. Sync API in the
spirit of WeasyPrint, with broader CSS coverage as the project matures. Both
single-PDF preview and high-throughput bulk generation are first-class goals.

This is **Track B** of the original design — pure native renderer. Track A
(Rust-orchestrated Chromium) was deliberately rejected because the user wanted
something genuinely new, not another browser wrapper.

## Roadmap status (live)

| Phase | Status | Notes                                                                                |
| :---: | :----: | ------------------------------------------------------------------------------------ |
|   0   |   ✓    | Workspace, PyO3, maturin build, blank-PDF smoke test                                 |
|  1.1  |   ✓    | HTML parsing via `scraper`/`html5ever` → DOM walker                                  |
|  1.2  |   ✓    | Bundled fallback font (Inter Regular Latin subset, SIL OFL, ~68 KB)                  |
|  1.3  |   ✓    | Naïve text emission via `krilla::Surface::draw_text`                                 |
|  1.4  |   ✓    | Word-wrap line breaking using `skrifa` glyph advances                                |
|  1.5  |   ✓    | Block layout (paragraphs stack vertically) + multi-page                              |
| 1.6a  |   ✓    | UA stylesheet (per-tag `BlockStyle`)                                                 |
| 1.6b  |   ✓    | Inline `<style>` cascade: tag/class/id/descendant selectors; 4 properties            |
| 1.6c  |   ✓    | Specificity, `!important`, inheritance via parent-chain walk, anonymous-block wrap   |
| 1.7a  |   ✓    | `color` property: parser (named/hex/rgb/rgba), inherited, plumbed into krilla fill   |
| 1.7b  |   ✓    | `background-color`, `padding-*`, `border-*` via PlacedBox paint pass                 |
| 1.7c  |   ✓    | `rem` unit, padding/margin/border shorthand, inline `style="..."` (4-tuple specificity) |
|  2a   |   ✓    | Block-level `<img>` (PNG/JPEG via `data:` URL, HTML+CSS sizing, alt fallback)         |
|  2b   |   →    | **NEXT.** Web fonts via `@font-face`                                                  |
|  2c   |        | Tables (`<table>`/`<tr>`/`<td>`) — proper 2D layout                                    |
|   3   |        | `BulkSession`, Rayon parallelism, `pip install quickpdf` v0.1                        |
|   4   |        | Flex/Grid (taffy), `@page` rules, position abs/rel                                   |
|   5   |        | Incremental relayout (template-aware bulk), broader CSS                              |

**Test posture today:** 189 Rust unit tests + 50 Python integration tests, all
green in ~0.5 s combined.

## Build + test (always)

```sh
cd quickpdf
.venv/Scripts/maturin.exe develop --release    # rebuild after Rust edits
.venv/Scripts/python.exe -m pytest tests/ -q    # Python integration (fast)
cargo test -p quickpdf-core --lib               # Rust unit tests (fast)
cargo check -p quickpdf-core                    # type-check only
```

**One-time setup on a fresh clone:**

```sh
python -m venv .venv
.venv/Scripts/python.exe -m pip install --upgrade pip maturin pytest pypdf
.venv/Scripts/maturin.exe develop --release
```

`maturin develop` produces `python/quickpdf/_native.pyd` (Windows) /
`_native.so` (Unix). It's gitignored. Editable install — re-importing
`quickpdf` after a rebuild picks up the change.

## Architecture

```
quickpdf/
├── Cargo.toml                       # Rust workspace
├── pyproject.toml                   # maturin config (module=quickpdf._native)
├── crates/
│   ├── quickpdf-core/               # pure Rust renderer; NO Python
│   │   ├── assets/fonts/Inter-Regular.ttf  ← bundled, OFL-licensed
│   │   └── src/
│   │       ├── lib.rs               # html_to_pdf entrypoint + plan_pages_styled
│   │       ├── parse.rs             # html5ever DOM walker, blocks(), inline_styles()
│   │       ├── font.rs              # FALLBACK_TTF (include_bytes!)
│   │       ├── text.rs              # TextMetrics, wrap_lines (greedy word-wrap)
│   │       ├── image.rs             # parse_data_url for data:image/png|jpeg URLs (Phase 2a)
│   │       └── style/               # CSS pipeline (Phase 1.6 + 1.7 family)
│   │           ├── mod.rs           # BlockStyle, ua_style, resolve, InlineStyles
│   │           ├── sheet.rs         # CSS parser → Vec<Rule>; shorthand expansion; inline parser
│   │           ├── matcher.rs       # selector parse + match + Specificity (4-tuple)
│   │           └── cascade.rs       # apply_declarations, parse_length_em (rem-aware), BlockStyleBuilder, inherit
│   └── quickpdf-py/                 # PyO3 cdylib → quickpdf._native
│       └── src/lib.rs               # html_to_pdf binding + debug helpers
├── python/quickpdf/
│   ├── __init__.py                  # public sync facade
│   ├── _native.pyi                  # type stubs
│   └── py.typed
├── tests/test_render.py             # Python integration suite
└── README.md                        # user-facing
```

### Crate dependency choices (already decided — don't relitigate)

| Concern                     | Crate                | Why                                                 |
| --------------------------- | -------------------- | --------------------------------------------------- |
| HTML parsing                | `scraper` 0.26       | Wraps `html5ever` ergonomically; pulls cssparser    |
| PDF emission                | `krilla` 0.7         | Modern, Typst-team-maintained                       |
| Glyph advances              | `skrifa` 0.37        | **Pin to krilla's version** (avoid duplicate trees) |
| DOM node handles            | `ego-tree` 0.11      | Already transitive via scraper                      |
| Python bindings             | `pyo3` 0.23 + maturin | abi3-py39 → one wheel covers all Python ≥ 3.9      |
| Concurrency (later)         | `rayon`              | Phase 3 only; not yet wired                         |
| CSS parsing                 | hand-rolled          | Slice A's tokenizer; cssparser turned out heavier   |
| Selector matching           | hand-rolled          | Slice B; subset spec is small enough                |

**Explicitly rejected** (don't second-guess):

- **Stylo** — too entangled with Servo internals; build cost outweighs benefit.
- **Lightningcss** — heavier than needed for our subset.
- **Chromium-CDP wrapper** — that was Track A; we're on Track B.
- **WebKit/Servo binary embed** — distribution nightmare on Windows.

## Conventions / hard constraints

- **Sync at the seam.** PyO3 entry points drop the GIL during Rust work.
  Bulk parallelism (Phase 3) happens inside Rust via Rayon. Python callers
  always see a plain `for` loop — no asyncio.
- **No JS execution, ever.** HTML is fully expanded before being passed in.
  This matches the upstream LocalSFMC flow (AMPScript runs first → fully
  rendered HTML → quickpdf).
- **`<script>` ignored, `<style>` parsed.** External stylesheets via `<link
  rel="stylesheet">` and inline `style="..."` attribute are deferred to 1.7+.
- **Self-contained wheel is sacred.** No subprocess, no system-font requirement,
  no runtime download of binaries. Adding a Chromium binary defeats the whole
  project — it's not an optimisation, it's a defection to Track A.
- **Keep skrifa pinned to krilla's version.** Mismatch produces two skrifa
  copies in the binary (bigger wheel, slower build).
- **`Block::element_id()` is a stable handle.** Use `Document::element_for_block(b)`
  to recover the `ElementRef` for cascade matching; never fabricate `NodeId`s.
- **Font bundling.** `crates/quickpdf-core/assets/fonts/Inter-Regular.ttf` is
  embedded via `include_bytes!`. The OFL `Inter-Regular.LICENSE.txt` lives next
  to it and **must be preserved on any redistribution**.

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

## Phase 1.6 parallel-sprint pattern (proven, repeat for 1.7)

Phases 1.6b and 1.6c both used a 4-agent pattern that landed cleanly:
- 1 **Plan agent** as coordinator, producing N frozen interface contracts.
- N **general-purpose agents** as developers, each owning one slice file with
  hard "don't touch other files" constraints.
- 1 **integrator** (the main thread) wiring everything together.

The pattern: Plan agent writes the contracts (saved to a `.claude-1.7-contracts.md`
artifact for durability), then launch the slice agents in parallel via a single
message with `Agent` + `run_in_background: true`. The integrator handles any
cross-file fixups that the slice contracts deliberately deferred.

Lessons from 1.6c worth keeping:
- Slice agents must accept that adding a field to a shared struct (e.g.
  `Declaration::important`) breaks an unrelated file's test helper, and
  that's the integrator's job to reconcile on merge — NOT a contract violation.
- `cargo check -p quickpdf-core` is the right green-bar gate for slice agents
  because it skips `#[cfg(test)]` bodies and won't trip on the cross-file fixup.
- The integrator should run the **full** `cargo test -p quickpdf-core --lib`
  immediately after merging the cross-file fix.

## Repo / GitHub state

- Public repo: **https://github.com/Uppah/quickpdf**
- Default branch: `main`
- License: MIT OR Apache-2.0 (dual; both files at root)
- Initial commit `ae33e4c` includes Phase 0 + 1.1–1.6b, 24 source files, no
  build artefacts.
- `gh` CLI is authed as user `Uppah`; further pushes are routine.

## Recent decisions worth not re-litigating

- **`Cargo.lock` is committed.** We ship binary wheels; reproducibility wins.
- **Two-pass page planner** (`plan_pages_styled` builds `Vec<Vec<PlacedLine>>`
  before any PDF emission). The naïve "emit-as-you-go" approach hit a
  borrow-checker wall around `&mut Document` + `Page<'a>` self-reference —
  see commit message for context. Don't try to "simplify" it back.
- **`scraper` instead of `markup5ever_rcdom`.** The unofficial rcdom forks
  have version-skew issues; scraper bundles a working html5ever pair.
- **CSS uses hand-rolled parser/matcher, not `cssparser`/`selectors`.** The
  spec we accept (Phase 1.6b–1.6c) is small enough that the hand-rolled
  versions are simpler than wrestling the typed API. Don't swap them out
  unless we hit a wall in 1.7+.
- **Block-level set is fixed in `parse.rs::is_block`.** Adding new block-level
  tags is fine; removing existing ones will silently change paragraph splitting
  for existing tests.

## Files an integrator most often touches

- `crates/quickpdf-core/src/lib.rs` — render entrypoint + planner.
- `crates/quickpdf-core/src/parse.rs` — `Document`, `Paragraph`.
- `crates/quickpdf-core/src/style/mod.rs` — `BlockStyle`, `ua_style`, `resolve`.
- `python/quickpdf/__init__.py` — public Python API (currently just `html_to_pdf`).
- `tests/test_render.py` — end-to-end behavioural tests.

When in doubt, run the build command above and let the failing test point you
at the right file.
