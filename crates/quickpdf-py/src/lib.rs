//! PyO3 bindings for quickpdf-core.
//!
//! Phase 0: a single `html_to_pdf(html: str) -> bytes` function. The
//! `BulkSession` and richer options arrive with Phase 3 once layout works.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use quickpdf_core::{
    html_to_pdf as core_html_to_pdf, ParsedDocument, PageSize, RenderOptions,
};

fn parse_page_size(arg: Option<&str>) -> PyResult<PageSize> {
    match arg.map(|s| s.to_ascii_lowercase()) {
        None => Ok(PageSize::A4),
        Some(s) if s == "a4" => Ok(PageSize::A4),
        Some(s) if s == "letter" => Ok(PageSize::Letter),
        Some(other) => Err(PyValueError::new_err(format!(
            "unsupported page_size {other:?} (expected 'A4' or 'Letter')"
        ))),
    }
}

#[pyfunction]
#[pyo3(signature = (html, *, page_size=None, print_background=true))]
fn html_to_pdf<'py>(
    py: Python<'py>,
    html: &str,
    page_size: Option<&str>,
    print_background: bool,
) -> PyResult<Bound<'py, PyBytes>> {
    let options = RenderOptions {
        page_size: parse_page_size(page_size)?,
        print_background,
    };
    // Drop the GIL while Rust does its work — important once real layout exists.
    let bytes = py
        .allow_threads(|| core_html_to_pdf(html, &options))
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(PyBytes::new(py, &bytes))
}

/// Debug helper: parse HTML and return the visible text content. Mirrors what
/// the eventual layout pipeline will see as the "leaf text" stream. Useful as
/// a Phase 1 testing hook from Python — it lets us assert "the parser sees X"
/// without waiting for paint.
#[pyfunction]
fn _debug_visible_text(py: Python<'_>, html: &str) -> PyResult<String> {
    Ok(py.allow_threads(|| ParsedDocument::parse(html).visible_text()))
}

/// Debug helper: number of element nodes in the parsed DOM.
#[pyfunction]
fn _debug_element_count(py: Python<'_>, html: &str) -> PyResult<usize> {
    Ok(py.allow_threads(|| ParsedDocument::parse(html).element_count()))
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(html_to_pdf, m)?)?;
    m.add_function(wrap_pyfunction!(_debug_visible_text, m)?)?;
    m.add_function(wrap_pyfunction!(_debug_element_count, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
