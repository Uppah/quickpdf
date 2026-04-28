"""Type stubs for the native PyO3 module."""

from typing import Optional

__version__: str


def html_to_pdf(
    html: str,
    *,
    page_size: Optional[str] = None,
    print_background: bool = True,
) -> bytes: ...


def _debug_visible_text(html: str) -> str: ...


def _debug_element_count(html: str) -> int: ...
