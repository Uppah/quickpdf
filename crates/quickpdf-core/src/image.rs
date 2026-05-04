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
    // Strip the `data:` prefix.
    let after_data = src.strip_prefix("data:")?;
    // Split on the first `,` — left = MIME + params, right = payload.
    let (params, payload) = after_data.split_once(',')?;
    // We only accept base64-encoded data URLs. Plain (URL-encoded) data URLs
    // are rare in HTML and out of scope for Phase 2a.
    let mime = params.strip_suffix(";base64")?;
    let kind = match mime {
        "image/png" => ImageKind::Png,
        "image/jpeg" | "image/jpg" => ImageKind::Jpeg,
        _ => return None,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.as_bytes())
        .ok()?;
    Some((kind, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// Tiny known-good PNG (1x1 red pixel). Verified valid by hand.
    const TINY_PNG_BYTES: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00,
        0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89,
        0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63,
        0xfc, 0xcf, 0xc0, 0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a,
        0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
        0x42, 0x60, 0x82,
    ];

    /// Tiny known-good JPEG (1x1 white pixel, baseline DCT, no EXIF).
    const TINY_JPEG_BYTES: &[u8] = &[
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00,
        0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xff, 0xdb,
        0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06, 0x05, 0x08, 0x07,
        0x07, 0x07, 0x09, 0x09, 0x08, 0x0a, 0x0c, 0x14, 0x0d, 0x0c, 0x0b,
        0x0b, 0x0c, 0x19, 0x12, 0x13, 0x0f, 0x14, 0x1d, 0x1a, 0x1f, 0x1e,
        0x1d, 0x1a, 0x1c, 0x1c, 0x20, 0x24, 0x2e, 0x27, 0x20, 0x22, 0x2c,
        0x23, 0x1c, 0x1c, 0x28, 0x37, 0x29, 0x2c, 0x30, 0x31, 0x34, 0x34,
        0x34, 0x1f, 0x27, 0x39, 0x3d, 0x38, 0x32, 0x3c, 0x2e, 0x33, 0x34,
        0x32, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01,
        0x01, 0x11, 0x00, 0xff, 0xc4, 0x00, 0x14, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0xff, 0xc4, 0x00, 0x14, 0x10, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00,
        0x3f, 0x00, 0x37, 0xff, 0xd9,
    ];

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn empty_input_returns_none() {
        assert!(parse_data_url("").is_none());
    }

    #[test]
    fn non_data_url_returns_none() {
        assert!(parse_data_url("https://example.com/x.png").is_none());
        assert!(parse_data_url("file:///x.png").is_none());
        assert!(parse_data_url("/relative/x.png").is_none());
    }

    #[test]
    fn valid_png_data_url_parses() {
        let url = format!("data:image/png;base64,{}", b64(TINY_PNG_BYTES));
        let (kind, bytes) = parse_data_url(&url).expect("png parses");
        assert_eq!(kind, ImageKind::Png);
        assert_eq!(bytes, TINY_PNG_BYTES);
    }

    #[test]
    fn valid_jpeg_data_url_parses() {
        let url = format!("data:image/jpeg;base64,{}", b64(TINY_JPEG_BYTES));
        let (kind, bytes) = parse_data_url(&url).expect("jpeg parses");
        assert_eq!(kind, ImageKind::Jpeg);
        assert_eq!(bytes, TINY_JPEG_BYTES);
    }

    #[test]
    fn jpg_alias_also_parses() {
        // Browsers accept both `image/jpeg` and `image/jpg`.
        let url = format!("data:image/jpg;base64,{}", b64(TINY_JPEG_BYTES));
        let (kind, _) = parse_data_url(&url).expect("jpg alias parses");
        assert_eq!(kind, ImageKind::Jpeg);
    }
}
