//! Selector parsing + matching — Slice B of the Phase 1.6b sprint.
//!
//! See `~/.claude/plans/cheerful-riding-castle.md` and the coordinator's
//! design notes for the contract this slice must satisfy.
//!
//! Scope: a tiny, hand-rolled selector engine. We support only the subset
//! of CSS selectors needed for Phase 1.6b's user/inline `<style>` blocks:
//! tag, `.class`, `#id`, compound selectors (`p.foo#bar`), and the
//! descendant combinator (whitespace). Anything fancier is silently
//! dropped at parse time so callers can blindly feed us whatever a real
//! stylesheet contains without us blowing up.

use scraper::{CaseSensitivity, ElementRef};

/// One simple selector component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimpleSelector {
    Tag(String),  // lowercased
    Class(String),
    Id(String),
}

/// A compound selector: all simples must match the same element.
/// e.g. `p.foo#bar` → vec![Tag("p"), Class("foo"), Id("bar")].
#[derive(Debug, Clone)]
pub struct Compound {
    pub parts: Vec<SimpleSelector>,
}

/// A full selector: chain of compound selectors joined by descendant
/// combinators (whitespace). For MVP only descendant — no `>`, `+`, `~`.
/// e.g. `div p .x` → three Compound entries; rightmost is the subject.
#[derive(Debug, Clone)]
pub struct Selector {
    pub compounds: Vec<Compound>,
}

/// CSS Selectors Level 3 specificity, extended with an inline-style bucket.
///
/// - `0` (a) = inline-style flag (0 or 1). Set by [`Specificity::INLINE`]
///   for declarations harvested from an element's `style="..."` attribute;
///   selector-derived specificity always leaves this bucket at 0.
/// - `1` (b) = number of ID selectors.
/// - `2` (c) = number of class selectors.
/// - `3` (d) = number of type/tag selectors.
///
/// Universal selector (`*`) and combinators contribute nothing.
///
/// `Ord`/`PartialOrd` compare lexicographically: inline beats id beats class
/// beats tag, so `INLINE > (0,1,0,0) > (0,0,99,0) > (0,0,0,99)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Specificity(pub u32, pub u32, pub u32, pub u32);

impl Specificity {
    pub const ZERO: Specificity = Specificity(0, 0, 0, 0);
    /// Sentinel for an inline `style="..."` declaration. Always beats
    /// any selector-based rule of equal `important` rank.
    pub const INLINE: Specificity = Specificity(1, 0, 0, 0);
}

/// Parse a comma-separated selector list. Each comma yields one `Selector`.
/// Silently drops selectors that use unsupported syntax (attribute, pseudo,
/// sibling, `*`, `>`, `+`, `~`).
pub fn parse_selector_list(input: &str) -> Vec<Selector> {
    let mut out = Vec::new();
    for piece in input.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        if let Some(sel) = parse_one_selector(piece) {
            out.push(sel);
        }
    }
    out
}

/// Compute the CSS specificity of a `Selector`. Sums simples across
/// **every** compound in the selector's chain (descendant combinator does
/// not change the count — `div p` has the same specificity as `p p`).
///
/// Selector-derived specificity always leaves the inline bucket (a) at 0;
/// only [`Specificity::INLINE`] sets it.
pub fn specificity(selector: &Selector) -> Specificity {
    let mut b = 0u32;
    let mut c = 0u32;
    let mut d = 0u32;
    for compound in &selector.compounds {
        for part in &compound.parts {
            match part {
                SimpleSelector::Id(_) => b += 1,
                SimpleSelector::Class(_) => c += 1,
                SimpleSelector::Tag(_) => d += 1,
            }
        }
    }
    Specificity(0, b, c, d)
}

/// Parse a single (already-trimmed, non-empty) selector. Returns `None` if
/// the selector uses any feature we don't yet support.
fn parse_one_selector(input: &str) -> Option<Selector> {
    // Hard-reject characters that signal unsupported syntax. We check up
    // front so we don't have to thread error-handling through the rest of
    // the parser. `*` is rejected here too because we don't support the
    // universal selector at all (not even as the implicit head of a
    // compound).
    if input
        .chars()
        .any(|c| matches!(c, '[' | ']' | ':' | '>' | '+' | '~' | '*'))
    {
        return None;
    }

    let mut compounds = Vec::new();
    for raw in input.split_ascii_whitespace() {
        let compound = parse_compound(raw)?;
        compounds.push(compound);
    }
    if compounds.is_empty() {
        return None;
    }
    Some(Selector { compounds })
}

/// Parse one compound selector token (no whitespace inside). The first
/// token may be a bare tag name; subsequent tokens must each start with
/// `.` (class) or `#` (id). Returns `None` for malformed input.
fn parse_compound(raw: &str) -> Option<Compound> {
    if raw.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0;

    // Optional leading tag name. If the compound starts with an
    // identifier character, consume it as the tag.
    if is_ident_start(bytes[0] as char) {
        let start = i;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        let tag = &raw[start..i];
        parts.push(SimpleSelector::Tag(tag.to_ascii_lowercase()));
    }

    // Remaining tokens, each prefixed by `.` or `#`.
    while i < bytes.len() {
        let prefix = bytes[i] as char;
        i += 1;
        let start = i;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        if start == i {
            // Empty name after `.` or `#` — malformed.
            return None;
        }
        let name = &raw[start..i];
        match prefix {
            '.' => parts.push(SimpleSelector::Class(name.to_string())),
            '#' => parts.push(SimpleSelector::Id(name.to_string())),
            _ => return None,
        }
    }

    if parts.is_empty() {
        return None;
    }
    Some(Compound { parts })
}

/// CSS identifier start: ASCII letter or underscore. We don't support the
/// full Unicode identifier grammar — Phase 1.6b stylesheets are
/// human-authored and stick to the boring subset.
fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// CSS identifier continuation: identifier-start plus digits and `-`.
fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Does this selector match the given element?
pub fn matches(selector: &Selector, element: ElementRef<'_>) -> bool {
    let compounds = &selector.compounds;
    if compounds.is_empty() {
        return false;
    }
    // The rightmost compound is the subject and must match `element`
    // itself. Earlier compounds must each match SOME ancestor, walking
    // outward, but they don't need to be contiguous.
    let last = compounds.len() - 1;
    if !compound_matches(&compounds[last], element) {
        return false;
    }
    if last == 0 {
        return true;
    }
    // Walk ancestors looking for the remaining compounds in reverse
    // order. Each compound must be matched by some ancestor; once
    // matched, we move to the next compound and continue from the next
    // ancestor.
    let mut remaining = last; // index of the next compound to satisfy (working leftward)
    let mut current = element.parent().and_then(ElementRef::wrap);
    while let Some(anc) = current {
        // We're looking for compounds[remaining - 1] (the one immediately
        // to the left of the most recently satisfied compound).
        let target = remaining - 1;
        if compound_matches(&compounds[target], anc) {
            if target == 0 {
                return true;
            }
            remaining = target;
        }
        current = anc.parent().and_then(ElementRef::wrap);
    }
    false
}

/// Does `compound` match `element`? Every simple selector in the compound
/// must apply to the same element.
fn compound_matches(compound: &Compound, element: ElementRef<'_>) -> bool {
    compound.parts.iter().all(|p| simple_matches(p, element))
}

fn simple_matches(simple: &SimpleSelector, element: ElementRef<'_>) -> bool {
    match simple {
        // html5ever lowercases tag names, and we lowercased the selector
        // tag at parse time, so byte-equality is correct here.
        SimpleSelector::Tag(name) => element.value().name() == name.as_str(),
        SimpleSelector::Class(name) => element
            .value()
            .has_class(name, CaseSensitivity::CaseSensitive),
        SimpleSelector::Id(name) => element.value().id() == Some(name.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::Html;
    use scraper::Selector as ScraperSelector;

    /// Helper: parse a fragment and return the first element matching
    /// `css`. Uses `scraper`'s own selector engine (unrelated to our
    /// `Selector` struct) just to navigate the tree in tests.
    fn first<'a>(html: &'a Html, css: &str) -> ElementRef<'a> {
        let s = ScraperSelector::parse(css).expect("test css selector");
        html.select(&s).next().expect("element not found")
    }

    #[test]
    fn matches_tag_only() {
        let sels = parse_selector_list("p");
        assert_eq!(sels.len(), 1);

        let html = Html::parse_fragment("<p>x</p>");
        let p = first(&html, "p");
        assert!(matches(&sels[0], p));

        let html = Html::parse_fragment("<div>x</div>");
        let d = first(&html, "div");
        assert!(!matches(&sels[0], d));
    }

    #[test]
    fn matches_class_and_id() {
        let html = Html::parse_fragment(r#"<p class="foo bar" id="bar">x</p>"#);
        let p = first(&html, "p");

        let foo = parse_selector_list(".foo");
        assert_eq!(foo.len(), 1);
        assert!(matches(&foo[0], p));

        let baz = parse_selector_list(".baz");
        assert_eq!(baz.len(), 1);
        assert!(!matches(&baz[0], p));

        let id_bar = parse_selector_list("#bar");
        assert_eq!(id_bar.len(), 1);
        assert!(matches(&id_bar[0], p));

        let id_baz = parse_selector_list("#baz");
        assert_eq!(id_baz.len(), 1);
        assert!(!matches(&id_baz[0], p));
    }

    #[test]
    fn matches_compound() {
        let sel = parse_selector_list("p.foo");
        assert_eq!(sel.len(), 1);

        let html = Html::parse_fragment(r#"<p class="foo">x</p>"#);
        assert!(matches(&sel[0], first(&html, "p")));

        let html = Html::parse_fragment(r#"<p class="bar">x</p>"#);
        assert!(!matches(&sel[0], first(&html, "p")));

        let html = Html::parse_fragment(r#"<div class="foo">x</div>"#);
        assert!(!matches(&sel[0], first(&html, "div")));
    }

    #[test]
    fn matches_descendant() {
        let sel = parse_selector_list("div p");
        assert_eq!(sel.len(), 1);

        let html = Html::parse_fragment("<div><p>x</p></div>");
        assert!(matches(&sel[0], first(&html, "p")));

        let html = Html::parse_fragment("<section><p>x</p></section>");
        assert!(!matches(&sel[0], first(&html, "p")));
    }

    #[test]
    fn parses_selector_list_drops_unsupported() {
        let sels = parse_selector_list("p, p:hover, [href], > p, p + p, .ok");
        assert_eq!(sels.len(), 2);

        // First survivor is the bare-tag `p`.
        assert_eq!(sels[0].compounds.len(), 1);
        assert_eq!(sels[0].compounds[0].parts.len(), 1);
        assert_eq!(
            sels[0].compounds[0].parts[0],
            SimpleSelector::Tag("p".to_string())
        );

        // Second survivor is `.ok`.
        assert_eq!(sels[1].compounds.len(), 1);
        assert_eq!(sels[1].compounds[0].parts.len(), 1);
        assert_eq!(
            sels[1].compounds[0].parts[0],
            SimpleSelector::Class("ok".to_string())
        );
    }

    /// Helper: parse a single selector or panic. Used in specificity tests
    /// where we want a `Selector` directly without the list ceremony.
    fn one(input: &str) -> Selector {
        let sels = parse_selector_list(input);
        assert_eq!(sels.len(), 1, "expected exactly one selector for {input:?}");
        sels.into_iter().next().unwrap()
    }

    #[test]
    fn specificity_tag_only_is_one_in_c() {
        assert_eq!(specificity(&one("p")), Specificity(0, 0, 0, 1));
    }

    #[test]
    fn specificity_class_only_is_one_in_b() {
        assert_eq!(specificity(&one(".foo")), Specificity(0, 0, 1, 0));
    }

    #[test]
    fn specificity_id_only_is_one_in_a() {
        assert_eq!(specificity(&one("#bar")), Specificity(0, 1, 0, 0));
    }

    #[test]
    fn specificity_compound_sums_parts() {
        assert_eq!(specificity(&one("p.foo#bar")), Specificity(0, 1, 1, 1));
    }

    #[test]
    fn specificity_descendant_sums_compounds() {
        assert_eq!(specificity(&one("div p")), Specificity(0, 0, 0, 2));
        assert_eq!(specificity(&one("div p .x")), Specificity(0, 0, 1, 2));
    }

    #[test]
    fn specificity_id_beats_class_beats_tag() {
        assert!(Specificity(0, 1, 0, 0) > Specificity(0, 0, 99, 0));
        assert!(Specificity(0, 0, 1, 0) > Specificity(0, 0, 0, 99));
    }

    #[test]
    fn specificity_lexicographic_within_bucket() {
        assert!(Specificity(0, 1, 2, 3) < Specificity(0, 1, 2, 4));
        assert!(Specificity(0, 1, 2, 3) < Specificity(0, 1, 3, 0));
    }

    #[test]
    fn specificity_zero_for_empty_selector() {
        let empty = Selector { compounds: vec![] };
        assert_eq!(specificity(&empty), Specificity::ZERO);
    }

    #[test]
    fn specificity_multi_id_multi_class() {
        assert_eq!(specificity(&one("#a #b .c.d e")), Specificity(0, 2, 2, 1));
    }

    #[test]
    fn specificity_inline_constant_beats_any_selector() {
        assert!(Specificity::INLINE > Specificity(0, 999, 999, 999));
    }

    #[test]
    fn specificity_default_zero_matches_const() {
        assert_eq!(Specificity::default(), Specificity::ZERO);
    }
}
