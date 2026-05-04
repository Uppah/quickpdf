//! CSS stylesheet parsing — Slice A of the Phase 1.6b sprint.
//!
//! See `~/.claude/plans/cheerful-riding-castle.md` and the coordinator's
//! design notes for the contract this slice must satisfy.
//!
//! Implementation note: this is a small hand-written tokenizer rather than
//! a `cssparser`-based parser. The grammar we accept is intentionally tiny
//! (`selector { name: value; ... }`), and the malformed-input contract
//! ("never panic, never error, just skip") is easier to satisfy with a
//! straight character-stream walk than with cssparser's typed AST. No new
//! external dependency needed.

use scraper::{ElementRef, Node};

/// One CSS rule: selector list + declaration block.
#[derive(Debug, Clone)]
pub struct Rule {
    /// Raw selector text exactly as written, e.g. "div p, .x". Slice B
    /// splits on commas and parses each selector itself.
    pub selector_text: String,
    pub declarations: Vec<Declaration>,
    /// Source order index, 0-based across the full stylesheet.
    pub source_order: usize,
}

/// One `name: value;` pair. `value` is preserved as the raw substring
/// between `:` and `;`, trimmed. Slice C parses it into typed values.
#[derive(Debug, Clone)]
pub struct Declaration {
    pub name: String,  // lowercased
    pub value: String, // trimmed, original casing
    /// True iff the declaration ended in a CSS `!important` marker (any
    /// whitespace between `!` and `important`, case-insensitive on
    /// `important`). Stripping is repeated, so `red !important !important`
    /// produces `value = "red"`, `important = true`. Default: `false`.
    pub important: bool,
}

/// One `@font-face` block captured from author CSS. Phase 2b's font
/// registry walks these to register web fonts; the registry parses
/// the `font-family` and `src` descriptors itself.
///
/// `declarations` includes every declaration inside the block, normalised
/// the same way qualified-rule declarations are: comments stripped,
/// shorthand expanded, `!important` flag honoured. Non-`font-family` /
/// non-`src` descriptors (e.g. `font-weight`, `unicode-range`) are
/// preserved for forward compatibility but ignored by Phase 2b.
#[derive(Debug, Clone)]
pub struct FontFace {
    pub declarations: Vec<Declaration>,
    /// Source order across the full stylesheet (shared numbering with
    /// `Rule.source_order`). Used by the registry for last-wins
    /// disambiguation when two `@font-face` blocks declare the same
    /// family name.
    pub source_order: usize,
}

/// Parsed stylesheet: qualified rules and `@font-face` blocks. Other
/// at-rules (`@media`, `@import`, `@keyframes`, …) are still dropped
/// silently by `skip_at_rule`.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    pub font_faces: Vec<FontFace>,
}

/// Parse the body of an inline `style="..."` attribute into a flat list of
/// declarations. Behaves identically to a `<style>` block's body: comments
/// are stripped, declarations are split on top-level `;`, and `!important`
/// is honoured. Used by `parse::Document::inline_styles`.
pub fn parse_inline_declarations(source: &str) -> Vec<Declaration> {
    parse_declaration_block(source)
}

/// Parse a stylesheet source string into a `Stylesheet` aggregate
/// containing both qualified rules and `@font-face` blocks. Always
/// returns — malformed rules are silently skipped (browsers do the
/// same). Phase 2b's font registry consumes the `font_faces` field;
/// the cascade consumes `rules` exactly as before.
pub fn parse_stylesheet_full(source: &str) -> Stylesheet {
    let bytes = source.as_bytes();
    let mut pos = 0;
    let mut rules: Vec<Rule> = Vec::new();
    let mut font_faces: Vec<FontFace> = Vec::new();
    let mut order: usize = 0;

    while pos < bytes.len() {
        pos = skip_ws_and_comments(bytes, pos);
        if pos >= bytes.len() {
            break;
        }

        // At-rule? `@font-face` is captured; everything else is dropped.
        if bytes[pos] == b'@' {
            match try_capture_font_face(source, bytes, pos, order) {
                Some((face, next_pos)) => {
                    font_faces.push(face);
                    pos = next_pos;
                    order += 1;
                }
                None => {
                    pos = skip_at_rule(bytes, pos);
                }
            }
            continue;
        }

        // Stray `}` at top level — skip it and keep going.
        if bytes[pos] == b'}' {
            pos += 1;
            continue;
        }

        // Otherwise: read a qualified rule (selector { decls }).
        match read_qualified_rule(source, bytes, pos) {
            Some((rule_opt, next_pos)) => {
                if let Some((selector_text, declarations)) = rule_opt {
                    rules.push(Rule {
                        selector_text,
                        declarations,
                        source_order: order,
                    });
                    order += 1;
                }
                pos = next_pos;
            }
            None => {
                // Couldn't find a `{` matching the prelude — input ran out
                // without a block opener. Treat the rest of the stream as
                // malformed and stop.
                break;
            }
        }
    }

    Stylesheet { rules, font_faces }
}

/// Back-compat wrapper. Existing callers and ~30 unit tests in this
/// file consume `Vec<Rule>` directly; this preserves their contract.
/// New callers should prefer `parse_stylesheet_full` for the aggregate.
pub fn parse_stylesheet(source: &str) -> Vec<Rule> {
    parse_stylesheet_full(source).rules
}

/// Try to recognise an `@font-face` rule starting at `bytes[pos] == b'@'`.
/// Returns `Some((face, next_pos))` on a successful capture, or `None`
/// if this isn't an `@font-face` rule — in which case the caller falls
/// back to `skip_at_rule` to consume it.
///
/// Recognition: the byte after `@` plus the next 9 ASCII bytes are
/// matched case-insensitively against `"font-face"`. Whitespace and
/// comments may follow before the `{`. If the keyword matches but no
/// `{` follows (truncated input, syntax error), we still return None
/// so the caller's `skip_at_rule` consumes the malformed remainder.
fn try_capture_font_face(
    source: &str,
    bytes: &[u8],
    pos: usize,
    source_order: usize,
) -> Option<(FontFace, usize)> {
    debug_assert_eq!(bytes.get(pos), Some(&b'@'));

    // 1. Match the keyword (case-insensitive) at bytes[pos+1..pos+10].
    const KEYWORD: &[u8] = b"font-face";
    if pos + 1 + KEYWORD.len() > bytes.len() {
        return None;
    }
    for (i, want) in KEYWORD.iter().enumerate() {
        let got = bytes[pos + 1 + i];
        if !got.eq_ignore_ascii_case(want) {
            return None;
        }
    }

    // 2. After the keyword, the next char must be whitespace, `{`, or
    //    comment-start. Anything else (e.g. `@font-face-extra`) means
    //    we matched a prefix of a different at-rule — bail out.
    let after_kw = pos + 1 + KEYWORD.len();
    if after_kw < bytes.len() {
        let c = bytes[after_kw];
        let ok = is_css_ws(c) || c == b'{' || (c == b'/' && bytes.get(after_kw + 1) == Some(&b'*'));
        if !ok {
            return None;
        }
    }

    // 3. Skip whitespace + comments to find the `{`.
    let mut p = skip_ws_and_comments(bytes, after_kw);
    if p >= bytes.len() || bytes[p] != b'{' {
        // Keyword matched but no opening brace before EOF/garbage. Caller's
        // `skip_at_rule` fallback handles the cleanup.
        return None;
    }

    // 4. Walk the balanced block; capture the body for declaration parsing.
    let block_open = p;
    let block_close_plus_one = skip_balanced_block(bytes, block_open);
    // Reuse the same closer-validation that read_qualified_rule uses: if
    // the block never closed, we treat the whole stream as truncated and
    // bail (caller's skip_at_rule will eat to EOF too).
    if block_close_plus_one == bytes.len() && !ends_with_close_brace(bytes, block_close_plus_one) {
        return None;
    }
    p = block_close_plus_one;

    let body_start = block_open + 1;
    let body_end = block_close_plus_one.saturating_sub(1).max(body_start);
    let body = &source[body_start..body_end];
    let declarations = parse_declaration_block(body);

    Some((
        FontFace {
            declarations,
            source_order,
        },
        p,
    ))
}

/// Walk the parsed HTML and return the concatenated text content of every
/// `<style>` element in document order, joined with `\n`.
pub fn collect_style_blocks(doc: &crate::parse::Document) -> String {
    let mut blocks: Vec<String> = Vec::new();
    let root = doc.html.root_element();
    visit_collect_styles(root, &mut blocks);
    blocks.join("\n")
}

// ---------------------------------------------------------------------------
// Internal: tokenizer-ish helpers.
// ---------------------------------------------------------------------------

/// Advance past ASCII whitespace and `/* ... */` comments. Unterminated
/// comments swallow to EOF (matches browser behaviour).
fn skip_ws_and_comments(bytes: &[u8], mut pos: usize) -> usize {
    loop {
        // Whitespace.
        while pos < bytes.len() && is_css_ws(bytes[pos]) {
            pos += 1;
        }
        // Comment.
        if pos + 1 < bytes.len() && bytes[pos] == b'/' && bytes[pos + 1] == b'*' {
            pos += 2;
            while pos + 1 < bytes.len() && !(bytes[pos] == b'*' && bytes[pos + 1] == b'/') {
                pos += 1;
            }
            // Either we hit `*/` or ran out. In either case, advance.
            pos = (pos + 2).min(bytes.len());
            continue;
        }
        break;
    }
    pos
}

#[inline]
fn is_css_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

/// Skip an at-rule starting at `bytes[pos] == b'@'`. Two shapes:
///   - block at-rule (`@media ... { ... }`): skip prelude, then balanced block.
///   - statement at-rule (`@import url(...);`): skip until the next `;`.
/// Returns the position after the at-rule (or end of input).
fn skip_at_rule(bytes: &[u8], mut pos: usize) -> usize {
    // We're sitting on '@'. Walk forward until we see `{`, `;`, or EOF.
    while pos < bytes.len() {
        let b = bytes[pos];
        if b == b'{' {
            // Block at-rule. Skip the balanced block.
            return skip_balanced_block(bytes, pos);
        }
        if b == b';' {
            return pos + 1;
        }
        // Skip strings so a `;` inside `url("a;b")` doesn't fool us.
        if b == b'"' || b == b'\'' {
            pos = skip_string(bytes, pos);
            continue;
        }
        if b == b'/' && pos + 1 < bytes.len() && bytes[pos + 1] == b'*' {
            pos = skip_ws_and_comments(bytes, pos);
            continue;
        }
        pos += 1;
    }
    pos
}

/// Starting at `bytes[pos] == b'{'`, return the position one past the
/// matching `}`. Tolerant of nesting, strings, and comments. If the block
/// never closes, returns `bytes.len()`.
fn skip_balanced_block(bytes: &[u8], mut pos: usize) -> usize {
    debug_assert_eq!(bytes.get(pos), Some(&b'{'));
    let mut depth: i32 = 0;
    while pos < bytes.len() {
        let b = bytes[pos];
        match b {
            b'{' => {
                depth += 1;
                pos += 1;
            }
            b'}' => {
                depth -= 1;
                pos += 1;
                if depth == 0 {
                    return pos;
                }
            }
            b'"' | b'\'' => {
                pos = skip_string(bytes, pos);
            }
            b'/' if pos + 1 < bytes.len() && bytes[pos + 1] == b'*' => {
                pos = skip_ws_and_comments(bytes, pos);
            }
            _ => pos += 1,
        }
    }
    bytes.len()
}

/// Skip a single CSS string starting at `bytes[pos] in {'"','\''}`. Honors
/// `\` escapes (so `"a\"b"` is one string). Unterminated strings swallow
/// to EOF — browsers also recover by treating the rest as part of the string.
fn skip_string(bytes: &[u8], mut pos: usize) -> usize {
    let quote = bytes[pos];
    pos += 1;
    while pos < bytes.len() {
        let b = bytes[pos];
        if b == b'\\' && pos + 1 < bytes.len() {
            pos += 2;
            continue;
        }
        if b == quote {
            return pos + 1;
        }
        pos += 1;
    }
    pos
}

/// Try to read one qualified rule starting at `pos`. Returns:
///   - `Some((Some((selector, decls)), next_pos))` — well-formed rule.
///   - `Some((None, next_pos))` — rule was malformed but we recovered to a
///     known boundary (consumed an unbalanced block, etc.); caller continues.
///   - `None` — couldn't find a `{` before EOF, so the input is truncated
///     and the caller should stop. (Matches the "drop unterminated rule"
///     behaviour the spec asks for at top level.)
#[allow(clippy::type_complexity)]
fn read_qualified_rule(
    source: &str,
    bytes: &[u8],
    start: usize,
) -> Option<(Option<(String, Vec<Declaration>)>, usize)> {
    // 1. Walk forward to the prelude/block boundary `{`. Strings are honored
    //    so a `{` inside `[attr="{"]` doesn't fool us. A stray `;` here
    //    technically makes the prelude a malformed declaration list at top
    //    level — recover by dropping up to the next `;` or `}`.
    let prelude_start = start;
    let mut pos = start;
    while pos < bytes.len() {
        let b = bytes[pos];
        match b {
            b'{' => break,
            b'"' | b'\'' => {
                pos = skip_string(bytes, pos);
            }
            b'/' if pos + 1 < bytes.len() && bytes[pos + 1] == b'*' => {
                pos = skip_ws_and_comments(bytes, pos);
            }
            b'}' | b';' => {
                // Stray closer / semicolon before any `{` — selector is bare
                // garbage. Drop it and tell the caller to keep scanning.
                return Some((None, pos + 1));
            }
            _ => pos += 1,
        }
    }
    if pos >= bytes.len() {
        // No `{` ever found → truncated rule.
        return None;
    }

    let prelude_end = pos;
    let selector_text = source[prelude_start..prelude_end].trim().to_string();

    // 2. Block: from `{` to matching `}`.
    let block_open = pos;
    let block_close_plus_one = skip_balanced_block(bytes, block_open);
    // If the block never closed, treat as malformed and stop entirely so the
    // remainder of the stylesheet doesn't get accidentally parsed as decls.
    if block_close_plus_one == bytes.len() {
        // Was there actually a `}` or did we hit EOF? If we exited because
        // depth reached zero, block_close_plus_one points one past `}`;
        // otherwise it equals bytes.len() AND bytes doesn't end with `}`.
        if !ends_with_close_brace(bytes, block_close_plus_one) {
            return None;
        }
    }
    let body_start = block_open + 1;
    let body_end = block_close_plus_one.saturating_sub(1); // strip `}`
    let body_end = body_end.max(body_start);
    let body = &source[body_start..body_end];

    // 3. Empty selector → drop the rule but keep parsing the rest.
    if selector_text.is_empty() {
        return Some((None, block_close_plus_one));
    }

    let declarations = parse_declaration_block(body);
    Some((
        Some((selector_text, declarations)),
        block_close_plus_one,
    ))
}

/// Returns true iff position `p` (one past `}`) actually had a `}` at p-1.
/// In our flow, `skip_balanced_block` returns `bytes.len()` for an
/// unterminated block, and `p` for a terminated one. So checking byte p-1
/// disambiguates.
fn ends_with_close_brace(bytes: &[u8], p: usize) -> bool {
    p > 0 && bytes.get(p - 1) == Some(&b'}')
}

/// Parse a declaration block body (everything between `{` and `}`) into a
/// list of declarations. Splits on `;` (top-level — strings/comments are
/// honored), then for each piece splits on the first `:`.
fn parse_declaration_block(body: &str) -> Vec<Declaration> {
    // Strip `/* ... */` comments first so they can't interfere with the
    // name/value parse below. Strings are honored so `content: "/* not */"`
    // survives intact.
    let body = strip_comments(body);
    let mut out = Vec::new();
    for piece in split_top_level_semicolons(&body) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let Some(colon_idx) = piece.find(':') else {
            continue;
        };
        let name = piece[..colon_idx].trim().to_ascii_lowercase();
        let raw_value = piece[colon_idx + 1..].trim();
        let (value, important) = strip_important(raw_value);
        if name.is_empty() || value.is_empty() {
            continue;
        }
        // Reject names that contain whitespace or look obviously bogus —
        // helps drop garbage like "foo bar: baz". A valid CSS property is
        // an ident: `[a-zA-Z_-][a-zA-Z0-9_-]*`. We're permissive on the
        // first char (already lowercased) but bail on internal whitespace.
        if name.chars().any(|c| c.is_whitespace()) {
            continue;
        }
        out.push(Declaration {
            name,
            value,
            important,
        });
    }
    // Phase 1.7c: expand `padding`, `margin`, and `border` shorthands into
    // their per-side / per-component longhands so the cascade stays uniform.
    expand_shorthands(out)
}

// ---------------------------------------------------------------------------
// Phase 1.7c: shorthand expansion.
//
// `padding` / `margin` / `border` are recognised here. Each shorthand is
// replaced by its longhands; unparseable shorthands are silently dropped.
// Each generated longhand inherits the original shorthand's `important`
// flag. The expansion runs after the initial declaration parse so callers
// of `parse_declaration_block` (and the public inline-style wrapper) only
// ever see longhands.
// ---------------------------------------------------------------------------

/// Replace any shorthand declaration in `decls` with its longhand expansion.
/// Non-shorthand declarations pass through unchanged. An unparseable
/// shorthand is dropped entirely (consistent with how malformed input
/// disappears elsewhere in this file).
fn expand_shorthands(decls: Vec<Declaration>) -> Vec<Declaration> {
    let mut out: Vec<Declaration> = Vec::with_capacity(decls.len());
    for decl in decls {
        match decl.name.as_str() {
            "padding" => {
                if let Some(longhands) =
                    expand_padding_or_margin_shorthand("padding", &decl.value, decl.important)
                {
                    out.extend(longhands);
                }
            }
            "margin" => {
                if let Some(longhands) =
                    expand_padding_or_margin_shorthand("margin", &decl.value, decl.important)
                {
                    out.extend(longhands);
                }
            }
            "border" => {
                if let Some(longhands) = expand_border_shorthand(&decl.value, decl.important) {
                    out.extend(longhands);
                }
            }
            _ => out.push(decl),
        }
    }
    out
}

/// Split a shorthand value on whitespace at parenthesis depth 0. So
/// `rgb(0, 0, 0) solid 1px` produces three components, not five.
///
/// Strings (`"..."`/`'...'`) appear in zero real-world shorthand values,
/// so this implementation deliberately treats quote bytes as ordinary
/// characters; the parser elsewhere in this file already handles strings
/// in declaration values. If a future shorthand grows a string component,
/// extend this helper.
fn split_shorthand_values(value: &str) -> Vec<String> {
    let bytes = value.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut start: Option<usize> = None;
    let mut pos = 0;
    while pos < bytes.len() {
        let b = bytes[pos];
        if b == b'(' {
            depth += 1;
            if start.is_none() {
                start = Some(pos);
            }
            pos += 1;
        } else if b == b')' {
            if depth > 0 {
                depth -= 1;
            }
            if start.is_none() {
                start = Some(pos);
            }
            pos += 1;
        } else if depth == 0 && is_css_ws(b) {
            if let Some(s) = start.take() {
                out.push(value[s..pos].to_string());
            }
            pos += 1;
        } else {
            if start.is_none() {
                start = Some(pos);
            }
            pos += 1;
        }
    }
    if let Some(s) = start.take() {
        out.push(value[s..].to_string());
    }
    out
}

/// Expand a `padding` or `margin` shorthand value into 4 longhand
/// declarations (`<prefix>-top`, `-right`, `-bottom`, `-left`). Returns
/// `None` for >4 components or 0 components. The cascade rejects bad
/// length tokens later, so the expansion does not validate values.
fn expand_padding_or_margin_shorthand(
    prefix: &str,
    value: &str,
    important: bool,
) -> Option<Vec<Declaration>> {
    let parts = split_shorthand_values(value);
    let (t, r, b, l): (&str, &str, &str, &str) = match parts.len() {
        1 => (
            parts[0].as_str(),
            parts[0].as_str(),
            parts[0].as_str(),
            parts[0].as_str(),
        ),
        2 => (
            parts[0].as_str(),
            parts[1].as_str(),
            parts[0].as_str(),
            parts[1].as_str(),
        ),
        3 => (
            parts[0].as_str(),
            parts[1].as_str(),
            parts[2].as_str(),
            parts[1].as_str(),
        ),
        4 => (
            parts[0].as_str(),
            parts[1].as_str(),
            parts[2].as_str(),
            parts[3].as_str(),
        ),
        _ => return None,
    };
    let mk = |side: &str, v: &str| Declaration {
        name: format!("{prefix}-{side}"),
        value: v.to_string(),
        important,
    };
    Some(vec![
        mk("top", t),
        mk("right", r),
        mk("bottom", b),
        mk("left", l),
    ])
}

/// Expand a `border` shorthand value (e.g. `1px solid red`) into up to
/// three longhand declarations (`border-width`, `border-style`,
/// `border-color`). Components may appear in any order. Returns `None`
/// only if no recognised component is present (so the whole shorthand is
/// dropped).
///
/// Recognition rules:
///   - A token that ends with a known length unit (`px`, `pt`, `em`,
///     `rem`, `%`) is treated as `border-width`. Bare `0` also counts.
///   - A token equal to `solid`, `none`, or `hidden` is treated as
///     `border-style`.
///   - Anything else (including `rgb(...)` blobs) tentatively becomes
///     `border-color`. The cascade's `parse_color` rejects garbage, so an
///     unparseable colour just no-ops at apply time.
///
/// At most one of each component is emitted; later occurrences of the same
/// component replace earlier ones (matches CSS shorthand semantics where
/// the rightmost token wins).
fn expand_border_shorthand(value: &str, important: bool) -> Option<Vec<Declaration>> {
    let parts = split_shorthand_values(value);
    if parts.is_empty() {
        return None;
    }
    let mut width: Option<String> = None;
    let mut style: Option<String> = None;
    let mut color: Option<String> = None;
    for part in &parts {
        if looks_like_border_length(part) {
            width = Some(part.clone());
        } else if matches!(part.as_str(), "solid" | "none" | "hidden") {
            style = Some(part.clone());
        } else {
            color = Some(part.clone());
        }
    }
    // A "recognised" component is a length or a style keyword. A bare colour
    // word ("red", "foo", "#abc") on its own is too ambiguous — without any
    // disambiguating length-or-style token, the whole shorthand is dropped.
    if width.is_none() && style.is_none() {
        return None;
    }
    let mut out: Vec<Declaration> = Vec::new();
    if let Some(v) = width {
        out.push(Declaration {
            name: "border-width".to_string(),
            value: v,
            important,
        });
    }
    if let Some(v) = style {
        out.push(Declaration {
            name: "border-style".to_string(),
            value: v,
            important,
        });
    }
    if let Some(v) = color {
        out.push(Declaration {
            name: "border-color".to_string(),
            value: v,
            important,
        });
    }
    Some(out)
}

/// Heuristic: does this token look like a CSS length suitable for
/// `border-width`? Mirrors the unit set accepted by
/// `cascade::parse_length_em` (`px`, `pt`, `em`, `rem`, `%`). A bare `0`
/// is also treated as a length so `border: 0 solid` works.
fn looks_like_border_length(token: &str) -> bool {
    let t = token.trim();
    if t.is_empty() {
        return false;
    }
    if t == "0" {
        return true;
    }
    for unit in &["rem", "px", "pt", "em", "%"] {
        if let Some(n) = t.strip_suffix(unit) {
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-') {
                return true;
            }
        }
    }
    false
}

/// Strip a trailing CSS `!important` marker from `value`, repeatedly.
/// Returns `(stripped_value, important)`.
///
/// What counts as a trailing `!important`:
/// - The literal byte `!`,
/// - then zero or more ASCII whitespace,
/// - then the ASCII letters `important` in any case,
/// - then optional trailing whitespace,
/// - at the **end** of the value.
///
/// Repeated markers are all stripped: `red !important !important` →
/// `("red", true)`. If the marker is not at the very end (e.g.
/// `red !important extra`), the value is preserved verbatim.
fn strip_important(value: &str) -> (String, bool) {
    let mut current: String = value.to_string();
    let mut important = false;
    loop {
        // Trim trailing whitespace.
        let trimmed = current.trim_end();
        // Check for case-insensitive trailing "important" (9 letters).
        if trimmed.len() < 9 {
            current = trimmed.to_string();
            break;
        }
        let tail = &trimmed[trimmed.len() - 9..];
        if !tail.eq_ignore_ascii_case("important") {
            current = trimmed.to_string();
            break;
        }
        // Strip the 9-letter "important" suffix.
        let after_word = &trimmed[..trimmed.len() - 9];
        // Trim trailing whitespace between `!` and `important`.
        let after_ws = after_word.trim_end();
        // Must end with `!`.
        if !after_ws.ends_with('!') {
            // Tail looks like "important" but no `!` — not a marker.
            current = trimmed.to_string();
            break;
        }
        // Strip the `!` and mark important.
        let stripped = &after_ws[..after_ws.len() - 1];
        important = true;
        current = stripped.to_string();
        // Loop to handle repeated markers.
    }
    let final_value = current.trim().to_string();
    (final_value, important)
}

/// Remove `/* ... */` comments from a string. Strings are honored so a
/// comment-looking sequence inside a quoted value is preserved.
fn strip_comments(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while pos < bytes.len() {
        let b = bytes[pos];
        if b == b'/' && pos + 1 < bytes.len() && bytes[pos + 1] == b'*' {
            pos += 2;
            while pos + 1 < bytes.len() && !(bytes[pos] == b'*' && bytes[pos + 1] == b'/') {
                pos += 1;
            }
            // Either `*/` or EOF; advance past it.
            pos = (pos + 2).min(bytes.len());
            // Insert a single space so e.g. `a/* x */b` doesn't become `ab`.
            out.push(' ');
            continue;
        }
        if b == b'"' || b == b'\'' {
            let end = skip_string(bytes, pos);
            // Safe: skip_string respects char boundaries (only walks
            // bytes equal to the quote/`\\`, and indices land at byte
            // boundaries because we entered at a quote).
            out.push_str(&s[pos..end]);
            pos = end;
            continue;
        }
        // Push the next character (handles multi-byte UTF-8 correctly by
        // using the str slice rather than indexing bytes directly).
        let ch_end = next_char_boundary(s, pos);
        out.push_str(&s[pos..ch_end]);
        pos = ch_end;
    }
    out
}

/// Return the byte index of the character boundary immediately after `pos`
/// in `s`. Assumes `pos` is itself at a char boundary and `pos < s.len()`.
fn next_char_boundary(s: &str, pos: usize) -> usize {
    let mut i = pos + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Split a declaration-list body on `;`, respecting strings and `/*...*/`.
/// Returns string slices into the input.
fn split_top_level_semicolons(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut pos = 0;
    let mut paren_depth: i32 = 0;
    while pos < bytes.len() {
        let b = bytes[pos];
        match b {
            b';' if paren_depth == 0 => {
                out.push(&body[start..pos]);
                pos += 1;
                start = pos;
            }
            // Phase 2b: `;` is allowed inside `(...)` because the
            // `data:font/ttf;base64,...` URL syntax embeds a literal
            // semicolon. The pre-2b parser ignored paren depth and
            // truncated such declaration values; tracking it here is
            // a backwards-compatible fix.
            b'(' => {
                paren_depth += 1;
                pos += 1;
            }
            b')' => {
                if paren_depth > 0 {
                    paren_depth -= 1;
                }
                pos += 1;
            }
            b'"' | b'\'' => {
                pos = skip_string(bytes, pos);
            }
            b'/' if pos + 1 < bytes.len() && bytes[pos + 1] == b'*' => {
                pos = skip_ws_and_comments(bytes, pos);
            }
            _ => pos += 1,
        }
    }
    if start < bytes.len() {
        out.push(&body[start..]);
    }
    out
}

// ---------------------------------------------------------------------------
// Internal: <style> collection.
// ---------------------------------------------------------------------------

fn visit_collect_styles(elem: ElementRef<'_>, out: &mut Vec<String>) {
    let name = elem.value().name();
    if name.eq_ignore_ascii_case("style") {
        // Concatenate this element's direct text content (browsers treat the
        // contents of <style> as opaque text, so we just glue text-node
        // children together verbatim — no whitespace collapsing).
        let mut buf = String::new();
        for child in elem.children() {
            if let Node::Text(t) = child.value() {
                buf.push_str(&t.text);
            }
        }
        out.push(buf);
        // Don't recurse; <style> shouldn't have element children we care about.
        return;
    }
    for child in elem.children() {
        if let Some(child_elem) = ElementRef::wrap(child) {
            visit_collect_styles(child_elem, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_rule() {
        let rules = parse_stylesheet("p { color: red; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selector_text, "p");
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].name, "color");
        assert_eq!(rules[0].declarations[0].value, "red");
        assert_eq!(rules[0].source_order, 0);
    }

    #[test]
    fn parses_multiple_rules_in_order() {
        let rules = parse_stylesheet("h1 { font-size: 24px; } p { font-size: 12px; }");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].selector_text, "h1");
        assert_eq!(rules[0].source_order, 0);
        assert_eq!(rules[1].selector_text, "p");
        assert_eq!(rules[1].source_order, 1);
    }

    #[test]
    fn groups_multiple_declarations() {
        let rules = parse_stylesheet("p { font-size: 14px; margin-top: 10px; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 2);
        assert_eq!(rules[0].declarations[0].name, "font-size");
        assert_eq!(rules[0].declarations[0].value, "14px");
        assert_eq!(rules[0].declarations[1].name, "margin-top");
        assert_eq!(rules[0].declarations[1].value, "10px");
    }

    #[test]
    fn lowercases_property_names_preserves_value_case() {
        let rules = parse_stylesheet("p { Font-Size: 14PX; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].name, "font-size");
        assert_eq!(rules[0].declarations[0].value, "14PX");
    }

    #[test]
    fn skips_at_rules_and_malformed() {
        let rules = parse_stylesheet(
            "@media print { p { x: y; } } p { color: red; } broken { ;",
        );
        assert_eq!(rules.len(), 1, "got rules: {rules:#?}");
        assert_eq!(rules[0].selector_text, "p");
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].name, "color");
        assert_eq!(rules[0].declarations[0].value, "red");
    }

    #[test]
    fn collect_style_blocks_concatenates_in_order() {
        let doc = crate::parse::Document::parse(
            "<head><style>a{x:1;}</style></head><body><style>b{y:2;}</style><p>x</p></body>",
        );
        let collected = collect_style_blocks(&doc);
        assert_eq!(collected, "a{x:1;}\nb{y:2;}");
    }

    // ---- A few extra sanity checks (not in the required list but they
    // protect the malformed-input contract from drifting). ----

    #[test]
    fn empty_input_yields_no_rules() {
        assert!(parse_stylesheet("").is_empty());
        assert!(parse_stylesheet("   \n\t  ").is_empty());
    }

    #[test]
    fn handles_comments() {
        let rules = parse_stylesheet("/* hi */ p /* x */ { /* y */ color: red; /* z */ }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].name, "color");
        assert_eq!(rules[0].declarations[0].value, "red");
    }

    #[test]
    fn declaration_without_colon_is_dropped() {
        let rules = parse_stylesheet("p { color red; font-size: 12px; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].name, "font-size");
    }

    #[test]
    fn empty_value_is_dropped() {
        let rules = parse_stylesheet("p { color: ; font-size: 12px; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].name, "font-size");
    }

    #[test]
    fn selector_list_kept_verbatim() {
        // Slice B parses the comma-split — we just preserve the text.
        let rules = parse_stylesheet("h1, h2, .x { color: red; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selector_text, "h1, h2, .x");
    }

    #[test]
    fn missing_final_semicolon_is_ok() {
        let rules = parse_stylesheet("p { color: red }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].value, "red");
    }

    #[test]
    fn collect_style_blocks_returns_empty_when_no_styles() {
        let doc = crate::parse::Document::parse("<p>hi</p>");
        assert!(collect_style_blocks(&doc).is_empty());
    }

    // ---- Phase 1.6c Slice B: !important parsing tests. ----

    #[test]
    fn important_basic_sets_flag_and_strips() {
        let rules = parse_stylesheet("p { color: red !important; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].name, "color");
        assert_eq!(rules[0].declarations[0].value, "red");
        assert!(rules[0].declarations[0].important);
    }

    #[test]
    fn important_no_space_between_bang_and_word() {
        let rules = parse_stylesheet("p { color: red!important; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].value, "red");
        assert!(rules[0].declarations[0].important);
    }

    #[test]
    fn important_with_whitespace_between_bang_and_word() {
        let rules = parse_stylesheet("p { color: red ! important; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].value, "red");
        assert!(rules[0].declarations[0].important);
    }

    #[test]
    fn important_uppercase_keyword() {
        let rules = parse_stylesheet("p { color: red !IMPORTANT; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].value, "red");
        assert!(rules[0].declarations[0].important);
    }

    #[test]
    fn important_mixed_case_keyword() {
        let rules = parse_stylesheet("p { color: red !Important; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].value, "red");
        assert!(rules[0].declarations[0].important);
    }

    #[test]
    fn important_repeated_strips_all() {
        let rules = parse_stylesheet("p { color: red !important !important !important; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].value, "red");
        assert!(rules[0].declarations[0].important);
    }

    #[test]
    fn important_word_alone_is_not_a_marker() {
        let rules = parse_stylesheet("p { color: important; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].value, "important");
        assert!(!rules[0].declarations[0].important);
    }

    #[test]
    fn important_marker_must_be_trailing() {
        let rules = parse_stylesheet("p { color: red !important extra; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].value, "red !important extra");
        assert!(!rules[0].declarations[0].important);
    }

    #[test]
    fn important_only_value_is_dropped_as_empty() {
        // `!important` with nothing else strips to "" which triggers the
        // empty-value drop path.
        let rules = parse_stylesheet("p { color: !important; font-size: 12px; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].name, "font-size");
    }

    #[test]
    fn important_default_false_for_plain_decl() {
        let rules = parse_stylesheet("p { color: red; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 1);
        assert_eq!(rules[0].declarations[0].value, "red");
        assert!(!rules[0].declarations[0].important);
    }

    #[test]
    fn important_works_with_multiple_decls_in_block() {
        let rules = parse_stylesheet(
            "p { color: red !important; font-size: 12px; margin-top: 4px !IMPORTANT; }",
        );
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].declarations.len(), 3);
        assert_eq!(rules[0].declarations[0].name, "color");
        assert_eq!(rules[0].declarations[0].value, "red");
        assert!(rules[0].declarations[0].important);
        assert_eq!(rules[0].declarations[1].name, "font-size");
        assert_eq!(rules[0].declarations[1].value, "12px");
        assert!(!rules[0].declarations[1].important);
        assert_eq!(rules[0].declarations[2].name, "margin-top");
        assert_eq!(rules[0].declarations[2].value, "4px");
        assert!(rules[0].declarations[2].important);
    }

    #[test]
    fn strip_important_unit_table() {
        use super::strip_important;
        // The full edge-case behaviour table from the contract.
        assert_eq!(strip_important("red"), ("red".to_string(), false));
        assert_eq!(strip_important("red !important"), ("red".to_string(), true));
        assert_eq!(strip_important("red!important"), ("red".to_string(), true));
        assert_eq!(strip_important("red ! important"), ("red".to_string(), true));
        assert_eq!(strip_important("red !IMPORTANT"), ("red".to_string(), true));
        assert_eq!(strip_important("red !Important"), ("red".to_string(), true));
        assert_eq!(strip_important("red !important "), ("red".to_string(), true));
        assert_eq!(
            strip_important("red !important !important"),
            ("red".to_string(), true)
        );
        assert_eq!(
            strip_important("red !important !important !important"),
            ("red".to_string(), true)
        );
        assert_eq!(strip_important("important"), ("important".to_string(), false));
        assert_eq!(
            strip_important("red important"),
            ("red important".to_string(), false)
        );
        assert_eq!(
            strip_important("red !important extra"),
            ("red !important extra".to_string(), false)
        );
        assert_eq!(strip_important(" !important "), ("".to_string(), true));
        assert_eq!(strip_important("12px"), ("12px".to_string(), false));
    }

    // ---- Phase 1.7c Slice A: shorthand expansion. ----

    /// Find a longhand declaration by name in a flat list. Returns its
    /// value as `&str`. Panics if the name is missing — every test that
    /// uses this helper expects the longhand to have been emitted.
    fn longhand<'a>(decls: &'a [Declaration], name: &str) -> &'a Declaration {
        decls
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("missing {name} in {decls:#?}"))
    }

    #[test]
    fn padding_one_value_expands_to_four_longhands() {
        let rules = parse_stylesheet("p { padding: 12px; }");
        assert_eq!(rules.len(), 1);
        let d = &rules[0].declarations;
        assert_eq!(d.len(), 4);
        for side in ["padding-top", "padding-right", "padding-bottom", "padding-left"] {
            assert_eq!(longhand(d, side).value, "12px");
            assert!(!longhand(d, side).important);
        }
        // The shorthand itself must NOT survive.
        assert!(d.iter().all(|x| x.name != "padding"));
    }

    #[test]
    fn padding_two_values_top_bottom_left_right() {
        let rules = parse_stylesheet("p { padding: 10px 20px; }");
        assert_eq!(rules.len(), 1);
        let d = &rules[0].declarations;
        assert_eq!(d.len(), 4);
        assert_eq!(longhand(d, "padding-top").value, "10px");
        assert_eq!(longhand(d, "padding-bottom").value, "10px");
        assert_eq!(longhand(d, "padding-right").value, "20px");
        assert_eq!(longhand(d, "padding-left").value, "20px");
    }

    #[test]
    fn padding_three_values_top_lr_bottom() {
        let rules = parse_stylesheet("p { padding: 10px 20px 30px; }");
        assert_eq!(rules.len(), 1);
        let d = &rules[0].declarations;
        assert_eq!(d.len(), 4);
        assert_eq!(longhand(d, "padding-top").value, "10px");
        assert_eq!(longhand(d, "padding-right").value, "20px");
        assert_eq!(longhand(d, "padding-left").value, "20px");
        assert_eq!(longhand(d, "padding-bottom").value, "30px");
    }

    #[test]
    fn padding_four_values_clockwise() {
        let rules = parse_stylesheet("p { padding: 1px 2px 3px 4px; }");
        assert_eq!(rules.len(), 1);
        let d = &rules[0].declarations;
        assert_eq!(d.len(), 4);
        assert_eq!(longhand(d, "padding-top").value, "1px");
        assert_eq!(longhand(d, "padding-right").value, "2px");
        assert_eq!(longhand(d, "padding-bottom").value, "3px");
        assert_eq!(longhand(d, "padding-left").value, "4px");
    }

    #[test]
    fn padding_extra_values_dropped() {
        // 5 values is not a valid padding shorthand — the whole shorthand
        // is dropped, and any siblings in the same block survive.
        let rules = parse_stylesheet("p { padding: 1px 2px 3px 4px 5px; color: red; }");
        assert_eq!(rules.len(), 1);
        let d = &rules[0].declarations;
        // Only `color` should remain; the padding shorthand expanded to nothing.
        assert!(d.iter().all(|x| !x.name.starts_with("padding")));
        assert_eq!(longhand(d, "color").value, "red");
    }

    #[test]
    fn margin_shorthand_expands_like_padding() {
        let rules = parse_stylesheet("p { margin: 10px 20px; }");
        assert_eq!(rules.len(), 1);
        let d = &rules[0].declarations;
        assert_eq!(d.len(), 4);
        assert_eq!(longhand(d, "margin-top").value, "10px");
        assert_eq!(longhand(d, "margin-bottom").value, "10px");
        assert_eq!(longhand(d, "margin-right").value, "20px");
        assert_eq!(longhand(d, "margin-left").value, "20px");
    }

    #[test]
    fn margin_emits_all_four_sides_even_if_cascade_only_uses_two() {
        // The cascade currently only consumes margin-top/margin-bottom.
        // Phase 1.7c still emits all four longhands so future phases can
        // pick up the horizontal sides without a parser change.
        let rules = parse_stylesheet("p { margin: 7px; }");
        assert_eq!(rules.len(), 1);
        let d = &rules[0].declarations;
        let names: Vec<&str> = d.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"margin-top"));
        assert!(names.contains(&"margin-right"));
        assert!(names.contains(&"margin-bottom"));
        assert!(names.contains(&"margin-left"));
        for side in ["margin-top", "margin-right", "margin-bottom", "margin-left"] {
            assert_eq!(longhand(d, side).value, "7px");
        }
    }

    #[test]
    fn border_shorthand_three_components_in_any_order() {
        // Canonical order.
        let rules = parse_stylesheet("p { border: 1px solid red; }");
        let d = &rules[0].declarations;
        assert_eq!(longhand(d, "border-width").value, "1px");
        assert_eq!(longhand(d, "border-style").value, "solid");
        assert_eq!(longhand(d, "border-color").value, "red");

        // Reversed order — components are recognised positionally-free.
        let rules = parse_stylesheet("p { border: red solid 2px; }");
        let d = &rules[0].declarations;
        assert_eq!(longhand(d, "border-width").value, "2px");
        assert_eq!(longhand(d, "border-style").value, "solid");
        assert_eq!(longhand(d, "border-color").value, "red");

        // Mixed.
        let rules = parse_stylesheet("p { border: solid 3px blue; }");
        let d = &rules[0].declarations;
        assert_eq!(longhand(d, "border-width").value, "3px");
        assert_eq!(longhand(d, "border-style").value, "solid");
        assert_eq!(longhand(d, "border-color").value, "blue");
    }

    #[test]
    fn border_shorthand_with_rgb_color_keeps_parens_intact() {
        // `rgb(0, 0, 0)` must NOT be split on the spaces inside parens.
        let rules = parse_stylesheet("p { border: rgb(0, 0, 0) solid 1px; }");
        let d = &rules[0].declarations;
        assert_eq!(longhand(d, "border-width").value, "1px");
        assert_eq!(longhand(d, "border-style").value, "solid");
        assert_eq!(longhand(d, "border-color").value, "rgb(0, 0, 0)");
    }

    #[test]
    fn border_shorthand_partial_expands_what_it_can() {
        // Just `1px solid` — no colour token. Two longhands emitted.
        let rules = parse_stylesheet("p { border: 1px solid; }");
        let d = &rules[0].declarations;
        assert_eq!(longhand(d, "border-width").value, "1px");
        assert_eq!(longhand(d, "border-style").value, "solid");
        assert!(d.iter().all(|x| x.name != "border-color"));

        // `solid red` — no length. Style + color emitted.
        let rules = parse_stylesheet("p { border: solid red; }");
        let d = &rules[0].declarations;
        assert_eq!(longhand(d, "border-style").value, "solid");
        assert_eq!(longhand(d, "border-color").value, "red");
        assert!(d.iter().all(|x| x.name != "border-width"));

        // `2px red` — length + colorish, no style.
        let rules = parse_stylesheet("p { border: 2px red; }");
        let d = &rules[0].declarations;
        assert_eq!(longhand(d, "border-width").value, "2px");
        assert_eq!(longhand(d, "border-color").value, "red");
        assert!(d.iter().all(|x| x.name != "border-style"));
    }

    #[test]
    fn border_shorthand_unrecognised_is_dropped() {
        // No length, no style keyword → drop the whole shorthand.
        let rules = parse_stylesheet("p { border: foo bar; color: red; }");
        let d = &rules[0].declarations;
        assert!(d.iter().all(|x| !x.name.starts_with("border")));
        assert_eq!(longhand(d, "color").value, "red");
    }

    #[test]
    fn shorthand_preserves_important_flag() {
        // The shorthand's `!important` propagates to every emitted longhand.
        let rules = parse_stylesheet("p { padding: 5px !important; }");
        let d = &rules[0].declarations;
        assert_eq!(d.len(), 4);
        for side in ["padding-top", "padding-right", "padding-bottom", "padding-left"] {
            let lh = longhand(d, side);
            assert_eq!(lh.value, "5px");
            assert!(lh.important, "{side} should be !important");
        }

        // Same for border.
        let rules = parse_stylesheet("p { border: 1px solid red !important; }");
        let d = &rules[0].declarations;
        for name in ["border-width", "border-style", "border-color"] {
            assert!(longhand(d, name).important, "{name} should be !important");
        }
    }

    #[test]
    fn shorthand_inside_normal_block_round_trips_with_other_decls() {
        // Shorthand expansion must not disturb adjacent longhand decls — they
        // appear in source order with the shorthand's expansion spliced in.
        let rules = parse_stylesheet(
            "p { color: red; padding: 4px 8px; font-size: 12px; }",
        );
        assert_eq!(rules.len(), 1);
        let d = &rules[0].declarations;
        // 1 (color) + 4 (padding longhands) + 1 (font-size) = 6.
        assert_eq!(d.len(), 6);
        assert_eq!(d[0].name, "color");
        assert_eq!(d[0].value, "red");
        // Padding longhands occupy indices 1..=4.
        assert_eq!(d[1].name, "padding-top");
        assert_eq!(d[1].value, "4px");
        assert_eq!(d[2].name, "padding-right");
        assert_eq!(d[2].value, "8px");
        assert_eq!(d[3].name, "padding-bottom");
        assert_eq!(d[3].value, "4px");
        assert_eq!(d[4].name, "padding-left");
        assert_eq!(d[4].value, "8px");
        assert_eq!(d[5].name, "font-size");
        assert_eq!(d[5].value, "12px");
    }

    // ---- Phase 2b Slice A: Stylesheet aggregate. ----

    #[test]
    fn parse_stylesheet_full_empty_returns_empty_aggregate() {
        let sheet = parse_stylesheet_full("");
        assert!(sheet.rules.is_empty());
        assert!(sheet.font_faces.is_empty());
    }

    #[test]
    fn parse_stylesheet_full_rules_match_legacy_parse_stylesheet() {
        let src = "h1 { font-size: 24px; } p { font-size: 12px; }";
        let aggregate = parse_stylesheet_full(src);
        let legacy = parse_stylesheet(src);
        assert_eq!(aggregate.rules.len(), legacy.len());
        for (a, l) in aggregate.rules.iter().zip(legacy.iter()) {
            assert_eq!(a.selector_text, l.selector_text);
            assert_eq!(a.source_order, l.source_order);
        }
    }

    #[test]
    fn font_face_basic_block_captured() {
        let src = "@font-face { font-family: Acme; src: url(data:font/ttf;base64,AAA); }";
        let sheet = parse_stylesheet_full(src);
        assert_eq!(sheet.font_faces.len(), 1);
        assert_eq!(sheet.rules.len(), 0);
        let face = &sheet.font_faces[0];
        assert_eq!(face.source_order, 0);
        // Both descriptors land in the declaration list, normalised by
        // parse_declaration_block.
        let names: Vec<&str> = face.declarations.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"font-family"));
        assert!(names.contains(&"src"));
    }

    #[test]
    fn font_face_keyword_is_case_insensitive() {
        for variant in ["@font-face", "@Font-Face", "@FONT-FACE", "@fOnT-fAcE"] {
            let src = format!("{variant} {{ font-family: A; src: url(x); }}");
            let sheet = parse_stylesheet_full(&src);
            assert_eq!(sheet.font_faces.len(), 1, "variant {variant} not recognised");
        }
    }

    #[test]
    fn font_face_tolerates_whitespace_before_brace() {
        let src = "@font-face\n  \t{ font-family: Acme; src: url(x); }";
        let sheet = parse_stylesheet_full(src);
        assert_eq!(sheet.font_faces.len(), 1);
    }

    #[test]
    fn font_face_source_order_is_shared_with_rules() {
        // Three top-level items: rule, font-face, rule. Source order
        // assignments must be 0, 1, 2 in that interleaved order.
        let src = "p { color: red; } \
                   @font-face { font-family: A; src: url(x); } \
                   h1 { color: blue; }";
        let sheet = parse_stylesheet_full(src);
        assert_eq!(sheet.rules.len(), 2);
        assert_eq!(sheet.font_faces.len(), 1);
        assert_eq!(sheet.rules[0].source_order, 0);   // p
        assert_eq!(sheet.font_faces[0].source_order, 1); // @font-face
        assert_eq!(sheet.rules[1].source_order, 2);   // h1
    }

    #[test]
    fn font_face_unterminated_block_dropped_silently() {
        // No closing brace — must not panic, must not corrupt subsequent
        // parsing. Phase 2b's policy: drop the malformed @font-face and
        // any tail content inside it (the caller's `skip_at_rule`
        // fallback consumes through the next balanced } or EOF).
        let src = "@font-face { font-family: A; src: url(x); /* no closer */ ";
        let sheet = parse_stylesheet_full(src);
        // No crash; no faces captured.
        assert_eq!(sheet.font_faces.len(), 0);
    }

    #[test]
    fn font_face_other_at_rules_still_dropped() {
        // @media, @import, etc. continue to be silently skipped — only
        // @font-face is carved out.
        let src = "@media print { p { x: y; } } \
                   @font-face { font-family: A; src: url(x); } \
                   @import url('foo.css');";
        let sheet = parse_stylesheet_full(src);
        assert_eq!(sheet.font_faces.len(), 1);
        assert_eq!(sheet.rules.len(), 0);
    }

    #[test]
    fn font_face_empty_block_yields_empty_declarations() {
        // `@font-face { }` is technically valid CSS but useless. The
        // registry will drop it (no font-family). At the parser layer
        // we capture it with an empty declaration list.
        let sheet = parse_stylesheet_full("@font-face { }");
        assert_eq!(sheet.font_faces.len(), 1);
        assert_eq!(sheet.font_faces[0].declarations.len(), 0);
    }
}
