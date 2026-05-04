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

pub use cascade::{Color, TextAlign};

/// Per-element inline `style="..."` declarations, keyed by the element's
/// `NodeId`. Built once per `Document` (via `parse::Document::inline_styles`)
/// and threaded through `resolve` for every paragraph render. An element
/// missing from the map has no inline style.
pub type InlineStyles<'a> =
    std::collections::HashMap<ego_tree::NodeId, &'a [sheet::Declaration]>;

/// Resolve the final `BlockStyle` for a paragraph element by combining
/// UA defaults, the author cascade (with full specificity + `!important`),
/// inline `style="..."` declarations, and inheritance from the element's
/// ancestor chain.
///
/// Cascade order, ascending (so the last-applied wins inside
/// `cascade::apply_declarations`):
///
/// 1. `important` flag — `false` before `true`. `!important` declarations
///    always win over non-important regardless of specificity.
/// 2. Specificity — lexicographic `(inline, ids, classes, tags)`. Inline
///    declarations carry [`matcher::Specificity::INLINE`], so they beat any
///    selector-derived rule of equal `important` rank.
/// 3. Source order — earlier rules lose ties to later rules.
///
/// Inheritance walks root-to-element, folding `cascade::inherit` so the
/// inherited properties (`font-size`, `text-align`, `color`) propagate down.
pub fn resolve(
    element: scraper::ElementRef<'_>,
    rules: &[sheet::Rule],
    inline: &InlineStyles<'_>,
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
        let local = resolve_local(e, rules, inline);
        inherited = Some(match inherited.as_ref() {
            Some(parent) => cascade::inherit(parent, local),
            None => local,
        });
    }
    inherited.unwrap_or(BlockStyle::DEFAULT)
}

/// Resolve an element's *local* `BlockStyle` — UA defaults plus the
/// author cascade for rules that match this element, plus any inline
/// `style="..."` declarations registered for this element's `NodeId`.
/// Does NOT walk ancestors; that's `resolve`'s job.
fn resolve_local(
    element: scraper::ElementRef<'_>,
    rules: &[sheet::Rule],
    inline: &InlineStyles<'_>,
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

    // Inline `style="..."` declarations for this element. They carry the
    // sentinel INLINE specificity (which beats any selector-derived
    // specificity) and a sentinel source-order of `usize::MAX` (harmless,
    // since INLINE never ties any selector spec).
    if let Some(inline_decls) = inline.get(&element.id()) {
        for d in *inline_decls {
            decls.push((
                d.important,
                matcher::Specificity::INLINE,
                usize::MAX,
                d.clone(),
            ));
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
/// (Phase 1: 12pt; `:root { font-size: ... }` lands later).
#[derive(Debug, Clone)]
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
    /// Foreground (text) colour. Inherited per CSS spec — see
    /// `cascade::inherit` and `BlockStyleBuilder`.
    pub color: Color,
    /// Background colour. `None` = transparent (no fill emitted).
    /// Non-inherited per CSS spec.
    pub background_color: Option<Color>,
    /// Padding on each side, in em (resolved against the block's own
    /// font size, matching the CSS convention for em-based padding).
    /// Non-inherited.
    pub padding_top_em: f32,
    pub padding_right_em: f32,
    pub padding_bottom_em: f32,
    pub padding_left_em: f32,
    /// Border width in em (uniform on all sides for Phase 1.7b — per-side
    /// borders land later). Zero means "no border, don't paint a stroke".
    pub border_width_em: f32,
    /// Border colour. Used only when `border_width_em > 0`.
    pub border_color: Color,
    /// Author-set width in em (relative to the block's resolved font
    /// size). `None` means "no explicit width" — layout falls back to
    /// HTML attrs or intrinsic dimensions. Phase 2a only sets this for
    /// `<img>`; in future phases other block types may consume it.
    pub width_em: Option<f32>,
    /// Author-set height in em. Same semantics as `width_em`.
    pub height_em: Option<f32>,
    /// Resolved `font-family` fallback list. `None` means "no
    /// author-set value"; the planner uses bundled Inter at registry
    /// index 0. Items are lowercased, quote-stripped, with generic
    /// keywords (sans-serif/serif/monospace/...) dropped at parse
    /// time. Inherited per CSS spec.
    pub font_family: Option<Vec<String>>,
}

impl BlockStyle {
    pub const DEFAULT: BlockStyle = BlockStyle {
        font_size_em: 1.0,
        bold: false,
        margin_top_em: 0.0,
        margin_bottom_em: 0.0,
        indent_em: 0.0,
        text_align: TextAlign::Left,
        color: Color::BLACK,
        background_color: None,
        padding_top_em: 0.0,
        padding_right_em: 0.0,
        padding_bottom_em: 0.0,
        padding_left_em: 0.0,
        border_width_em: 0.0,
        border_color: Color::BLACK,
        width_em: None,
        height_em: None,
        font_family: None,
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

    // -------------------------------------------------------------------
    // Phase 1.7c — inline `style="..."` cascade integration tests.
    // -------------------------------------------------------------------

    use scraper::{Html, Selector as ScraperSelector};
    use sheet::{Declaration, Rule};

    /// Build an empty `InlineStyles` map. Used by tests that exercise the
    /// pre-1.7c cascade path (no inline overrides) but still need to satisfy
    /// the new `resolve` signature.
    fn empty_inline<'a>() -> InlineStyles<'a> {
        std::collections::HashMap::new()
    }

    /// Helper: pick the first element matching `css` in a freshly-parsed
    /// fragment.
    fn first<'a>(html: &'a Html, css: &str) -> scraper::ElementRef<'a> {
        let s = ScraperSelector::parse(css).expect("test css selector");
        html.select(&s).next().expect("no element matched")
    }

    /// Helper: build a single declaration with `important = false`.
    fn decl(name: &str, value: &str) -> Declaration {
        Declaration {
            name: name.to_string(),
            value: value.to_string(),
            important: false,
        }
    }

    /// Helper: build a single declaration with `important = true`.
    fn decl_important(name: &str, value: &str) -> Declaration {
        Declaration {
            name: name.to_string(),
            value: value.to_string(),
            important: true,
        }
    }

    /// Helper: build a `Rule` with one selector text and a list of decls.
    fn rule(selector: &str, decls: Vec<Declaration>, source_order: usize) -> Rule {
        Rule {
            selector_text: selector.to_string(),
            declarations: decls,
            source_order,
        }
    }

    #[test]
    fn inline_style_beats_id_selector() {
        // Author rule sets font-size 12px on `#x`; inline style sets it
        // to 24px. Inline must win because INLINE > any selector specificity
        // at the same `important` rank.
        let html = Html::parse_fragment(r#"<p id="x">hello</p>"#);
        let p = first(&html, "p");

        let rules = vec![rule("#x", vec![decl("font-size", "12px")], 0)];
        let inline_decls = vec![decl("font-size", "24px")];
        let mut inline: InlineStyles<'_> = std::collections::HashMap::new();
        inline.insert(p.id(), inline_decls.as_slice());

        let style = resolve(p, &rules, &inline);
        assert!(
            (style.font_size_em - 24.0 / 12.0).abs() < 1e-4,
            "expected font-size 24px (=2.0em), got {}",
            style.font_size_em
        );
    }

    #[test]
    fn inline_style_does_not_beat_important_id() {
        // Selector rule with `!important` at 48px should beat a non-important
        // inline `font-size: 24px`, because the !important rank trumps spec.
        let html = Html::parse_fragment(r#"<p id="x">hello</p>"#);
        let p = first(&html, "p");

        let rules = vec![rule(
            "#x",
            vec![decl_important("font-size", "48px")],
            0,
        )];
        let inline_decls = vec![decl("font-size", "24px")];
        let mut inline: InlineStyles<'_> = std::collections::HashMap::new();
        inline.insert(p.id(), inline_decls.as_slice());

        let style = resolve(p, &rules, &inline);
        assert!(
            (style.font_size_em - 48.0 / 12.0).abs() < 1e-4,
            "expected !important id-rule to win at 48px, got font_size_em {}",
            style.font_size_em
        );
    }

    #[test]
    fn inline_important_beats_important_id() {
        // Both sides are `!important`; inline still wins on specificity.
        let html = Html::parse_fragment(r#"<p id="x">hello</p>"#);
        let p = first(&html, "p");

        let rules = vec![rule(
            "#x",
            vec![decl_important("color", "blue")],
            0,
        )];
        let inline_decls = vec![decl_important("color", "red")];
        let mut inline: InlineStyles<'_> = std::collections::HashMap::new();
        inline.insert(p.id(), inline_decls.as_slice());

        let style = resolve(p, &rules, &inline);
        assert_eq!(style.color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn inline_style_inherits_to_descendants() {
        // Inline `color: red` on the <section> should inherit to the inner
        // <p> via the standard parent-chain inheritance fold.
        let html = Html::parse_fragment(r#"<section><p>hi</p></section>"#);
        let section = first(&html, "section");
        let p = first(&html, "p");

        let inline_decls = vec![decl("color", "red")];
        let mut inline: InlineStyles<'_> = std::collections::HashMap::new();
        inline.insert(section.id(), inline_decls.as_slice());

        let style = resolve(p, &[], &inline);
        assert_eq!(style.color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn no_inline_style_falls_back_to_rules() {
        // Empty inline map: behaviour identical to the pre-1.7c cascade.
        let html = Html::parse_fragment(r#"<p class="big">hi</p>"#);
        let p = first(&html, "p");

        let rules = vec![rule(".big", vec![decl("color", "red")], 0)];
        let inline = empty_inline();

        let style = resolve(p, &rules, &inline);
        assert_eq!(style.color, Color::rgb(255, 0, 0));
    }

    // ---- Phase 2b Slice C: font-family cascade. ----

    #[test]
    fn default_block_style_has_no_font_family() {
        assert!(BlockStyle::DEFAULT.font_family.is_none());
    }

    #[test]
    fn inline_style_on_anonymous_block_parent_resolves() {
        // Per parse.rs, an anonymous paragraph reuses its parent element's
        // NodeId. Threading inline styles by NodeId means the parent's inline
        // style applies to its anonymous-block paragraph too — verify by
        // resolving from the parent element directly with that NodeId in
        // the inline map.
        let html = Html::parse_fragment(r#"<div>hello <p>world</p></div>"#);
        let div = first(&html, "div");

        let inline_decls = vec![decl("color", "red")];
        let mut inline: InlineStyles<'_> = std::collections::HashMap::new();
        inline.insert(div.id(), inline_decls.as_slice());

        // The anonymous paragraph's `element_id` points at <div>'s NodeId,
        // so resolving the <div> element with this inline map sees the
        // declarations and applies them. (`Document::element_for` returns
        // the parent element exactly, hence resolving `div` is equivalent.)
        let style = resolve(div, &[], &inline);
        assert_eq!(style.color, Color::rgb(255, 0, 0));
    }
}
