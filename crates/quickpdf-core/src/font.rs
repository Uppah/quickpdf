//! Embedded fallback font and Phase 2b font registry.
//!
//! quickpdf ships one open-license sans-serif font in the wheel so PDF
//! rendering "just works" on a fresh container with no system fonts.
//! Phase 2b adds `FontRegistry`: a per-document map from family name to
//! concrete font (krilla `Font` instance + raw bytes for skrifa-side glyph
//! metrics). The bundled Inter is always pre-registered at handle 0 and
//! serves as the silent fallback when a `font-family` cascade chain has
//! no registered hit.
//!
//! License: SIL Open Font License 1.1 (see assets/fonts/Inter-Regular.LICENSE.txt)
//! Source: https://github.com/rsms/inter

/// Inter Regular, Latin subset (~68 KB). Wide enough for English/Danish text
/// and our Phase 1 test fixtures; Phase 2 will swap in a broader subset or
/// dynamic font loading.
pub static FALLBACK_TTF: &[u8] =
    include_bytes!("../assets/fonts/Inter-Regular.ttf");

/// Family name reported for the embedded fallback. Use this when callers
/// haven't specified a `font-family` and we need to label glyph runs.
pub const FALLBACK_FAMILY: &str = "Inter";

// ---------------------------------------------------------------------------
// Phase 2b: font registry. Builds a per-document map from family name to
// concrete font (krilla `Font` instance + raw bytes for skrifa-side glyph
// metrics). The bundled Inter is always pre-registered at handle 0 and
// serves as the silent fallback when a `font-family` cascade chain has
// no registered hit.
// ---------------------------------------------------------------------------

use crate::style::sheet::FontFace;
use base64::Engine;
use std::collections::HashMap;
use std::sync::Arc;

/// Opaque handle into `FontRegistry::fonts`. Index 0 is always the
/// bundled Inter fallback; any unsuccessful lookup returns 0.
pub type FontHandle = usize;

/// One registered font: raw bytes (kept for skrifa glyph-advance
/// measurement) plus the krilla `Font` instance used at PDF emit time.
#[derive(Clone)]
pub struct RegisteredFont {
    /// Owned via `Arc<[u8]>` so multiple call sites (planner's
    /// `TextMetrics::new`, krilla's `Font::new`) can hold cheap
    /// references without copying.
    pub bytes: Arc<[u8]>,
    pub krilla_font: krilla::text::Font,
}

/// A document-scoped registry of known font faces. Built once per
/// `html_to_pdf` call and consumed by the planner and emitter.
pub struct FontRegistry {
    /// Index 0 is always Inter (the bundled fallback).
    pub fonts: Vec<RegisteredFont>,
    /// Lowercased family name → handle. Inter is registered as "inter"
    /// so authors can name it explicitly via `font-family: Inter`.
    pub by_family: HashMap<String, FontHandle>,
}

impl FontRegistry {
    /// Build a registry from the parsed `@font-face` rules. The bundled
    /// Inter is always pre-registered at handle 0 (under the lowercased
    /// key `"inter"`). Faces are processed in `source_order`; duplicate
    /// family names overwrite earlier handles (last-wins, mirroring
    /// the rest of the cascade). Faces with no decodable `src:` are
    /// silently dropped.
    pub fn build(font_faces: &[FontFace]) -> Self {
        let mut fonts: Vec<RegisteredFont> = Vec::with_capacity(1 + font_faces.len());
        let mut by_family: HashMap<String, FontHandle> = HashMap::new();

        // Pre-register Inter at handle 0.
        let inter_bytes: Arc<[u8]> = Arc::from(FALLBACK_TTF);
        if let Some(font) = krilla::text::Font::new(inter_bytes.to_vec().into(), 0) {
            fonts.push(RegisteredFont {
                bytes: inter_bytes,
                krilla_font: font,
            });
            by_family.insert(FALLBACK_FAMILY.to_ascii_lowercase(), 0);
        } else {
            // Should never happen: FALLBACK_TTF is verified by an
            // existing unit test. If it does happen, downstream code
            // expects fonts[0] to exist; panic loudly so we notice.
            panic!("bundled Inter failed to load — corrupt FALLBACK_TTF?");
        }

        // Walk faces in source order. Each one may add a new font (or
        // overwrite an existing handle, last-wins). Faces with no
        // decodable src are silently dropped.
        for face in font_faces {
            let descriptors = match extract_descriptors(face) {
                Some(d) => d,
                None => continue,
            };
            let bytes = match load_first_decodable_src(descriptors.src_value) {
                Some(b) => b,
                None => continue,
            };
            let arc_bytes: Arc<[u8]> = Arc::from(bytes);
            let krilla_font = match krilla::text::Font::new(arc_bytes.to_vec().into(), 0) {
                Some(f) => f,
                None => continue,
            };
            // Last-wins on duplicate family names: overwrite the handle.
            let new_handle = fonts.len();
            fonts.push(RegisteredFont {
                bytes: arc_bytes,
                krilla_font,
            });
            by_family.insert(descriptors.family, new_handle);
        }

        FontRegistry { fonts, by_family }
    }

    /// Walk a resolved family chain (lowercased, quote-stripped) and
    /// return the first registered handle. Returns 0 (Inter) if the
    /// chain is empty or no name matches.
    pub fn lookup(&self, family_chain: &[String]) -> FontHandle {
        for name in family_chain {
            if let Some(&h) = self.by_family.get(name) {
                return h;
            }
        }
        0
    }
}

// ---------------------------------------------------------------------------
// Phase 2b: @font-face descriptor extraction and src-list parsing.
// ---------------------------------------------------------------------------

/// Internal: per-face data extracted from the declaration list.
struct ParsedDescriptors<'a> {
    family: String,       // lowercased, quote-stripped, trimmed
    src_value: &'a str,   // raw value of the src descriptor
}

/// Pull out the first `font-family` and `src` declarations (last-wins
/// among duplicates within a single block). Returns `None` if either
/// descriptor is missing or the family normalises to an empty string.
fn extract_descriptors(face: &FontFace) -> Option<ParsedDescriptors<'_>> {
    let mut family_raw: Option<&str> = None;
    let mut src_value: Option<&str> = None;
    for d in &face.declarations {
        match d.name.as_str() {
            "font-family" => family_raw = Some(d.value.as_str()),
            "src" => src_value = Some(d.value.as_str()),
            _ => {}
        }
    }
    let raw = family_raw?;
    let src_value = src_value?;

    // The font-family DESCRIPTOR is technically a single name, but real
    // authoring tools sometimes emit a comma list ("Acme", fallback).
    // We honour only the first comma-separated entry.
    let first = raw.split(',').next()?.trim();
    let stripped = strip_outer_quotes(first).trim();
    if stripped.is_empty() {
        return None;
    }
    Some(ParsedDescriptors {
        family: stripped.to_ascii_lowercase(),
        src_value,
    })
}

/// Strip a single pair of matching surrounding `"..."` or `'...'` quotes.
/// No-op if the input isn't quoted.
fn strip_outer_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// One entry in a `src:` list. Phase 2b only acts on `Url`; `Local` is
/// captured for symmetry but always skipped (per spec — no system probe).
#[derive(Debug, Clone, PartialEq, Eq)]
enum SrcEntry {
    Url(String),
    Local(String),
    /// Anything we couldn't classify. Caller skips.
    Unknown,
}

/// Tokenise a `src:` descriptor value into entries. The grammar is
/// comma-separated; each entry is one of:
///   - `url(<url>)` optionally followed by `format(<hint>)`
///   - `local(<name>)`
///
/// Whitespace between tokens is tolerated. `format(...)` hints are
/// captured but discarded — the registry sniffs bytes itself. Quotes
/// inside `url(...)` / `local(...)` are stripped.
fn parse_src_list(value: &str) -> Vec<SrcEntry> {
    let mut out: Vec<SrcEntry> = Vec::new();
    for piece in split_top_level_commas(value) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        // Strip an optional trailing `format(...)` so the head is purely
        // url(...) or local(...).
        let head = match split_off_format_hint(piece) {
            Some((h, _hint)) => h.trim(),
            None => piece,
        };
        if let Some(inner) = head.strip_prefix("url(").and_then(|s| s.strip_suffix(')')) {
            let url = strip_outer_quotes(inner.trim()).trim();
            out.push(SrcEntry::Url(url.to_string()));
        } else if let Some(inner) = head.strip_prefix("local(").and_then(|s| s.strip_suffix(')')) {
            let name = strip_outer_quotes(inner.trim()).trim();
            out.push(SrcEntry::Local(name.to_string()));
        } else {
            out.push(SrcEntry::Unknown);
        }
    }
    out
}

/// Split a string on top-level commas, respecting parens and quotes.
/// Used to break a `src:` descriptor into entries.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out: Vec<&str> = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0;
    let mut pos = 0;
    let mut in_quote: Option<u8> = None;
    while pos < bytes.len() {
        let b = bytes[pos];
        match in_quote {
            Some(q) => {
                if b == b'\\' && pos + 1 < bytes.len() {
                    pos += 2;
                    continue;
                }
                if b == q {
                    in_quote = None;
                }
                pos += 1;
            }
            None => {
                if b == b'"' || b == b'\'' {
                    in_quote = Some(b);
                    pos += 1;
                } else if b == b'(' {
                    depth += 1;
                    pos += 1;
                } else if b == b')' {
                    if depth > 0 {
                        depth -= 1;
                    }
                    pos += 1;
                } else if b == b',' && depth == 0 {
                    out.push(&s[start..pos]);
                    pos += 1;
                    start = pos;
                } else {
                    pos += 1;
                }
            }
        }
    }
    if start < bytes.len() {
        out.push(&s[start..]);
    }
    out
}

/// If `entry` ends with a top-level `format(...)` clause, split it off
/// and return `(head, hint_inner)`. Otherwise return `None`.
fn split_off_format_hint(entry: &str) -> Option<(&str, &str)> {
    // Search for the last top-level `format(` in the trimmed entry.
    // We only support the trailing-hint shape, which is the only one
    // CSS Fonts 4 actually defines.
    let trimmed = entry.trim_end();
    let bytes = trimmed.as_bytes();
    if !trimmed.ends_with(')') {
        return None;
    }
    // Walk backward: find the matching `format(` for the trailing `)`.
    let mut depth = 0i32;
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                depth -= 1;
                if depth == 0 {
                    // Check the preceding 6 bytes are "format" (case-insensitive).
                    if i >= 6 {
                        let kw = &trimmed[i - 6..i];
                        if kw.eq_ignore_ascii_case("format") {
                            let head = trimmed[..i - 6].trim();
                            let hint = &trimmed[i + 1..bytes.len() - 1];
                            return Some((head, hint));
                        }
                    }
                    return None;
                }
            }
            _ => {}
        }
    }
    None
}

/// Decode a single `url(data:...)` payload into raw font bytes if the
/// MIME and magic bytes both clear the gate. Returns `None` for any
/// failure mode (unsupported scheme, non-data URL, unaccepted MIME,
/// missing `;base64,` segment, malformed base64, wrong magic).
fn decode_data_url(url: &str) -> Option<Vec<u8>> {
    // 1. Must start with "data:" (case-insensitive).
    if url.len() < 5 || !url[..5].eq_ignore_ascii_case("data:") {
        return None;
    }
    let after = &url[5..];

    // 2. Find the first ';' which separates the MIME from the encoding.
    //    No ';' → malformed for our purposes.
    let semi = after.find(';')?;
    let mime = after[..semi].trim().to_ascii_lowercase();

    // 3. MIME accept list. Permissive: real authoring tools emit several
    //    variants for the same payload format.
    let mime_ok = matches!(
        mime.as_str(),
        "font/ttf"
            | "font/otf"
            | "application/font-sfnt"
            | "application/x-font-ttf"
            | "application/x-font-otf"
            | "application/octet-stream"
    );
    if !mime_ok {
        return None;
    }

    // 4. Require ";base64," (we don't support percent-encoded data URLs
    //    for binary payloads — same posture as Phase 2a's image parser).
    let rest = &after[semi..];
    let payload = rest.strip_prefix(";base64,")?;

    // 5. Decode.
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.as_bytes())
        .ok()?;

    // 6. Magic-byte sniff: TrueType (0x00010000) or OpenType/CFF ("OTTO").
    if bytes.len() < 4 {
        return None;
    }
    let magic_ok = bytes[..4] == [0x00, 0x01, 0x00, 0x00] || &bytes[..4] == b"OTTO";
    if !magic_ok {
        return None;
    }

    Some(bytes)
}

/// Walk a face's src list and load the first decodable url() entry.
/// Returns `None` if every entry was unacceptable (skipped or failed).
fn load_first_decodable_src(src_value: &str) -> Option<Vec<u8>> {
    for entry in parse_src_list(src_value) {
        if let SrcEntry::Url(url) = entry {
            if let Some(bytes) = decode_data_url(&url) {
                return Some(bytes);
            }
        }
        // SrcEntry::Local and SrcEntry::Unknown: skip, try next.
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::sheet::Declaration;

    #[test]
    fn fallback_font_is_a_real_font() {
        // OpenType / TrueType files start with "OTTO" (CFF) or 0x00010000 (true).
        // Either is fine for our purposes.
        assert!(FALLBACK_TTF.len() > 1024, "font too small: {}", FALLBACK_TTF.len());
        let magic = &FALLBACK_TTF[..4];
        assert!(
            magic == b"OTTO" || magic == &[0x00, 0x01, 0x00, 0x00],
            "unexpected font magic {:02X?}",
            magic
        );
    }

    // ---- Phase 2b: FontRegistry. ----

    #[test]
    fn registry_with_no_faces_has_inter_at_handle_zero() {
        let registry = FontRegistry::build(&[]);
        assert_eq!(registry.fonts.len(), 1);
        assert_eq!(registry.by_family.get("inter").copied(), Some(0));
    }

    #[test]
    fn registry_lookup_empty_chain_returns_zero() {
        let registry = FontRegistry::build(&[]);
        assert_eq!(registry.lookup(&[]), 0);
    }

    #[test]
    fn registry_lookup_unknown_family_returns_zero() {
        let registry = FontRegistry::build(&[]);
        assert_eq!(registry.lookup(&["notregistered".to_string()]), 0);
    }

    #[test]
    fn registry_lookup_inter_by_name_returns_zero() {
        let registry = FontRegistry::build(&[]);
        // Authors can spell out the bundled font's name; it resolves to
        // handle 0, same as the silent fallback.
        assert_eq!(registry.lookup(&["inter".to_string()]), 0);
    }

    /// Build a `FontFace` from a set of (name, value) declaration pairs
    /// — bypasses the full stylesheet parser so each test can pin one
    /// behaviour at a time. `important` is always false (irrelevant for
    /// font-face descriptors per CSS spec).
    fn fixture_face(decls: &[(&str, &str)], source_order: usize) -> FontFace {
        FontFace {
            declarations: decls
                .iter()
                .map(|(n, v)| Declaration {
                    name: n.to_string(),
                    value: v.to_string(),
                    important: false,
                })
                .collect(),
            source_order,
        }
    }

    /// Base64-encode the bundled Inter bytes so tests can build valid
    /// `data:font/ttf;base64,...` URLs without external fixtures. Inter
    /// is a real TTF and passes the magic-byte sniff.
    fn inter_data_url() -> String {
        let b64 = base64::engine::general_purpose::STANDARD.encode(FALLBACK_TTF);
        format!("data:font/ttf;base64,{b64}")
    }

    #[test]
    fn registry_registers_one_valid_face() {
        let url = inter_data_url();
        let face = fixture_face(
            &[
                ("font-family", "Acme"),
                ("src", &format!("url({url})")),
            ],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        // Inter at 0, Acme at 1.
        assert_eq!(registry.fonts.len(), 2);
        assert_eq!(registry.by_family.get("acme").copied(), Some(1));
        assert_eq!(registry.lookup(&["acme".to_string()]), 1);
    }

    #[test]
    fn registry_drops_face_with_no_font_family() {
        let face = fixture_face(&[("src", &format!("url({})", inter_data_url()))], 0);
        let registry = FontRegistry::build(&[face]);
        // Only Inter remains; the orphaned src is dropped.
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_drops_face_with_empty_font_family() {
        let face = fixture_face(
            &[
                ("font-family", "\"\""),
                ("src", &format!("url({})", inter_data_url())),
            ],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_drops_face_with_no_src() {
        let face = fixture_face(&[("font-family", "Acme")], 0);
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
        assert!(registry.by_family.get("acme").is_none());
    }

    #[test]
    fn registry_drops_face_with_only_local_src() {
        let face = fixture_face(
            &[("font-family", "Acme"), ("src", "local(\"Arial\")")],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
        assert!(registry.by_family.get("acme").is_none());
    }

    #[test]
    fn registry_drops_face_with_only_http_src() {
        let face = fixture_face(
            &[
                ("font-family", "Acme"),
                ("src", "url(https://example.com/Acme.woff2)"),
            ],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_drops_face_with_only_woff2_data_url() {
        // data:font/woff2 is not in the accept list. Even if it were,
        // the magic-byte sniff would reject WOFF2's "wOF2" header.
        let url = "data:font/woff2;base64,d09GMgABAAAAAAAA";
        let face = fixture_face(
            &[("font-family", "Acme"), ("src", &format!("url({url})"))],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_walks_multi_src_to_first_acceptable() {
        // First entry: woff2 (rejected). Second: ttf (accepted). The
        // registry should pick the second.
        let url_ttf = inter_data_url();
        let value = format!(
            "url(data:font/woff2;base64,d09GMg==) format(\"woff2\"), \
             url({url_ttf}) format(\"truetype\")"
        );
        let face = fixture_face(&[("font-family", "Acme"), ("src", &value)], 0);
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 2);
        assert_eq!(registry.by_family.get("acme").copied(), Some(1));
    }

    #[test]
    fn registry_drops_face_with_base64_garbage() {
        let face = fixture_face(
            &[
                ("font-family", "Acme"),
                ("src", "url(data:font/ttf;base64,!!!notbase64!!!)"),
            ],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_drops_face_with_wrong_magic_in_ttf_mime() {
        // Valid base64 of bytes that don't pass the magic sniff (random
        // 4-byte payload). MIME claims TTF but bytes aren't TrueType.
        let payload = base64::engine::general_purpose::STANDARD.encode([0xDE, 0xAD, 0xBE, 0xEF]);
        let url = format!("data:font/ttf;base64,{payload}");
        let face = fixture_face(
            &[("font-family", "Acme"), ("src", &format!("url({url})"))],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 1);
    }

    #[test]
    fn registry_accepts_octet_stream_with_ttf_magic() {
        // application/octet-stream is accepted only when the magic
        // bytes confirm the payload is TTF/OTF.
        let b64 = base64::engine::general_purpose::STANDARD.encode(FALLBACK_TTF);
        let url = format!("data:application/octet-stream;base64,{b64}");
        let face = fixture_face(
            &[("font-family", "Acme"), ("src", &format!("url({url})"))],
            0,
        );
        let registry = FontRegistry::build(&[face]);
        assert_eq!(registry.fonts.len(), 2);
        assert_eq!(registry.by_family.get("acme").copied(), Some(1));
    }

    #[test]
    fn registry_duplicate_family_last_wins() {
        // Two @font-face blocks both declare "Acme"; the second registers
        // a new handle and overwrites the first in by_family.
        let url = inter_data_url();
        let f1 = fixture_face(
            &[("font-family", "Acme"), ("src", &format!("url({url})"))],
            0,
        );
        let f2 = fixture_face(
            &[("font-family", "Acme"), ("src", &format!("url({url})"))],
            1,
        );
        let registry = FontRegistry::build(&[f1, f2]);
        // Both faces register; by_family points at the second (handle 2).
        assert_eq!(registry.fonts.len(), 3);
        assert_eq!(registry.by_family.get("acme").copied(), Some(2));
    }

    #[test]
    fn registry_lookup_walks_chain_left_to_right() {
        // Build a registry with two registered families; lookup picks
        // whichever appears first in the chain.
        let url = inter_data_url();
        let f1 = fixture_face(
            &[("font-family", "Alpha"), ("src", &format!("url({url})"))],
            0,
        );
        let f2 = fixture_face(
            &[("font-family", "Beta"), ("src", &format!("url({url})"))],
            1,
        );
        let registry = FontRegistry::build(&[f1, f2]);
        assert_eq!(
            registry.lookup(&["alpha".to_string(), "beta".to_string()]),
            registry.by_family.get("alpha").copied().unwrap()
        );
        assert_eq!(
            registry.lookup(&["unknown".to_string(), "beta".to_string()]),
            registry.by_family.get("beta").copied().unwrap()
        );
        assert_eq!(
            registry.lookup(&["unknown1".to_string(), "unknown2".to_string()]),
            0
        );
    }
}
