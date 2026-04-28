//! Property cascade — Slice C of the Phase 1.6b sprint.
//!
//! Applies a list of CSS `Declaration`s (already parsed by Slice A's
//! `sheet.rs`) to a UA-default `BlockStyle`, in source order. The last
//! declaration for any given property wins; full specificity, inheritance,
//! and `!important` are deferred to Phase 1.6c.
//!
//! # Unit conversion (lengths)
//!
//! Values are normalised to em against a 12 pt = 12 px base:
//!
//! - `Npx`  → `LengthEm(N / 12.0)`  (CSS 12 px ≈ 1 em at our base)
//! - `Npt`  → `LengthEm(N / 12.0)`  (12 pt = 1 em)
//! - `Nem`  → `LengthEm(N)`
//! - `N%`   → `LengthEm(N / 100.0)`
//! - everything else (`rem`, `ex`, `vh`, …) → `None`
//!
//! # `font-weight`
//!
//! - `"normal"` → `Weight(400)`
//! - `"bold"`   → `Weight(700)`
//! - bare integer in `100..=900` stepping by 100 → `Weight(n)`
//! - everything else → `None`
//!
//! # `text-align`
//!
//! - `"left"`   → `TextAlign(Left)`
//! - `"center"` → `TextAlign(Center)`
//! - `"right"`  → `TextAlign(Right)`
//! - everything else → `None`

use crate::style::sheet::Declaration;
use crate::style::BlockStyle;

/// A parsed CSS property value, normalised so the cascade can apply it
/// directly to a `BlockStyle` without any further unit math.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedValue {
    /// Resolved to em (px → /12, pt → /12, em as-is, % → /100).
    LengthEm(f32),
    /// 100..900 normalized; "normal" → 400, "bold" → 700.
    Weight(u16),
    TextAlign(TextAlign),
}

/// CSS `text-align` keyword. Phase 1.6 supports `left`, `center`, `right`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// Apply CSS declarations to a UA-default `BlockStyle`. Declarations are
/// applied in source order; the last value for any property wins (full
/// specificity is Phase 1.6c). Unknown properties and unparseable values
/// are silently ignored.
pub fn apply_declarations(base: BlockStyle, decls: &[Declaration]) -> BlockStyle {
    let mut out = base;
    for decl in decls {
        let Some(parsed) = parse_value(&decl.name, &decl.value) else {
            continue;
        };
        match (decl.name.as_str(), parsed) {
            ("font-size", ParsedValue::LengthEm(x)) => out.font_size_em = x,
            ("font-weight", ParsedValue::Weight(w)) => out.bold = w >= 600,
            ("margin-top", ParsedValue::LengthEm(x)) => out.margin_top_em = x,
            ("margin-bottom", ParsedValue::LengthEm(x)) => out.margin_bottom_em = x,
            ("text-align", ParsedValue::TextAlign(a)) => out.text_align = a,
            // Property/value-shape mismatch (e.g. `font-size: bold`) — ignore.
            _ => {}
        }
    }
    out
}

/// Parse a single property value. Public for unit tests and for future
/// reuse. Returns `None` if the value can't be parsed for the given property.
pub fn parse_value(prop: &str, value: &str) -> Option<ParsedValue> {
    let value = value.trim();
    match prop {
        "font-size" | "margin-top" | "margin-bottom" => {
            parse_length_em(value).map(ParsedValue::LengthEm)
        }
        "font-weight" => parse_weight(value).map(ParsedValue::Weight),
        "text-align" => parse_text_align(value).map(ParsedValue::TextAlign),
        _ => None,
    }
}

/// Parse a CSS length and return its em-equivalent, per the table in this
/// file's module docs. Strict: any unit not listed yields `None`.
fn parse_length_em(value: &str) -> Option<f32> {
    // Identify the longest known unit suffix. Keep this list in sync with the
    // module docs so accepted vs. rejected units stay obvious to a reader.
    let (num_str, divisor) = if let Some(n) = value.strip_suffix("px") {
        (n, 12.0_f32)
    } else if let Some(n) = value.strip_suffix("pt") {
        (n, 12.0_f32)
    } else if let Some(n) = value.strip_suffix("em") {
        (n, 1.0_f32)
    } else if let Some(n) = value.strip_suffix('%') {
        (n, 100.0_f32)
    } else {
        return None;
    };
    let n: f32 = num_str.trim().parse().ok()?;
    if !n.is_finite() {
        return None;
    }
    Some(n / divisor)
}

fn parse_weight(value: &str) -> Option<u16> {
    match value {
        "normal" => Some(400),
        "bold" => Some(700),
        n => {
            let parsed: u16 = n.parse().ok()?;
            if (100..=900).contains(&parsed) && parsed % 100 == 0 {
                Some(parsed)
            } else {
                None
            }
        }
    }
}

fn parse_text_align(value: &str) -> Option<TextAlign> {
    match value {
        "left" => Some(TextAlign::Left),
        "center" => Some(TextAlign::Center),
        "right" => Some(TextAlign::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(name: &str, value: &str) -> Declaration {
        Declaration {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn parses_lengths_into_em() {
        assert_eq!(
            parse_value("font-size", "24px"),
            Some(ParsedValue::LengthEm(2.0))
        );
        match parse_value("font-size", "14pt") {
            Some(ParsedValue::LengthEm(v)) => {
                assert!((v - 14.0 / 12.0).abs() < 1e-6, "got {v}");
            }
            other => panic!("expected LengthEm for 14pt, got {other:?}"),
        }
        assert_eq!(
            parse_value("font-size", "1.5em"),
            Some(ParsedValue::LengthEm(1.5))
        );
        assert_eq!(
            parse_value("font-size", "150%"),
            Some(ParsedValue::LengthEm(1.5))
        );
        assert_eq!(parse_value("font-size", "5rem"), None);
    }

    #[test]
    fn parses_weight_keywords_and_numbers() {
        assert_eq!(
            parse_value("font-weight", "bold"),
            Some(ParsedValue::Weight(700))
        );
        assert_eq!(
            parse_value("font-weight", "normal"),
            Some(ParsedValue::Weight(400))
        );
        assert_eq!(
            parse_value("font-weight", "600"),
            Some(ParsedValue::Weight(600))
        );
        assert_eq!(parse_value("font-weight", "foo"), None);
        assert_eq!(parse_value("font-weight", "650"), None);
    }

    #[test]
    fn apply_overrides_font_size_and_weight() {
        let base = crate::style::ua_style("p");
        let out = apply_declarations(
            base,
            &[d("font-size", "24px"), d("font-weight", "bold")],
        );
        assert_eq!(out.font_size_em, 2.0);
        assert!(out.bold);
    }

    #[test]
    fn last_declaration_wins() {
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[d("font-size", "12px"), d("font-size", "24px")],
        );
        assert_eq!(out.font_size_em, 2.0);
    }

    #[test]
    fn unknown_props_and_bad_values_ignored() {
        let base = crate::style::ua_style("p");
        let original_font = base.font_size_em;
        let original_bold = base.bold;
        let out = apply_declarations(
            base,
            &[
                d("color", "red"),
                d("font-size", "banana"),
                d("margin-top", "10px"),
            ],
        );
        assert_eq!(out.font_size_em, original_font);
        assert_eq!(out.bold, original_bold);
        assert!(
            (out.margin_top_em - 10.0 / 12.0).abs() < 1e-6,
            "got {}",
            out.margin_top_em
        );
    }

    #[test]
    fn text_align_round_trip() {
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[d("text-align", "center")],
        );
        assert_eq!(out.text_align, TextAlign::Center);
        let out2 = apply_declarations(
            BlockStyle::DEFAULT,
            &[d("text-align", "banana")],
        );
        assert_eq!(out2.text_align, TextAlign::Left);
    }
}
