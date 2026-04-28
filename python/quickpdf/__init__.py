"""quickpdf — native HTML→PDF rendering for Python.

Phase 0 surface: only ``html_to_pdf`` is wired. ``HTML``/``BulkSession`` arrive
in Phase 3 once block + inline + table layout exists.
"""

from __future__ import annotations

from os import PathLike
from pathlib import Path
from typing import Optional, Union

from . import _native

__version__ = _native.__version__

__all__ = ["html_to_pdf", "__version__"]


PageSize = str  # "A4" | "Letter" — typed loosely until we add tuple support


def html_to_pdf(
    html: str,
    *,
    page_size: Optional[PageSize] = None,
    print_background: bool = True,
    output: Union[str, PathLike, None] = None,
) -> bytes:
    """Render an HTML string to PDF bytes.

    Phase 0: emits a blank page of the requested size. The HTML argument is
    accepted but not yet rendered — that lands in Phase 1.

    Args:
        html: HTML source to render. Must be a complete document fragment.
        page_size: ``"A4"`` (default) or ``"Letter"``.
        print_background: Whether to paint background colours/images. Default True.
        output: Optional path. If given, the PDF is also written to this path
            and the bytes are still returned for callers that want both.

    Returns:
        The PDF as a ``bytes`` object. Always starts with ``b"%PDF-"``.
    """
    pdf_bytes = _native.html_to_pdf(
        html,
        page_size=page_size,
        print_background=print_background,
    )
    if output is not None:
        Path(output).write_bytes(pdf_bytes)
    return pdf_bytes
