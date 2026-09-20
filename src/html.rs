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
use crate::expand;
use crate::diff::{FileDiff, Hunk, LineKind, UnifiedDiff};
use crate::model::{Event, Side};
use crate::review::{Thread, build_threads};
use pulldown_cmark::{Parser as MdParser, html::push_html as md_push_html};
use std::collections::HashMap;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::{IncludeBackground, styled_line_to_highlighted_html};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use ulid::Ulid;

/// Per-view drawing state shared by the render functions.
#[derive(Default)]
struct Marks {
    /// A stable color (index into PALETTE) per thread drawn on lines.
    color_of: HashMap<Ulid, usize>,
    /// The lines a thread was written about, for threads shown as deleted
    /// (drawn at the point where the lines were) or unplaced.
    was: HashMap<Ulid, Vec<String>>,
    /// Threads whose lines this view's head doesn't have, and why.
    absent: HashMap<Ulid, anchor::Absence>,
    /// The lines (first, last) a thread sits on in this view, for its label:
    /// where it is here, not where it was written.
    lines: HashMap<Ulid, (u32, u32)>,
}

/// Renders a whole bundle: one view per recorded revision that has a diff
/// (a fresh `init` snapshot has none), oldest first.
pub fn render_bundle(loaded: &crate::bundle::Loaded) -> anyhow::Result<String> {
    let mut parsed = Vec::new();
    for revision in loaded.revisions() {
        let Some(text) = loaded
            .revision_diff(revision)
            .filter(|t| !t.trim().is_empty())
        else {
            continue;
        };
        let diff = crate::diff::parse(&text).map_err(|e| anyhow::anyhow!("{e}"))?;
        let source = match &revision.source {
            crate::model::Source::Git(g) => g.spec.clone(),
            crate::model::Source::Files { .. } => "ディレクトリ".to_string(),
        };
        let label = format!(
            "#{} {source} ({})",
            parsed.len() + 1,
            revision.created_at.date()
        );
        parsed.push((label, diff, revision, loaded.manifest(revision)));
    }
    if parsed.is_empty() {
        anyhow::bail!("バンドルに記録された差分がありません");
    }
    let views: Vec<RevisionView> = parsed
        .iter()
        .map(|(label, diff, revision, tree)| RevisionView {
            label: label.clone(),
            diff,
            files: &revision.files,
            tree,
        })
        .collect();
    Ok(render(&loaded.events, &views, &loaded.blobs()))
}

/// One revision of the review to show: its diff and per-file digests, and a
/// short label for the switcher.
pub struct RevisionView<'a> {
    pub label: String,
    pub diff: &'a UnifiedDiff,
    pub files: &'a [crate::model::FileDigest],
    /// The head tree the revision recorded (for files its diff leaves alone).
    pub tree: &'a [crate::model::TreeFile],
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
    body.push_str(r#"<div class="diffnote-topbar"><header class="diffnote-summary"><h1>diffnote レビュー</h1>"#);
    body.push_str(&format!(
        "<p>スレッド {} 件(解決済み {} 件)</p></header>\n",
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
    body.push_str("</div>\n");
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
    let versions = anchor::ViewVersions {
        files: view.files,
        tree: view.tree,
    };
    // Every thread is placed by where its lines are; the diff shown is the
    // view's diff plus context around whatever it doesn't already show.
    let placements: Vec<Placement> = threads
        .iter()
        .map(|t| anchor::resolve_placement(&t.anchor, view.diff, &versions, blobs))
        .collect();
    let wants: Vec<expand::Want> = placements.iter().filter_map(expand::want_of).collect();
    let (expanded, _) = expand::expand(view.diff, &wants, &versions, blobs);
    let diff = &expanded;
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
    let mut marks = Marks::default();
    let mut outdated: HashMap<String, Vec<&Thread>> = HashMap::new();

    for (thread, placement) in threads.iter().zip(placements) {
        // A card is drawn after a row of the diff; without one (the text to
        // build it from isn't held) the thread is listed as unplaced.
        let placement = match expand::want_of(&placement) {
            Some(w) if !expand::has_row(diff, &w) => Placement::Unplaced { file: w.file },
            _ => placement,
        };
        match placement {
            Placement::Global => global.push(thread),
            Placement::File(file) => by_file.entry(file).or_default().push(thread),
            Placement::Line {
                file,
                side,
                line_start,
                line_end,
                old_range,
            } => {
                let color = marks.color_of.len() % PALETTE.len();
                marks.color_of.insert(thread.root_id, color);
                marks.lines.insert(thread.root_id, (line_start, line_end));
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
            Placement::Point {
                file,
                before,
                was,
                kind,
            } => {
                marks.absent.insert(thread.root_id, kind);
                marks.was.insert(thread.root_id, was);
                by_line
                    .entry((file, Side::New, before.saturating_sub(1).max(1)))
                    .or_default()
                    .push(thread);
            }
            Placement::Unplaced { file } => {
                marks
                    .was
                    .insert(thread.root_id, anchor::original_text(&thread.anchor, blobs));
                outdated.entry(file).or_default().push(thread)
            }
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
            body.push_str(&render_thread_html(t, &marks));
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
            &marks,
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
    marks: &Marks,
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
        binary = if is_binary { " (バイナリ)" } else { "" },
        rename = if is_rename { " (名前変更)" } else { "" },
    ));

    for t in file_threads {
        out.push_str(&render_thread_html(t, marks));
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
                    marks,
                ));
            }
            out.push_str("</table></div>");
        }
        Some(_) => {}
    }

    if !outdated_threads.is_empty() {
        out.push_str(r#"<section class="diffnote-outdated"><h3>未配置のコメント</h3>"#);
        for t in outdated_threads {
            out.push_str(&render_outdated(t, marks));
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
    marks: &Marks,
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
        let (row_class, row_attrs) = commented_row_markup(class, &covering, marks);
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
                out.push_str(&thread_row(t, marks));
            }
        }
        if let Some(l) = line.old_line
            && let Some(threads) = by_line.get(&(file.to_string(), Side::Old, l))
        {
            for t in threads {
                out.push_str(&thread_row(t, marks));
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
    marks: &Marks,
) -> (String, String) {
    if covering.is_empty() {
        return (base_class.to_string(), String::new());
    }
    let class = format!("{base_class} diffnote-line--commented");
    let mut bars = Vec::new();
    let mut ids = Vec::new();
    for (i, id) in covering.iter().enumerate() {
        let color = marks.color_of.get(id).map(|c| PALETTE[*c]).unwrap_or("#999");
        let offset = 3 + i as u32 * 4;
        bars.push(format!("inset {offset}px 0 0 0 {color}"));
        ids.push(id.to_string());
    }
    let attrs = format!(
        r#" style="--diffnote-bars: {bars}" data-diffnote-threads="{ids}""#,
        bars = escape_html(&bars.join(", ")),
        ids = ids.join(" "),
    );
    (class, attrs)
}

fn thread_row(t: &Thread, marks: &Marks) -> String {
    format!(
        r#"<tr class="diffnote-thread-row"><td colspan="3">{}</td></tr>"#,
        render_thread_html(t, marks)
    )
}

/// A short, explicit "which line(s) is this about" label, alongside the
/// `diffnote-line--commented` highlight -- not relying on color alone to
/// show a range comment's extent.
fn range_label(lines: Option<&(u32, u32)>) -> String {
    match lines {
        Some(&(a, b)) if a == b => format!("(L{a})"),
        Some(&(a, b)) => format!("(L{a}\u{2013}L{b})"),
        None => String::new(),
    }
}

fn render_thread_html(t: &Thread, marks: &Marks) -> String {
    let mut out = String::new();
    let swatch = marks.color_of.get(&t.root_id).map_or(String::new(), |c| {
        format!(
            r#"<span class="diffnote-thread__swatch" style="background:{}"></span>"#,
            PALETTE[*c]
        )
    });
    out.push_str(&format!(
        r#"<details class="diffnote-thread{resolved_class}" id="thread-{id}" data-diffnote-thread-id="{id}" data-diffnote-color="{color}"{open}>"#,
        resolved_class = if t.resolved {
            " diffnote-thread--resolved"
        } else {
            ""
        },
        id = t.root_id,
        color = marks.color_of.get(&t.root_id).map_or("#57606a", |c| PALETTE[*c]),
        open = if t.resolved { "" } else { " open" },
    ));
    out.push_str(&format!(
        "<summary>{swatch}{} {}{}</summary>",
        if t.resolved {
            "解決済み"
        } else {
            "未解決"
        },
        range_label(marks.lines.get(&t.root_id)),
        match marks.absent.get(&t.root_id) {
            Some(anchor::Absence::Deleted) => " (削除された行)",
            Some(anchor::Absence::NotYet) => " (この版にはまだない行)",
            Some(anchor::Absence::Unknown) => " (この版にない行)",
            None => "",
        },
    ));
    if marks.absent.contains_key(&t.root_id) {
        out.push_str(&render_snippet(marks.was.get(&t.root_id), "diffnote-deleted__snippet"));
    }
    out.push_str(&render_comment_article(&t.author, &t.body));
    for r in &t.replies {
        out.push_str(&render_comment_article(&r.author, &r.body));
    }
    out.push_str("</details>");
    out
}

fn render_snippet(lines: Option<&Vec<String>>, class: &str) -> String {
    let Some(lines) = lines.filter(|l| !l.is_empty()) else {
        return String::new();
    };
    let mut out = format!(r#"<pre class="{class}">"#);
    for line in lines {
        out.push_str(&escape_html(line));
        out.push('\n');
    }
    out.push_str("</pre>");
    out
}

fn render_outdated(t: &Thread, marks: &Marks) -> String {
    let mut out = String::new();
    out.push_str(r#"<div class="diffnote-outdated__entry">"#);
    out.push_str(&render_snippet(marks.was.get(&t.root_id), "diffnote-outdated__snippet"));
    out.push_str(&render_thread_html(t, marks));
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
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>diffnote レビュー</title>
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
  --diffnote-color-fg: #1f2328;
  --diffnote-color-muted: #59636e;
  --diffnote-color-bg: #ffffff;
  --diffnote-color-added-bg: #e6ffec;
  --diffnote-color-added-gutter: #ccffd8;
  --diffnote-color-removed-bg: #ffebe9;
  --diffnote-color-removed-gutter: #ffd7d5;
  --diffnote-color-context-bg: #ffffff;
  --diffnote-color-gutter: #f6f8fa;
  --diffnote-color-border: #d1d9e0;
  --diffnote-color-thread-bg: #f6f8fa;
  --diffnote-color-resolved-bg: #eef0f2;
  --diffnote-color-hunk-bg: #ddf4ff;
  --diffnote-color-hunk-fg: #0550ae;
  --diffnote-color-accent: #0969da;
  --diffnote-color-commented-border: #f9a825;
  --diffnote-font: -apple-system, BlinkMacSystemFont, "Segoe UI", "Hiragino Sans", "Yu Gothic UI", "Noto Sans JP", sans-serif;
  --diffnote-font-mono: ui-monospace, SFMono-Regular, Menlo, Consolas, "Noto Sans Mono CJK JP", monospace;
  --diffnote-topbar-h: 44px;
}
* { box-sizing: border-box; }
body { font-family: var(--diffnote-font); font-size: 14px; line-height: 1.5; color: var(--diffnote-color-fg); background: var(--diffnote-color-bg); margin: 0; }
.diffnote-review { max-width: none; margin: 0; }

/* Top bar: title, counts and revision tabs, always in view. */
.diffnote-topbar { position: sticky; top: 0; z-index: 10; display: flex; align-items: center; gap: 1.5rem; height: var(--diffnote-topbar-h); padding: 0 16px; background: var(--diffnote-color-gutter); border-bottom: 1px solid var(--diffnote-color-border); }
.diffnote-summary { display: flex; align-items: baseline; gap: 1rem; white-space: nowrap; }
.diffnote-summary h1 { font-size: 15px; margin: 0; }
.diffnote-summary p { margin: 0; color: var(--diffnote-color-muted); font-size: 13px; }
.diffnote-revisions { min-width: 0; overflow-x: auto; }
.diffnote-revisions ul { list-style: none; margin: 0; padding: 0; display: flex; gap: 4px; }
.diffnote-revisions a { display: block; white-space: nowrap; color: var(--diffnote-color-fg); text-decoration: none; border: 1px solid transparent; border-radius: 6px; padding: 3px 10px; font-size: 13px; }
.diffnote-revisions a:hover { background: var(--diffnote-color-border); }
.diffnote-revisions a.is-current { background: var(--diffnote-color-bg); border-color: var(--diffnote-color-border); font-weight: 600; }

/* A revision: file list on the left, the changes on the right. */
.diffnote-revision { display: grid; grid-template-columns: 280px minmax(0, 1fr); column-gap: 16px; align-items: start; padding: 0 16px 48px; }
.diffnote-revision > * { grid-column: 2; min-width: 0; }
.diffnote-revision__title { display: none; }
.diffnote-js .diffnote-revision:not(.is-current) { display: none; }
.diffnote-filelist { grid-column: 1; grid-row: 1 / span 200; position: sticky; top: calc(var(--diffnote-topbar-h) + 12px); max-height: calc(100vh - var(--diffnote-topbar-h) - 24px); overflow: auto; margin-top: 12px; border-right: 1px solid var(--diffnote-color-border); padding-right: 8px; font-size: 12.5px; }
.diffnote-filelist ul { list-style: none; margin: 0; padding: 0; }
.diffnote-filelist li { display: flex; align-items: center; justify-content: space-between; gap: 6px; border-radius: 6px; }
.diffnote-filelist a { flex: 1; min-width: 0; padding: 3px 8px; color: var(--diffnote-color-fg); text-decoration: none; font-family: var(--diffnote-font-mono); word-break: break-all; border-radius: 6px; }
.diffnote-filelist a:hover { background: var(--diffnote-color-gutter); }
.diffnote-filelist a.is-visible { background: var(--diffnote-color-hunk-bg); }
.diffnote-badge { background: var(--diffnote-color-accent); color: #fff; border-radius: 1em; padding: 0 7px; font-size: 11px; line-height: 18px; }

.diffnote-global-comments { margin-top: 12px; }

/* Files: a sticky header bar per file, like a code review page. */
.diffnote-file { border: 1px solid var(--diffnote-color-border); border-radius: 6px; margin-top: 12px; scroll-margin-top: calc(var(--diffnote-topbar-h) + 8px); }
.diffnote-file > details > summary { position: sticky; top: var(--diffnote-topbar-h); z-index: 5; padding: 6px 12px; cursor: pointer; background: var(--diffnote-color-gutter); border-bottom: 1px solid var(--diffnote-color-border); border-radius: 6px 6px 0 0; }
.diffnote-file h2 { display: inline; font-size: 13px; font-weight: 600; font-family: var(--diffnote-font-mono); }
.diffnote-file__missing { margin: 0; padding: 8px 12px; color: #7a5900; background: #fffbea; }
.diffnote-diff-scroll { overflow-x: auto; }
.diffnote-diff { width: 100%; border-collapse: collapse; font-family: var(--diffnote-font-mono); font-size: 12.5px; line-height: 20px; }
.diffnote-diff td { padding: 0; white-space: pre; vertical-align: top; --dn-l: 0 0 0 0 transparent; --dn-r: 0 0 0 0 transparent; --dn-t: 0 0 0 0 transparent; --dn-b: 0 0 0 0 transparent; box-shadow: var(--dn-l), var(--dn-r), var(--dn-t), var(--dn-b); }
.diffnote-line__gutter-old, .diffnote-line__gutter-new { width: 1%; min-width: 3.4em; padding: 0 8px !important; text-align: right; color: var(--diffnote-color-muted); background: var(--diffnote-color-gutter); user-select: none; }
.diffnote-line__content { width: 100%; padding: 0 12px 0 22px !important; position: relative; }
.diffnote-line__content::before { position: absolute; left: 8px; color: var(--diffnote-color-muted); }
.diffnote-line--added { background: var(--diffnote-color-added-bg); }
.diffnote-line--added .diffnote-line__gutter-old, .diffnote-line--added .diffnote-line__gutter-new { background: var(--diffnote-color-added-gutter); }
.diffnote-line--added .diffnote-line__content::before { content: "+"; }
.diffnote-line--removed { background: var(--diffnote-color-removed-bg); }
.diffnote-line--removed .diffnote-line__gutter-old, .diffnote-line--removed .diffnote-line__gutter-new { background: var(--diffnote-color-removed-gutter); }
.diffnote-line--removed .diffnote-line__content::before { content: "-"; }
.diffnote-hunk-header td { background: var(--diffnote-color-hunk-bg); color: var(--diffnote-color-hunk-fg); padding: 2px 12px; }

/* Lines a comment is about: colored bars at the left edge... */
.diffnote-line--commented > td:first-child { --dn-l: var(--diffnote-bars); }
.diffnote-line--commented .diffnote-line__gutter-old, .diffnote-line--commented .diffnote-line__gutter-new { color: var(--diffnote-color-fg); font-weight: 600; }
/* ...and, while a comment is hovered, focused or pinned, its whole range: a
   tint in the comment's color inside an outline that runs from the first line
   to the last, and the comment itself outlined in the same color. */
.diffnote-diff .diffnote-range > td { background-image: linear-gradient(color-mix(in srgb, var(--rc) 14%, transparent), color-mix(in srgb, var(--rc) 14%, transparent)); }
.diffnote-range > td:first-child { --dn-l: inset 4px 0 0 0 var(--rc); }
.diffnote-range > td:last-child { --dn-r: inset -3px 0 0 0 var(--rc); }
.diffnote-range-first > td { --dn-t: inset 0 3px 0 0 var(--rc); }
.diffnote-range-last > td { --dn-b: inset 0 -3px 0 0 var(--rc); }
.diffnote-thread.diffnote-hover { outline: 2px solid var(--rc); outline-offset: 1px; }

/* Comment cards sit under the last line of their range, lined up with the code. */
.diffnote-thread-row > td { background: var(--diffnote-color-bg); padding: 4px 16px 4px 124px !important; white-space: normal; font-family: var(--diffnote-font); font-size: 14px; line-height: 1.5; }
.diffnote-thread { border: 1px solid var(--diffnote-color-border); border-radius: 6px; background: var(--diffnote-color-thread-bg); padding: 4px 12px; margin: 3px 0; max-width: 960px; }
.diffnote-thread--resolved { background: var(--diffnote-color-resolved-bg); color: var(--diffnote-color-muted); }
.diffnote-thread summary { cursor: pointer; font-weight: 600; font-size: 13px; }
.diffnote-thread__swatch { display: inline-block; width: 10px; height: 10px; border-radius: 50%; margin-right: 6px; vertical-align: baseline; }
.diffnote-comment { border-top: 1px solid var(--diffnote-color-border); padding: 6px 0; }
.diffnote-comment:first-of-type { border-top: none; }
.diffnote-comment__author { font-weight: 600; margin: 0; font-size: 12.5px; color: var(--diffnote-color-muted); }
.diffnote-comment__body { font-size: 14px; }
.diffnote-comment__body p { margin: 2px 0; }
.diffnote-comment__body pre { overflow-x: auto; background: var(--diffnote-color-bg); border: 1px solid var(--diffnote-color-border); border-radius: 6px; padding: 8px 12px; font-family: var(--diffnote-font-mono); font-size: 12.5px; }
.diffnote-comment__body code { font-family: var(--diffnote-font-mono); font-size: 12.5px; }
.diffnote-deleted__snippet { margin: 6px 0; padding: 4px 8px; background: var(--diffnote-color-removed-bg); font-family: var(--diffnote-font-mono); font-size: 12.5px; white-space: pre-wrap; border-radius: 4px; }
.diffnote-outdated { padding: 8px 12px; background: #fffbea; border-top: 1px solid var(--diffnote-color-border); }
.diffnote-outdated h3 { margin: 0 0 4px; font-size: 13px; }
.diffnote-outdated__snippet { background: #fff; border: 1px dashed var(--diffnote-color-border); padding: 6px 8px; font-family: var(--diffnote-font-mono); font-size: 12.5px; overflow-x: auto; }
.diffnote-file > details > .diffnote-thread { margin: 8px 12px; }

@media (max-width: 900px) {
  .diffnote-topbar { height: auto; flex-wrap: wrap; gap: 4px 12px; padding: 6px 12px; }
  .diffnote-revision { display: block; padding: 0 8px 32px; }
  .diffnote-filelist { position: static; max-height: 40vh; margin: 8px 0; border-right: 0; }
  .diffnote-file > details > summary { top: 0; position: static; }
  .diffnote-thread-row > td { padding-left: 12px !important; }
}
"#;

/// Purely local DOM interaction: no fetch, no network, no storage -- safe
/// under a bare `file://` URL. Switches revisions, shows the range of the
/// comment under the mouse (or pinned by a click) on its lines and outlines
/// its card -- the precise way to tell overlapping ranges apart -- and marks
/// which files are on screen in the file list.
const SCRIPT: &str = r#"
(function () {
  var slice = Array.prototype.slice;
  var views = slice.call(document.querySelectorAll('.diffnote-revision'));
  var links = slice.call(document.querySelectorAll('[data-diffnote-revision-link]'));

  // --- Which comment's range is shown ------------------------------------
  // One at a time: the one under the mouse or focus, or the pinned one
  // (click a comment or one of its lines; click elsewhere or Esc to let go).
  var THREAD = '[data-diffnote-thread-id]';
  var LINE = 'tr[data-diffnote-threads]';
  var active = null;
  var pinned = null;

  function scopeOf(el) { return el.closest('.diffnote-revision') || document; }
  function cardOf(scope, id) { return scope.querySelector('[data-diffnote-thread-id="' + id + '"]'); }
  function rowsOf(scope, id) { return slice.call(scope.querySelectorAll('tr[data-diffnote-threads~="' + id + '"]')); }

  function clear() {
    if (!active) return;
    active.rows.forEach(function (r) {
      r.classList.remove('diffnote-range', 'diffnote-range-first', 'diffnote-range-last');
      r.style.removeProperty('--rc');
    });
    if (active.card) {
      active.card.classList.remove('diffnote-hover');
      active.card.style.removeProperty('--rc');
    }
    active = null;
  }

  function activate(scope, id) {
    if (active && active.id === id && active.scope === scope) return;
    clear();
    var rows = rowsOf(scope, id);
    var card = cardOf(scope, id);
    var color = (card && card.getAttribute('data-diffnote-color')) || '#0969da';
    rows.forEach(function (r, k) {
      r.classList.add('diffnote-range');
      r.style.setProperty('--rc', color);
      if (k === 0) r.classList.add('diffnote-range-first');
      if (k === rows.length - 1) r.classList.add('diffnote-range-last');
    });
    if (card) {
      card.classList.add('diffnote-hover');
      card.style.setProperty('--rc', color);
    }
    active = { id: id, scope: scope, rows: rows, card: card };
  }

  // A line can be in several ranges: take the smallest, the most specific.
  function pick(scope, el) {
    var own = el.getAttribute('data-diffnote-thread-id');
    if (own) return own;
    var ids = (el.getAttribute('data-diffnote-threads') || '').split(' ').filter(Boolean);
    var best = null, size = Infinity;
    ids.forEach(function (id) {
      var n = rowsOf(scope, id).length;
      if (n < size) { best = id; size = n; }
    });
    return best;
  }

  function target(node) {
    return node && node.closest ? node.closest(THREAD + ', ' + LINE) : null;
  }

  document.addEventListener('mouseover', function (e) {
    if (pinned) return;
    var el = target(e.target);
    if (!el) return;
    var id = pick(scopeOf(el), el);
    if (id) activate(scopeOf(el), id);
  });
  document.addEventListener('mouseout', function (e) {
    if (pinned) return;
    if (target(e.relatedTarget)) return;
    clear();
  });
  document.addEventListener('focusin', function (e) {
    if (pinned) return;
    var el = target(e.target);
    if (!el) return;
    var id = pick(scopeOf(el), el);
    if (id) activate(scopeOf(el), id);
  });
  document.addEventListener('click', function (e) {
    var el = target(e.target);
    if (!el) { pinned = null; clear(); return; }
    var scope = scopeOf(el);
    var id = pick(scope, el);
    if (!id) return;
    if (pinned === id) { pinned = null; return; }
    pinned = id;
    activate(scope, id);
  });
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape') { pinned = null; clear(); }
  });

  // --- Revisions ---------------------------------------------------------
  function show(i) {
    pinned = null;
    clear();
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

  // --- The file list follows what is on screen ---------------------------
  if ('IntersectionObserver' in window) {
    views.forEach(function (v) {
      var byId = {};
      slice.call(v.querySelectorAll('.diffnote-filelist a')).forEach(function (a) {
        byId[(a.getAttribute('href') || '').slice(1)] = a;
      });
      var io = new IntersectionObserver(function (entries) {
        entries.forEach(function (en) {
          var a = byId[en.target.id];
          if (a) a.classList.toggle('is-visible', en.isIntersecting);
        });
      }, { rootMargin: '-48px 0px -55% 0px' });
      slice.call(v.querySelectorAll('.diffnote-file')).forEach(function (f) { io.observe(f); });
    });
  }
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{self, Additions};
    use crate::digest::digest;
    use crate::files::{Tree, diff_trees};
    use crate::model::{Anchor, FileRef, GitSource, LineRange, Revision, SnapshotMode, Source};
    use time::OffsetDateTime;

    const R1_BASE: &str = "a\nb\nc\nd\n";
    const R1_HEAD: &str = "a\nB\nc\nd\n";
    const R2_HEAD: &str = "top\na\nB\nc\nD\n";

    fn tree(text: &str) -> Tree {
        [("f.txt".to_string(), text.as_bytes().to_vec())].into()
    }

    fn range(start: u32, len: u32, text: &str) -> LineRange {
        LineRange {
            file: "f.txt".to_string(),
            digest: digest(text),
            start,
            len,
        }
    }

    fn comment(id: Ulid, anchor: Anchor, body: &str) -> Event {
        Event::Comment {
            id,
            parent: None,
            author: "r@example.com".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            anchor: Some(anchor),
            body: body.into(),
        }
    }

    /// Saves and reloads a bundle with one revision per `(base, head, source)`
    /// (each with its diff and both file versions snapshotted), then all of
    /// `extra` events.
    fn bundle_of(
        revisions: &[(&str, &str, Source)],
        extra: Vec<Event>,
    ) -> (tempfile::TempDir, bundle::Loaded) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.diffnote");
        let mut events = vec![Event::Meta {
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            description: None,
            context_lines: 3,
        }];
        for (i, (base, head, source)) in revisions.iter().enumerate() {
            let (diff_text, files) = diff_trees(&tree(base), &tree(head));
            let key = digest(format!("revision {i}"));
            events.push(Event::Revision(Revision {
                id: Ulid::new(),
                created_at: OffsetDateTime::UNIX_EPOCH,
                digest: key.clone(),
                source: source.clone(),
                snapshot_mode: SnapshotMode::Full,
                files,
                tree: Vec::new(),
            }));
            let additions = Additions {
                diff: Some((key, diff_text)),
                blobs: vec![head.as_bytes().to_vec(), base.as_bytes().to_vec()],
            };
            let loaded = bundle::load(&path).unwrap();
            bundle::save(&path, &loaded, &events, &additions).unwrap();
        }
        events.extend(extra);
        let loaded = bundle::load(&path).unwrap();
        bundle::save(&path, &loaded, &events, &Additions::default()).unwrap();
        (dir, bundle::load(&path).unwrap())
    }

    fn files_source(base: Option<&str>) -> Source {
        Source::Files {
            base: base.map(str::to_string),
        }
    }

    /// The HTML of view `i`.
    fn view(html: &str, i: usize) -> &str {
        let start = html
            .find(&format!(r#"id="rev-{i}""#))
            .unwrap_or_else(|| panic!("no view {i}"));
        let rest = &html[start..];
        let end = rest[1..]
            .find(r#"<section class="diffnote-revision"#)
            .map_or(rest.len(), |e| e + 1);
        // The whole review sits in one <article>; comments have their own.
        let end = rest[..end].find("\n</article>\n<script>").unwrap_or(end);
        &rest[..end]
    }

    /// The (old, new) gutter numbers of the diff rows highlighted for `id`.
    fn rows_of(view: &str, id: Ulid) -> Vec<(String, String)> {
        let marker = format!(r#"data-diffnote-threads="{id}""#);
        let cell = |row: &str, class: &str| {
            let at = row.find(&format!(r#"class="{class}">"#)).unwrap()
                + class.len()
                + r#"class="">"#.len();
            row[at..row[at..].find("</td>").unwrap() + at].to_string()
        };
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(at) = view[from..].find(&marker) {
            let at = from + at;
            let start = view[..at].rfind("<tr").unwrap();
            let end = view[at..].find("</tr>").unwrap() + at;
            let row = &view[start..end];
            out.push((
                cell(row, "diffnote-line__gutter-old"),
                cell(row, "diffnote-line__gutter-new"),
            ));
            from = end;
        }
        out
    }

    fn all_ids(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = html;
        while let Some(at) = rest.find(r#" id=""#) {
            rest = &rest[at + 5..];
            out.push(rest[..rest.find('"').unwrap()].to_string());
        }
        out
    }

    struct Scenario {
        html: String,
        t1: Ulid,
        t2: Ulid,
        t3: Ulid,
        t4: Ulid,
        global: Ulid,
        _dir: tempfile::TempDir,
    }

    /// Two revisions of one file, and threads made on each of them.
    fn scenario() -> Scenario {
        let (t1, t2, t3, t4, global) = (
            Ulid::new(),
            Ulid::new(),
            Ulid::new(),
            Ulid::new(),
            Ulid::new(),
        );
        let events = vec![
            // Written on revision 1: `b` became `B`.
            comment(
                t1,
                Anchor::Span {
                    base: Some(range(2, 1, R1_BASE)),
                    head: Some(range(2, 1, R1_HEAD)),
                },
                "about B",
            ),
            Event::Comment {
                id: Ulid::new(),
                parent: Some(t1),
                author: "author@example.com".into(),
                created_at: OffsetDateTime::UNIX_EPOCH,
                anchor: None,
                body: "a reply".into(),
            },
            // Written on revision 2: `d` became `D`.
            comment(
                t2,
                Anchor::Span {
                    base: Some(range(4, 1, R1_HEAD)),
                    head: Some(range(5, 1, R2_HEAD)),
                },
                "about D",
            ),
            // A whole-file thread that has been resolved.
            comment(
                t3,
                Anchor::File {
                    base: None,
                    head: Some(FileRef {
                        file: "f.txt".into(),
                        digest: digest(R1_HEAD),
                    }),
                },
                "file thread",
            ),
            Event::Resolve {
                parent: t3,
                author: "r@example.com".into(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            },
            // Written on revision 2, about a line revision 1 doesn't have.
            comment(
                t4,
                Anchor::Span {
                    base: Some(range(1, 0, R1_HEAD)),
                    head: Some(range(1, 1, R2_HEAD)),
                },
                "about top",
            ),
            comment(
                global,
                Anchor::Global {
                    base: None,
                    head: Some("rev".into()),
                },
                "overall",
            ),
        ];
        let (dir, loaded) = bundle_of(
            &[
                (R1_BASE, R1_HEAD, files_source(None)),
                (R1_HEAD, R2_HEAD, files_source(Some("x"))),
            ],
            events,
        );
        Scenario {
            html: render_bundle(&loaded).unwrap(),
            t1,
            t2,
            t3,
            t4,
            global,
            _dir: dir,
        }
    }

    #[test]
    fn every_revision_is_a_view_and_the_latest_is_the_current_one() {
        let s = scenario();
        assert_eq!(s.html.matches(r#"<section class="diffnote-revision"#).count(), 2);
        assert!(view(&s.html, 1).starts_with(r#"id="rev-1" data-diffnote-revision="1""#));
        assert!(s.html.contains(r#"class="diffnote-revision is-current" id="rev-1""#));
        assert!(s.html.contains(r#"class="diffnote-revision" id="rev-0""#));
        // The switcher links to both, and the script only switches views.
        assert!(s.html.contains(r##"href="#rev-0""##));
        assert!(s.html.contains(r##"href="#rev-1""##));
        assert!(s.html.contains("diffnote-js"));
    }

    #[test]
    fn element_ids_are_unique_across_the_views() {
        let s = scenario();
        let ids = all_ids(&s.html);
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "duplicate ids in {ids:?}");
        assert!(ids.contains(&"r0-file-f-txt".to_string()));
        assert!(ids.contains(&"r1-file-f-txt".to_string()));
        // File-list links point at their own view's file section.
        assert!(view(&s.html, 0).contains(r##"href="#r0-file-f-txt""##));
        assert!(view(&s.html, 1).contains(r##"href="#r1-file-f-txt""##));
    }

    #[test]
    fn every_thread_appears_in_every_view() {
        let s = scenario();
        for id in [s.t1, s.t2, s.t3, s.t4, s.global] {
            for i in 0..2 {
                assert_eq!(
                    view(&s.html, i)
                        .matches(&format!(r#"data-diffnote-thread-id="{id}""#))
                        .count(),
                    1,
                    "thread {id} in view {i}"
                );
            }
        }
        for i in 0..2 {
            assert!(view(&s.html, i).contains("a reply"), "reply in view {i}");
        }
    }

    #[test]
    fn a_thread_is_at_its_exact_lines_in_the_revision_it_was_made_on() {
        let s = scenario();
        // Revision 1: `b` (old 2) became `B` (new 2).
        let rows = rows_of(view(&s.html, 0), s.t1);
        assert!(rows.contains(&("2".into(), "".into())), "{rows:?}");
        assert!(rows.contains(&("".into(), "2".into())), "{rows:?}");
        // Revision 2: `d` (old 4) became `D` (new 5).
        let rows = rows_of(view(&s.html, 1), s.t2);
        assert!(rows.contains(&("4".into(), "".into())), "{rows:?}");
        assert!(rows.contains(&("".into(), "5".into())), "{rows:?}");
    }

    #[test]
    fn a_thread_from_another_revision_follows_its_lines_into_the_view() {
        let s = scenario();
        // Revision 1's `B` is now a context line, at old 2 / new 3.
        let rows = rows_of(view(&s.html, 1), s.t1);
        assert_eq!(rows, vec![("2".to_string(), "3".to_string())]);
        // Revision 2's `d` -> `D` shows in revision 1 on the unchanged `d`
        // (old 4 / new 4): the new text doesn't exist there.
        let rows = rows_of(view(&s.html, 0), s.t2);
        assert_eq!(rows, vec![("4".to_string(), "4".to_string())]);
    }

    /// The text of a thread card's summary line in a view.
    fn summary_of(view: &str, id: Ulid) -> String {
        let from = view.find(&format!(r#"data-diffnote-thread-id="{id}""#)).unwrap();
        let rest = &view[from..];
        let start = rest.find("<summary>").unwrap();
        rest[start..rest.find("</summary>").unwrap()].to_string()
    }

    #[test]
    fn a_threads_line_label_is_where_it_is_in_this_view() {
        let s = scenario();
        // `B` is line 2 in revision 1 and line 3 in revision 2 (`top` came
        // first), whichever revision the thread was written on.
        assert!(summary_of(view(&s.html, 0), s.t1).contains("(L2)"));
        assert!(summary_of(view(&s.html, 1), s.t1).contains("(L3)"));
    }

    #[test]
    fn the_title_and_revision_tabs_share_one_bar() {
        let s = scenario();
        let bar = &s.html[s.html.find(r#"<div class="diffnote-topbar">"#).unwrap()..];
        let bar = &bar[..bar.find("</div>").unwrap()];
        assert!(bar.contains(r#"<header class="diffnote-summary">"#), "{bar}");
        assert!(bar.contains(r#"<nav class="diffnote-revisions">"#), "{bar}");
    }

    #[test]
    fn a_thread_card_carries_its_color_for_the_range_highlight() {
        let s = scenario();
        let card = format!(r##"data-diffnote-thread-id="{}" data-diffnote-color="#"##, s.t1);
        assert!(view(&s.html, 0).contains(&card), "{card}");
        // The lines it covers name the thread, so the script can find them.
        let rows = view(&s.html, 0)
            .matches(&format!(r#"data-diffnote-threads="{}"#, s.t1))
            .count();
        assert!(rows >= 1);
    }

    #[test]
    fn line_bars_are_a_custom_property_not_a_box_shadow_on_the_row() {
        // The gutters have their own background, so the bars are drawn by the
        // first cell from `--diffnote-bars`; an inline box-shadow on the row
        // would sit under it.
        let s = scenario();
        assert!(s.html.contains(r#"style="--diffnote-bars: inset 3px 0 0 0 #"#));
        assert!(!s.html.contains(r#"style="box-shadow"#));
    }

    #[test]
    fn the_page_scales_to_a_phones_width() {
        let s = scenario();
        assert!(s.html.contains(r#"<meta name="viewport" content="width=device-width, initial-scale=1">"#));
    }

    #[test]
    fn a_thread_on_lines_an_earlier_view_does_not_have_yet_says_so() {
        let s = scenario();
        let (v0, v1) = (view(&s.html, 0), view(&s.html, 1));
        // `top` was added by revision 2; revision 1 doesn't have it, so in
        // that view the thread sits at the point where the line would be,
        // with what it said quoted -- not among the unplaced ones.
        assert!(rows_of(v0, s.t4).is_empty());
        assert!(!v0.contains("diffnote-outdated"));
        let at = v0
            .find(&format!(r#"data-diffnote-thread-id="{}""#, s.t4))
            .unwrap();
        let card = &v0[at..v0[at..].find("</details>").unwrap() + at];
        assert!(card.contains("この版にはまだない行"), "{card}");
        assert!(!card.contains("削除された行"), "not deleted: it isn't there yet");
        assert!(card.contains(r#"<pre class="diffnote-deleted__snippet">top"#), "{card}");
        // In revision 2, where the line exists, it is an ordinary thread on
        // line 1.
        assert_eq!(rows_of(v1, s.t4), vec![("".to_string(), "1".to_string())]);
        assert!(!v1.contains("削除された行") && !v1.contains("まだない"));
    }

    #[test]
    fn a_thread_whose_file_version_is_not_in_the_bundle_is_unplaced_in_every_view() {
        let lost = Ulid::new();
        let (_dir, loaded) = bundle_of(
            &[
                (R1_BASE, R1_HEAD, files_source(None)),
                (R1_HEAD, R2_HEAD, files_source(Some("x"))),
            ],
            vec![comment(
                lost,
                Anchor::Span {
                    base: None,
                    head: Some(range(1, 1, "a version that was never kept")),
                },
                "about something lost",
            )],
        );
        let html = render_bundle(&loaded).unwrap();
        for i in 0..2 {
            let v = view(&html, i);
            let unplaced = v.find("diffnote-outdated").expect("an unplaced section");
            assert!(v[unplaced..].contains("about something lost"), "view {i}");
            assert!(rows_of(v, lost).is_empty());
        }
    }

    #[test]
    fn resolved_threads_are_shown_collapsed_in_every_view() {
        let s = scenario();
        for i in 0..2 {
            let v = view(&s.html, i);
            let at = v
                .find(&format!(r#"id="r{i}-thread-{}""#, s.t3))
                .unwrap_or_else(|| panic!("resolved thread missing in view {i}"));
            let tag = &v[v[..at].rfind("<details").unwrap()..at + 80];
            assert!(tag.contains("diffnote-thread--resolved"), "{tag}");
            assert!(!tag.contains(" open"), "{tag}");
            // ...whereas an unresolved one is open.
            let at = v.find(&format!(r#"id="r{i}-thread-{}""#, s.t1)).unwrap();
            let tag = &v[at..v[at..].find('>').unwrap() + at];
            assert!(tag.contains(" open"), "{tag}");
        }
    }

    #[test]
    fn global_threads_are_shown_once_per_view() {
        let s = scenario();
        for i in 0..2 {
            let v = view(&s.html, i);
            assert_eq!(v.matches("overall").count(), 1, "view {i}");
            assert!(v.contains("diffnote-global-comments"));
        }
    }

    #[test]
    fn one_revision_has_no_switcher_and_is_shown() {
        let (_dir, loaded) = bundle_of(&[(R1_BASE, R1_HEAD, files_source(None))], Vec::new());
        let html = render_bundle(&loaded).unwrap();
        assert!(!html.contains(r#"<nav class="diffnote-revisions""#));
        assert_eq!(html.matches(r#"<section class="diffnote-revision"#).count(), 1);
        assert!(html.contains(r#"class="diffnote-revision is-current" id="rev-0""#));
    }

    #[test]
    fn revision_labels_are_escaped() {
        let git = Source::Git(GitSource {
            base: "b".into(),
            head: "h".into(),
            spec: "<script>alert(1)</script>&x".into(),
        });
        let (_dir, loaded) = bundle_of(
            &[(R1_BASE, R1_HEAD, git), (R1_HEAD, R2_HEAD, files_source(None))],
            Vec::new(),
        );
        let html = render_bundle(&loaded).unwrap();
        assert!(!html.contains("<script>alert(1)"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;&amp;x"));
    }

    #[test]
    fn a_revision_without_a_diff_is_not_a_view_and_a_bundle_of_only_those_is_an_error() {
        // Same tree on both sides: an `init` snapshot, whose diff is empty.
        let (_dir, loaded) = bundle_of(&[(R1_BASE, R1_BASE, files_source(None))], Vec::new());
        assert!(render_bundle(&loaded).is_err());
        let (_dir, loaded) = bundle_of(
            &[
                (R1_BASE, R1_BASE, files_source(None)),
                (R1_BASE, R1_HEAD, files_source(None)),
            ],
            Vec::new(),
        );
        let html = render_bundle(&loaded).unwrap();
        assert_eq!(html.matches(r#"<section class="diffnote-revision"#).count(), 1);
    }

    /// A bundle of one revision over two files: `f.txt` changes, `README.md`
    /// doesn't (and is in the revision's recorded tree).
    fn bundle_with_readme(readme: &str, extra: Vec<Event>) -> (tempfile::TempDir, bundle::Loaded) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.diffnote");
        let two = |f: &str| -> Tree {
            [
                ("f.txt".to_string(), f.as_bytes().to_vec()),
                ("README.md".to_string(), readme.as_bytes().to_vec()),
            ]
            .into()
        };
        let (diff_text, files) = diff_trees(&two(R1_BASE), &two(R1_HEAD));
        let mut events = vec![Event::Meta {
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            description: None,
            context_lines: 3,
        }];
        events.push(Event::Revision(Revision {
            id: Ulid::new(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            digest: digest("r"),
            source: files_source(None),
            snapshot_mode: SnapshotMode::Changed,
            files,
            tree: vec![
                crate::model::TreeFile {
                    path: "f.txt".into(),
                    digest: digest(R1_HEAD),
                },
                crate::model::TreeFile {
                    path: "README.md".into(),
                    digest: digest(readme),
                },
            ],
        }));
        events.extend(extra);
        let additions = Additions {
            diff: Some((digest("r"), diff_text)),
            blobs: vec![
                R1_BASE.as_bytes().to_vec(),
                R1_HEAD.as_bytes().to_vec(),
                readme.as_bytes().to_vec(),
            ],
        };
        bundle::save(&path, &bundle::load(&path).unwrap(), &events, &additions).unwrap();
        (dir, bundle::load(&path).unwrap())
    }

    /// The unplaced section's opening tag (the class name alone is also in
    /// the stylesheet).
    const UNPLACED: &str = r#"<section class="diffnote-outdated">"#;

    fn range_of(file: &str, text: &str, start: u32, len: u32) -> LineRange {
        LineRange {
            file: file.to_string(),
            digest: digest(text),
            start,
            len,
        }
    }

    #[test]
    fn a_thread_on_a_file_the_diff_never_touches_is_shown_with_context() {
        let readme = "# title\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\n";
        let id = Ulid::new();
        let (_dir, loaded) = bundle_with_readme(
            readme,
            vec![comment(
                id,
                Anchor::Span {
                    base: None,
                    head: Some(range_of("README.md", readme, 4, 1)),
                },
                "about line 4 of the readme",
            )],
        );
        let html = render_bundle(&loaded).unwrap();
        // Its own file section, not the unplaced list.
        assert!(html.contains(r#"id="r0-file-README-md""#));
        assert!(!html.contains(UNPLACED), "placed, not unplaced");
        // Line 4 is highlighted, with three lines of context either side.
        assert_eq!(rows_of(&html, id), vec![("4".to_string(), "4".to_string())]);
        let section = &html[html.find(r#"id="r0-file-README-md""#).unwrap()..];
        for n in 1..=7 {
            assert!(section.contains(&format!(r#"diffnote-line__gutter-new">{n}<"#)), "line {n}");
        }
        assert!(!section.contains(r#"diffnote-line__gutter-new">8<"#), "no more than 3 lines around");
        assert!(section.contains("about line 4 of the readme"));
        // ...as a file that only has context: no added or removed lines in it.
        let readme_part = &section[..section.find("</section>").unwrap()];
        assert!(!readme_part.contains("diffnote-line--added"));
        assert!(!readme_part.contains("diffnote-line--removed"));
    }

    #[test]
    fn a_thread_on_lines_far_from_any_change_is_shown_with_context_in_its_own_file() {
        let old: String = (1..=40).map(|n| format!("l{n}\n")).collect();
        let new = old.replace("l40\n", "L40\n");
        let id = Ulid::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.diffnote");
        let (diff_text, files) = diff_trees(&tree(&old), &tree(&new));
        let events = vec![
            Event::Revision(Revision {
                id: Ulid::new(),
                created_at: OffsetDateTime::UNIX_EPOCH,
                digest: digest("r"),
                source: files_source(None),
                snapshot_mode: SnapshotMode::Full,
                files,
                tree: Vec::new(),
            }),
            comment(
                id,
                Anchor::Span {
                    base: Some(range_of("f.txt", &old, 10, 2)),
                    head: Some(range_of("f.txt", &new, 10, 2)),
                },
                "far from the change",
            ),
        ];
        let additions = Additions {
            diff: Some((digest("r"), diff_text)),
            blobs: vec![old.into_bytes(), new.into_bytes()],
        };
        bundle::save(&path, &bundle::load(&path).unwrap(), &events, &additions).unwrap();
        let html = render_bundle(&bundle::load(&path).unwrap()).unwrap();
        assert!(!html.contains(UNPLACED));
        // Lines 10 and 11 are the thread's; 7..=14 are shown around them.
        let rows = rows_of(&html, id);
        assert_eq!(
            rows,
            vec![
                ("10".to_string(), "10".to_string()),
                ("11".to_string(), "11".to_string())
            ]
        );
        assert!(html.contains(r#"diffnote-line__gutter-new">7<"#));
        assert!(html.contains(r#"diffnote-line__gutter-new">14<"#));
        assert!(!html.contains(r#"diffnote-line__gutter-new">15<"#));
        assert!(!html.contains(r#"diffnote-line__gutter-new">6<"#));
        // The real hunk is still there, after it.
        assert!(html.contains(r#"diffnote-line__gutter-new">40<"#));
    }

    #[test]
    fn a_thread_whose_context_text_is_missing_is_unplaced_not_lost() {
        let readme = "# title\nline 2\n";
        let id = Ulid::new();
        let (_dir, loaded) = bundle_with_readme(
            readme,
            vec![comment(
                id,
                Anchor::Span {
                    base: None,
                    // A version of README that isn't in the store.
                    head: Some(range_of("README.md", "held nowhere\n", 1, 1)),
                },
                "lost text",
            )],
        );
        let html = render_bundle(&loaded).unwrap();
        assert!(html.contains(UNPLACED));
        assert_eq!(html.matches("lost text").count(), 1);
    }
}
