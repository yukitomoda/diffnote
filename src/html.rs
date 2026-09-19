//! Self-contained static HTML export, so a review can be viewed without
//! installing diffnote.
//!
//! Everything (CSS, JS, per-line syntax highlighting, comment Markdown) is
//! rendered to plain HTML at export time in Rust -- the output is a single
//! file, safe to open via a bare `file://` URL: no fetch/import of sibling
//! files, no secure-context-only APIs, no `localStorage`. Collapsing is
//! done with native `<details>`/`<summary>` (works with the inline script
//! disabled); the one inline `<script>` (see `SCRIPT`) only links hovering
//! a diff line's color band to its thread card and back, pure local DOM
//! event handling. Class names are stable and BEM-style, and theme colors
//! are `:root` CSS custom properties, so a future custom-CSS feature can
//! mostly work via variable overrides.
//!
//! Re-anchoring reuses the exact same `anchor::resolve` logic `diffnote
//! edit` uses, against whatever diff is passed in here -- but read-only:
//! nothing is ever written back, a `Relocated` result just changes where a
//! thread is drawn for this one export.

use crate::anchor::{self, Placement};
use crate::diff::{FileDiff, Hunk, LineKind, UnifiedDiff};
use crate::model::{Anchor, Event, Side};
use crate::review::{Thread, build_threads};
use pulldown_cmark::{Parser as MdParser, html::push_html as md_push_html};
use std::collections::HashMap;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::{IncludeBackground, styled_line_to_highlighted_html};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use ulid::Ulid;

/// One revision of the review to show: its diff and per-file digests, and a
/// short label for the switcher.
pub struct RevisionView<'a> {
    pub label: String,
    pub diff: &'a UnifiedDiff,
    pub files: &'a [crate::model::FileDigest],
}

/// Renders every revision as its own pre-rendered view (oldest first, the
/// last one -- the latest -- shown by default). Every thread appears in every
/// view: at its position where it can be placed against that revision's
/// diff, in that view's "unplaced" section where it can't. The tiny inline
/// script only switches which view is visible; without it all views are
/// simply stacked.
pub fn render(
    events: &[Event],
    views: &[RevisionView],
    blobs: &crate::digest::Blobs,
) -> String {
    let threads = build_threads(events);
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme_set = ThemeSet::load_defaults();
    let theme = &theme_set.themes["InspiredGitHub"];

    let mut body = String::new();
    body.push_str(r#"<header class="diffnote-summary"><h1>diffnote review</h1>"#);
    body.push_str(&format!(
        "<p>{} thread(s), {} resolved</p></header>\n",
        threads.len(),
        threads.iter().filter(|t| t.resolved).count()
    ));
    if views.len() > 1 {
        body.push_str(r#"<nav class="diffnote-revisions"><ul>"#);
        for (i, view) in views.iter().enumerate() {
            body.push_str(&format!(
                r##"<li><a href="#rev-{i}" data-diffnote-revision-link="{i}">{}</a></li>"##,
                escape_html(&view.label)
            ));
        }
        body.push_str("</ul></nav>\n");
    }
    for (i, view) in views.iter().enumerate() {
        let current = if i + 1 == views.len() { " is-current" } else { "" };
        let inner = render_view(&threads, view, blobs, &syntax_set, theme);
        // Element ids must be unique across the views.
        let inner = inner
            .replace(r#"id="file-"#, &format!(r#"id="r{i}-file-"#))
            .replace(r##"href="#file-"##, &format!(r##"href="#r{i}-file-"##))
            .replace(r#"id="thread-"#, &format!(r#"id="r{i}-thread-"#));
        body.push_str(&format!(
            r#"<section class="diffnote-revision{current}" id="rev-{i}" data-diffnote-revision="{i}"><h2 class="diffnote-revision__title">{}</h2>{inner}</section>"#,
            escape_html(&view.label)
        ));
    }
    wrap_document(&body)
}

fn render_view(
    threads: &[Thread],
    view: &RevisionView,
    blobs: &crate::digest::Blobs,
    syntax_set: &SyntaxSet,
    theme: &Theme,
) -> String {
    let (diff, current_files) = (view.diff, view.files);
    let mut global: Vec<&Thread> = Vec::new();
    let mut by_file: HashMap<String, Vec<&Thread>> = HashMap::new();
    // Where a thread's card is drawn: keyed by the *last* line of its range.
    let mut by_line: HashMap<(String, Side, u32), Vec<&Thread>> = HashMap::new();
    // Every line covered by a Span anchor's range, and *which* thread(s)
    // cover it -- so overlapping range comments can each get their own
    // color band instead of collapsing into one undifferentiated highlight.
    let mut highlighted: HashMap<(String, Side, u32), Vec<Ulid>> = HashMap::new();
    // A stable color (index into PALETTE) per thread that ends up as a Line
    // placement, assigned in document order so it's deterministic run to run.
    let mut color_of: HashMap<Ulid, usize> = HashMap::new();
    let mut outdated: HashMap<String, Vec<&Thread>> = HashMap::new();

    for thread in threads {
        match anchor::resolve_placement(&thread.anchor, diff, current_files, blobs) {
            Placement::Global => global.push(thread),
            Placement::File(file) => by_file.entry(file).or_default().push(thread),
            Placement::Line {
                file,
                side,
                line_start,
                line_end,
                old_range,
                relocated: _,
            } => {
                let color = color_of.len() % PALETTE.len();
                color_of.insert(thread.root_id, color);
                for line in line_start..=line_end {
                    highlighted
                        .entry((file.clone(), side, line))
                        .or_default()
                        .push(thread.root_id);
                }
                // A range that wrapped both removed and new-side lines
                // still highlights its old-side span too, not just the
                // new-side one used for the card/re-anchoring.
                if let Some((old_start, old_end)) = old_range {
                    for line in old_start..=old_end {
                        highlighted
                            .entry((file.clone(), Side::Old, line))
                            .or_default()
                            .push(thread.root_id);
                    }
                }
                by_line
                    .entry((file, side, line_end))
                    .or_default()
                    .push(thread)
            }
            Placement::Outdated { file } | Placement::OutsideDiff { file } => outdated.entry(file).or_default().push(thread),
        }
    }

    // A thread's file might not appear in `diff` at all (e.g. exporting
    // against a different diff than the one the review was written
    // against, or a Hunk/File-scope anchor whose file was since removed --
    // v1 doesn't re-verify those). Such threads must still be shown
    // *somewhere*, not silently dropped because their file never gets
    // visited by a `for file_diff in &diff.files` loop.
    let mut file_order: Vec<String> = diff.files.iter().map(file_key).collect();
    let mut seen: std::collections::HashSet<String> = file_order.iter().cloned().collect();
    for key in by_file
        .keys()
        .chain(outdated.keys())
        .chain(by_line.keys().map(|(f, _, _)| f))
    {
        if seen.insert(key.clone()) {
            file_order.push(key.clone());
        }
    }

    let mut body = String::new();
    body.push_str(r#"<nav class="diffnote-filelist"><ul>"#);
    for key in &file_order {
        let count = by_file.get(key).map_or(0, Vec::len)
            + diff_file_comment_count(key, &by_line)
            + outdated.get(key).map_or(0, Vec::len);
        body.push_str(&format!(
            r##"<li><a href="#file-{id}">{name}</a>{badge}</li>"##,
            id = html_id(key),
            name = escape_html(key),
            badge = if count > 0 {
                format!(r#" <span class="diffnote-badge">{count}</span>"#)
            } else {
                String::new()
            },
        ));
    }
    body.push_str("</ul></nav>\n");

    if !global.is_empty() {
        body.push_str(r#"<section class="diffnote-global-comments">"#);
        for t in &global {
            body.push_str(&render_thread_html(t, &color_of));
        }
        body.push_str("</section>\n");
    }

    for key in &file_order {
        body.push_str(&render_file(
            anchor::find_file(diff, key),
            key,
            by_file.get(key).map(Vec::as_slice).unwrap_or(&[]),
            &by_line,
            &highlighted,
            &color_of,
            outdated.get(key).map(Vec::as_slice).unwrap_or(&[]),
            syntax_set,
            theme,
        ));
    }

    body
}

fn diff_file_comment_count(
    file: &str,
    by_line: &HashMap<(String, Side, u32), Vec<&Thread>>,
) -> usize {
    by_line
        .iter()
        .filter(|((f, _, _), _)| f == file)
        .map(|(_, v)| v.len())
        .sum()
}

fn file_key(file_diff: &FileDiff) -> String {
    file_diff
        .new_path
        .clone()
        .or_else(|| file_diff.old_path.clone())
        .unwrap_or_else(|| "?".to_string())
}

#[allow(clippy::too_many_arguments)]
fn render_file(
    file_diff: Option<&FileDiff>,
    key: &str,
    file_threads: &[&Thread],
    by_line: &HashMap<(String, Side, u32), Vec<&Thread>>,
    highlighted: &HashMap<(String, Side, u32), Vec<Ulid>>,
    color_of: &HashMap<Ulid, usize>,
    outdated_threads: &[&Thread],
    syntax_set: &SyntaxSet,
    theme: &Theme,
) -> String {
    let mut out = String::new();
    let comment_count = file_threads.len()
        + diff_file_comment_count(key, by_line)
        + outdated_threads.len();
    let open_attr = if comment_count > 0 { " open" } else { "" };
    let is_binary = file_diff.is_some_and(|f| f.is_binary);
    let is_rename = file_diff.is_some_and(|f| f.is_rename);

    out.push_str(&format!(
        r#"<section class="diffnote-file" id="file-{id}"><details{open_attr}><summary><h2>{name}{binary}{rename}</h2></summary>"#,
        id = html_id(key),
        name = escape_html(key),
        binary = if is_binary { " (binary)" } else { "" },
        rename = if is_rename { " (renamed)" } else { "" },
    ));

    for t in file_threads {
        out.push_str(&render_thread_html(t, color_of));
    }

    match file_diff {
        None => {
            out.push_str(
                r#"<p class="diffnote-file__missing">このファイルは指定したdiffに含まれていません（コメント作成時点と異なるdiffを指定している可能性があります）。</p>"#,
            );
        }
        Some(file_diff) if !file_diff.hunks.is_empty() => {
            let syntax = guess_syntax(key, syntax_set);
            out.push_str(r#"<div class="diffnote-diff-scroll"><table class="diffnote-diff">"#);
            for hunk in &file_diff.hunks {
                out.push_str(&render_hunk(
                    key,
                    hunk,
                    syntax,
                    syntax_set,
                    theme,
                    by_line,
                    highlighted,
                    color_of,
                ));
            }
            out.push_str("</table></div>");
        }
        Some(_) => {}
    }

    if !outdated_threads.is_empty() {
        out.push_str(r#"<section class="diffnote-outdated"><h3>未配置のコメント</h3>"#);
        for t in outdated_threads {
            out.push_str(&render_outdated(t, color_of));
        }
        out.push_str("</section>");
    }

    out.push_str("</details></section>\n");
    out
}

#[allow(clippy::too_many_arguments)]
fn render_hunk(
    file: &str,
    hunk: &Hunk,
    syntax: &SyntaxReference,
    syntax_set: &SyntaxSet,
    theme: &Theme,
    by_line: &HashMap<(String, Side, u32), Vec<&Thread>>,
    highlighted: &HashMap<(String, Side, u32), Vec<Ulid>>,
    color_of: &HashMap<Ulid, usize>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        r#"<tr class="diffnote-hunk-header"><td colspan="3">@@ -{},{} +{},{} @@ {}</td></tr>"#,
        hunk.old_start,
        hunk.old_lines,
        hunk.new_start,
        hunk.new_lines,
        escape_html(hunk.section_heading.as_deref().unwrap_or(""))
    ));

    let mut highlighter = HighlightLines::new(syntax, theme);
    for line in &hunk.lines {
        let class = match line.kind {
            LineKind::Context => "diffnote-line--context",
            LineKind::Added => "diffnote-line--added",
            LineKind::Removed => "diffnote-line--removed",
        };
        // A line is part of a comment's range if it's marked for *either*
        // side -- a context line has both an old and a new line number, and
        // a comment could be anchored to whichever one it was written
        // against. A line can be covered by more than one (overlapping)
        // thread; each gets its own color band plus a shared hover hook.
        let mut covering: Vec<Ulid> = Vec::new();
        if let Some(l) = line.new_line
            && let Some(ids) = highlighted.get(&(file.to_string(), Side::New, l))
        {
            covering.extend(ids.iter().copied());
        }
        if let Some(l) = line.old_line
            && let Some(ids) = highlighted.get(&(file.to_string(), Side::Old, l))
        {
            covering.extend(ids.iter().copied());
        }
        covering.dedup();
        let (row_class, row_attrs) = commented_row_markup(class, &covering, color_of);
        let content_html = highlight_line(&mut highlighter, &line.content, syntax_set);
        out.push_str(&format!(
            r#"<tr class="{row_class}"{row_attrs}><td class="diffnote-line__gutter-old">{old}</td><td class="diffnote-line__gutter-new">{new}</td><td class="diffnote-line__content"><code>{content}</code></td></tr>"#,
            old = line
                .old_line
                .map(|n| n.to_string())
                .unwrap_or_default(),
            new = line
                .new_line
                .map(|n| n.to_string())
                .unwrap_or_default(),
            content = content_html,
        ));
        if let Some(l) = line.new_line
            && let Some(threads) = by_line.get(&(file.to_string(), Side::New, l))
        {
            for t in threads {
                out.push_str(&thread_row(t, color_of));
            }
        }
        if let Some(l) = line.old_line
            && let Some(threads) = by_line.get(&(file.to_string(), Side::Old, l))
        {
            for t in threads {
                out.push_str(&thread_row(t, color_of));
            }
        }
    }
    out
}

/// Colors assigned round-robin to threads, avoiding red/green (already used
/// for the added/removed diff backgrounds).
const PALETTE: [&str; 8] = [
    "#1f77b4", "#ff7f0e", "#9467bd", "#8c564b", "#e377c2", "#17becf", "#bcbd22", "#7f7f7f",
];

/// Builds a diff line's `<tr>` class list plus extra attributes (a stacked
/// `box-shadow` with one band per covering thread's color, and a
/// `data-diffnote-threads` list for the hover script) for however many
/// threads' ranges include this line -- zero, one, or several overlapping.
fn commented_row_markup(
    base_class: &str,
    covering: &[Ulid],
    color_of: &HashMap<Ulid, usize>,
) -> (String, String) {
    if covering.is_empty() {
        return (base_class.to_string(), String::new());
    }
    let class = format!("{base_class} diffnote-line--commented");
    let mut shadows = Vec::new();
    let mut ids = Vec::new();
    for (i, id) in covering.iter().enumerate() {
        let color = color_of.get(id).map(|c| PALETTE[*c]).unwrap_or("#999");
        let offset = 3 + i as u32 * 4;
        shadows.push(format!("inset {offset}px 0 0 0 {color}"));
        ids.push(id.to_string());
    }
    let attrs = format!(
        r#" style="box-shadow: {shadows}" data-diffnote-threads="{ids}""#,
        shadows = escape_html(&shadows.join(", ")),
        ids = ids.join(" "),
    );
    (class, attrs)
}

fn thread_row(t: &Thread, color_of: &HashMap<Ulid, usize>) -> String {
    format!(
        r#"<tr class="diffnote-thread-row"><td colspan="3">{}</td></tr>"#,
        render_thread_html(t, color_of)
    )
}

/// A short, explicit "which line(s) is this about" label, alongside the
/// `diffnote-line--commented` highlight -- not relying on color alone to
/// show a range comment's extent.
fn range_label(anchor: &Anchor) -> String {
    match anchor {
        Anchor::Global { .. } | Anchor::File { .. } => String::new(),
        Anchor::Span { base, head } => match head.as_ref().filter(|h| !h.is_empty()).or(base.as_ref()) {
            Some(side) if side.len() == 1 => format!("(L{})", side.start),
            Some(side) if side.len() > 1 => format!("(L{}\u{2013}L{})", side.start, side.end()),
            _ => String::new(),
        },
    }
}

fn render_thread_html(t: &Thread, color_of: &HashMap<Ulid, usize>) -> String {
    let mut out = String::new();
    let swatch = color_of.get(&t.root_id).map_or(String::new(), |c| {
        format!(
            r#"<span class="diffnote-thread__swatch" style="background:{}"></span>"#,
            PALETTE[*c]
        )
    });
    out.push_str(&format!(
        r#"<details class="diffnote-thread{resolved_class}" id="thread-{id}" data-diffnote-thread-id="{id}"{open}>"#,
        resolved_class = if t.resolved {
            " diffnote-thread--resolved"
        } else {
            ""
        },
        id = t.root_id,
        open = if t.resolved { "" } else { " open" },
    ));
    out.push_str(&format!(
        "<summary>{swatch}{} {}</summary>",
        if t.resolved {
            "解決済み"
        } else {
            "未解決"
        },
        range_label(&t.anchor),
    ));
    out.push_str(&render_comment_article(&t.author, &t.body));
    for r in &t.replies {
        out.push_str(&render_comment_article(&r.author, &r.body));
    }
    out.push_str("</details>");
    out
}

fn render_outdated(t: &Thread, color_of: &HashMap<Ulid, usize>) -> String {
    let mut out = String::new();
    out.push_str(r#"<div class="diffnote-outdated__entry">"#);
    if let Anchor::Span { base, head } = &t.anchor
        && let Some(side) = head.as_ref().filter(|h| !h.is_empty()).or(base.as_ref())
    {
        let context = &side.context;
        out.push_str(r#"<pre class="diffnote-outdated__snippet">"#);
        for line in context
            .before
            .iter()
            .chain(context.target.iter())
            .chain(context.after.iter())
        {
            out.push_str(&escape_html(line));
            out.push('\n');
        }
        out.push_str("</pre>");
    }
    out.push_str(&render_thread_html(t, color_of));
    out.push_str("</div>");
    out
}

fn render_comment_article(author: &str, body: &str) -> String {
    format!(
        r#"<article class="diffnote-comment"><p class="diffnote-comment__author">{}</p><div class="diffnote-comment__body">{}</div></article>"#,
        escape_html(author),
        markdown_to_html(body),
    )
}

fn markdown_to_html(body: &str) -> String {
    let mut out = String::new();
    md_push_html(&mut out, MdParser::new(body));
    out
}

fn highlight_line(
    highlighter: &mut HighlightLines,
    content: &str,
    syntax_set: &SyntaxSet,
) -> String {
    // syntect expects each line to keep its trailing newline for correct
    // stateful parsing (e.g. line comments).
    let mut line = content.to_string();
    line.push('\n');
    let Ok(ranges) = highlighter.highlight_line(&line, syntax_set) else {
        return escape_html(content);
    };
    styled_line_to_highlighted_html(&ranges[..], IncludeBackground::No)
        .unwrap_or_else(|_| escape_html(content))
}

fn guess_syntax<'a>(file: &str, syntax_set: &'a SyntaxSet) -> &'a SyntaxReference {
    std::path::Path::new(file)
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| syntax_set.find_syntax_by_extension(ext))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

fn html_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn wrap_document(body: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="utf-8">
<title>diffnote review</title>
<style>
{css}
</style>
</head>
<body>
<article class="diffnote-review">
{body}
</article>
<script>
{js}
</script>
</body>
</html>
"##,
        css = STYLE,
        js = SCRIPT,
    )
}

const STYLE: &str = r#"
:root {
  --diffnote-color-added-bg: #e6ffed;
  --diffnote-color-removed-bg: #ffeef0;
  --diffnote-color-context-bg: #ffffff;
  --diffnote-color-border: #d0d7de;
  --diffnote-color-thread-bg: #f6f8fa;
  --diffnote-color-resolved-bg: #f0f0f0;
  --diffnote-color-commented-border: #f9a825;
  --diffnote-font-mono: ui-monospace, SFMono-Regular, Consolas, monospace;
}
body { font-family: system-ui, sans-serif; margin: 0; padding: 1rem; }
.diffnote-review { max-width: 1000px; margin: 0 auto; }
.diffnote-filelist ul { list-style: none; padding: 0; }
.diffnote-revisions ul { list-style: none; padding: 0; display: flex; flex-wrap: wrap; gap: 0.5rem; }
.diffnote-revisions a { border: 1px solid var(--diffnote-color-border); border-radius: 0.4em; padding: 0.15em 0.6em; text-decoration: none; }
.diffnote-revisions a.is-current { background: var(--diffnote-color-thread-bg); font-weight: bold; }
.diffnote-revision { margin-top: 1.5rem; }
.diffnote-js .diffnote-revision:not(.is-current) { display: none; }
.diffnote-badge { background: var(--diffnote-color-thread-bg); border: 1px solid var(--diffnote-color-border); border-radius: 1em; padding: 0 0.5em; font-size: 0.85em; }
.diffnote-file { border: 1px solid var(--diffnote-color-border); border-radius: 6px; margin-bottom: 1rem; }
.diffnote-file > details > summary { padding: 0.5em 1em; cursor: pointer; }
.diffnote-file h2 { display: inline; font-size: 1em; font-family: var(--diffnote-font-mono); }
.diffnote-diff-scroll { overflow-x: auto; }
.diffnote-diff { width: 100%; border-collapse: collapse; font-family: var(--diffnote-font-mono); font-size: 0.85em; }
.diffnote-diff td { padding: 0 0.5em; white-space: pre; vertical-align: top; }
.diffnote-line__gutter-old, .diffnote-line__gutter-new { color: #999; text-align: right; user-select: none; width: 3em; }
.diffnote-line--added { background: var(--diffnote-color-added-bg); }
.diffnote-line--removed { background: var(--diffnote-color-removed-bg); }
.diffnote-line--commented { box-shadow: inset 3px 0 0 0 var(--diffnote-color-commented-border); }
.diffnote-line--commented .diffnote-line__gutter-old, .diffnote-line--commented .diffnote-line__gutter-new { color: var(--diffnote-color-commented-border); font-weight: bold; }
.diffnote-hunk-header td { background: var(--diffnote-color-thread-bg); color: #666; }
.diffnote-thread-row td { background: #fff; padding: 0.5em 1em; }
.diffnote-thread { border: 1px solid var(--diffnote-color-border); border-radius: 6px; background: var(--diffnote-color-thread-bg); padding: 0.3em 0.6em; margin: 0.3em 0; }
.diffnote-thread--resolved { background: var(--diffnote-color-resolved-bg); opacity: 0.8; }
.diffnote-thread summary { cursor: pointer; font-weight: bold; }
.diffnote-comment { border-top: 1px solid var(--diffnote-color-border); padding: 0.3em 0; }
.diffnote-comment:first-of-type { border-top: none; }
.diffnote-comment__author { font-weight: bold; margin: 0; font-size: 0.9em; }
.diffnote-comment__body { font-size: 0.95em; }
.diffnote-comment__body p:first-child { margin-top: 0; }
.diffnote-outdated { padding: 0.5em 1em; background: #fffbea; }
.diffnote-outdated__snippet { background: #fff; border: 1px dashed var(--diffnote-color-border); padding: 0.5em; font-family: var(--diffnote-font-mono); font-size: 0.85em; overflow-x: auto; }
.diffnote-file__missing { padding: 0.5em 1em; color: #7a5900; background: #fffbea; }
.diffnote-thread__swatch { display: inline-block; width: 0.8em; height: 0.8em; border-radius: 50%; margin-right: 0.4em; vertical-align: middle; }
.diffnote-line.diffnote-hover { outline: 2px solid #333; outline-offset: -2px; }
.diffnote-thread.diffnote-hover, .diffnote-thread-row .diffnote-hover { outline: 2px solid #333; }
"#;

/// Purely local DOM interaction: no fetch, no network, no storage -- safe
/// under a bare `file://` URL. Links a diff line's color band(s) to the
/// matching thread card(s) so hovering either highlights both, which is the
/// precise way to disambiguate overlapping range comments (their color
/// bands can only go so far once there are more threads than palette slots).
const SCRIPT: &str = r#"
(function () {
  var views = Array.prototype.slice.call(document.querySelectorAll('.diffnote-revision'));
  var links = Array.prototype.slice.call(document.querySelectorAll('[data-diffnote-revision-link]'));
  function show(i) {
    views.forEach(function (v, k) { v.classList.toggle('is-current', k === i); });
    links.forEach(function (a, k) { a.classList.toggle('is-current', k === i); });
  }
  if (views.length > 1) {
    document.documentElement.classList.add('diffnote-js');
    var start = views.length - 1;
    var m = /^#rev-(\d+)$/.exec(location.hash);
    if (m && +m[1] < views.length) start = +m[1];
    show(start);
    links.forEach(function (a, k) {
      a.addEventListener('click', function (e) { e.preventDefault(); show(k); });
    });
  }
  function idsOf(el) {
    var s = el.getAttribute('data-diffnote-threads') || el.getAttribute('data-diffnote-thread-id') || '';
    return s.split(' ').filter(Boolean);
  }
  function setHover(el, on) {
    idsOf(el).forEach(function (id) {
      var selector = '[data-diffnote-threads~="' + id + '"], [data-diffnote-thread-id="' + id + '"]';
      var scope = el.closest('.diffnote-revision') || document;
      scope.querySelectorAll(selector).forEach(function (match) {
        match.classList.toggle('diffnote-hover', on);
      });
    });
  }
  document.addEventListener('mouseover', function (e) {
    var el = e.target.closest('[data-diffnote-threads], [data-diffnote-thread-id]');
    if (el) setHover(el, true);
  });
  document.addEventListener('mouseout', function (e) {
    var el = e.target.closest('[data-diffnote-threads], [data-diffnote-thread-id]');
    if (el) setHover(el, false);
  });
})();
"#;
