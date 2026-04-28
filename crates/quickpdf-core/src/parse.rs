//! HTML parsing — html5ever (via scraper) → DOM.
//!
//! Phase 1.1 surface: just enough to walk the tree and extract text content.
//! Phase 1.2+ adds metadata (font/style hints) needed by layout.

use scraper::{ElementRef, Html, Node};
use scraper::node::Element;
use ego_tree::NodeId;

use crate::style::sheet::{self, Rule};

/// Parsed HTML document. Owns the underlying DOM arena.
pub struct Document {
    pub(crate) html: Html,
}

impl Document {
    /// Parse a full HTML document from a string. Always succeeds — html5ever
    /// recovers from malformed input the same way browsers do.
    pub fn parse(source: &str) -> Self {
        Self {
            html: Html::parse_document(source),
        }
    }

    /// Total number of element nodes in the document tree (excluding text and
    /// comment nodes). Useful for tests and rough complexity heuristics.
    pub fn element_count(&self) -> usize {
        self.html
            .tree
            .nodes()
            .filter(|n| matches!(n.value(), Node::Element(_)))
            .count()
    }

    /// Concatenate all visible text in document order. Skips `<script>`,
    /// `<style>`, and HTML comments; collapses runs of whitespace to a single
    /// space, matching how browsers render text content for a quick "what
    /// would the user see" view.
    ///
    /// This exists for Phase 1.1 verification and as a primitive we'll
    /// reuse later for inline text shaping. It is *not* the layout pipeline.
    pub fn visible_text(&self) -> String {
        let mut out = String::new();
        let root = self.html.root_element();
        collect_text(root, &mut out);
        collapse_whitespace(&out)
    }

    /// Visible text grouped by block-level element. Each entry is one
    /// paragraph's worth of inline content; empty paragraphs are dropped.
    ///
    /// This is the seam Phase 1.5 layout uses: render each entry as its own
    /// stack-direction block, with a vertical gap between blocks. Phase 1.6+
    /// will return styled tokens instead of plain strings.
    pub fn block_texts(&self) -> Vec<String> {
        self.paragraphs().into_iter().map(|p| p.text).collect()
    }

    /// Visible text grouped into paragraphs, each tagged with the element
    /// whose inline content it represents. The renderer uses the tag to look
    /// up UA-default font size, weight, and margins (Phase 1.6+).
    ///
    /// "Paragraph" here = a block-level element whose children are all inline
    /// (no nested blocks). Anonymous-block creation for orphan text inside a
    /// container is deferred to Phase 1.6b — for now, raw text mixed between
    /// block siblings is dropped.
    pub fn paragraphs(&self) -> Vec<Paragraph> {
        let mut out: Vec<Paragraph> = Vec::new();
        let root = self.html.root_element();
        collect_paragraphs(root, &mut out);
        out
    }

    /// Parse all `<style>` blocks in the document into a flat rule list.
    /// Phase 1.6b: inline `<style>` only — external `<link rel="stylesheet">`
    /// arrives in Phase 1.6c. Cheap to call (linear in stylesheet length);
    /// callers may cache the result for the duration of a render.
    pub fn user_stylesheet(&self) -> Vec<Rule> {
        sheet::parse_stylesheet(&sheet::collect_style_blocks(self))
    }

    /// Resolve a `Paragraph`'s element handle back to a live `ElementRef`.
    /// Returns `None` if the handle is stale (which shouldn't happen — the
    /// `Document` owns the tree — but we treat it defensively).
    pub fn element_for(&self, p: &Paragraph) -> Option<ElementRef<'_>> {
        let node = self.html.tree.get(p.element_id)?;
        ElementRef::wrap(node)
    }
}

/// One paragraph's worth of inline text plus the block-level tag that
/// produced it. The tag drives UA-default styles; the `element_id` lets
/// the cascade match author selectors (Phase 1.6b) against the original DOM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    pub tag: String,
    pub text: String,
    /// Stable handle into `Document::html.tree`. Use `Document::element_for`
    /// to recover the live `ElementRef`.
    pub element_id: NodeId,
}

/// Internal helper: drop the now-internal `Element` import warning when only
/// some functions use it. Keep this unused-but-public access pattern explicit
/// so future module changes don't accidentally drop the import.
#[allow(dead_code)]
fn _force_element_ref<'a>(e: &'a Element) -> &'a Element {
    e
}

fn is_skipped(tag: &str) -> bool {
    matches!(tag, "script" | "style" | "head" | "noscript" | "template")
}

/// HTML block-level elements we recognise for paragraph splitting in Phase 1.5.
/// Mirrors the WHATWG "block" set, minus tags we don't render yet (forms, media).
fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "br"
            | "dd"
            | "details"
            | "dialog"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "summary"
            | "table"
            | "tr"
            | "ul"
    )
}

fn collect_text(elem: ElementRef<'_>, out: &mut String) {
    let name = elem.value().name();
    if is_skipped(name) {
        return;
    }
    for child in elem.children() {
        match child.value() {
            Node::Text(t) => out.push_str(&t.text),
            Node::Element(_) => {
                if let Some(child_elem) = ElementRef::wrap(child) {
                    collect_text(child_elem, out);
                }
            }
            _ => {}
        }
    }
}

/// Walk the tree and emit one `Paragraph` per "leaf" block-level element,
/// where leaf = no block-level descendants. Recurses into containers.
fn collect_paragraphs(elem: ElementRef<'_>, out: &mut Vec<Paragraph>) {
    let name = elem.value().name();
    if is_skipped(name) {
        return;
    }
    let has_block_child = elem.children().any(|c| {
        if let Node::Element(e) = c.value() {
            !is_skipped(e.name()) && is_block(e.name())
        } else {
            false
        }
    });
    if has_block_child || !is_block(name) {
        // Container (or non-block root) — recurse into children. Orphan text
        // mixed with block children is dropped for Phase 1.6a; anonymous-block
        // wrapping comes in Phase 1.6b.
        for child in elem.children() {
            if let Some(child_elem) = ElementRef::wrap(child) {
                collect_paragraphs(child_elem, out);
            }
        }
        return;
    }
    // Leaf block: gather inline text content.
    let mut text = String::new();
    collect_text(elem, &mut text);
    let collapsed = collapse_whitespace(&text);
    if !collapsed.is_empty() {
        out.push(Paragraph {
            tag: name.to_string(),
            text: collapsed,
            element_id: elem.id(),
        });
    }
}

fn collapse_whitespace<S: AsRef<str>>(s: S) -> String {
    let s = s.as_ref();
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws && !out.is_empty() {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trivial() {
        let d = Document::parse("<h1>hi</h1>");
        assert!(d.element_count() > 0);
        assert_eq!(d.visible_text(), "hi");
    }

    #[test]
    fn skips_script_and_style() {
        let html = r#"
            <html>
                <head><style>body { color: red; }</style></head>
                <body>
                    <script>alert("nope")</script>
                    <p>visible <b>text</b> here</p>
                </body>
            </html>
        "#;
        let d = Document::parse(html);
        assert_eq!(d.visible_text(), "visible text here");
    }

    #[test]
    fn collapses_whitespace_runs() {
        let d = Document::parse("<p>a   b\n\n  c</p>");
        assert_eq!(d.visible_text(), "a b c");
    }

    #[test]
    fn counts_elements() {
        let d = Document::parse("<div><p>a</p><p>b</p></div>");
        // html5ever wraps in <html><head></head><body>...</body></html> so
        // we expect at least: html, head, body, div, p, p = 6
        assert!(d.element_count() >= 6, "got {}", d.element_count());
    }

    #[test]
    fn block_texts_splits_on_block_boundaries() {
        let d = Document::parse("<p>first</p><p>second</p>");
        assert_eq!(d.block_texts(), vec!["first".to_string(), "second".to_string()]);
    }

    #[test]
    fn block_texts_handles_headings_and_lists() {
        let html = "<h1>Title</h1><p>intro</p><ul><li>one</li><li>two</li></ul>";
        let d = Document::parse(html);
        assert_eq!(
            d.block_texts(),
            vec![
                "Title".to_string(),
                "intro".to_string(),
                "one".to_string(),
                "two".to_string(),
            ]
        );
    }

    #[test]
    fn block_texts_keeps_inline_run_together() {
        // <span> is inline, so its text stays merged into the surrounding <p>.
        let d = Document::parse("<p>hello <span>shiny</span> world</p>");
        assert_eq!(d.block_texts(), vec!["hello shiny world".to_string()]);
    }

    #[test]
    fn block_texts_drops_empty_paragraphs() {
        let d = Document::parse("<p></p><p>real</p><div>   </div>");
        assert_eq!(d.block_texts(), vec!["real".to_string()]);
    }

    #[test]
    fn paragraphs_tag_each_block() {
        let d = Document::parse("<h1>Title</h1><p>body</p><ul><li>x</li></ul>");
        let ps = d.paragraphs();
        let tagged: Vec<(&str, &str)> = ps
            .iter()
            .map(|p| (p.tag.as_str(), p.text.as_str()))
            .collect();
        assert_eq!(
            tagged,
            vec![("h1", "Title"), ("p", "body"), ("li", "x")]
        );
    }

    #[test]
    fn paragraphs_skip_block_containers() {
        // <div> wraps <p>, so the only paragraph reported is the <p>.
        let d = Document::parse("<div><p>only this</p></div>");
        let ps = d.paragraphs();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].tag, "p");
        assert_eq!(ps[0].text, "only this");
    }
}
