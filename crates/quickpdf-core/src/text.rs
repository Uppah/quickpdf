//! Text measurement + greedy word-wrap.
//!
//! Phase 1.4: measure per-character advance widths via skrifa and break a
//! string into lines that fit within a target width. ASCII-correct; complex
//! shaping (kerning, ligatures, BiDi, scripts that need rustybuzz) is a
//! Phase 2/4 problem.

use skrifa::instance::{LocationRef, Size};
use skrifa::metrics::GlyphMetrics;
use skrifa::raw::TableProvider;
use skrifa::{FontRef, MetadataProvider};

/// Lightweight wrapper bundling the bits skrifa needs to measure text
/// advances. Construct once per `(font_data, font_size)` pair and reuse for
/// many calls — building it parses font tables.
pub struct TextMetrics<'a> {
    font: FontRef<'a>,
    glyph_metrics: GlyphMetrics<'a>,
    /// Multiply font-unit advance by this to get points at `font_size`.
    units_to_pt: f32,
}

impl<'a> TextMetrics<'a> {
    pub fn new(font_data: &'a [u8], font_size_pt: f32) -> Option<Self> {
        let font = FontRef::new(font_data).ok()?;
        let upem = font.head().ok()?.units_per_em() as f32;
        let glyph_metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
        Some(Self {
            font,
            glyph_metrics,
            units_to_pt: font_size_pt / upem,
        })
    }

    /// Advance width in points for a single character. Returns 0 for chars
    /// the font can't map (caller decides whether to substitute or skip).
    pub fn char_advance(&self, ch: char) -> f32 {
        let glyph_id = self.font.charmap().map(ch).unwrap_or_default();
        self.glyph_metrics
            .advance_width(glyph_id)
            .unwrap_or(0.0)
            * self.units_to_pt
    }

    /// Advance width in points for a string segment.
    pub fn measure(&self, text: &str) -> f32 {
        text.chars().map(|c| self.char_advance(c)).sum()
    }
}

/// Greedy word-wrap. Returns the input split into lines, each fitting in
/// `max_width_pt`. Words longer than the line width get their own line and
/// will overflow — Phase 2 will add break-anywhere fallback for very long
/// tokens (URLs, etc.).
///
/// Whitespace runs collapse to single spaces between words.
pub fn wrap_lines(metrics: &TextMetrics<'_>, text: &str, max_width_pt: f32) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let space_w = metrics.char_advance(' ');
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0.0_f32;

    for word in text.split_whitespace() {
        let word_w = metrics.measure(word);
        if current.is_empty() {
            current.push_str(word);
            current_w = word_w;
        } else if current_w + space_w + word_w <= max_width_pt {
            current.push(' ');
            current.push_str(word);
            current_w += space_w + word_w;
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
            current_w = word_w;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FALLBACK_TTF;

    #[test]
    fn measures_individual_characters() {
        let m = TextMetrics::new(FALLBACK_TTF, 12.0).unwrap();
        // Space is narrower than 'M' in any sane font.
        assert!(m.char_advance(' ') < m.char_advance('M'));
        // Advance is positive for ASCII letters.
        assert!(m.char_advance('A') > 0.0);
    }

    #[test]
    fn measure_string_sums_chars() {
        let m = TextMetrics::new(FALLBACK_TTF, 12.0).unwrap();
        let direct = m.measure("Hi");
        let summed = m.char_advance('H') + m.char_advance('i');
        assert!((direct - summed).abs() < 0.01, "{direct} vs {summed}");
    }

    #[test]
    fn wrap_short_text_one_line() {
        let m = TextMetrics::new(FALLBACK_TTF, 12.0).unwrap();
        let lines = wrap_lines(&m, "hello", 200.0);
        assert_eq!(lines, vec!["hello".to_string()]);
    }

    #[test]
    fn wrap_long_text_breaks_at_word_boundary() {
        let m = TextMetrics::new(FALLBACK_TTF, 12.0).unwrap();
        let lines = wrap_lines(
            &m,
            "the quick brown fox jumps over the lazy dog",
            60.0, // narrow
        );
        assert!(lines.len() >= 3, "expected multi-line wrap, got {lines:?}");
        // Reassembled lines preserve word order and content.
        let reassembled = lines.join(" ");
        assert_eq!(reassembled, "the quick brown fox jumps over the lazy dog");
    }

    #[test]
    fn wrap_collapses_whitespace_between_words() {
        let m = TextMetrics::new(FALLBACK_TTF, 12.0).unwrap();
        let lines = wrap_lines(&m, "a   b\n\nc", 200.0);
        assert_eq!(lines, vec!["a b c".to_string()]);
    }

    #[test]
    fn wrap_empty_yields_no_lines() {
        let m = TextMetrics::new(FALLBACK_TTF, 12.0).unwrap();
        assert!(wrap_lines(&m, "", 200.0).is_empty());
        assert!(wrap_lines(&m, "   \n  ", 200.0).is_empty());
    }
}
