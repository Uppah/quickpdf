//! User-agent default stylesheet + Phase 1.6b cascade infrastructure.
//!
//! - 1.6a: hard-coded per-tag defaults (this file).
//! - 1.6b: inline `<style>` parsing (sheet), simple-selector matching
//!   (matcher), and last-declaration-wins cascade (cascade) — submodules.
//! - 1.6c (later): full cascade with specificity and inheritance.

pub mod cascade;
pub mod matcher;
pub mod sheet;

pub use cascade::TextAlign;

/// Resolve the final `BlockStyle` for a paragraph element by combining the
/// UA default (from `ua_style(tag)`) with any author rules from the document
/// stylesheet whose selectors match the element. Phase 1.6b cascade is
/// "last-declaration-wins" — the full specificity calculation is 1.6c.
pub fn resolve(
    element: scraper::ElementRef<'_>,
    rules: &[sheet::Rule],
) -> BlockStyle {
    let tag = element.value().name();
    let ua = ua_style(tag);

    // Walk rules in source order. For each rule whose selector list contains
    // ANY selector that matches our element, append all its declarations to
    // the working list. Cascade applies them in source order.
    let mut decls: Vec<sheet::Declaration> = Vec::new();
    for rule in rules {
        let selectors = matcher::parse_selector_list(&rule.selector_text);
        let any_matches = selectors.iter().any(|s| matcher::matches(s, element));
        if any_matches {
            decls.extend(rule.declarations.iter().cloned());
        }
    }

    cascade::apply_declarations(ua, &decls)
}

/// Style hints attached to a paragraph by the user-agent stylesheet.
/// Em-relative values are resolved against the document's base font size
/// (Phase 1: 12pt, Phase 1.6c: from `:root { font-size: ... }`).
#[derive(Debug, Clone, Copy)]
pub struct BlockStyle {
    /// Font size as a multiplier of the document's base size. 1.0 = body.
    pub font_size_em: f32,
    /// Bold text. Phase 1.6 has no bold font available, so this is observed
    /// but not yet rendered differently. Phase 4 wires in Inter-Bold.
    pub bold: bool,
    /// Vertical space above the block, in em (relative to the block's own
    /// font size — same convention CSS uses for `margin-top`).
    pub margin_top_em: f32,
    /// Vertical space below the block.
    pub margin_bottom_em: f32,
    /// Left indent, in em. Used for list items so `<li>` appears indented.
    pub indent_em: f32,
    /// Horizontal alignment of inline content within the block.
    pub text_align: TextAlign,
}

impl BlockStyle {
    pub const DEFAULT: BlockStyle = BlockStyle {
        font_size_em: 1.0,
        bold: false,
        margin_top_em: 0.0,
        margin_bottom_em: 0.0,
        indent_em: 0.0,
        text_align: TextAlign::Left,
    };
}

/// User-agent default style for an HTML tag name (lowercased).
///
/// Values are taken from the WHATWG rendering spec / Chromium's UA stylesheet,
/// rounded to two decimals where the spec uses fractions like 0.67em.
pub fn ua_style(tag: &str) -> BlockStyle {
    match tag {
        "h1" => BlockStyle {
            font_size_em: 2.00,
            bold: true,
            margin_top_em: 0.67,
            margin_bottom_em: 0.67,
            ..BlockStyle::DEFAULT
        },
        "h2" => BlockStyle {
            font_size_em: 1.50,
            bold: true,
            margin_top_em: 0.83,
            margin_bottom_em: 0.83,
            ..BlockStyle::DEFAULT
        },
        "h3" => BlockStyle {
            font_size_em: 1.17,
            bold: true,
            margin_top_em: 1.00,
            margin_bottom_em: 1.00,
            ..BlockStyle::DEFAULT
        },
        "h4" => BlockStyle {
            font_size_em: 1.00,
            bold: true,
            margin_top_em: 1.33,
            margin_bottom_em: 1.33,
            ..BlockStyle::DEFAULT
        },
        "h5" => BlockStyle {
            font_size_em: 0.83,
            bold: true,
            margin_top_em: 1.67,
            margin_bottom_em: 1.67,
            ..BlockStyle::DEFAULT
        },
        "h6" => BlockStyle {
            font_size_em: 0.67,
            bold: true,
            margin_top_em: 2.33,
            margin_bottom_em: 2.33,
            ..BlockStyle::DEFAULT
        },
        "p" | "blockquote" | "address" | "pre" => BlockStyle {
            margin_top_em: 1.00,
            margin_bottom_em: 1.00,
            ..BlockStyle::DEFAULT
        },
        "li" => BlockStyle {
            indent_em: 2.50,
            ..BlockStyle::DEFAULT
        },
        "dt" => BlockStyle {
            bold: true,
            ..BlockStyle::DEFAULT
        },
        "dd" => BlockStyle {
            indent_em: 2.50,
            ..BlockStyle::DEFAULT
        },
        _ => BlockStyle::DEFAULT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h1_is_bigger_and_bold() {
        let s = ua_style("h1");
        assert!(s.font_size_em > 1.5);
        assert!(s.bold);
    }

    #[test]
    fn p_is_baseline_size_with_block_margins() {
        let s = ua_style("p");
        assert_eq!(s.font_size_em, 1.0);
        assert!(!s.bold);
        assert!(s.margin_top_em > 0.0);
    }

    #[test]
    fn li_is_indented_no_extra_margin() {
        let s = ua_style("li");
        assert!(s.indent_em > 1.0);
        assert_eq!(s.margin_top_em, 0.0);
    }

    #[test]
    fn unknown_tags_get_default() {
        let s = ua_style("foobar");
        assert_eq!(s.font_size_em, 1.0);
        assert_eq!(s.margin_top_em, 0.0);
    }

    #[test]
    fn heading_sizes_strictly_decrease() {
        let sizes: Vec<f32> = ["h1", "h2", "h3", "h4", "h5", "h6"]
            .iter()
            .map(|t| ua_style(t).font_size_em)
            .collect();
        for w in sizes.windows(2) {
            assert!(w[0] > w[1], "expected {} > {}", w[0], w[1]);
        }
    }
}
