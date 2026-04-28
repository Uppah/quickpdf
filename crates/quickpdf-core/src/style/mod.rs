//! User-agent default stylesheet + Phase 1.6c cascade infrastructure.
//!
//! - 1.6a: hard-coded per-tag defaults (this file).
//! - 1.6b: inline `<style>` parsing (sheet), simple-selector matching
//!   (matcher), and last-declaration-wins cascade (cascade) — submodules.
//! - 1.6c: full specificity ordering, `!important`, and inheritance via
//!   parent-chain walk + `cascade::inherit`.

pub mod cascade;
pub mod matcher;
pub mod sheet;

pub use cascade::TextAlign;

/// Resolve the final `BlockStyle` for a paragraph element by combining
/// UA defaults, the author cascade (with full specificity + `!important`),
/// and inheritance from the element's ancestor chain.
///
/// Cascade order, ascending (so the last-applied wins inside
/// `cascade::apply_declarations`):
///
/// 1. `important` flag — `false` before `true`. `!important` declarations
///    always win over non-important regardless of specificity.
/// 2. Specificity — lexicographic `(ids, classes, tags)`.
/// 3. Source order — earlier rules lose ties to later rules.
///
/// Inheritance walks root-to-element, folding `cascade::inherit` so the
/// inherited properties (`font-size`, `text-align`) propagate down.
pub fn resolve(
    element: scraper::ElementRef<'_>,
    rules: &[sheet::Rule],
) -> BlockStyle {
    // Build the ancestor chain root-first so we can fold inheritance forward.
    let mut chain: Vec<scraper::ElementRef<'_>> = Vec::new();
    let mut cur = Some(element);
    while let Some(e) = cur {
        chain.push(e);
        cur = e.parent().and_then(scraper::ElementRef::wrap);
    }
    chain.reverse();

    let mut inherited: Option<BlockStyle> = None;
    for e in chain {
        let local = resolve_local(e, rules);
        inherited = Some(match inherited.as_ref() {
            Some(parent) => cascade::inherit(parent, local),
            None => local,
        });
    }
    inherited.unwrap_or(BlockStyle::DEFAULT)
}

/// Resolve an element's *local* `BlockStyle` — UA defaults plus the
/// author cascade for rules that match this element. Does NOT walk
/// ancestors; that's `resolve`'s job.
fn resolve_local(
    element: scraper::ElementRef<'_>,
    rules: &[sheet::Rule],
) -> BlockStyle {
    let tag = element.value().name();
    let ua = ua_style(tag);

    // For each rule, find the highest-specificity selector in its list that
    // matches this element. If any selector matches, record every declaration
    // tagged with (important, specificity, source_order) so the sort below
    // can produce the correct cascade order.
    let mut decls: Vec<(bool, matcher::Specificity, usize, sheet::Declaration)> =
        Vec::new();
    for rule in rules {
        let selectors = matcher::parse_selector_list(&rule.selector_text);
        let winning_spec = selectors
            .iter()
            .filter(|s| matcher::matches(s, element))
            .map(matcher::specificity)
            .max();
        if let Some(spec) = winning_spec {
            for d in &rule.declarations {
                decls.push((d.important, spec, rule.source_order, d.clone()));
            }
        }
    }
    // Ascending: (false, low-spec, early) ... (true, high-spec, late).
    // `apply_declarations` walks the slice in order with last-wins semantics,
    // so the highest-priority declaration ends up applied last.
    decls.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));

    let flat: Vec<sheet::Declaration> = decls.into_iter().map(|(_, _, _, d)| d).collect();
    cascade::apply_declarations(ua, &flat)
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
