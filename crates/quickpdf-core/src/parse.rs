//! HTML parsing — html5ever (via scraper) → DOM.
//!
//! Phase 1.1 surface: just enough to walk the tree and extract text content.
//! Phase 1.2+ adds metadata (font/style hints) needed by layout.

use scraper::{ElementRef, Html, Node};
use scraper::node::Element;
use ego_tree::NodeId;

use crate::style::sheet::{self, Declaration, Rule};

/// Sentinel `TextBlock::tag` value for an anonymous block created by
/// `collect_blocks` to wrap orphan inline content inside a container
/// element that also has block children. Anonymous text blocks reuse the
/// **parent** element's `element_id`.
pub const ANONYMOUS_TAG: &str = "anonymous";

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
        self.blocks()
            .into_iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some(t.text),
                Block::Image(_) => None,
            })
            .collect()
    }

    /// Document content grouped into block-level units in document order.
    /// Each entry is either a `TextBlock` (one paragraph of inline text)
    /// or an `ImageBlock` (one `<img>` lifted into the stream). Anonymous
    /// text blocks wrap orphan inline content inside mixed-content
    /// containers; they reuse the parent's `element_id`.
    pub fn blocks(&self) -> Vec<Block> {
        let mut out: Vec<Block> = Vec::new();
        let root = self.html.root_element();
        collect_blocks(root, &mut out);
        out
    }

    /// Visible text grouped into paragraphs, each tagged with the element
    /// whose inline content it represents. Retained for transition tests;
    /// new callers should use `blocks()` and match on `Block::Text`.
    pub fn paragraphs(&self) -> Vec<TextBlock> {
        self.blocks()
            .into_iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some(t),
                Block::Image(_) => None,
            })
            .collect()
    }

    /// Parse all `<style>` blocks in the document into a flat rule list.
    /// Phase 1.6b: inline `<style>` only — external `<link rel="stylesheet">`
    /// arrives in Phase 1.6c. Cheap to call (linear in stylesheet length);
    /// callers may cache the result for the duration of a render.
    pub fn user_stylesheet(&self) -> Vec<Rule> {
        sheet::parse_stylesheet(&sheet::collect_style_blocks(self))
    }

    /// Resolve a block's element handle back to a live `ElementRef`.
    /// Returns `None` if the handle is stale (which shouldn't happen — the
    /// `Document` owns the tree — but we treat it defensively).
    ///
    /// Takes `&TextBlock` for backward compatibility with
    /// lib.rs during the T5→T10 transition. T10 (integrator) updates this
    /// to take `&Block` once all callers are migrated.
    #[deprecated(note = "use element_for_block(&Block); this transition shim is removed when lib.rs migrates in Plan T10")]
    pub fn element_for(&self, p: &TextBlock) -> Option<ElementRef<'_>> {
        let node = self.html.tree.get(p.element_id)?;
        ElementRef::wrap(node)
    }

    /// New API: resolve any `Block`'s element handle back to a live
    /// `ElementRef`. Replaces `element_for` once `lib.rs` is updated (T10).
    pub fn element_for_block(&self, b: &Block) -> Option<ElementRef<'_>> {
        let node = self.html.tree.get(b.element_id())?;
        ElementRef::wrap(node)
    }

    /// Walk the DOM and return per-element inline `style="..."` declarations,
    /// in document order. Skipped subtrees (script/style/head/noscript/template)
    /// contribute nothing. Empty `style=""` attributes are dropped. Elements
    /// whose `style` value parses to zero declarations are also dropped.
    pub fn inline_styles(&self) -> Vec<(NodeId, Vec<Declaration>)> {
        let mut out = Vec::new();
        let root = self.html.root_element();
        visit_inline_styles(root, &mut out);
        out
    }
}

// `f32` fields in `ImageBlock` (width_attr, height_attr) prevent `Eq` —
// HTML allows fractional pixel values like `<img width="120.5">`, so we
// keep the float type to preserve precision. Block and ImageBlock therefore
// derive only `PartialEq`.
/// One block-level unit in document order. Either text content (the old
/// `Paragraph`) or an image. The renderer matches on this enum to drive
/// either the text-flow path or the image-paint path.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(TextBlock),
    Image(ImageBlock),
}

impl Block {
    /// The DOM element this block was emitted for. Anonymous text blocks
    /// reuse the parent block container's element id.
    pub fn element_id(&self) -> ego_tree::NodeId {
        match self {
            Block::Text(t) => t.element_id,
            Block::Image(i) => i.element_id,
        }
    }
}

/// One paragraph's worth of inline text plus the block-level tag that
/// produced it. The tag drives UA-default styles; the `element_id` lets
/// the cascade match author selectors against the original DOM. Equivalent
/// to the pre-2a `Paragraph` struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextBlock {
    pub tag: String,
    pub text: String,
    pub element_id: ego_tree::NodeId,
}

/// A `<img>` lifted into the block stream. `src` is the raw attribute value
/// (caller decodes via `crate::image::parse_data_url`). `width_attr` and
/// `height_attr` capture the HTML `width` / `height` attributes if they
/// parsed as plain numbers (CSS pixels — same as PDF points at our 1:1
/// conversion baseline). `alt` is recorded for the integrator's fallback
/// path when decoding fails.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageBlock {
    pub element_id: ego_tree::NodeId,
    pub src: String,
    pub width_attr: Option<f32>,
    pub height_attr: Option<f32>,
    pub alt: Option<String>,
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
            | "img"
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

/// Walk the tree and emit one `Block` per block-level element. For text
/// containers this matches the pre-2a `collect_paragraphs` behavior — leaf
/// blocks emit `Block::Text`, mixed-content containers emit anonymous
/// `Block::Text` runs around their block children. For `<img>` we emit a
/// `Block::Image` carrying `src`, optional `width`/`height` attrs, and
/// optional `alt`.
fn collect_blocks(elem: ElementRef<'_>, out: &mut Vec<Block>) {
    let name = elem.value().name();
    if is_skipped(name) {
        return;
    }

    // <img> is an empty (void) block element. Don't recurse into children.
    if name == "img" {
        let attr_f32 = |key: &str| -> Option<f32> {
            elem.value().attr(key).and_then(|v| v.trim().parse::<f32>().ok())
        };
        out.push(Block::Image(ImageBlock {
            element_id: elem.id(),
            src: elem.value().attr("src").unwrap_or("").to_string(),
            width_attr: attr_f32("width"),
            height_attr: attr_f32("height"),
            alt: elem.value().attr("alt").map(|s| s.to_string()),
        }));
        return;
    }

    let has_block_child = elem.children().any(|c| {
        if let Node::Element(e) = c.value() {
            !is_skipped(e.name()) && is_block(e.name())
        } else {
            false
        }
    });
    if is_block(name) && has_block_child {
        // Mixed-content block: walk children, accumulate inline text/element
        // runs into an anonymous buffer. Block element children flush the
        // buffer and recurse normally.
        let parent_id = elem.id();
        let mut buffer = String::new();
        let flush =
            |buffer: &mut String, out: &mut Vec<Block>| {
                let collapsed = collapse_whitespace(buffer.as_str());
                if !collapsed.is_empty() {
                    out.push(Block::Text(TextBlock {
                        tag: ANONYMOUS_TAG.to_string(),
                        text: collapsed,
                        element_id: parent_id,
                    }));
                }
                buffer.clear();
            };
        for child in elem.children() {
            match child.value() {
                Node::Text(t) => buffer.push_str(&t.text),
                Node::Element(e) => {
                    let child_name = e.name();
                    if is_skipped(child_name) {
                        continue;
                    }
                    if let Some(child_elem) = ElementRef::wrap(child) {
                        if is_block(child_name) {
                            flush(&mut buffer, out);
                            collect_blocks(child_elem, out);
                        } else {
                            collect_text(child_elem, &mut buffer);
                        }
                    }
                }
                _ => {}
            }
        }
        flush(&mut buffer, out);
        return;
    }
    if !is_block(name) {
        for child in elem.children() {
            if let Some(child_elem) = ElementRef::wrap(child) {
                collect_blocks(child_elem, out);
            }
        }
        return;
    }
    // Leaf block: gather inline text content.
    let mut text = String::new();
    collect_text(elem, &mut text);
    let collapsed = collapse_whitespace(&text);
    if !collapsed.is_empty() {
        out.push(Block::Text(TextBlock {
            tag: name.to_string(),
            text: collapsed,
            element_id: elem.id(),
        }));
    }
}

fn visit_inline_styles(
    elem: ElementRef<'_>,
    out: &mut Vec<(NodeId, Vec<Declaration>)>,
) {
    let name = elem.value().name();
    if is_skipped(name) {
        return;
    }
    if let Some(raw) = elem.value().attr("style") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let decls = sheet::parse_inline_declarations(trimmed);
            if !decls.is_empty() {
                out.push((elem.id(), decls));
            }
        }
    }
    for child in elem.children() {
        if let Some(child_elem) = ElementRef::wrap(child) {
            visit_inline_styles(child_elem, out);
        }
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
        let ps = d.blocks();
        let tagged: Vec<(&str, &str)> = ps
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some((t.tag.as_str(), t.text.as_str())),
                Block::Image(_) => None,
            })
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
        let ps: Vec<_> = d.blocks().into_iter().filter_map(|b| match b {
            Block::Text(t) => Some(t),
            Block::Image(_) => None,
        }).collect();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].tag, "p");
        assert_eq!(ps[0].text, "only this");
    }

    #[test]
    fn anonymous_constant_value() {
        assert_eq!(ANONYMOUS_TAG, "anonymous");
    }

    #[test]
    fn anonymous_wraps_orphan_text_around_block_child() {
        let d = Document::parse("<div>before<p>middle</p>after</div>");
        let ps = d.blocks();
        let tagged: Vec<(&str, &str)> = ps
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some((t.tag.as_str(), t.text.as_str())),
                Block::Image(_) => None,
            })
            .collect();
        assert_eq!(
            tagged,
            vec![
                (ANONYMOUS_TAG, "before"),
                ("p", "middle"),
                (ANONYMOUS_TAG, "after"),
            ]
        );
    }

    #[test]
    fn anonymous_uses_parent_element_id() {
        let d = Document::parse("<div>before<p>middle</p>after</div>");
        let ps = d.blocks();
        // Find the parent <div> and assert the anonymous blocks
        // resolve back through Document::element_for_block to that <div>.
        let anon: Vec<&Block> = ps
            .iter()
            .filter(|b| matches!(b, Block::Text(t) if t.tag == ANONYMOUS_TAG))
            .collect();
        assert_eq!(anon.len(), 2);
        for a in anon {
            let resolved = d.element_for_block(a).expect("anon element_id resolves");
            assert_eq!(resolved.value().name(), "div");
        }
    }

    #[test]
    fn anonymous_drops_whitespace_only_runs() {
        let d = Document::parse("<div>   <p>x</p>   </div>");
        let ps: Vec<_> = d.blocks().into_iter().filter_map(|b| match b {
            Block::Text(t) => Some(t),
            Block::Image(_) => None,
        }).collect();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].tag, "p");
        assert_eq!(ps[0].text, "x");
    }

    #[test]
    fn pure_leaf_block_unchanged() {
        // <div>only text</div> has no block children → still a leaf-block
        // tagged "div", same as before Phase 1.6c.
        let d = Document::parse("<div>only text</div>");
        let ps: Vec<_> = d.blocks().into_iter().filter_map(|b| match b {
            Block::Text(t) => Some(t),
            Block::Image(_) => None,
        }).collect();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].tag, "div");
        assert_eq!(ps[0].text, "only text");
    }

    #[test]
    fn pure_block_container_unchanged() {
        let d = Document::parse("<div><p>only</p></div>");
        let ps: Vec<_> = d.blocks().into_iter().filter_map(|b| match b {
            Block::Text(t) => Some(t),
            Block::Image(_) => None,
        }).collect();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].tag, "p");
        assert_eq!(ps[0].text, "only");
    }

    #[test]
    fn multiple_block_children_with_orphan_runs() {
        let d = Document::parse(
            "<div>before<p>p1</p>between<p>p2</p>after</div>",
        );
        let ps = d.blocks();
        let tagged: Vec<(&str, &str)> = ps
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some((t.tag.as_str(), t.text.as_str())),
                Block::Image(_) => None,
            })
            .collect();
        assert_eq!(
            tagged,
            vec![
                (ANONYMOUS_TAG, "before"),
                ("p", "p1"),
                (ANONYMOUS_TAG, "between"),
                ("p", "p2"),
                (ANONYMOUS_TAG, "after"),
            ]
        );
    }

    #[test]
    fn inline_elements_feed_anonymous_buffer() {
        let d = Document::parse("<div>a<span>b</span><p>c</p>d</div>");
        let ps = d.blocks();
        let tagged: Vec<(&str, &str)> = ps
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some((t.tag.as_str(), t.text.as_str())),
                Block::Image(_) => None,
            })
            .collect();
        assert_eq!(
            tagged,
            vec![
                (ANONYMOUS_TAG, "ab"),
                ("p", "c"),
                (ANONYMOUS_TAG, "d"),
            ]
        );
    }

    #[test]
    fn nested_anonymous_uses_correct_parent_id() {
        // Outer div has an anon "x" tied to the div, plus a <section>
        // that itself has mixed content → anon "y" tied to the section,
        // followed by p "z".
        let d = Document::parse(
            "<div>x<section>y<p>z</p></section></div>",
        );
        let ps = d.blocks();
        assert_eq!(ps.len(), 3);

        match &ps[0] {
            Block::Text(t) => {
                assert_eq!(t.tag, ANONYMOUS_TAG);
                assert_eq!(t.text, "x");
                let parent0 = d.element_for_block(&ps[0]).unwrap();
                assert_eq!(parent0.value().name(), "div");
            }
            Block::Image(_) => panic!("expected text block"),
        }

        match &ps[1] {
            Block::Text(t) => {
                assert_eq!(t.tag, ANONYMOUS_TAG);
                assert_eq!(t.text, "y");
                let parent1 = d.element_for_block(&ps[1]).unwrap();
                assert_eq!(parent1.value().name(), "section");
            }
            Block::Image(_) => panic!("expected text block"),
        }

        match &ps[2] {
            Block::Text(t) => {
                assert_eq!(t.tag, "p");
                assert_eq!(t.text, "z");
            }
            Block::Image(_) => panic!("expected text block"),
        }
    }

    #[test]
    fn skipped_subtree_text_does_not_leak_into_anonymous() {
        // <script> and <style> are is_skipped — their text content should
        // not contribute to the anonymous buffer.
        let d = Document::parse(
            "<div>a<script>NOPE</script><p>b</p><style>x{color:red}</style>c</div>",
        );
        let ps = d.blocks();
        let tagged: Vec<(&str, &str)> = ps
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some((t.tag.as_str(), t.text.as_str())),
                Block::Image(_) => None,
            })
            .collect();
        assert_eq!(
            tagged,
            vec![
                (ANONYMOUS_TAG, "a"),
                ("p", "b"),
                (ANONYMOUS_TAG, "c"),
            ]
        );
    }

    // ---- Phase 1.7c Slice B: inline `style="..."` extraction tests. ----

    #[test]
    fn inline_styles_empty_for_no_style_attrs() {
        let d = Document::parse("<p>plain</p><div>also plain</div>");
        assert!(d.inline_styles().is_empty());
    }

    #[test]
    fn inline_styles_collects_one_per_element() {
        let d = Document::parse(r#"<p style="color: red">x</p>"#);
        let inline = d.inline_styles();
        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].1.len(), 1);
        assert_eq!(inline[0].1[0].name, "color");
        assert_eq!(inline[0].1[0].value, "red");
        assert!(!inline[0].1[0].important);
    }

    #[test]
    fn inline_styles_skips_empty_style_attribute() {
        let d = Document::parse(r#"<p style="">x</p>"#);
        assert!(d.inline_styles().is_empty());
    }

    #[test]
    fn inline_styles_skips_whitespace_only_style_attribute() {
        let d = Document::parse(r#"<p style="   	  ">x</p>"#);
        assert!(d.inline_styles().is_empty());
    }

    #[test]
    fn inline_styles_in_document_order() {
        let d = Document::parse(
            r#"<p style="color: red">a</p><p style="color: blue">b</p><p style="color: green">c</p>"#,
        );
        let inline = d.inline_styles();
        assert_eq!(inline.len(), 3);
        assert_eq!(inline[0].1[0].value, "red");
        assert_eq!(inline[1].1[0].value, "blue");
        assert_eq!(inline[2].1[0].value, "green");
    }

    #[test]
    fn inline_styles_skipped_in_script_subtree() {
        // The DOM allows the `style` attribute on a <script> tag in
        // theory, but our walker treats <script> as a skipped subtree.
        let d = Document::parse(
            r#"<script style="color: red">var x = 1;</script><p style="color: blue">y</p>"#,
        );
        let inline = d.inline_styles();
        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].1[0].value, "blue");
    }

    #[test]
    fn inline_styles_skipped_in_style_subtree() {
        let d = Document::parse(
            r#"<style style="color: red">p { color: green; }</style><p style="color: blue">y</p>"#,
        );
        let inline = d.inline_styles();
        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].1[0].value, "blue");
    }

    #[test]
    fn inline_styles_handles_important() {
        let d = Document::parse(r#"<p style="color: red !important">x</p>"#);
        let inline = d.inline_styles();
        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].1.len(), 1);
        assert_eq!(inline[0].1[0].name, "color");
        assert_eq!(inline[0].1[0].value, "red");
        assert!(inline[0].1[0].important);
    }

    #[test]
    fn inline_styles_node_id_resolves_via_element_for() {
        // Build a synthetic Block::Text with the inline-style NodeId and
        // confirm element_for_block round-trips back to the right element.
        let d = Document::parse(r#"<p style="color: red">x</p>"#);
        let inline = d.inline_styles();
        assert_eq!(inline.len(), 1);
        let node_id = inline[0].0;
        let synthetic = Block::Text(TextBlock {
            tag: "p".to_string(),
            text: "x".to_string(),
            element_id: node_id,
        });
        let resolved = d.element_for_block(&synthetic).expect("node id resolves");
        assert_eq!(resolved.value().name(), "p");
        // The resolved element's own NodeId matches the one we collected.
        assert_eq!(resolved.id(), node_id);
    }

    #[test]
    fn inline_styles_drops_unparseable_into_zero_decls() {
        // `style="garbage"` has no `:`, so parse_inline_declarations returns
        // an empty Vec and the element is dropped from the result.
        let d = Document::parse(
            r#"<p style="garbage no colons here">x</p><span style="color: red">y</span>"#,
        );
        let inline = d.inline_styles();
        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].1[0].name, "color");
        assert_eq!(inline[0].1[0].value, "red");
    }
}
