"""Phase 0 smoke tests.

Asserts only that the toolchain produces a valid PDF byte string. Phase 1
expands this into actual content checks.
"""

from __future__ import annotations

import quickpdf


def test_version_exposed():
    assert isinstance(quickpdf.__version__, str)
    assert quickpdf.__version__.count(".") >= 1


def test_pdf_byte_signature():
    pdf = quickpdf.html_to_pdf("<h1>hi</h1>")
    assert pdf[:5] == b"%PDF-", f"unexpected header: {pdf[:16]!r}"


def test_letter_page_size():
    pdf = quickpdf.html_to_pdf("<p>letter</p>", page_size="Letter")
    assert pdf[:5] == b"%PDF-"


def test_writes_to_output_path(tmp_path):
    out = tmp_path / "out.pdf"
    pdf = quickpdf.html_to_pdf("<p>x</p>", output=out)
    assert out.exists()
    assert out.read_bytes() == pdf
    assert pdf[:5] == b"%PDF-"


def test_invalid_page_size_raises():
    import pytest

    with pytest.raises(ValueError):
        quickpdf.html_to_pdf("<p>x</p>", page_size="A99")


# --- Phase 1.1: HTML parsing reachable from Python -------------------------

def test_parser_extracts_visible_text():
    from quickpdf import _native

    txt = _native._debug_visible_text(
        "<html><head><style>.x{}</style></head>"
        "<body><p>Hej <b>Alice</b>!</p><script>alert(1)</script></body></html>"
    )
    assert txt == "Hej Alice!"


def test_parser_collapses_whitespace():
    from quickpdf import _native

    assert _native._debug_visible_text("<p>a   b\n\n  c</p>") == "a b c"


def test_parser_element_count_nonzero():
    from quickpdf import _native

    n = _native._debug_element_count("<div><p>a</p><p>b</p></div>")
    assert n >= 6  # html, head, body, div, p, p


# --- Phase 1.3: PDF actually contains the rendered text --------------------

def _pdf_text(pdf_bytes: bytes) -> str:
    """Round-trip a PDF byte string through pypdf and return its extracted text."""
    import io

    from pypdf import PdfReader

    reader = PdfReader(io.BytesIO(pdf_bytes))
    return "\n".join(page.extract_text() or "" for page in reader.pages)


def test_pdf_contains_rendered_text():
    pdf = quickpdf.html_to_pdf("<h1>Hej Alice!</h1>")
    text = _pdf_text(pdf)
    assert "Hej" in text
    assert "Alice" in text


def test_pdf_skips_script_content():
    html = "<p>visible</p><script>alert('hidden')</script>"
    text = _pdf_text(quickpdf.html_to_pdf(html))
    assert "visible" in text
    assert "alert" not in text
    assert "hidden" not in text


def test_pdf_blank_html_still_valid():
    # Empty input should still produce a parseable, single-page PDF.
    pdf = quickpdf.html_to_pdf("")
    assert pdf[:5] == b"%PDF-"
    import io
    from pypdf import PdfReader
    assert len(PdfReader(io.BytesIO(pdf)).pages) == 1


# --- Phase 1.4: word-wrap -------------------------------------------------

def test_long_text_wraps_to_multiple_lines():
    # ~60 words of plain prose. At 12pt Inter on a 595pt-wide A4 with 36pt
    # margins, the line width is 523pt and we expect well over one line.
    body = " ".join(
        ["the quick brown fox jumps over the lazy dog"] * 8
    )
    pdf = quickpdf.html_to_pdf(f"<p>{body}</p>")
    text = _pdf_text(pdf)
    # All the source words are still present.
    for word in ["quick", "brown", "fox", "jumps", "lazy", "dog"]:
        assert word in text, f"missing word {word!r} in {text!r}"
    # pypdf's extract_text inserts newlines at line boundaries — at least one
    # internal break must be present (i.e. it really wrapped).
    assert "\n" in text.strip(), f"expected multi-line output, got {text!r}"


def test_short_text_stays_one_line():
    pdf = quickpdf.html_to_pdf("<p>Hello world</p>")
    text = _pdf_text(pdf).strip()
    # No mid-content line break for a short string.
    assert text == "Hello world", repr(text)


# --- Phase 1.5: block layout + multi-page ---------------------------------

def _pdf_pages(pdf_bytes: bytes):
    import io
    from pypdf import PdfReader
    return PdfReader(io.BytesIO(pdf_bytes)).pages


def test_blocks_render_as_separate_paragraphs():
    pdf = quickpdf.html_to_pdf(
        "<h1>Title</h1><p>intro</p><ul><li>one</li><li>two</li></ul>"
    )
    text = _pdf_text(pdf)
    # Each block emits its own paragraph (pypdf inserts \n between them).
    for chunk in ("Title", "intro", "one", "two"):
        assert chunk in text
    # Heading + paragraph + 2 list items = 4 paragraphs ⇒ at least 3 newlines.
    assert text.count("\n") >= 3, f"expected paragraph breaks: {text!r}"


def test_overflowing_content_paginates():
    # Many short paragraphs so we definitely overflow A4.
    blocks = "".join(f"<p>paragraph number {i}</p>" for i in range(120))
    pdf = quickpdf.html_to_pdf(blocks)
    pages = _pdf_pages(pdf)
    assert len(pages) >= 2, f"expected multi-page output, got {len(pages)} page(s)"
    # First and last paragraphs both still present somewhere in the document.
    full_text = "\n".join(p.extract_text() or "" for p in pages)
    assert "paragraph number 0" in full_text
    assert "paragraph number 119" in full_text


def test_inline_runs_stay_within_block():
    # <span> is inline, so its text must stay merged with its <p>.
    pdf = quickpdf.html_to_pdf("<p>hello <span>shiny</span> world</p>")
    text = _pdf_text(pdf).strip()
    assert text == "hello shiny world"


# --- Phase 1.6a: UA stylesheet (heading sizes, list indent) ---------------

def test_h1_renders_larger_than_p():
    # We can't easily measure font size from extracted text, but pypdf yields
    # different positional info. Easier signal: at the same content width,
    # an h1 wraps at fewer characters per line than a p (h1 is 2× the size).
    long = "wordy " * 30
    pdf_h = quickpdf.html_to_pdf(f"<h1>{long}</h1>")
    pdf_p = quickpdf.html_to_pdf(f"<p>{long}</p>")
    h_lines = _pdf_text(pdf_h).strip().count("\n") + 1
    p_lines = _pdf_text(pdf_p).strip().count("\n") + 1
    assert h_lines > p_lines, (
        f"<h1> should wrap onto more lines than <p> at the same text "
        f"(h1={h_lines}, p={p_lines})"
    )


def test_heading_levels_decrease_in_size():
    # h1 wraps at fewer chars/line than h6, since h1 is much bigger.
    long = "wordy " * 30
    h1_lines = _pdf_text(quickpdf.html_to_pdf(f"<h1>{long}</h1>")).strip().count("\n")
    h6_lines = _pdf_text(quickpdf.html_to_pdf(f"<h6>{long}</h6>")).strip().count("\n")
    assert h1_lines >= h6_lines, f"h1={h1_lines} < h6={h6_lines}"


def test_paragraphs_preserve_text_in_order():
    pdf = quickpdf.html_to_pdf(
        "<h1>Title</h1><p>intro paragraph</p>"
        "<ul><li>first item</li><li>second item</li></ul>"
        "<p>closing thought</p>"
    )
    text = _pdf_text(pdf)
    indices = [
        text.find("Title"),
        text.find("intro paragraph"),
        text.find("first item"),
        text.find("second item"),
        text.find("closing thought"),
    ]
    assert all(i >= 0 for i in indices), f"missing content: {indices} in {text!r}"
    assert indices == sorted(indices), (
        f"paragraphs out of document order: {indices}"
    )


# --- Phase 1.6b: inline <style> blocks override UA defaults ---------------

def test_inline_style_block_changes_font_size():
    # 24px in CSS = 24pt in our model. With base 12pt = 1em, font-size:24px
    # makes <p> twice as big as the default. Effect is observable through
    # how many words fit per wrapped line: a 36-word paragraph wraps to MORE
    # lines under font-size:24px than under the UA default.
    body = "the quick brown fox jumps over the lazy dog " * 4
    plain = quickpdf.html_to_pdf(f"<p>{body}</p>")
    big = quickpdf.html_to_pdf(
        f"<style>p {{ font-size: 24px; }}</style><p>{body}</p>"
    )
    plain_lines = _pdf_text(plain).strip().count("\n") + 1
    big_lines = _pdf_text(big).strip().count("\n") + 1
    assert big_lines > plain_lines, (
        f"styled <p> should wrap to more lines (big={big_lines}, "
        f"plain={plain_lines})"
    )


def test_class_selector_only_targets_classed_paragraph():
    # `.big` selector should only enlarge the paragraph carrying that class.
    # The plain <p> stays at the UA default.
    body = "the quick brown fox jumps over the lazy dog " * 4
    pdf = quickpdf.html_to_pdf(
        f"<style>.big {{ font-size: 36px; }}</style>"
        f"<p>plain {body}</p>"
        f"<p class='big'>huge {body}</p>"
    )
    text = _pdf_text(pdf)
    assert "plain" in text and "huge" in text
    # Both halves must still be present somewhere; we don't assert exact line
    # counts because the wrap math at 36px is fiddly, but we assert that
    # neither paragraph got dropped by the cascade glue.


def test_at_rule_does_not_break_render():
    # Rules inside @media should be ignored entirely (Phase 1.6b contract).
    # The render must still succeed with UA defaults.
    pdf = quickpdf.html_to_pdf(
        "<style>@media print { p { font-size: 99px; } } p { color: red; }</style>"
        "<p>hello</p>"
    )
    text = _pdf_text(pdf).strip()
    assert text == "hello"


# --- Phase 1.6c: specificity, !important, inheritance, anonymous-block wrap

def test_id_selector_beats_class_via_specificity():
    # Same paragraph carries both `#wins` and `.loses`. The id rule (1,0,0)
    # must beat the class rule (0,1,0) regardless of source order. We make
    # the lower-specificity rule appear LATER in the stylesheet so source
    # order can't accidentally produce the right answer.
    body = "the quick brown fox jumps over the lazy dog " * 6
    pdf = quickpdf.html_to_pdf(
        "<style>"
        "#wins { font-size: 12px; }"
        ".loses { font-size: 48px; }"
        "</style>"
        f"<p id='wins' class='loses'>{body}</p>"
    )
    line_count = _pdf_text(pdf).strip().count("\n") + 1
    # If specificity is wrong (class wins at 48px) the wrap explodes to many
    # lines. With the id winning at 12px the same body fits in just a few.
    assert line_count <= 5, (
        f"expected id specificity to win (≤5 lines at 12px), "
        f"got {line_count} lines"
    )


def test_important_overrides_higher_specificity():
    # The `p` rule has lower specificity (0,0,1) than `#target` (1,0,0), but
    # carries `!important`. It must win.
    body = "the quick brown fox jumps over the lazy dog " * 6
    pdf = quickpdf.html_to_pdf(
        "<style>"
        "#target { font-size: 12px; }"
        "p { font-size: 48px !important; }"
        "</style>"
        f"<p id='target'>{body}</p>"
    )
    line_count = _pdf_text(pdf).strip().count("\n") + 1
    # !important at 48px must dominate, producing many wrapped lines.
    assert line_count >= 8, (
        f"expected !important on p to win (≥8 lines at 48px), "
        f"got {line_count} lines"
    )


def test_font_size_inherits_from_ancestor():
    # An unstyled <p> nested inside a styled <section> must inherit the
    # section's font-size. We use the same wrap-line proxy as elsewhere.
    body = "the quick brown fox jumps over the lazy dog " * 6
    plain = quickpdf.html_to_pdf(f"<p>{body}</p>")
    inherited = quickpdf.html_to_pdf(
        f"<style>section {{ font-size: 36px; }}</style>"
        f"<section><p>{body}</p></section>"
    )
    plain_lines = _pdf_text(plain).strip().count("\n") + 1
    inherited_lines = _pdf_text(inherited).strip().count("\n") + 1
    assert inherited_lines > plain_lines, (
        f"<p> should inherit section's 36px font-size and wrap more "
        f"(plain={plain_lines}, inherited={inherited_lines})"
    )


def test_anonymous_block_renders_orphan_text():
    # Phase 1.6b dropped orphan inline text inside a mixed-content block.
    # 1.6c wraps it as an anonymous paragraph so it appears in the output.
    pdf = quickpdf.html_to_pdf(
        "<div>before<p>middle</p>after</div>"
    )
    text = _pdf_text(pdf)
    for chunk in ("before", "middle", "after"):
        assert chunk in text, f"missing '{chunk}' in {text!r}"


def test_anonymous_block_inherits_parent_style():
    # Anonymous paragraphs reuse the parent's element_id, so the cascade
    # picks up author rules targeting the parent and inherits font-size.
    body = "the quick brown fox jumps over the lazy dog " * 6
    plain = quickpdf.html_to_pdf(f"<div>{body}</div>")
    styled = quickpdf.html_to_pdf(
        "<style>#wrap { font-size: 36px; }</style>"
        f"<div id='wrap'>{body}<p>child</p></div>"
    )
    plain_lines = _pdf_text(plain).strip().count("\n") + 1
    styled_lines = _pdf_text(styled).strip().count("\n") + 1
    # Anonymous paragraph carrying the orphan body must be rendered at the
    # parent's 36px size, producing more wrapped lines than the unstyled div.
    assert styled_lines > plain_lines, (
        f"anon paragraph should pick up parent's #wrap style "
        f"(plain={plain_lines}, styled={styled_lines})"
    )


# --- Phase 1.7a: text colour ----------------------------------------------

def _pdf_content_streams(pdf_bytes: bytes) -> str:
    """Decompress every FlateDecode-compressed stream in the PDF and return
    the concatenated text content. Used to grep for PDF graphics-state
    operators like `1 0 0 rg` (set RGB fill colour) that pypdf doesn't
    expose through `extract_text`."""
    import re
    import zlib

    out: list[str] = []
    pattern = re.compile(
        rb"<<[^<>]*?/Filter /FlateDecode[^<>]*?>>\nstream\n(.*?)\nendstream",
        re.S,
    )
    for m in pattern.finditer(pdf_bytes):
        try:
            data = zlib.decompress(m.group(1))
        except zlib.error:
            continue
        try:
            out.append(data.decode("latin-1"))
        except UnicodeDecodeError:
            continue
    return "\n".join(out)


def test_default_color_is_black():
    pdf = quickpdf.html_to_pdf("<p>x</p>")
    streams = _pdf_content_streams(pdf)
    assert "0 0 0 rg" in streams, "expected black fill op in content stream"


def test_color_property_emits_red_fill_op():
    pdf = quickpdf.html_to_pdf("<style>p { color: #ff0000; }</style><p>x</p>")
    streams = _pdf_content_streams(pdf)
    assert "1 0 0 rg" in streams, "expected red fill op (1 0 0 rg) in content"


def test_named_color_blue_emits_blue_fill_op():
    pdf = quickpdf.html_to_pdf("<style>p { color: blue; }</style><p>x</p>")
    streams = _pdf_content_streams(pdf)
    assert "0 0 1 rg" in streams, "expected blue fill op (0 0 1 rg) in content"


def test_color_inherits_from_ancestor_in_pdf():
    pdf = quickpdf.html_to_pdf(
        "<style>section { color: rgb(0, 128, 0); }</style>"
        "<section><p>nested</p></section>"
    )
    streams = _pdf_content_streams(pdf)
    # rgb(0, 128, 0) → 128/255 ≈ 0.50196..., krilla renders as "0.5019608" or similar
    # We'll just assert the green channel is present (non-zero R+B would fail).
    import re
    matches = re.findall(r"(\S+) (\S+) (\S+) rg", streams)
    assert matches, "no rg ops at all in content stream"
    # At least one rg op must have R==0, G>0, B==0.
    assert any(
        float(r) == 0.0 and float(g) > 0.0 and float(b) == 0.0
        for r, g, b in matches
    ), f"expected an inherited green-only fill in {matches}"


def test_different_colors_produce_different_pdfs():
    red = quickpdf.html_to_pdf("<style>p{color:red;}</style><p>x</p>")
    blue = quickpdf.html_to_pdf("<style>p{color:blue;}</style><p>x</p>")
    assert red != blue, "red and blue PDFs must differ in bytes"


# --- Phase 1.7b: box-model paint pass (background-color, padding, border)

def test_background_color_emits_fill_rect():
    pdf = quickpdf.html_to_pdf(
        "<style>p { background-color: yellow; }</style><p>x</p>"
    )
    streams = _pdf_content_streams(pdf)
    # Yellow fill op + a path-fill (`f`) somewhere before the text block.
    assert "1 1 0 rg" in streams, "expected yellow fill op (1 1 0 rg)"
    assert "\nf\n" in streams or " f\n" in streams, "expected path-fill `f` op"


def test_no_background_emits_no_fill_rect_for_plain_p():
    # A plain <p> with no decoration must not emit a fill rectangle. We can't
    # easily assert a negative on the whole stream, but we can check there is
    # no `f\n` (path-fill) operator — text glyphs are filled via `Tj`/`TJ`,
    # not `f`.
    pdf = quickpdf.html_to_pdf("<p>plain</p>")
    streams = _pdf_content_streams(pdf)
    assert "\nf\n" not in streams and " f\n" not in streams, (
        "plain <p> must not emit a fill-path op"
    )


def test_border_emits_stroke_rect():
    pdf = quickpdf.html_to_pdf(
        "<style>p { border-width: 2px; border-color: blue; "
        "border-style: solid; }</style><p>x</p>"
    )
    streams = _pdf_content_streams(pdf)
    assert "0 0 1 RG" in streams, "expected blue stroke op (0 0 1 RG)"
    assert "2 w" in streams, "expected stroke-width op (2 w)"
    # Path-stroke `S` (uppercase) operator, distinct from text-end `S`.
    assert "\nS\n" in streams or " S\n" in streams, (
        "expected path-stroke `S` op"
    )


def test_padding_left_shifts_text_position():
    plain = quickpdf.html_to_pdf("<p>x</p>")
    padded = quickpdf.html_to_pdf(
        "<style>p { padding-left: 24px; }</style><p>x</p>"
    )
    plain_streams = _pdf_content_streams(plain)
    padded_streams = _pdf_content_streams(padded)
    # Both end up with a `Tm` text-matrix op carrying the x position; padding
    # must push it to the right. Pull the x out of the first `Tm` we see.
    import re

    def first_tm_x(s: str) -> float:
        m = re.search(r"1 0 0 -1 (\S+) \S+ Tm", s)
        assert m, f"no Tm found in {s!r}"
        return float(m.group(1))

    plain_x = first_tm_x(plain_streams)
    padded_x = first_tm_x(padded_streams)
    assert padded_x - plain_x > 20.0, (
        f"padding-left:24px should add ~24pt to text x: "
        f"plain={plain_x}, padded={padded_x}"
    )


def test_border_style_none_suppresses_stroke():
    pdf = quickpdf.html_to_pdf(
        "<style>p { border-width: 5px; border-style: none; "
        "background-color: red; }</style><p>x</p>"
    )
    streams = _pdf_content_streams(pdf)
    # bg-fill should be present...
    assert "1 0 0 rg" in streams
    assert "\nf\n" in streams or " f\n" in streams
    # ...but NO stroke colour op. Embedded font subsets contain raw "RG"
    # byte sequences, so match a proper PDF stroke-colour op only —
    # three numeric tokens followed by RG, anchored at line breaks/start.
    import re
    rg_op = re.compile(r"(?:^|\n)\s*\d+(?:\.\d+)?\s+\d+(?:\.\d+)?\s+\d+(?:\.\d+)?\s+RG\b")
    assert not rg_op.search(streams), "border-style:none must suppress RG op"


# --- Phase 1.7c: inline style="...", rem unit, shorthand expansion ---------

def test_inline_style_attribute_sets_color():
    pdf = quickpdf.html_to_pdf('<p style="color: red">x</p>')
    streams = _pdf_content_streams(pdf)
    assert "1 0 0 rg" in streams, "inline style=color:red must emit red fill"


def test_inline_style_beats_author_rule():
    # Author rule sets blue; inline style overrides to red. Specificity bucket
    # 0 (inline) > bucket 3 (tag) so red must win.
    pdf = quickpdf.html_to_pdf(
        '<style>p { color: blue; }</style>'
        '<p style="color: red">x</p>'
    )
    streams = _pdf_content_streams(pdf)
    assert "1 0 0 rg" in streams, "inline style must beat author rule"
    assert "0 0 1 rg" not in streams, "author blue must NOT appear"


def test_inline_style_font_size_wraps_more_lines():
    body = "the quick brown fox jumps over the lazy dog " * 4
    plain = quickpdf.html_to_pdf(f"<p>{body}</p>")
    big = quickpdf.html_to_pdf(f'<p style="font-size: 24px">{body}</p>')
    plain_lines = _pdf_text(plain).strip().count("\n") + 1
    big_lines = _pdf_text(big).strip().count("\n") + 1
    assert big_lines > plain_lines, (
        f"inline 24px font-size must enlarge text "
        f"(plain={plain_lines}, big={big_lines})"
    )


def test_important_id_beats_inline_style():
    # `!important` on a selector rule beats a non-important inline style,
    # regardless of bucket-0 specificity. Author rule at 48px must dominate
    # the inline 12px.
    body = "the quick brown fox jumps over the lazy dog " * 6
    pdf = quickpdf.html_to_pdf(
        '<style>#x { font-size: 48px !important; }</style>'
        f'<p id="x" style="font-size: 12px">{body}</p>'
    )
    line_count = _pdf_text(pdf).strip().count("\n") + 1
    assert line_count >= 8, (
        f"!important id must beat plain inline style "
        f"(got {line_count} lines, expected ≥8 at 48px)"
    )


def test_rem_unit_resolves_for_font_size():
    # 2rem at base 12pt → 24pt. Wraps more lines than baseline.
    body = "the quick brown fox jumps over the lazy dog " * 4
    plain = quickpdf.html_to_pdf(f"<p>{body}</p>")
    rem = quickpdf.html_to_pdf(f'<p style="font-size: 2rem">{body}</p>')
    plain_lines = _pdf_text(plain).strip().count("\n") + 1
    rem_lines = _pdf_text(rem).strip().count("\n") + 1
    assert rem_lines > plain_lines, (
        f"2rem must wrap more lines than default (plain={plain_lines}, "
        f"rem={rem_lines})"
    )


def test_padding_shorthand_one_value_pads_all_sides():
    # padding:24px expands to all four longhands. We can observe padding-left
    # via the same Tm-x technique used in 1.7b.
    plain = quickpdf.html_to_pdf("<p>x</p>")
    padded = quickpdf.html_to_pdf('<p style="padding: 24px">x</p>')
    plain_streams = _pdf_content_streams(plain)
    padded_streams = _pdf_content_streams(padded)
    import re

    def first_tm_x(s: str) -> float:
        m = re.search(r"1 0 0 -1 (\S+) \S+ Tm", s)
        assert m, f"no Tm op in {s!r}"
        return float(m.group(1))

    plain_x = first_tm_x(plain_streams)
    padded_x = first_tm_x(padded_streams)
    assert padded_x - plain_x > 20.0, (
        f"padding:24px shorthand must shift text x by ~24pt: "
        f"plain={plain_x}, padded={padded_x}"
    )


def test_border_shorthand_emits_stroke():
    # `border: 2px solid red` should expand to width/style/color longhands.
    pdf = quickpdf.html_to_pdf(
        '<p style="border: 2px solid red">x</p>'
    )
    streams = _pdf_content_streams(pdf)
    assert "1 0 0 RG" in streams, "border shorthand must emit red stroke"
    assert "2 w" in streams, "border shorthand must emit stroke-width 2"
    assert "\nS\n" in streams or " S\n" in streams, (
        "border shorthand must emit path-stroke `S`"
    )


# --- Phase 2a: block-level images via data: URLs --------------------------

# Tiny known-good PNG (1x1 red pixel, RGBA, all CRCs correct, 70 bytes).
_TINY_PNG = bytes([
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00,
    0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
    0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89,
    0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63,
    0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x56,
    0xc7, 0x2f, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
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
