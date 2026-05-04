# quickpdf

Native Rust HTML→PDF rendering for Python. Fast preview, fast bulk, no Chromium.

> **Status: Phase 2 in progress** (2a images and 2b web fonts shipped; 2c tables
> next). 233 Rust unit tests + 55 Python integration tests, all green.
>
> What works today:
> - `quickpdf.html_to_pdf(html)` returns a valid PDF that paints the visible
>   text content of the input HTML.
> - Block layout: paragraphs, headings, lists, mixed-content containers stack
>   vertically with CSS-spec margins; content that overflows the bottom margin
>   flows onto a new page.
> - Word-wrap at the page width using skrifa-measured glyph advances; whitespace
>   runs collapse the way browsers render them.
> - **Full author-CSS cascade** with specificity (4-tuple), `!important`,
>   inheritance via parent-chain walk, anonymous-block wrapping for orphan text,
>   inline `style="..."` attributes (winning over selector rules), and `rem`
>   unit support. Selectors: tag, `.class`, `#id`, descendant combinator.
> - **Box model**: `color`, `background-color`, `padding-*`, `border-*`,
>   `margin-*`, `font-size`, `font-weight`, `text-align`, `width`/`height`
>   longhands; `padding` / `margin` / `border` shorthands expand to their
>   longhands.
> - **Block-level images** (`<img>`): PNG and JPEG embedded as `data:` URLs,
>   with HTML `width`/`height` attrs, CSS `width`/`height`, paint-as-unit
>   pagination, oversize proportional shrink, and `alt` fallback when the
>   src can't be decoded.
> - **Web fonts via `@font-face`**: declare a font with
>   `src: url(data:font/ttf;base64,...)` (or `font/otf`; permissive MIME
>   accept list with magic-byte sniff), then use `font-family: <name>` on
>   any block. Multi-`src` lists walk left-to-right; unsupported entries
>   (HTTP, `local()`, WOFF/WOFF2) are skipped; unresolved families fall
>   back silently to the bundled Inter.
> - `<script>` / `<head>` content is excluded; `<style>` content is parsed.
>
> What does NOT work yet:
> - Tables (`<table>`/`<tr>`/`<td>`) — Phase 2c.
> - Bold/italic rendering. The bundled font is regular only; UA `bold` flag
>   is recorded but no font swap yet (Phase 4 adds weight/style variants).
> - Inline `<span>` styling within paragraphs (paragraph-level only today).
> - WOFF / WOFF2 font sources, system-font probing, HTTP font fetching.
> - External `<link rel="stylesheet">`.
> - Flex / Grid (Phase 4), `@page` rules, hyperlinks.

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
| 1.6c  |   ✓    | Full cascade: specificity, inheritance, `!important`        |
|  1.7  |   ✓    | Colours, backgrounds, padding, borders, shorthands, `rem`   |
|  2a   |   ✓    | Block-level `<img>` (PNG/JPEG via `data:` URL)              |
|  2b   |   ✓    | Web fonts via `@font-face` (`data:font/ttf\|otf` URLs)      |
|  2c   |   →    | Tables (`<table>`/`<tr>`/`<td>`) — proper 2D layout         |
|   3   |        | `BulkSession`, Rayon parallelism, `pip install quickpdf`    |
|   4   |        | Flex/Grid (taffy), `@page` rules, position abs/rel          |
|   5   |        | Incremental relayout (template-aware bulk), broader CSS     |

## Embedded fonts

The wheel ships with [Inter](https://github.com/rsms/inter) Regular (Latin
subset, ~68 KB), licensed under the SIL Open Font License 1.1. The full
license is preserved in
[`crates/quickpdf-core/assets/fonts/Inter-Regular.LICENSE.txt`](crates/quickpdf-core/assets/fonts/Inter-Regular.LICENSE.txt).
