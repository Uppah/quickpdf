//! Embedded fallback font.
//!
//! quickpdf ships one open-license sans-serif font in the wheel so PDF
//! rendering "just works" on a fresh container with no system fonts. Phase 4
//! will add a font registry that prefers system fonts, falling back to this
//! when the requested family isn't available.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
