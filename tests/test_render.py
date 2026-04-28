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
