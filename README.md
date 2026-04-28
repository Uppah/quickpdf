# quickpdf

Native Rust HTML→PDF rendering for Python. Fast preview, fast bulk, no Chromium.

> **Status: Phase 1 in progress.** HTML parsing + bundled fallback font + naive
> text emission are wired end-to-end. Real layout (block, inline, line
> breaking, tables) is the next chunk of work. See
> `~/.claude/plans/cheerful-riding-castle.md` for the full plan.
>
> What works today:
> - `quickpdf.html_to_pdf(html)` returns a valid PDF that paints the visible
>   text content of the input HTML in the embedded Inter font.
> - Word-wrap at the page width using skrifa-measured glyph advances.
> - Block-level elements (`<p>`, `<h1>`-`<h6>`, `<ul>`/`<li>`, `<div>`, …)
>   render as separate paragraphs with vertical gaps between them.
> - Multi-page output: content that overflows the bottom margin flows onto
>   a new page automatically.
> - **UA-default styling**: headings render at h1=2em … h6=0.67em, list items
>   are indented, paragraph margins follow CSS defaults.
> - **Inline `<style>` cascade**: author rules override UA defaults. Selectors:
>   tag, `.class`, `#id`, descendant combinator. Properties: `font-size`
>   (px/pt/em/%), `font-weight`, `margin-top`/`-bottom`, `text-align`.
>   Last-declaration-wins; full specificity is Phase 1.6c.
> - `<script>` / `<style>` / `<head>` content is correctly excluded from output;
>   `<style>` content is parsed for the cascade.
> - Whitespace runs collapse the way browsers render them.
>
> What does NOT work yet:
> - Specificity, `!important`, inheritance (Phase 1.6c).
> - External `<link rel="stylesheet">` and inline `style="..."` attribute.
> - Bold/italic rendering (the bundled font is regular only — Phase 4 adds
>   weight/style variants; UA `bold` flag is recorded but no font swap yet).
> - Colours, backgrounds, borders, padding (Phase 1.7).
> - Tables, images, hyperlinks.

## Why

| Library          | Full CSS | No browser dep | Bulk-friendly | Cold-start |
| ---------------- | :------: | :------------: | :-----------: | :--------: |
| Playwright/CDP   |    ✓     |       ✗        |       ~       |   slow     |
| WeasyPrint       |    ✗     |       ✓        |       ~       |   medium   |
| **quickpdf**     |   →✓     |       ✓        |       ✓       |   fast     |

## Layout

```
quickpdf/
├── Cargo.toml                   # Rust workspace
├── pyproject.toml               # maturin build config
├── crates/
│   ├── quickpdf-core/           # pure Rust renderer
│   └── quickpdf-py/             # PyO3 bindings (cdylib → quickpdf._native)
├── python/quickpdf/             # Python facade + type stubs
├── tests/                       # pytest smoke tests
└── benchmarks/                  # vs Playwright / WeasyPrint
```

## Toolchain

- Rust ≥ 1.75 (`rustup default stable`)
- Python ≥ 3.9
- `pip install maturin pytest pypdf`

## Development build

```sh
cd quickpdf
maturin develop --release       # builds the native module into the active venv
pytest tests/ -q
```

`maturin develop` puts a `.pyd`/`.so` named `quickpdf._native` next to the
Python source, so `import quickpdf` works inside the venv without needing
`pip install`.

## Quick check

```python
import quickpdf

pdf = quickpdf.html_to_pdf("<h1>hi</h1>")
assert pdf.startswith(b"%PDF-")
print("OK", len(pdf), "bytes")
```

## Roadmap

| Phase | Status | Scope                                                       |
| :---: | :----: | ----------------------------------------------------------- |
|   0   |   ✓    | Workspace + PyO3 + krilla emit blank PDF                    |
|  1.1  |   ✓    | HTML parsing via scraper/html5ever                          |
|  1.2  |   ✓    | Bundled fallback font (Inter Regular, Latin subset, OFL)    |
|  1.3  |   ✓    | Naive single-line text emission                             |
|  1.4  |   ✓    | Line breaking (word wrap via skrifa glyph advances)         |
|  1.5  |   ✓    | Block layout (paragraphs stack vertically) + multi-page     |
| 1.6a  |   ✓    | UA stylesheet: heading sizes, list indent, block margins    |
| 1.6b  |   ✓    | Inline `<style>` parsing + tag/class/id/descendant cascade  |
| 1.6c  |        | Full cascade: specificity, inheritance, `!important`        |
|  1.7  |        | Borders, padding, margin, colours                           |
|   2   |        | Tables + images + web fonts → renders email-style HTML      |
|   3   |        | `BulkSession`, Rayon parallelism, `pip install quickpdf`    |
|   4   |        | Flex/Grid (taffy), `@page` rules, position abs/rel          |
|   5   |        | Incremental relayout (template-aware bulk), broader CSS     |

## Embedded fonts

The wheel ships with [Inter](https://github.com/rsms/inter) Regular (Latin
subset, ~68 KB), licensed under the SIL Open Font License 1.1. The full
license is preserved in
[`crates/quickpdf-core/assets/fonts/Inter-Regular.LICENSE.txt`](crates/quickpdf-core/assets/fonts/Inter-Regular.LICENSE.txt).
