//! A comment's Markdown as a tree, for the page to draw.
//!
//! Nothing in the tree is HTML: raw HTML that a comment contains is text, and
//! a link is kept only if it goes to `http`, `https` or `mailto` (a
//! `javascript:` link is drawn as its text). The page makes elements from the
//! nodes, so what a comment says can't run.
//!
//! A node is a string (text) or an object with `t` its type and, by type:
//!
//! - blocks: `p` (paragraph), `h` (`l` the level), `quote`, `ul`, `ol` (`start`),
//!   `li` (the children of an item are blocks, or, in a tight list, text and
//!   inline nodes directly), `pre` (`s` the code, `lang`), `hr`;
//! - tables: `table` (`al` the columns' alignments: `l`, `c`, `r` or `""`) of
//!   `thead` (its cells) and `tr`s (their cells), a cell being `td`;
//! - inline: `em`, `strong`, `del`, `code` (`s`), `a` (`href`), `br`, `file` (`id`,
//!   a link that names a file of the bundle, drawn as one to save), `image`
//!   (`id` of an image of the bundle, `alt`: only a link that names one, see
//!   `crate::image`).
//!
//! Others (any other image, footnotes, ...) are not drawn as such: their text is.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};

/// The nodes of `body`.
pub fn tree(body: &str) -> Vec<Value> {
    // Open nodes, innermost last: (type and what it carries, children so far).
    let mut stack: Vec<(Value, Vec<Value>)> = vec![(Value::Null, Vec::new())];
    let mut code: Option<String> = None;

    fn push(stack: &mut [(Value, Vec<Value>)], node: Value) {
        if let Some((_, children)) = stack.last_mut() {
            // Text next to text is one text.
            if let (Value::String(text), Some(Value::String(last))) = (&node, children.last_mut()) {
                last.push_str(text);
                return;
            }
            children.push(node);
        }
    }

    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(body, options) {
        match event {
            Event::Start(tag) => {
                if let Tag::CodeBlock(_) = tag {
                    code = Some(String::new());
                }
                stack.push((open(&tag), Vec::new()));
            }
            Event::End(end) => {
                if stack.len() < 2 {
                    continue;
                }
                let Some((mut node, children)) = stack.pop() else {
                    continue;
                };
                if matches!(end, TagEnd::CodeBlock) {
                    node["s"] = Value::String(code.take().unwrap_or_default());
                } else if node["t"] == "a" && node["href"].is_null() {
                    // A link that goes nowhere safe is its text.
                    for child in children {
                        push(&mut stack, child);
                    }
                    continue;
                } else if node["t"] == "image" {
                    // The image of the bundle: its alt text is what is inside.
                    let alt: String = children
                        .iter()
                        .filter_map(|c| c.as_str())
                        .collect::<Vec<_>>()
                        .join("");
                    node["alt"] = Value::String(alt);
                } else if matches!(node["t"].as_str(), Some("img" | "skip")) {
                    // Not drawn as such: what is inside stays.
                    for child in children {
                        push(&mut stack, child);
                    }
                    continue;
                } else if !children.is_empty() {
                    node["c"] = Value::Array(children);
                }
                push(&mut stack, node);
            }
            Event::Text(text) => match code.as_mut() {
                Some(code) => code.push_str(&text),
                None => push(&mut stack, Value::String(text.to_string())),
            },
            // What looks like HTML is text.
            Event::Html(text) | Event::InlineHtml(text) => match code.as_mut() {
                Some(code) => code.push_str(&text),
                None => push(&mut stack, Value::String(text.to_string())),
            },
            Event::Code(text) => push(&mut stack, json!({ "t": "code", "s": text.as_ref() })),
            // A line break stays one (as in a comment on GitHub), with or without
            // the two spaces or the backslash that Markdown asks for.
            Event::SoftBreak | Event::HardBreak => push(&mut stack, json!({ "t": "br" })),
            Event::Rule => push(&mut stack, json!({ "t": "hr" })),
            _ => {}
        }
    }
    stack
        .pop()
        .map(|(_, children)| children)
        .unwrap_or_default()
}

/// The node an opening tag starts (`skip` and `img` are unwrapped when they
/// close: their children are kept).
fn open(tag: &Tag) -> Value {
    match tag {
        Tag::Paragraph => json!({ "t": "p" }),
        Tag::Heading { level, .. } => json!({ "t": "h", "l": *level as u8 }),
        Tag::BlockQuote(_) => json!({ "t": "quote" }),
        Tag::CodeBlock(kind) => match kind {
            CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                json!({ "t": "pre", "lang": lang.split_whitespace().next().unwrap_or("") })
            }
            _ => json!({ "t": "pre" }),
        },
        Tag::List(None) => json!({ "t": "ul" }),
        Tag::List(Some(start)) => json!({ "t": "ol", "start": start }),
        Tag::Item => json!({ "t": "li" }),
        Tag::Emphasis => json!({ "t": "em" }),
        Tag::Strong => json!({ "t": "strong" }),
        Tag::Strikethrough => json!({ "t": "del" }),
        Tag::Table(aligns) => {
            let al: Vec<&str> = aligns
                .iter()
                .map(|a| match a {
                    Alignment::Left => "l",
                    Alignment::Center => "c",
                    Alignment::Right => "r",
                    Alignment::None => "",
                })
                .collect();
            json!({ "t": "table", "al": al })
        }
        Tag::TableHead => json!({ "t": "thead" }),
        Tag::TableRow => json!({ "t": "tr" }),
        Tag::TableCell => json!({ "t": "td" }),
        Tag::Link { dest_url, .. } => {
            if let Some(id) = dest_url
                .strip_prefix(crate::image::FILE_SCHEME)
                .filter(|id| crate::image::is_id(id))
            {
                json!({ "t": "file", "id": id })
            } else if safe_link(dest_url) {
                json!({ "t": "a", "href": dest_url.as_ref() })
            } else {
                json!({ "t": "a" })
            }
        }
        Tag::Image { dest_url, .. } => {
            match dest_url
                .strip_prefix(crate::image::SCHEME)
                .filter(|id| crate::image::is_id(id))
            {
                Some(id) => json!({ "t": "image", "id": id }),
                None => json!({ "t": "img" }),
            }
        }
        _ => json!({ "t": "skip" }),
    }
}

/// Whether a link may be followed: `http`, `https` or `mailto`, and nothing
/// else (not `javascript:`, `data:`, ... nor a relative one, which points
/// nowhere in a review that is opened from a file).
fn safe_link(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_of(body: &str) -> String {
        serde_json::to_string(&tree(body)).unwrap()
    }

    #[test]
    fn plain_text_is_a_paragraph_and_emphasis_code_and_links_are_inline_nodes() {
        assert_eq!(json_of("about B"), r#"[{"c":["about B"],"t":"p"}]"#);
        assert_eq!(
            json_of("a *b* **c** `d` [e](https://x.y/z)"),
            r#"[{"c":["a ",{"c":["b"],"t":"em"}," ",{"c":["c"],"t":"strong"}," ",{"s":"d","t":"code"}," ",{"c":["e"],"href":"https://x.y/z","t":"a"}],"t":"p"}]"#
        );
    }

    #[test]
    fn raw_html_is_text_and_never_a_node() {
        let out = json_of("hi <img src=x onerror=alert(1)> <script>alert(2)</script>");
        assert!(
            !out.contains(r#""t":"img""#) && !out.contains(r#""t":"script""#),
            "{out}"
        );
        assert!(
            out.contains("<img src=x onerror=alert(1)>"),
            "as text: {out}"
        );
        // Also as a block of its own.
        let out = json_of("<div onclick=x>\nhello\n</div>");
        assert!(out.contains("<div onclick=x>"), "{out}");
        assert!(!out.contains(r#""t":"div""#));
    }

    #[test]
    fn a_link_is_kept_only_if_it_is_safe() {
        for bad in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            " javascript:alert(1)",
            "data:text/html,<script>1</script>",
            "vbscript:x",
            "/relative",
            "//evil.example",
            "file:///etc/passwd",
        ] {
            let out = json_of(&format!("[click]({bad})"));
            assert!(!out.contains("href"), "{bad}: {out}");
            assert!(out.contains("click"), "its text stays: {out}");
        }
        for good in [
            "http://a.b",
            "https://a.b/c?d=e",
            "mailto:a@b.c",
            "HTTPS://A.B",
        ] {
            let out = json_of(&format!("[t]({good})"));
            assert!(out.contains(r#""href""#), "{good}: {out}");
        }
    }

    #[test]
    fn images_are_their_alt_text_and_nothing_is_loaded() {
        let out = json_of("![alt words](https://x.y/a.png)");
        assert!(!out.contains("img") && !out.contains("x.y"), "{out}");
        assert!(out.contains("alt words"));
    }

    #[test]
    fn an_image_of_the_bundle_is_a_node_and_any_other_is_its_alt_text() {
        let id = "a".repeat(64);
        assert_eq!(
            json_of(&format!("![a shot](diffnote-image:{id})")),
            format!(r#"[{{"c":[{{"alt":"a shot","id":"{id}","t":"image"}}],"t":"p"}}]"#)
        );
        // Not an id, or an address: only the text stays, nothing is loaded.
        for other in [
            "diffnote-image:xyz",
            "https://x.y/a.png",
            "javascript:alert(1)",
        ] {
            let out = json_of(&format!("![alt words]({other})"));
            assert!(
                !out.contains("image") && !out.contains("x.y"),
                "{other}: {out}"
            );
            assert!(out.contains("alt words"), "{other}: {out}");
        }
    }

    #[test]
    fn a_link_to_a_file_of_the_bundle_is_a_node_with_its_text() {
        let id = "b".repeat(64);
        assert_eq!(
            json_of(&format!("[the log](diffnote-file:{id})")),
            format!(r#"[{{"c":[{{"c":["the log"],"id":"{id}","t":"file"}}],"t":"p"}}]"#)
        );
        // Not an id: a link that goes nowhere is its text.
        let out = json_of("[x](diffnote-file:short)");
        assert!(!out.contains("file") && out.contains('x'), "{out}");
    }

    #[test]
    fn code_blocks_keep_their_text_and_language() {
        assert_eq!(
            json_of("```rust\nlet a = 1;\n<b>\n```"),
            r#"[{"lang":"rust","s":"let a = 1;\n<b>\n","t":"pre"}]"#
        );
        assert_eq!(json_of("    indented"), r#"[{"s":"indented","t":"pre"}]"#);
    }

    #[test]
    fn lists_quotes_headings_rules_and_breaks() {
        let out = json_of("# T\n\n- a\n- b\n\n1. x\n\n> q\n\n---\n\nl1  \nl2");
        assert!(out.contains(r#""t":"h""#) && out.contains(r#""l":1"#));
        assert!(out.contains(r#""t":"ul""#) && out.contains(r#""t":"li""#));
        assert!(out.contains(r#""t":"ol""#) && out.contains(r#""start":1"#));
        assert!(out.contains(r#""t":"quote""#) && out.contains(r#""t":"hr""#));
        assert!(out.contains(r#""t":"br""#));
    }

    #[test]
    fn tables_and_strikethrough_are_nodes() {
        assert_eq!(
            json_of("|a|b|\n|:-|-:|\n|1|~~2~~|"),
            r#"[{"al":["l","r"],"c":[{"c":[{"c":["a"],"t":"td"},{"c":["b"],"t":"td"}],"t":"thead"},{"c":[{"c":["1"],"t":"td"},{"c":[{"c":["2"],"t":"del"}],"t":"td"}],"t":"tr"}],"t":"table"}]"#
        );
        // Raw HTML in a cell is still text.
        let out = json_of("|a|\n|-|\n|<b onclick=x>|");
        assert!(
            !out.contains(r#""t":"b""#) && out.contains("<b onclick=x>"),
            "{out}"
        );
    }

    #[test]
    fn a_line_break_stays_one_and_an_empty_body_is_nothing() {
        assert_eq!(
            json_of("one\ntwo"),
            r#"[{"c":["one",{"t":"br"},"two"],"t":"p"}]"#
        );
        // (A hard break, with the two spaces or the backslash, is the same.)
        assert_eq!(json_of("one  \ntwo"), json_of("one\ntwo"));
        assert_eq!(json_of("one\\\ntwo"), json_of("one\ntwo"));
        // A blank line still starts a paragraph.
        assert_eq!(
            json_of("one\n\ntwo"),
            r#"[{"c":["one"],"t":"p"},{"c":["two"],"t":"p"}]"#
        );
        assert_eq!(json_of(""), "[]");
    }
}
