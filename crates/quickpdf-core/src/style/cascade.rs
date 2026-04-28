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

/// Builder that tracks **which** `BlockStyle` fields were explicitly set
/// by author rules, so inheritance can fill in the un-set ones from a
/// parent's resolved `BlockStyle`.
///
/// Inherited properties (CSS) — fall back to parent if `None`:
/// - `font_size_em`
/// - `text_align`
///
/// Non-inherited properties — fall back to `BlockStyle::DEFAULT` regardless
/// of parent:
/// - `bold` (CSS spec inherits `font-weight`, but we have no bold font yet;
///   revisit Phase 4)
/// - `margin_top_em`, `margin_bottom_em`, `indent_em`
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockStyleBuilder {
    pub font_size_em: Option<f32>,
    pub bold: Option<bool>,
    pub margin_top_em: Option<f32>,
    pub margin_bottom_em: Option<f32>,
    pub indent_em: Option<f32>,
    pub text_align: Option<TextAlign>,
}

impl BlockStyleBuilder {
    pub fn new() -> Self { Self::default() }

    /// Wrap an already-resolved `BlockStyle` as a builder where every
    /// field is treated as explicitly set.
    pub fn from_block(style: BlockStyle) -> Self {
        Self {
            font_size_em: Some(style.font_size_em),
            bold: Some(style.bold),
            margin_top_em: Some(style.margin_top_em),
            margin_bottom_em: Some(style.margin_bottom_em),
            indent_em: Some(style.indent_em),
            text_align: Some(style.text_align),
        }
    }

    /// Finalise into a `BlockStyle`. Inherited fields (`font_size_em`,
    /// `text_align`) fall back to `parent` if `Some`, else `BlockStyle::DEFAULT`.
    /// Non-inherited fields always fall back to `BlockStyle::DEFAULT`.
    pub fn build(self, parent: Option<&BlockStyle>) -> BlockStyle {
        let def = BlockStyle::DEFAULT;
        BlockStyle {
            font_size_em: self.font_size_em.unwrap_or_else(||
                parent.map(|p| p.font_size_em).unwrap_or(def.font_size_em)),
            bold: self.bold.unwrap_or(def.bold),
            margin_top_em: self.margin_top_em.unwrap_or(def.margin_top_em),
            margin_bottom_em: self.margin_bottom_em.unwrap_or(def.margin_bottom_em),
            indent_em: self.indent_em.unwrap_or(def.indent_em),
            text_align: self.text_align.unwrap_or_else(||
                parent.map(|p| p.text_align).unwrap_or(def.text_align)),
        }
    }
}

/// Transitional inheritance helper. Per-field, if `child` equals
/// `BlockStyle::DEFAULT` for that field, take `parent`'s value;
/// otherwise keep child's. Only `font_size_em` and `text_align` participate.
pub fn inherit(parent: &BlockStyle, child: BlockStyle) -> BlockStyle {
    let def = BlockStyle::DEFAULT;
    BlockStyle {
        font_size_em: if (child.font_size_em - def.font_size_em).abs() < f32::EPSILON {
            parent.font_size_em
        } else {
            child.font_size_em
        },
        bold: child.bold,
        margin_top_em: child.margin_top_em,
        margin_bottom_em: child.margin_bottom_em,
        indent_em: child.indent_em,
        text_align: if child.text_align == def.text_align {
            parent.text_align
        } else {
            child.text_align
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(name: &str, value: &str) -> Declaration {
        Declaration {
            name: name.to_string(),
            value: value.to_string(),
            important: false,
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

    #[test]
    fn builder_default_all_none_then_default_block_no_parent() {
        let b = BlockStyleBuilder::default();
        assert!(b.font_size_em.is_none());
        assert!(b.bold.is_none());
        assert!(b.margin_top_em.is_none());
        assert!(b.margin_bottom_em.is_none());
        assert!(b.indent_em.is_none());
        assert!(b.text_align.is_none());
        let s = b.build(None);
        let def = BlockStyle::DEFAULT;
        assert_eq!(s.font_size_em, def.font_size_em);
        assert_eq!(s.bold, def.bold);
        assert_eq!(s.margin_top_em, def.margin_top_em);
        assert_eq!(s.margin_bottom_em, def.margin_bottom_em);
        assert_eq!(s.indent_em, def.indent_em);
        assert_eq!(s.text_align, def.text_align);
    }

    #[test]
    fn builder_inherits_font_size_from_parent_when_unset() {
        let parent = BlockStyle {
            font_size_em: 2.0,
            ..BlockStyle::DEFAULT
        };
        let s = BlockStyleBuilder::new().build(Some(&parent));
        assert_eq!(s.font_size_em, 2.0);
    }

    #[test]
    fn builder_does_not_inherit_margin() {
        let parent = BlockStyle {
            margin_top_em: 5.0,
            margin_bottom_em: 5.0,
            indent_em: 5.0,
            ..BlockStyle::DEFAULT
        };
        let s = BlockStyleBuilder::new().build(Some(&parent));
        assert_eq!(s.margin_top_em, BlockStyle::DEFAULT.margin_top_em);
        assert_eq!(s.margin_bottom_em, BlockStyle::DEFAULT.margin_bottom_em);
        assert_eq!(s.indent_em, BlockStyle::DEFAULT.indent_em);
    }

    #[test]
    fn builder_explicit_value_wins_over_parent() {
        let parent = BlockStyle {
            font_size_em: 2.0,
            ..BlockStyle::DEFAULT
        };
        let mut b = BlockStyleBuilder::new();
        b.font_size_em = Some(0.5);
        let s = b.build(Some(&parent));
        assert_eq!(s.font_size_em, 0.5);
    }

    #[test]
    fn builder_inherits_text_align() {
        let parent = BlockStyle {
            text_align: TextAlign::Center,
            ..BlockStyle::DEFAULT
        };
        let s = BlockStyleBuilder::new().build(Some(&parent));
        assert_eq!(s.text_align, TextAlign::Center);
    }

    #[test]
    fn builder_from_block_round_trips() {
        let original = BlockStyle {
            font_size_em: 1.25,
            bold: true,
            margin_top_em: 0.5,
            margin_bottom_em: 0.75,
            indent_em: 1.5,
            text_align: TextAlign::Right,
        };
        let b = BlockStyleBuilder::from_block(original);
        // No parent — every field should round-trip from the builder.
        let s = b.build(None);
        assert_eq!(s.font_size_em, original.font_size_em);
        assert_eq!(s.bold, original.bold);
        assert_eq!(s.margin_top_em, original.margin_top_em);
        assert_eq!(s.margin_bottom_em, original.margin_bottom_em);
        assert_eq!(s.indent_em, original.indent_em);
        assert_eq!(s.text_align, original.text_align);
    }

    #[test]
    fn inherit_helper_takes_parent_font_size_when_child_is_default() {
        let parent = BlockStyle {
            font_size_em: 2.0,
            ..BlockStyle::DEFAULT
        };
        let child = BlockStyle::DEFAULT;
        let s = inherit(&parent, child);
        assert_eq!(s.font_size_em, 2.0);
    }

    #[test]
    fn inherit_helper_keeps_child_font_size_when_set() {
        let parent = BlockStyle {
            font_size_em: 2.0,
            ..BlockStyle::DEFAULT
        };
        let child = BlockStyle {
            font_size_em: 0.5,
            ..BlockStyle::DEFAULT
        };
        let s = inherit(&parent, child);
        assert_eq!(s.font_size_em, 0.5);
    }

    #[test]
    fn inherit_helper_does_not_change_margins() {
        let parent = BlockStyle {
            margin_top_em: 5.0,
            margin_bottom_em: 5.0,
            indent_em: 5.0,
            ..BlockStyle::DEFAULT
        };
        let child = BlockStyle {
            margin_top_em: 0.25,
            margin_bottom_em: 0.5,
            indent_em: 0.75,
            ..BlockStyle::DEFAULT
        };
        let s = inherit(&parent, child);
        assert_eq!(s.margin_top_em, 0.25);
        assert_eq!(s.margin_bottom_em, 0.5);
        assert_eq!(s.indent_em, 0.75);
    }
}
