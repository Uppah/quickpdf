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

/// Parse a stylesheet source string into a flat rule list. Always returns —
/// malformed rules are silently skipped (browsers do the same).
pub fn parse_stylesheet(source: &str) -> Vec<Rule> {
    let bytes = source.as_bytes();
    let mut pos = 0;
    let mut rules: Vec<Rule> = Vec::new();
    let mut order: usize = 0;

    while pos < bytes.len() {
        pos = skip_ws_and_comments(bytes, pos);
        if pos >= bytes.len() {
            break;
        }

        // At-rule? Skip the entire prelude + optional block.
        if bytes[pos] == b'@' {
            pos = skip_at_rule(bytes, pos);
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
                // malformed and stop. (e.g. "broken {" with no closer, or
                // trailing junk like "; foo" at EOF.)
                break;
            }
        }
    }

    rules
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
    out
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
    while pos < bytes.len() {
        let b = bytes[pos];
        match b {
            b';' => {
                out.push(&body[start..pos]);
                pos += 1;
                start = pos;
            }
            b'"' | b'\'' => {
                pos = skip_string(bytes, pos);
            }
            b'/' if pos + 1 < bytes.len() && bytes[pos + 1] == b'*' => {
                pos = skip_ws_and_comments(bytes, pos);
            }
            // A `(` introduces a balanced parenthesis run — used in `url(...)`,
            // `calc(...)`, `var(...)`, etc. We don't actually parse these; we
            // just don't want a `;` inside `url("a;b")` to split a declaration.
            // (Strings inside already protect us, but bare `;` inside `(...)`
            // is invalid CSS and there's no real-world example, so we leave it.)
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
}
