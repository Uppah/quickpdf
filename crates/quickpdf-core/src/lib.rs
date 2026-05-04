//! quickpdf-core — native HTML→PDF rendering core.
//!
//! Phase 1.1: HTML is parsed into a DOM but layout/paint are still stubs.
//! Real cascade + block/inline layout + text shaping land in Phases 1.2–1.5.

pub mod font;
pub mod image;
pub mod parse;
pub mod style;
pub mod text;

pub use parse::Document as ParsedDocument;

use krilla::Document;
use krilla::SerializeSettings;
use krilla::color::rgb as krilla_rgb;
use krilla::geom::{PathBuilder, Point, Rect, Size};
use krilla::paint::{Fill, Stroke};
use krilla::page::PageSettings;
use krilla::surface::Surface;
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

/// One placed box: rectangle to paint before any text on the page.
/// `fill` is `None` when the block has no `background-color`. `stroke`
/// is `Some((color, width_pt))` when the block has a non-zero
/// `border-width`. A box with neither set is never emitted.
#[derive(Debug, Clone)]
struct PlacedBox {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fill: Option<Color>,
    stroke: Option<(Color, f32)>,
}

/// One page's worth of paint operations. Boxes are painted before lines
/// so backgrounds sit behind their text content.
#[derive(Debug, Clone, Default)]
struct PagePlan {
    boxes: Vec<PlacedBox>,
    lines: Vec<PlacedLine>,
}

impl PagePlan {
    fn is_empty(&self) -> bool {
        self.boxes.is_empty() && self.lines.is_empty()
    }
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
    let inline_owned = parsed.inline_styles();
    let inline_map: style::InlineStyles<'_> = inline_owned
        .iter()
        .map(|(id, decls)| (*id, decls.as_slice()))
        .collect();

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
        &inline_map,
        content_width,
        MARGIN_PT,
        bottom_limit,
    )?;

    for page_plan in &pages {
        let mut page = document.start_page_with(PageSettings::new(size));
        {
            let mut surface = page.surface();
            // Paint background boxes first so backgrounds sit behind text.
            for b in &page_plan.boxes {
                paint_box(&mut surface, b);
            }

            let mut current_color: Option<Color> = None;
            for line in &page_plan.lines {
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

/// Paint a single `PlacedBox` on the given surface — fill first (if set),
/// then stroke (if set). Each operation builds its own rectangle path so
/// fill and stroke can use independent paint state without interfering.
fn paint_box(surface: &mut Surface, b: &PlacedBox) {
    if let Some(c) = b.fill {
        let Some(rect) = Rect::from_xywh(b.x, b.y, b.w, b.h) else {
            return;
        };
        let mut pb = PathBuilder::new();
        pb.push_rect(rect);
        let Some(path) = pb.finish() else {
            return;
        };
        surface.set_stroke(None);
        surface.set_fill(Some(Fill {
            paint: krilla_rgb::Color::new(c.r, c.g, c.b).into(),
            ..Fill::default()
        }));
        surface.draw_path(&path);
    }
    if let Some((c, w)) = b.stroke {
        // Inset the stroke rect by half the stroke width so the visible
        // edge stays inside the box's outer dimensions (PDF strokes
        // straddle the path).
        let inset = w * 0.5;
        let Some(rect) = Rect::from_xywh(
            b.x + inset,
            b.y + inset,
            (b.w - w).max(0.0),
            (b.h - w).max(0.0),
        ) else {
            return;
        };
        let mut pb = PathBuilder::new();
        pb.push_rect(rect);
        let Some(path) = pb.finish() else {
            return;
        };
        surface.set_fill(None);
        surface.set_stroke(Some(Stroke {
            paint: krilla_rgb::Color::new(c.r, c.g, c.b).into(),
            width: w,
            ..Stroke::default()
        }));
        surface.draw_path(&path);
    }
    // Reset stroke after a box so subsequent text isn't accidentally stroked.
    surface.set_stroke(None);
}

/// Plan page layout for tagged paragraphs. Each paragraph resolves its own
/// `BlockStyle` (UA defaults + matching author rules), drives font size,
/// indent, margins, padding, and box decoration, and flows lines into
/// pages, breaking at the bottom margin.
///
/// Phase 1.7b box-model rules:
///
/// - A block with `background-color` or non-zero `border-width` is treated
///   as a single positional unit ("paint as unit"): if it fits on the
///   current page, the box + its lines are placed without internal
///   pagination; if not, we try a fresh page; if it's still too tall, we
///   fall back to streaming text without box decoration (a documented
///   1.7b limitation — proper box pagination lands with Phase 4).
/// - A block without decoration uses the existing line-by-line streaming
///   path, with `padding-*` and `border-width` still shifting text origin
///   so geometry stays consistent.
fn plan_pages_styled(
    doc: &parse::Document,
    paragraphs: &[parse::TextBlock],
    user_rules: &[style::sheet::Rule],
    inline: &style::InlineStyles<'_>,
    content_width: f32,
    left_margin: f32,
    bottom_limit: f32,
) -> Result<Vec<PagePlan>, Error> {
    let top_baseline_for = |first_line_height: f32| MARGIN_PT + first_line_height;
    let page_content_height = bottom_limit - MARGIN_PT;

    let mut pages: Vec<PagePlan> = Vec::new();
    let mut current = PagePlan::default();
    let mut cursor_y: Option<f32> = None;

    for para in paragraphs {
        let style = match doc.element_for(para) {
            Some(elem) => style::resolve(elem, user_rules, inline),
            None => style::ua_style(&para.tag),
        };
        let font_size = DEFAULT_FONT_SIZE_PT * style.font_size_em;
        let line_height = font_size * DEFAULT_LINE_HEIGHT;
        let indent_pt = DEFAULT_FONT_SIZE_PT * style.indent_em;
        let pad_top = font_size * style.padding_top_em;
        let pad_right = font_size * style.padding_right_em;
        let pad_bot = font_size * style.padding_bottom_em;
        let pad_left = font_size * style.padding_left_em;
        let border_w = font_size * style.border_width_em;

        let box_left = left_margin + indent_pt;
        let box_width = (content_width - indent_pt).max(1.0);
        let text_x = box_left + border_w + pad_left;
        let text_width = (box_width - 2.0 * border_w - pad_left - pad_right).max(1.0);

        let metrics = text::TextMetrics::new(font::FALLBACK_TTF, font_size)
            .ok_or_else(|| Error::Pdf("could not measure font at requested size".into()))?;
        let lines = text::wrap_lines(&metrics, &para.text, text_width);
        if lines.is_empty() {
            continue;
        }

        // Top margin + paragraph gap; page break if it overflows.
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

        let has_decoration = style.background_color.is_some() || border_w > 0.0;
        let block_height_total =
            pad_top + lines.len() as f32 * line_height + pad_bot + 2.0 * border_w;

        // Try paint-as-unit if the block has visible decoration.
        let mut box_top_for_paint: Option<f32> = None;
        if has_decoration {
            let candidate_top = cursor_y.unwrap() - line_height;
            if candidate_top + block_height_total <= bottom_limit {
                box_top_for_paint = Some(candidate_top);
            } else if block_height_total <= page_content_height {
                pages.push(std::mem::take(&mut current));
                cursor_y = Some(top_baseline_for(line_height));
                box_top_for_paint = Some(MARGIN_PT);
            }
            // Else: too tall to fit on any page — stream without box.
        }

        if let Some(box_top) = box_top_for_paint {
            let stroke = if border_w > 0.0 {
                Some((style.border_color, border_w))
            } else {
                None
            };
            current.boxes.push(PlacedBox {
                x: box_left,
                y: box_top,
                w: box_width,
                h: block_height_total,
                fill: style.background_color,
                stroke,
            });
            let first_baseline = box_top + border_w + pad_top + line_height;
            for (i, line) in lines.into_iter().enumerate() {
                current.lines.push(PlacedLine {
                    y: first_baseline + i as f32 * line_height,
                    x: text_x,
                    font_size_pt: font_size,
                    text: line,
                    color: style.color,
                });
            }
            cursor_y = Some(box_top + block_height_total);
        } else {
            // Streaming path. Padding/border shift text origin; the box is
            // not painted (either because there is none, or because the
            // block is too tall to fit as a unit).
            if let Some(y) = cursor_y.as_mut() {
                *y += pad_top + border_w;
            }
            for line in lines {
                let y = cursor_y.expect("cursor_y is set by the time we reach lines");
                if y > bottom_limit {
                    pages.push(std::mem::take(&mut current));
                    cursor_y = Some(top_baseline_for(line_height));
                }
                let final_y = cursor_y.unwrap();
                current.lines.push(PlacedLine {
                    y: final_y,
                    x: text_x,
                    font_size_pt: font_size,
                    text: line,
                    color: style.color,
                });
                cursor_y = Some(final_y + line_height);
            }
            if let Some(y) = cursor_y.as_mut() {
                *y += pad_bot + border_w;
            }
        }

        if let Some(y) = cursor_y.as_mut() {
            *y += font_size * style.margin_bottom_em;
        }
    }

    if !current.is_empty() {
        pages.push(current);
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse some HTML, plan its pages, and return the per-page
    /// line lists. Used by the bulk of the existing tests, which only
    /// care about glyph placement.
    fn plan(html: &str) -> Vec<Vec<PlacedLine>> {
        plan_full(html).into_iter().map(|p| p.lines).collect()
    }

    /// Helper variant returning the full per-page plan (boxes + lines).
    /// Used by the Phase 1.7b box-model tests below.
    fn plan_full(html: &str) -> Vec<PagePlan> {
        let doc = parse::Document::parse(html);
        let paragraphs = doc.paragraphs();
        let rules = doc.user_stylesheet();
        let inline_owned = doc.inline_styles();
        let inline_map: style::InlineStyles<'_> = inline_owned
            .iter()
            .map(|(id, decls)| (*id, decls.as_slice()))
            .collect();
        plan_pages_styled(&doc, &paragraphs, &rules, &inline_map, 500.0, 36.0, 800.0).unwrap()
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

    // ---- Phase 1.7b: box-model paint pass.

    #[test]
    fn no_decoration_emits_no_box() {
        let pages = plan_full("<p>hello</p>");
        assert!(pages[0].boxes.is_empty(), "plain p should not emit a box");
        assert_eq!(pages[0].lines.len(), 1);
    }

    #[test]
    fn bg_color_emits_box_with_fill() {
        let pages = plan_full(
            "<style>p { background-color: yellow; }</style><p>x</p>",
        );
        assert_eq!(pages[0].boxes.len(), 1);
        let b = &pages[0].boxes[0];
        assert_eq!(b.fill, Some(Color::rgb(255, 255, 0)));
        assert_eq!(b.stroke, None);
        assert!(b.w > 0.0 && b.h > 0.0);
    }

    #[test]
    fn border_emits_box_with_stroke() {
        let pages = plan_full(
            "<style>p { border-width: 2px; border-color: red; \
                        border-style: solid; }</style><p>x</p>",
        );
        assert_eq!(pages[0].boxes.len(), 1);
        let b = &pages[0].boxes[0];
        assert_eq!(b.fill, None);
        let (col, w) = b.stroke.expect("expected stroke");
        assert_eq!(col, Color::rgb(255, 0, 0));
        assert!((w - 2.0).abs() < 0.01, "stroke width 2pt expected, got {w}");
    }

    #[test]
    fn border_style_none_suppresses_stroke() {
        let pages = plan_full(
            "<style>p { border-width: 4px; border-style: none; \
                        background-color: blue; }</style><p>x</p>",
        );
        // bg present so a box exists, but no stroke (border-style: none → width 0).
        assert_eq!(pages[0].boxes.len(), 1);
        assert_eq!(pages[0].boxes[0].stroke, None);
        assert_eq!(pages[0].boxes[0].fill, Some(Color::rgb(0, 0, 255)));
    }

    #[test]
    fn padding_shifts_text_origin_to_the_right() {
        let plain = plan_full("<p>x</p>");
        let padded = plan_full(
            "<style>p { padding-left: 24px; }</style><p>x</p>",
        );
        let plain_x = plain[0].lines[0].x;
        let padded_x = padded[0].lines[0].x;
        // 24px = 24pt at our 1pt/px conversion baseline.
        assert!(
            (padded_x - plain_x - 24.0).abs() < 0.5,
            "padding-left:24px should shift x by ~24pt (got {plain_x} vs {padded_x})"
        );
    }

    #[test]
    fn padding_top_pushes_text_down() {
        let plain = plan_full("<p>x</p>");
        let padded = plan_full(
            "<style>p { padding-top: 24px; background-color: yellow; }</style>\
             <p>x</p>",
        );
        let plain_y = plain[0].lines[0].y;
        let padded_y = padded[0].lines[0].y;
        assert!(
            padded_y > plain_y + 20.0,
            "padding-top should push first baseline downward \
             (plain={plain_y}, padded={padded_y})"
        );
    }

    #[test]
    fn box_geometry_matches_indent_and_content_width() {
        // Plain <p> at left_margin=36, content_width=500 (from plan_full's setup).
        let pages = plan_full(
            "<style>p { background-color: black; }</style><p>x</p>",
        );
        let b = &pages[0].boxes[0];
        assert!((b.x - 36.0).abs() < 0.01, "box x should be left margin: {}", b.x);
        assert!((b.w - 500.0).abs() < 0.01, "box w should be content width: {}", b.w);
    }

    // ---- Phase 1.7c: inline style="..." flows through the planner.

    #[test]
    fn inline_style_color_overrides_default() {
        let pages = plan(r#"<p style="color: red">x</p>"#);
        let line = pages[0].iter().find(|l| l.text == "x").unwrap();
        assert_eq!(line.color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn inline_style_font_size_overrides_default() {
        let pages = plan(r#"<p style="font-size: 24px">x</p>"#);
        let line = pages[0].iter().find(|l| l.text == "x").unwrap();
        assert!(
            (line.font_size_pt - 24.0).abs() < 0.01,
            "expected 24pt from inline style, got {}",
            line.font_size_pt
        );
    }

    #[test]
    fn inline_style_beats_author_rule() {
        let pages = plan(
            r#"<style>p { color: blue; }</style><p style="color: red">x</p>"#,
        );
        let line = pages[0].iter().find(|l| l.text == "x").unwrap();
        assert_eq!(line.color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn inline_padding_shorthand_expands_and_applies() {
        // Single-value padding shorthand → all four sides; padding-left
        // shifts text origin by the same amount as padding-left longhand.
        let plain = plan_full("<p>x</p>");
        let padded = plan_full(r#"<p style="padding: 24px">x</p>"#);
        let plain_x = plain[0].lines[0].x;
        let padded_x = padded[0].lines[0].x;
        assert!(
            (padded_x - plain_x - 24.0).abs() < 0.5,
            "inline padding:24px should shift x by ~24pt (got {plain_x} vs {padded_x})"
        );
    }

    #[test]
    fn rem_unit_resolves_against_base_font_size() {
        // 2rem at base 12pt → 24pt. Until :root cascade lands, rem == em.
        let pages = plan(r#"<p style="font-size: 2rem">x</p>"#);
        let line = pages[0].iter().find(|l| l.text == "x").unwrap();
        assert!(
            (line.font_size_pt - 24.0).abs() < 0.01,
            "expected 2rem → 24pt, got {}",
            line.font_size_pt
        );
    }
}
