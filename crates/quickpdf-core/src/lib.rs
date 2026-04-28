//! quickpdf-core — native HTML→PDF rendering core.
//!
//! Phase 1.1: HTML is parsed into a DOM but layout/paint are still stubs.
//! Real cascade + block/inline layout + text shaping land in Phases 1.2–1.5.

pub mod font;
pub mod parse;
pub mod style;
pub mod text;

pub use parse::Document as ParsedDocument;

use krilla::Document;
use krilla::SerializeSettings;
use krilla::geom::{Point, Size};
use krilla::color::rgb as krilla_rgb;
use krilla::paint::Fill;
use krilla::page::PageSettings;
use krilla::text::{Font, TextDirection};
use thiserror::Error;

use crate::style::Color;

/// A4 page size in PDF points (1pt = 1/72 in; 595×842 ≈ 210×297 mm).
pub const A4_WIDTH_PT: f32 = 595.0;
pub const A4_HEIGHT_PT: f32 = 842.0;

#[derive(Debug, Error)]
pub enum Error {
    #[error("PDF emission failed: {0}")]
    Pdf(String),
}

/// Page size as PDF points (width, height).
#[derive(Debug, Clone, Copy)]
pub enum PageSize {
    A4,
    Letter,
    Custom(f32, f32),
}

impl PageSize {
    pub fn dimensions(self) -> (f32, f32) {
        match self {
            PageSize::A4 => (A4_WIDTH_PT, A4_HEIGHT_PT),
            PageSize::Letter => (612.0, 792.0),
            PageSize::Custom(w, h) => (w, h),
        }
    }
}

/// Render options, mirroring the public Python API.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub page_size: PageSize,
    pub print_background: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            page_size: PageSize::A4,
            print_background: true,
        }
    }
}

/// Body-text starting offset in PDF points. Origin is top-left after krilla's
/// y-flip. Phase 1 hard-codes margins; Phase 2 will read them from `@page`.
const MARGIN_PT: f32 = 36.0; // 0.5 inch
/// Default body font size in PDF points (≈ CSS 12px at 1pt/px conversion).
const DEFAULT_FONT_SIZE_PT: f32 = 12.0;
/// Default leading multiplier (CSS-equivalent line-height ≈ 1.4).
const DEFAULT_LINE_HEIGHT: f32 = 1.4;
/// Vertical gap inserted between block-level paragraphs, in line-heights.
/// Phase 1.7 will read this from `margin-top`/`margin-bottom`.
const PARAGRAPH_GAP_LINES: f32 = 0.5;

/// One placed line ready to paint: where it goes, what to render at what
/// size, and what colour.
#[derive(Debug, Clone)]
struct PlacedLine {
    y: f32,
    x: f32,
    font_size_pt: f32,
    text: String,
    color: Color,
}

/// Render an HTML string to a PDF byte vector.
///
/// Phase 1.6b: parses author `<style>` blocks and applies their declarations
/// on top of the UA defaults via simple-selector matching (`p`, `.foo`,
/// `#bar`, descendant combinator). Last-declaration-wins; full specificity
/// is Phase 1.6c.
pub fn html_to_pdf(html: &str, options: &RenderOptions) -> Result<Vec<u8>, Error> {
    let parsed = parse::Document::parse(html);
    let paragraphs = parsed.paragraphs();
    let user_rules = parsed.user_stylesheet();

    let font = Font::new(font::FALLBACK_TTF.to_vec().into(), 0)
        .ok_or_else(|| Error::Pdf("could not load embedded fallback font".into()))?;

    let mut document = Document::new_with(SerializeSettings::default());
    let (page_w, page_h) = options.page_size.dimensions();
    let size = Size::from_wh(page_w, page_h)
        .ok_or_else(|| Error::Pdf(format!("invalid page size: {page_w}x{page_h}")))?;

    let content_width = page_w - 2.0 * MARGIN_PT;
    let bottom_limit = page_h - MARGIN_PT;

    let pages = plan_pages_styled(
        &parsed,
        &paragraphs,
        &user_rules,
        content_width,
        MARGIN_PT,
        bottom_limit,
    )?;

    for page_lines in &pages {
        let mut page = document.start_page_with(PageSettings::new(size));
        {
            let mut surface = page.surface();
            let mut current_color: Option<Color> = None;
            for line in page_lines {
                if current_color != Some(line.color) {
                    let fill = Fill {
                        paint: krilla_rgb::Color::new(
                            line.color.r,
                            line.color.g,
                            line.color.b,
                        )
                        .into(),
                        ..Fill::default()
                    };
                    surface.set_fill(Some(fill));
                    current_color = Some(line.color);
                }
                surface.draw_text(
                    Point::from_xy(line.x, line.y),
                    font.clone(),
                    line.font_size_pt,
                    &line.text,
                    false,
                    TextDirection::Auto,
                );
            }
            surface.finish();
        }
        page.finish();
    }

    // Edge case: no content at all → still emit one blank page so the PDF
    // is well-formed and our "always at least one page" invariant holds.
    if pages.is_empty() {
        let page = document.start_page_with(PageSettings::new(size));
        page.finish();
    }

    document.finish().map_err(|e| Error::Pdf(format!("{e:?}")))
}

/// Plan page layout for tagged paragraphs. Each paragraph resolves its own
/// `BlockStyle` (UA defaults + matching author rules), drives font size /
/// indent / margins, and flows lines into pages, breaking at the bottom
/// margin.
fn plan_pages_styled(
    doc: &parse::Document,
    paragraphs: &[parse::Paragraph],
    user_rules: &[style::sheet::Rule],
    content_width: f32,
    left_margin: f32,
    bottom_limit: f32,
) -> Result<Vec<Vec<PlacedLine>>, Error> {
    let top_baseline_for = |first_line_height: f32| MARGIN_PT + first_line_height;

    let mut pages: Vec<Vec<PlacedLine>> = Vec::new();
    let mut current: Vec<PlacedLine> = Vec::new();
    let mut cursor_y: Option<f32> = None;

    for (i, para) in paragraphs.iter().enumerate() {
        // Resolve the cascaded BlockStyle. If the element handle has somehow
        // gone stale, fall back to UA defaults so rendering still proceeds.
        let style = match doc.element_for(para) {
            Some(elem) => style::resolve(elem, user_rules),
            None => style::ua_style(&para.tag),
        };
        let font_size = DEFAULT_FONT_SIZE_PT * style.font_size_em;
        let line_height = font_size * DEFAULT_LINE_HEIGHT;
        let indent_pt = DEFAULT_FONT_SIZE_PT * style.indent_em;
        let para_x = left_margin + indent_pt;
        let para_width = (content_width - indent_pt).max(1.0);

        let metrics = text::TextMetrics::new(font::FALLBACK_TTF, font_size)
            .ok_or_else(|| Error::Pdf("could not measure font at requested size".into()))?;
        let lines = text::wrap_lines(&metrics, &para.text, para_width);
        if lines.is_empty() {
            continue;
        }

        // Vertical spacing: top_margin from this block, plus the half-em gap
        // we use between any two blocks (Phase 1.5 default kept as a baseline).
        let top_margin_pt = font_size * style.margin_top_em;
        if let Some(y) = cursor_y.as_mut() {
            *y += top_margin_pt + line_height * PARAGRAPH_GAP_LINES;
            if *y > bottom_limit {
                pages.push(std::mem::take(&mut current));
                cursor_y = Some(top_baseline_for(line_height));
            }
        } else {
            cursor_y = Some(top_baseline_for(line_height));
        }

        for line in lines {
            let y = cursor_y.expect("cursor_y is set by the time we reach lines");
            if y > bottom_limit {
                pages.push(std::mem::take(&mut current));
                cursor_y = Some(top_baseline_for(line_height));
            }
            let final_y = cursor_y.unwrap();
            current.push(PlacedLine {
                y: final_y,
                x: para_x,
                font_size_pt: font_size,
                text: line,
                color: style.color,
            });
            cursor_y = Some(final_y + line_height);
        }

        // Record bottom margin so the next block's top margin can be added on.
        if let Some(y) = cursor_y.as_mut() {
            *y += font_size * style.margin_bottom_em;
        }

        let _ = i; // index unused but keeps loop shape obvious for future cascade work
    }

    if !current.is_empty() {
        pages.push(current);
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse some HTML, plan its pages with a custom layout box and
    /// no author CSS. Returns the doc (held alive so element_ids stay valid)
    /// alongside the planned pages.
    fn plan(html: &str) -> Vec<Vec<PlacedLine>> {
        let doc = parse::Document::parse(html);
        let paragraphs = doc.paragraphs();
        let rules = doc.user_stylesheet();
        plan_pages_styled(&doc, &paragraphs, &rules, 500.0, 36.0, 800.0).unwrap()
    }

    #[test]
    fn styled_planner_h1_uses_larger_font() {
        let h1_pages = plan("<h1>Title</h1>");
        let p_pages = plan("<p>body</p>");
        assert!(
            h1_pages[0][0].font_size_pt > p_pages[0][0].font_size_pt,
            "h1 should render larger than p"
        );
    }

    #[test]
    fn styled_planner_li_indents() {
        let pages = plan("<p>body</p><ul><li>item</li></ul>");
        let p_line = pages[0].iter().find(|l| l.text == "body").unwrap();
        let li_line = pages[0].iter().find(|l| l.text == "item").unwrap();
        assert!(
            li_line.x > p_line.x,
            "li ({}) should be indented past p ({})",
            li_line.x,
            p_line.x
        );
    }

    #[test]
    fn styled_planner_paginates_overflow() {
        let html: String = (0..120)
            .map(|i| format!("<p>paragraph {i}</p>"))
            .collect();
        let pages = plan(&html);
        assert!(pages.len() >= 2, "expected multi-page, got {}", pages.len());
    }

    #[test]
    fn styled_planner_empty_input_yields_no_pages() {
        assert!(plan("").is_empty());
    }

    // Phase 1.6b: author CSS overrides UA defaults.
    #[test]
    fn author_font_size_overrides_ua_default() {
        let html = r#"<style>p { font-size: 24px; }</style><p>x</p>"#;
        let pages = plan(html);
        let line = pages[0].iter().find(|l| l.text == "x").unwrap();
        // 24px = 2em, base = 12pt → 24pt
        assert!(
            (line.font_size_pt - 24.0).abs() < 0.01,
            "expected 24pt, got {}",
            line.font_size_pt
        );
    }

    #[test]
    fn class_selector_targets_subset_only() {
        let html = r#"<style>.big { font-size: 36px; }</style>
            <p>plain</p><p class="big">huge</p>"#;
        let pages = plan(html);
        let plain = pages[0].iter().find(|l| l.text == "plain").unwrap();
        let huge = pages[0].iter().find(|l| l.text == "huge").unwrap();
        assert!(
            (plain.font_size_pt - 12.0).abs() < 0.01,
            "plain should keep 12pt UA default, got {}",
            plain.font_size_pt
        );
        assert!(
            (huge.font_size_pt - 36.0).abs() < 0.01,
            "huge should be 36pt, got {}",
            huge.font_size_pt
        );
    }

    // ---- Phase 1.6c: specificity, !important, inheritance, anonymous wrap.

    /// `#id` rule (specificity 1,0,0) beats `.class` rule (0,1,0) regardless
    /// of source order — even when the lower-specificity rule appears later.
    #[test]
    fn id_selector_beats_class_even_when_class_is_later() {
        let html = r#"<style>
            #target { font-size: 24px; }
            .x { font-size: 36px; }
        </style><p id="target" class="x">x</p>"#;
        let pages = plan(html);
        let line = pages[0].iter().find(|l| l.text == "x").unwrap();
        assert!(
            (line.font_size_pt - 24.0).abs() < 0.01,
            "expected 24pt (id wins), got {}",
            line.font_size_pt
        );
    }

    /// `!important` on a less-specific rule beats a more-specific rule
    /// without the marker.
    #[test]
    fn important_beats_higher_specificity() {
        let html = r#"<style>
            #target { font-size: 24px; }
            p { font-size: 48px !important; }
        </style><p id="target">x</p>"#;
        let pages = plan(html);
        let line = pages[0].iter().find(|l| l.text == "x").unwrap();
        assert!(
            (line.font_size_pt - 48.0).abs() < 0.01,
            "expected 48pt (!important wins), got {}",
            line.font_size_pt
        );
    }

    /// Two `!important` declarations with different specificities — the
    /// higher-specificity one wins.
    #[test]
    fn important_vs_important_falls_back_to_specificity() {
        let html = r#"<style>
            p { font-size: 18px !important; }
            #target { font-size: 30px !important; }
        </style><p id="target">x</p>"#;
        let pages = plan(html);
        let line = pages[0].iter().find(|l| l.text == "x").unwrap();
        assert!(
            (line.font_size_pt - 30.0).abs() < 0.01,
            "expected 30pt (id-important wins over tag-important), got {}",
            line.font_size_pt
        );
    }

    /// A child paragraph in an unstyled tag should inherit `font-size` from
    /// an ancestor that set it. `<div>` is a leaf block when it has no block
    /// children, so the test wraps in a structure where inheritance flows.
    #[test]
    fn font_size_inherits_from_styled_ancestor() {
        let html = r#"<style>
            section { font-size: 24px; }
        </style><section><p>inherits</p></section>"#;
        let pages = plan(html);
        let line = pages[0].iter().find(|l| l.text == "inherits").unwrap();
        assert!(
            (line.font_size_pt - 24.0).abs() < 0.01,
            "expected 24pt inherited from section, got {}",
            line.font_size_pt
        );
    }

    /// Anonymous-block wrap: orphan text inside a block container that also
    /// has block children renders as its own paragraph (rather than being
    /// dropped as in 1.6b).
    #[test]
    fn anonymous_block_orphan_text_is_rendered() {
        let html = "<div>before<p>middle</p>after</div>";
        let pages = plan(html);
        let texts: Vec<&str> = pages[0].iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"before"), "expected 'before', got {texts:?}");
        assert!(texts.contains(&"middle"), "expected 'middle', got {texts:?}");
        assert!(texts.contains(&"after"), "expected 'after', got {texts:?}");
    }

    /// Anonymous paragraphs inherit their parent's resolved style, since
    /// they share the parent's `element_id` for cascade matching.
    #[test]
    fn anonymous_block_inherits_parent_style() {
        let html = r#"<style>
            #wrap { font-size: 24px; }
        </style><div id="wrap">orphan<p>child</p></div>"#;
        let pages = plan(html);
        let orphan = pages[0].iter().find(|l| l.text == "orphan").unwrap();
        assert!(
            (orphan.font_size_pt - 24.0).abs() < 0.01,
            "anonymous para should resolve to parent's style (24pt), got {}",
            orphan.font_size_pt
        );
    }

    // ---- Phase 1.7a: text colour flows through the planner.

    #[test]
    fn color_default_is_black() {
        let pages = plan("<p>x</p>");
        let line = &pages[0][0];
        assert_eq!(line.color, Color::BLACK);
    }

    #[test]
    fn author_color_overrides_default() {
        let pages = plan(r#"<style>p { color: #ff0000; }</style><p>x</p>"#);
        let line = pages[0].iter().find(|l| l.text == "x").unwrap();
        assert_eq!(line.color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn color_inherits_from_ancestor() {
        let pages = plan(
            r#"<style>section { color: rgb(0, 128, 255); }</style>
            <section><p>nested</p></section>"#,
        );
        let line = pages[0].iter().find(|l| l.text == "nested").unwrap();
        assert_eq!(line.color, Color::rgb(0, 128, 255));
    }

    #[test]
    fn anonymous_block_inherits_parent_color() {
        // The orphan run inside <div id=wrap> picks up the id rule's color
        // because anonymous paragraphs share the parent's element_id.
        let pages = plan(
            r#"<style>#wrap { color: green; }</style>
            <div id="wrap">orphan<p>child</p></div>"#,
        );
        let orphan = pages[0].iter().find(|l| l.text == "orphan").unwrap();
        assert_eq!(orphan.color, Color::rgb(0, 128, 0));
    }
}
