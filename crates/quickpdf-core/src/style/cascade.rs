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
//! - `Nrem` → `LengthEm(N)`        (root font-size = 1em until `:root` cascade lands)
//! - `N%`   → `LengthEm(N / 100.0)`
//! - everything else (`ex`, `vh`, …) → `None`
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
    /// Opaque sRGB colour.
    Color(Color),
    /// Optional sRGB colour — `Some(c)` for a real colour, `None` for the
    /// CSS `transparent` keyword (Phase 1.7b uses this for `background-color`
    /// where transparency is the default and meaningful).
    OptionalColor(Option<Color>),
    /// CSS `border-style` keyword — `solid` is the only style we honour;
    /// `none`/`hidden` produce `BorderStyle::None`. Anything else is ignored.
    BorderStyle(BorderStyle),
}

/// CSS `border-style` keyword. Phase 1.7b only models presence/absence;
/// `dashed`/`dotted`/`double` etc. land later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    None,
    Solid,
}

/// CSS `text-align` keyword. Phase 1.6 supports `left`, `center`, `right`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// 8-bit-per-channel sRGB colour. Phase 1.7a only models opaque colours;
/// `rgba(...)`'s alpha is parsed and discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b }
    }
}

/// Apply CSS declarations to a UA-default `BlockStyle`. Declarations are
/// applied in source order; the last value for any property wins. Unknown
/// properties and unparseable values are silently ignored.
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
            ("color", ParsedValue::Color(c)) => out.color = c,
            ("background-color", ParsedValue::OptionalColor(c)) => {
                out.background_color = c;
            }
            ("padding-top", ParsedValue::LengthEm(x)) => out.padding_top_em = x,
            ("padding-right", ParsedValue::LengthEm(x)) => out.padding_right_em = x,
            ("padding-bottom", ParsedValue::LengthEm(x)) => out.padding_bottom_em = x,
            ("padding-left", ParsedValue::LengthEm(x)) => out.padding_left_em = x,
            ("border-width", ParsedValue::LengthEm(x)) => out.border_width_em = x,
            ("border-color", ParsedValue::Color(c)) => out.border_color = c,
            ("width", ParsedValue::LengthEm(x)) => out.width_em = Some(x),
            ("height", ParsedValue::LengthEm(x)) => out.height_em = Some(x),
            ("border-style", ParsedValue::BorderStyle(s)) => {
                if s == BorderStyle::None {
                    out.border_width_em = 0.0;
                }
            }
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
        "font-size"
        | "margin-top"
        | "margin-bottom"
        | "padding-top"
        | "padding-right"
        | "padding-bottom"
        | "padding-left"
        | "border-width" => parse_length_em(value).map(ParsedValue::LengthEm),
        "width" | "height" => {
            // Phase 2a: reject `%` because our cascade can't preserve the
            // CSS percentage-of-containing-block semantic. See spec §3.
            if value.trim_end().ends_with('%') {
                return None;
            }
            parse_length_em(value).map(ParsedValue::LengthEm)
        }
        "font-weight" => parse_weight(value).map(ParsedValue::Weight),
        "text-align" => parse_text_align(value).map(ParsedValue::TextAlign),
        "color" | "border-color" => parse_color(value).map(ParsedValue::Color),
        "background-color" => Some(ParsedValue::OptionalColor(
            parse_background_color(value)?,
        )),
        "border-style" => parse_border_style(value).map(ParsedValue::BorderStyle),
        _ => None,
    }
}

/// Parse a `background-color` value. Wraps `parse_color` but treats the
/// CSS `transparent` keyword as `Some(None)` (parsed-but-no-fill) rather
/// than `parse_color`'s `Some(BLACK)` fallback. Returns `None` for
/// unparseable input so the cascade leaves the existing value in place.
fn parse_background_color(value: &str) -> Option<Option<Color>> {
    let v = value.trim();
    if v.eq_ignore_ascii_case("transparent") {
        return Some(None);
    }
    parse_color(v).map(Some)
}

fn parse_border_style(value: &str) -> Option<BorderStyle> {
    match value {
        "solid" => Some(BorderStyle::Solid),
        "none" | "hidden" => Some(BorderStyle::None),
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
    } else if let Some(n) = value.strip_suffix("rem") {
        // Root font-size = 1em until a real `:root` cascade lands, so `rem`
        // is currently identical to `em`. See CLAUDE.md Phase 1.7c.
        (n, 1.0_f32)
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

/// Parse a CSS colour value into an opaque `Color`. Supported forms:
///
/// - 17 named colours (the 16 HTML 4 basics + `transparent`, which becomes
///   `Color::BLACK` since Phase 1.7a is opaque-only).
/// - `#rgb` and `#rrggbb` hex.
/// - `rgb(r, g, b)` and `rgba(r, g, b, a)` — alpha is parsed and discarded.
///   Components may be integers (0–255) or percentages (0%–100%).
///
/// Anything else (`hsl()`, `currentColor`, `color-mix()`, named extras
/// like `rebeccapurple`) returns `None` and the cascade falls through.
pub fn parse_color(value: &str) -> Option<Color> {
    let v = value.trim();
    if let Some(c) = parse_named_color(v) {
        return Some(c);
    }
    if let Some(rest) = v.strip_prefix('#') {
        return parse_hex_color(rest);
    }
    if let Some(args) = v.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        return parse_rgb_args(args, false);
    }
    if let Some(args) = v.strip_prefix("rgba(").and_then(|s| s.strip_suffix(')')) {
        return parse_rgb_args(args, true);
    }
    None
}

fn parse_named_color(name: &str) -> Option<Color> {
    let n = name.to_ascii_lowercase();
    let c = match n.as_str() {
        "black" => Color::rgb(0, 0, 0),
        "silver" => Color::rgb(192, 192, 192),
        "gray" | "grey" => Color::rgb(128, 128, 128),
        "white" => Color::rgb(255, 255, 255),
        "maroon" => Color::rgb(128, 0, 0),
        "red" => Color::rgb(255, 0, 0),
        "purple" => Color::rgb(128, 0, 128),
        "fuchsia" | "magenta" => Color::rgb(255, 0, 255),
        "green" => Color::rgb(0, 128, 0),
        "lime" => Color::rgb(0, 255, 0),
        "olive" => Color::rgb(128, 128, 0),
        "yellow" => Color::rgb(255, 255, 0),
        "navy" => Color::rgb(0, 0, 128),
        "blue" => Color::rgb(0, 0, 255),
        "teal" => Color::rgb(0, 128, 128),
        "aqua" | "cyan" => Color::rgb(0, 255, 255),
        "transparent" => Color::BLACK,
        _ => return None,
    };
    Some(c)
}

fn parse_hex_color(rest: &str) -> Option<Color> {
    let is_hex = |b: u8| b.is_ascii_hexdigit();
    let bytes = rest.as_bytes();
    match bytes.len() {
        3 if bytes.iter().all(|b| is_hex(*b)) => {
            // #rgb → expand each nibble: r → rr, g → gg, b → bb.
            let r = u8::from_str_radix(&rest[0..1], 16).ok()?;
            let g = u8::from_str_radix(&rest[1..2], 16).ok()?;
            let b = u8::from_str_radix(&rest[2..3], 16).ok()?;
            Some(Color::rgb(r * 17, g * 17, b * 17))
        }
        6 if bytes.iter().all(|b| is_hex(*b)) => {
            let r = u8::from_str_radix(&rest[0..2], 16).ok()?;
            let g = u8::from_str_radix(&rest[2..4], 16).ok()?;
            let b = u8::from_str_radix(&rest[4..6], 16).ok()?;
            Some(Color::rgb(r, g, b))
        }
        _ => None,
    }
}

fn parse_rgb_args(args: &str, expect_alpha: bool) -> Option<Color> {
    let parts: Vec<&str> = args.split(',').map(str::trim).collect();
    let needed = if expect_alpha { 4 } else { 3 };
    if parts.len() != needed {
        return None;
    }
    let r = parse_rgb_component(parts[0])?;
    let g = parse_rgb_component(parts[1])?;
    let b = parse_rgb_component(parts[2])?;
    if expect_alpha {
        // Validate alpha but discard it.
        let alpha: f32 = parts[3].parse().ok()?;
        if !(0.0..=1.0).contains(&alpha) {
            return None;
        }
    }
    Some(Color::rgb(r, g, b))
}

fn parse_rgb_component(raw: &str) -> Option<u8> {
    if let Some(pct) = raw.strip_suffix('%') {
        let n: f32 = pct.trim().parse().ok()?;
        if !(0.0..=100.0).contains(&n) {
            return None;
        }
        Some((n * 255.0 / 100.0).round() as u8)
    } else {
        let n: i32 = raw.parse().ok()?;
        if !(0..=255).contains(&n) {
            return None;
        }
        Some(n as u8)
    }
}

/// Builder that tracks **which** `BlockStyle` fields were explicitly set
/// by author rules, so inheritance can fill in the un-set ones from a
/// parent's resolved `BlockStyle`.
///
/// Inherited properties (CSS) — fall back to parent if `None`:
/// - `font_size_em`
/// - `text_align`
/// - `color`
///
/// Non-inherited properties — fall back to `BlockStyle::DEFAULT` regardless
/// of parent:
/// - `bold` (CSS spec inherits `font-weight`, but we have no bold font yet;
///   revisit Phase 4)
/// - `margin_top_em`, `margin_bottom_em`, `indent_em`
#[derive(Debug, Clone, Default)]
pub struct BlockStyleBuilder {
    pub font_size_em: Option<f32>,
    pub bold: Option<bool>,
    pub margin_top_em: Option<f32>,
    pub margin_bottom_em: Option<f32>,
    pub indent_em: Option<f32>,
    pub text_align: Option<TextAlign>,
    pub color: Option<Color>,
    pub background_color: Option<Option<Color>>,
    pub padding_top_em: Option<f32>,
    pub padding_right_em: Option<f32>,
    pub padding_bottom_em: Option<f32>,
    pub padding_left_em: Option<f32>,
    pub border_width_em: Option<f32>,
    pub border_color: Option<Color>,
    pub width_em: Option<Option<f32>>,
    pub height_em: Option<Option<f32>>,
    pub font_family: Option<Option<Vec<String>>>,
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
            color: Some(style.color),
            background_color: Some(style.background_color),
            padding_top_em: Some(style.padding_top_em),
            padding_right_em: Some(style.padding_right_em),
            padding_bottom_em: Some(style.padding_bottom_em),
            padding_left_em: Some(style.padding_left_em),
            border_width_em: Some(style.border_width_em),
            border_color: Some(style.border_color),
            width_em: Some(style.width_em),
            height_em: Some(style.height_em),
            font_family: Some(style.font_family),
        }
    }

    /// Finalise into a `BlockStyle`. Inherited fields (`font_size_em`,
    /// `text_align`, `color`) fall back to `parent` if `Some`, else
    /// `BlockStyle::DEFAULT`. Non-inherited fields always fall back to
    /// `BlockStyle::DEFAULT`.
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
            color: self.color.unwrap_or_else(||
                parent.map(|p| p.color).unwrap_or(def.color)),
            background_color: self.background_color.unwrap_or(def.background_color),
            padding_top_em: self.padding_top_em.unwrap_or(def.padding_top_em),
            padding_right_em: self.padding_right_em.unwrap_or(def.padding_right_em),
            padding_bottom_em: self.padding_bottom_em.unwrap_or(def.padding_bottom_em),
            padding_left_em: self.padding_left_em.unwrap_or(def.padding_left_em),
            border_width_em: self.border_width_em.unwrap_or(def.border_width_em),
            border_color: self.border_color.unwrap_or(def.border_color),
            width_em: self.width_em.unwrap_or(def.width_em),
            height_em: self.height_em.unwrap_or(def.height_em),
            font_family: self.font_family.unwrap_or(def.font_family),
        }
    }
}

/// Transitional inheritance helper. Per-field, if `child` equals
/// `BlockStyle::DEFAULT` for that field, take `parent`'s value;
/// otherwise keep child's. `font_size_em`, `text_align`, and `color`
/// participate.
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
        color: if child.color == def.color {
            parent.color
        } else {
            child.color
        },
        // Box-model properties are NOT inherited — pass child's through.
        background_color: child.background_color,
        padding_top_em: child.padding_top_em,
        padding_right_em: child.padding_right_em,
        padding_bottom_em: child.padding_bottom_em,
        padding_left_em: child.padding_left_em,
        border_width_em: child.border_width_em,
        border_color: child.border_color,
        // Width/height are not inherited per CSS — pass child's through.
        width_em: child.width_em,
        height_em: child.height_em,
        // font-family is inherited per CSS spec. For Option, the rule is
        // "child value wins; fall back to parent if child has none."
        // This differs from f32/Color arms above (which sentinel-compare
        // against DEFAULT) — for an Option, None is the natural sentinel.
        font_family: child
            .font_family
            .clone()
            .or_else(|| parent.font_family.clone()),
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
        assert_eq!(
            parse_value("font-size", "5rem"),
            Some(ParsedValue::LengthEm(5.0))
        );
    }

    #[test]
    fn parses_rem_alongside_em() {
        // 1rem and 1em are currently identical (root font-size = 1em).
        assert_eq!(
            parse_value("font-size", "1rem"),
            Some(ParsedValue::LengthEm(1.0))
        );
        assert_eq!(
            parse_value("font-size", "2.5rem"),
            Some(ParsedValue::LengthEm(2.5))
        );
        assert_eq!(
            parse_value("font-size", "0rem"),
            Some(ParsedValue::LengthEm(0.0))
        );
        // Negative values are syntactically fine; the cascade itself may or
        // may not clamp depending on property semantics.
        assert_eq!(
            parse_value("font-size", "-0.5rem"),
            Some(ParsedValue::LengthEm(-0.5))
        );
        // rem and em produce the same em-value at root scope.
        assert_eq!(
            parse_value("padding-top", "1rem"),
            parse_value("padding-top", "1em"),
        );
        // Garbage rem still rejected.
        assert_eq!(parse_value("font-size", "remmy"), None);
        assert_eq!(parse_value("font-size", "rem"), None);
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
            color: Color::rgb(10, 20, 30),
            ..BlockStyle::DEFAULT
        };
        let b = BlockStyleBuilder::from_block(original.clone());
        // No parent — every field should round-trip from the builder.
        let s = b.build(None);
        assert_eq!(s.font_size_em, original.font_size_em);
        assert_eq!(s.bold, original.bold);
        assert_eq!(s.margin_top_em, original.margin_top_em);
        assert_eq!(s.margin_bottom_em, original.margin_bottom_em);
        assert_eq!(s.indent_em, original.indent_em);
        assert_eq!(s.text_align, original.text_align);
        assert_eq!(s.color, original.color);
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

    // ---- Phase 1.7a: colour parsing + cascade + inheritance.

    #[test]
    fn parse_color_named() {
        assert_eq!(parse_color("red"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(parse_color("BLACK"), Some(Color::BLACK));
        assert_eq!(parse_color("white"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(parse_color("gray"), Some(Color::rgb(128, 128, 128)));
        assert_eq!(parse_color("grey"), Some(Color::rgb(128, 128, 128)));
        assert_eq!(parse_color("cyan"), Some(Color::rgb(0, 255, 255)));
        assert_eq!(parse_color("aqua"), Some(Color::rgb(0, 255, 255)));
        assert_eq!(parse_color("rebeccapurple"), None);
        assert_eq!(parse_color("transparent"), Some(Color::BLACK));
    }

    #[test]
    fn parse_color_hex() {
        assert_eq!(parse_color("#000000"), Some(Color::BLACK));
        assert_eq!(parse_color("#ffffff"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(parse_color("#FFFFFF"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(parse_color("#abc"), Some(Color::rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_color("#1a2b3c"), Some(Color::rgb(0x1a, 0x2b, 0x3c)));
        assert_eq!(parse_color("#xyz"), None);
        assert_eq!(parse_color("#12345"), None); // not 3 or 6
        assert_eq!(parse_color("#1234567"), None);
    }

    #[test]
    fn parse_color_rgb_function() {
        assert_eq!(parse_color("rgb(255, 0, 0)"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(parse_color("rgb(0,128,255)"), Some(Color::rgb(0, 128, 255)));
        assert_eq!(parse_color("rgb(100%, 0%, 50%)"),
            Some(Color::rgb(255, 0, 128)));
        assert_eq!(parse_color("rgb(256, 0, 0)"), None); // out of range
        assert_eq!(parse_color("rgb(0, 0)"), None); // wrong arity
    }

    #[test]
    fn parse_color_rgba_drops_alpha() {
        assert_eq!(
            parse_color("rgba(255, 0, 0, 0.5)"),
            Some(Color::rgb(255, 0, 0))
        );
        assert_eq!(
            parse_color("rgba(10, 20, 30, 0)"),
            Some(Color::rgb(10, 20, 30))
        );
        assert_eq!(parse_color("rgba(0, 0, 0, 1.5)"), None); // alpha > 1
    }

    #[test]
    fn apply_color_property_overrides_default() {
        let out = apply_declarations(BlockStyle::DEFAULT, &[d("color", "#ff0000")]);
        assert_eq!(out.color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn unparseable_color_is_ignored() {
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[d("color", "notacolor")],
        );
        assert_eq!(out.color, Color::BLACK);
    }

    #[test]
    fn builder_inherits_color_from_parent() {
        let parent = BlockStyle {
            color: Color::rgb(50, 100, 200),
            ..BlockStyle::DEFAULT
        };
        let s = BlockStyleBuilder::new().build(Some(&parent));
        assert_eq!(s.color, Color::rgb(50, 100, 200));
    }

    #[test]
    fn inherit_helper_takes_parent_color_when_child_is_default() {
        let parent = BlockStyle {
            color: Color::rgb(255, 0, 0),
            ..BlockStyle::DEFAULT
        };
        let child = BlockStyle::DEFAULT;
        let s = inherit(&parent, child);
        assert_eq!(s.color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn inherit_helper_keeps_child_color_when_set() {
        let parent = BlockStyle {
            color: Color::rgb(255, 0, 0),
            ..BlockStyle::DEFAULT
        };
        let child = BlockStyle {
            color: Color::rgb(0, 255, 0),
            ..BlockStyle::DEFAULT
        };
        let s = inherit(&parent, child);
        assert_eq!(s.color, Color::rgb(0, 255, 0));
    }

    // ---- Phase 1.7b: box-model properties.

    #[test]
    fn background_color_parses_to_optional_color() {
        assert_eq!(
            parse_value("background-color", "red"),
            Some(ParsedValue::OptionalColor(Some(Color::rgb(255, 0, 0))))
        );
        assert_eq!(
            parse_value("background-color", "transparent"),
            Some(ParsedValue::OptionalColor(None))
        );
        assert_eq!(parse_value("background-color", "notacolor"), None);
    }

    #[test]
    fn apply_background_color() {
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[d("background-color", "#00ff00")],
        );
        assert_eq!(out.background_color, Some(Color::rgb(0, 255, 0)));
    }

    #[test]
    fn apply_padding_longhands() {
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[
                d("padding-top", "12px"),
                d("padding-right", "6px"),
                d("padding-bottom", "12px"),
                d("padding-left", "6px"),
            ],
        );
        assert_eq!(out.padding_top_em, 1.0);
        assert_eq!(out.padding_right_em, 0.5);
        assert_eq!(out.padding_bottom_em, 1.0);
        assert_eq!(out.padding_left_em, 0.5);
    }

    #[test]
    fn apply_border_width_and_color() {
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[
                d("border-width", "2px"),
                d("border-color", "blue"),
                d("border-style", "solid"),
            ],
        );
        assert!((out.border_width_em - 2.0 / 12.0).abs() < 1e-6);
        assert_eq!(out.border_color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn border_style_none_zeroes_width() {
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[
                d("border-width", "5px"),
                d("border-style", "none"),
            ],
        );
        assert_eq!(out.border_width_em, 0.0);
    }

    #[test]
    fn box_model_is_not_inherited() {
        // background-color, padding, border are non-inherited per CSS.
        let parent = BlockStyle {
            background_color: Some(Color::rgb(255, 0, 0)),
            padding_top_em: 1.0,
            border_width_em: 0.5,
            ..BlockStyle::DEFAULT
        };
        let child = BlockStyle::DEFAULT;
        let s = inherit(&parent, child);
        assert_eq!(s.background_color, None);
        assert_eq!(s.padding_top_em, 0.0);
        assert_eq!(s.border_width_em, 0.0);
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

    // ---- Phase 2a Slice C: width / height longhands. ----

    #[test]
    fn parse_value_accepts_width_and_height_lengths() {
        // Same px / pt / em / rem path as padding-* and border-width.
        assert_eq!(
            parse_value("width", "120px"),
            Some(ParsedValue::LengthEm(120.0 / 12.0))
        );
        assert_eq!(
            parse_value("height", "5em"),
            Some(ParsedValue::LengthEm(5.0))
        );
        assert_eq!(
            parse_value("width", "24pt"),
            Some(ParsedValue::LengthEm(24.0 / 12.0))
        );
        assert_eq!(
            parse_value("height", "2rem"),
            Some(ParsedValue::LengthEm(2.0))
        );
    }

    #[test]
    fn parse_value_rejects_percent_for_width_and_height() {
        // CSS `width: 50%` resolves against the containing block, not the
        // font-size. Our cascade can't preserve that without a richer Length
        // type, so Phase 2a explicitly drops `%` for these properties.
        assert_eq!(parse_value("width", "50%"), None);
        assert_eq!(parse_value("width", "100%"), None);
        assert_eq!(parse_value("height", "50%"), None);
        // Sanity: `%` still works for font-size where it makes sense.
        assert_eq!(
            parse_value("font-size", "150%"),
            Some(ParsedValue::LengthEm(1.5))
        );
    }

    #[test]
    fn apply_declarations_sets_width_and_height_em() {
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[d("width", "120px"), d("height", "60px")],
        );
        assert_eq!(out.width_em, Some(120.0 / 12.0));
        assert_eq!(out.height_em, Some(60.0 / 12.0));
    }

    #[test]
    fn apply_declarations_ignores_unparseable_width() {
        // `width: auto` is a CSS keyword we don't honor in Phase 2a.
        let out = apply_declarations(
            BlockStyle::DEFAULT,
            &[d("width", "auto"), d("height", "200px")],
        );
        assert!(out.width_em.is_none(), "width:auto must leave width_em as None");
        assert_eq!(out.height_em, Some(200.0 / 12.0));
    }

    #[test]
    fn width_height_not_inherited() {
        let parent = BlockStyle {
            width_em: Some(10.0),
            height_em: Some(5.0),
            ..BlockStyle::DEFAULT
        };
        let child = BlockStyle::DEFAULT;
        let s = inherit(&parent, child);
        assert!(s.width_em.is_none());
        assert!(s.height_em.is_none());
    }
}
