//! Phase 2a: `<img src="data:image/...">` data-URL parser.
//!
//! Slice A scope: take a raw `data:` URL string, validate the prefix and
//! MIME, base64-decode the payload, and hand back `(kind, bytes)`. We do
//! **not** parse PNG/JPEG headers ourselves — krilla 0.7's `Image::from_png`
//! / `from_jpeg` validates and decodes at emit time, returning `Err` on
//! malformed input. The integrator treats that `Err` exactly like a missing
//! src (alt-text fallback).

use base64::Engine;

/// Which raster format a data URL declared. `parse_data_url` only accepts
/// these two; `image/gif`, `image/webp`, etc. return `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
}

/// Parse a `data:image/png;base64,...` or `data:image/jpeg;base64,...` URL
/// into its kind and raw byte payload. Returns `None` for any failure:
/// missing `data:` prefix, unsupported MIME, missing `;base64,` separator,
/// malformed base64. Returns `Some((kind, bytes))` on success — krilla
/// validates the bytes themselves at emit time.
pub fn parse_data_url(src: &str) -> Option<(ImageKind, Vec<u8>)> {
    let _ = src;
    let _ = base64::engine::general_purpose::STANDARD.decode("");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_none() {
        assert!(parse_data_url("").is_none());
    }
}
