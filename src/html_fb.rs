//! HTML → framebuffer rasterization (pure, native-testable).
//!
//! A deliberately tiny renderer: enough to show what an `index.html`
//! "says" on the screen, not a browser engine. We extract block-level text
//! (headings/paragraphs/lists), drop `<head>`/`<script>`/`<style>`, decode
//! the common entities, then word-wrap and blit with the bitmap font
//! ([`crate::raster::blit_glyph`]). Zero web-sys — hoisted out of
//! `app::display` (roadmap R5) so it runs under `cargo test`; the browser
//! display module just blits the returned RGBA buffer to a canvas.

/// One laid-out block of text. `scale` drives glyph size (headings are
/// bigger); `bullet` prefixes a list dash.
pub struct HtmlBlock {
    text: String,
    scale: i32,
    bullet: bool,
}

/// Extract the lowercased tag name from the inside of a `<...>` (handles a
/// leading `/` for close tags and trailing attributes/`/`).
fn tag_name(inner: &str) -> String {
    let t = inner.trim().trim_start_matches('/').trim_start();
    let end = t
        .find(|ch: char| ch.is_whitespace() || ch == '/')
        .unwrap_or(t.len());
    t[..end].to_ascii_lowercase()
}

/// Decode the handful of HTML entities that show up in plain prose.
pub fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        // `&amp;` last so a literal "&amp;lt;" doesn't double-decode.
        .replace("&amp;", "&")
}

/// Collapse runs of whitespace to single spaces and trim — HTML source
/// whitespace is not significant for our layout.
pub fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim_end().to_string()
}

/// Push the accumulated text run as a block (decoded + collapsed), then
/// clear it. No-op for an empty run.
fn flush_block(blocks: &mut Vec<HtmlBlock>, cur: &mut String, scale: i32, bullet: bool) {
    let text = collapse_ws(&decode_entities(cur));
    cur.clear();
    if !text.is_empty() {
        blocks.push(HtmlBlock { text, scale, bullet });
    }
}

/// Parse a subset of HTML into renderable text blocks. Inline tags
/// (`a`, `span`, `b`, `code`, …) are ignored — their text just flows into
/// the current block. `head`/`script`/`style` content is skipped wholesale.
pub fn html_to_blocks(src: &str) -> Vec<HtmlBlock> {
    let chars: Vec<char> = src.chars().collect();
    let mut blocks: Vec<HtmlBlock> = Vec::new();
    let mut cur = String::new();
    let mut scale: i32 = 1;
    let mut bullet = false;
    let mut skip_tag: Option<String> = None;

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            // Read up to the closing '>'.
            let mut j = i + 1;
            let mut inner = String::new();
            while j < chars.len() && chars[j] != '>' {
                inner.push(chars[j]);
                j += 1;
            }
            i = if j < chars.len() { j + 1 } else { j };

            let closing = inner.trim_start().starts_with('/');
            let name = tag_name(&inner);

            // Inside a skipped region, ignore everything but its close.
            if let Some(skip) = skip_tag.clone() {
                if closing && name == skip {
                    skip_tag = None;
                }
                continue;
            }

            match name.as_str() {
                "script" | "style" | "head" => {
                    if !closing {
                        skip_tag = Some(name);
                    }
                }
                "br" | "hr" => {
                    flush_block(&mut blocks, &mut cur, scale, bullet);
                    bullet = false;
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    flush_block(&mut blocks, &mut cur, scale, bullet);
                    bullet = false;
                    scale = if closing {
                        1
                    } else if name == "h1" {
                        3
                    } else {
                        2
                    };
                }
                "li" => {
                    flush_block(&mut blocks, &mut cur, scale, bullet);
                    scale = 1;
                    bullet = !closing;
                }
                "p" | "div" | "ul" | "ol" | "section" | "article" | "header" | "footer"
                | "nav" | "main" | "blockquote" | "pre" | "table" | "tr" | "title" | "body"
                | "html" | "figure" | "figcaption" => {
                    flush_block(&mut blocks, &mut cur, scale, bullet);
                    bullet = false;
                    scale = 1;
                }
                _ => { /* inline tag — let its text flow into the block */ }
            }
            continue;
        }

        if skip_tag.is_some() {
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    flush_block(&mut blocks, &mut cur, scale, bullet);
    blocks
}

/// Word-wrap `text` to at most `max_chars` per line, hard-breaking any
/// single word longer than the line.
pub fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= max_chars {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        }
        // Hard-break a word that overflows the line on its own.
        while line.chars().count() > max_chars {
            let head: String = line.chars().take(max_chars).collect();
            let tail: String = line.chars().skip(max_chars).collect();
            lines.push(head);
            line = tail;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Fill a fresh `fb_w`×`fb_h` RGBA framebuffer with an opaque colour.
pub fn filled_framebuffer(color: (u8, u8, u8), fb_w: i32, fb_h: i32) -> Vec<u8> {
    let (r, g, b) = color;
    let mut buf = vec![0u8; (fb_w.max(0) as usize) * (fb_h.max(0) as usize) * 4];
    let mut i = 0;
    while i + 3 < buf.len() {
        buf[i] = r;
        buf[i + 1] = g;
        buf[i + 2] = b;
        buf[i + 3] = 255;
        i += 4;
    }
    buf
}

/// Lay out parsed blocks into a `fb_w`×`fb_h` framebuffer. Monochrome:
/// near-black background, light text, white headings. Clips at the bottom
/// edge (no scrolling — this is a screenshot, not a scroll view).
pub fn paint_html_fb(blocks: &[HtmlBlock], fb_w: i32, fb_h: i32) -> Vec<u8> {
    let mut buf = filled_framebuffer((13, 13, 13), fb_w, fb_h);
    let left = 6i32;
    let right = fb_w - 6;
    let mut y = 6i32;

    for block in blocks {
        let scale = block.scale.clamp(1, 3);
        let advance = 6 * scale; // 5px glyph + 1px gap
        let line_h = 8 * scale; // 7px glyph + 1px gap
        let max_chars = (((right - left) / advance).max(1)) as usize;
        let color = if scale > 1 { (245, 245, 245) } else { (205, 205, 205) };
        let text = if block.bullet {
            format!("- {}", block.text)
        } else {
            block.text.clone()
        };

        for line in wrap_text(&text, max_chars) {
            if y + line_h > fb_h {
                return buf; // out of vertical room
            }
            let mut x = left;
            let vp = crate::raster::Viewport::full(fb_w, fb_h);
            for ch in line.chars() {
                crate::raster::blit_glyph(&mut buf, fb_w, &vp, x, y, ch as u32, color, scale);
                x += advance;
            }
            y += line_h;
        }
        y += 3; // gap between blocks
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RGBA of pixel (x, y) in a `w`-wide buffer.
    fn px(buf: &[u8], w: i32, x: i32, y: i32) -> (u8, u8, u8, u8) {
        let i = ((y * w + x) * 4) as usize;
        (buf[i], buf[i + 1], buf[i + 2], buf[i + 3])
    }

    #[test]
    fn decode_entities_handles_common_prose() {
        assert_eq!(decode_entities("a &lt;b&gt; &quot;c&quot; &amp; d"), "a <b> \"c\" & d");
        // `&amp;` decodes LAST so a literal "&amp;lt;" yields "&lt;", not "<".
        assert_eq!(decode_entities("&amp;lt;"), "&lt;");
        assert_eq!(decode_entities("x&nbsp;y&#39;z&apos;"), "x y'z'");
    }

    #[test]
    fn collapse_ws_squeezes_and_trims() {
        assert_eq!(collapse_ws("  a \n\t b  c  "), "a b c");
        assert_eq!(collapse_ws(""), "");
        assert_eq!(collapse_ws("   \n "), "");
    }

    #[test]
    fn html_to_blocks_extracts_headings_lists_and_skips_head() {
        let src = "<html><head><title>skip me</title><style>.x{}</style></head>\
                   <body><h1>Title</h1><p>Body &amp; text</p>\
                   <ul><li>one</li><li>two</li></ul>\
                   <script>var hidden = 1;</script></body></html>";
        let blocks = html_to_blocks(src);
        let flat: Vec<(&str, i32, bool)> =
            blocks.iter().map(|b| (b.text.as_str(), b.scale, b.bullet)).collect();
        assert_eq!(
            flat,
            vec![
                ("Title", 3, false),
                ("Body & text", 1, false),
                ("one", 1, true),
                ("two", 1, true),
            ]
        );
    }

    #[test]
    fn html_to_blocks_flows_inline_tags_into_one_block() {
        let blocks = html_to_blocks("<p>a <b>bold</b> and <a href=\"#\">link</a></p>");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "a bold and link");
    }

    #[test]
    fn wrap_text_wraps_and_hard_breaks() {
        assert_eq!(wrap_text("aa bb cc", 5), vec!["aa bb", "cc"]);
        // A single word longer than the line is hard-broken.
        assert_eq!(wrap_text("abcdefgh", 3), vec!["abc", "def", "gh"]);
        // max_chars clamps to >= 1 (no infinite loop / empty lines).
        assert_eq!(wrap_text("ab", 0), vec!["a", "b"]);
        assert!(wrap_text("", 10).is_empty());
    }

    #[test]
    fn filled_framebuffer_is_opaque_and_sized() {
        let buf = filled_framebuffer((7, 8, 9), 4, 3);
        assert_eq!(buf.len(), 4 * 3 * 4);
        for p in buf.chunks_exact(4) {
            assert_eq!(p, &[7, 8, 9, 255]);
        }
    }

    #[test]
    fn paint_html_fb_paints_text_pixels_on_dark_ground() {
        let w = 64;
        let h = 64;
        let buf = paint_html_fb(&html_to_blocks("<p>HI</p>"), w, h);
        assert_eq!(buf.len(), (w * h * 4) as usize);
        // Background is the near-black fill…
        assert_eq!(px(&buf, w, w - 1, h - 1), (13, 13, 13, 255));
        // …and SOME pixel carries the body-text grey (a glyph was blitted).
        let lit = buf.chunks_exact(4).any(|p| p[0] == 205 && p[1] == 205 && p[2] == 205);
        assert!(lit, "expected at least one glyph pixel");
    }

    #[test]
    fn paint_html_fb_clips_at_bottom_instead_of_panicking() {
        // A framebuffer too short for even one line: layout returns early.
        let blocks = html_to_blocks("<p>abc</p><p>def</p>");
        let buf = paint_html_fb(&blocks, 32, 8);
        assert_eq!(buf.len(), 32 * 8 * 4);
    }
}
