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
use crate::expand;
use crate::model::{Event, Side};
use crate::review::{Thread, build_threads};
use pulldown_cmark::{Parser as MdParser, html::push_html as md_push_html};
use std::collections::HashMap;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::{IncludeBackground, styled_line_to_highlighted_html};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use ulid::Ulid;

/// The syntax and color definitions, built once: each syntax compiles its
/// patterns the first time it highlights something, which costs far more than
/// drawing the lines, so a page (and a server) must not start over each time.
static SYNTAXES: std::sync::LazyLock<SyntaxSet> =
    std::sync::LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: std::sync::LazyLock<ThemeSet> = std::sync::LazyLock::new(ThemeSet::load_defaults);

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
    /// The file a thread is about (a review-wide thread has none), for its
    /// location and the thread list.
    files: HashMap<Ulid, String>,
    /// Where in its file a thread starts (the first line, or where the lines
    /// were), to order the thread list.
    starts: HashMap<Ulid, u32>,
    /// Whether the page can be changed from the browser (the served one): its
    /// thread cards get buttons and a reply box.
    interactive: bool,
}

/// The revisions that have a diff to show, each with its label, diff and
/// tree (a fresh `init` snapshot has no diff, so is left out).
struct Shown<'a> {
    label: String,
    diff: UnifiedDiff,
    revision: &'a crate::model::Revision,
    tree: Vec<crate::model::TreeFile>,
}

fn shown_revisions(loaded: &crate::bundle::Loaded) -> anyhow::Result<Vec<Shown<'_>>> {
    let mut shown = Vec::new();
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
            shown.len() + 1,
            revision.created_at.date()
        );
        shown.push(Shown {
            label,
            diff,
            revision,
            tree: loaded.manifest(revision),
        });
    }
    if shown.is_empty() {
        anyhow::bail!("バンドルに記録された差分がありません");
    }
    Ok(shown)
}

fn revision_views<'a>(shown: &'a [Shown<'a>]) -> Vec<RevisionView<'a>> {
    shown
        .iter()
        .map(|s| RevisionView {
            label: s.label.clone(),
            diff: &s.diff,
            files: &s.revision.files,
            tree: &s.tree,
        })
        .collect()
}

/// Renders a whole bundle: one view per recorded revision that has a diff
/// (a fresh `init` snapshot has none), oldest first.
pub fn render_bundle(loaded: &crate::bundle::Loaded) -> anyhow::Result<String> {
    let shown = shown_revisions(loaded)?;
    let views = revision_views(&shown);
    Ok(render(&loaded.events, &views, &loaded.blobs()))
}

/// Like [`render_bundle`], for a page whose threads can be changed from the
/// browser: each card has buttons and a reply box, and the page's script talks
/// to the server that serves it.
pub fn render_bundle_interactive(loaded: &crate::bundle::Loaded) -> anyhow::Result<String> {
    let shown = shown_revisions(loaded)?;
    let views = revision_views(&shown);
    Ok(render_with(&loaded.events, &views, &loaded.blobs(), true))
}

/// What the browser needs to update after a thread changed: for every
/// revision's view, the thread's new card and its entry in the thread list
/// (with the ids that view's page uses), and the counts.
pub struct ThreadFragments {
    pub views: Vec<ViewFragment>,
    pub open: usize,
    pub all: usize,
}

pub struct ViewFragment {
    pub revision: usize,
    pub card: String,
    pub list_item: String,
}

/// The fragments for the thread `id` of `loaded` as the interactive page
/// draws it, or `None` if there is no such thread (or nothing to show).
pub fn thread_fragments(loaded: &crate::bundle::Loaded, id: Ulid) -> Option<ThreadFragments> {
    let shown = shown_revisions(loaded).ok()?;
    let views = revision_views(&shown);
    let threads = build_threads(&loaded.events);
    let thread = threads.iter().find(|t| t.root_id == id)?;
    let blobs = loaded.blobs();
    let fragments = views
        .iter()
        .enumerate()
        .map(|(i, view)| {
            let placed = place(&threads, view, &blobs, true);
            ViewFragment {
                revision: i,
                card: prefix_ids(&render_thread_html(thread, &placed.marks), i),
                list_item: prefix_ids(&thread_list_item(thread, &placed.marks), i),
            }
        })
        .collect();
    Some(ThreadFragments {
        views: fragments,
        open: threads.iter().filter(|t| !t.resolved).count(),
        all: threads.len(),
    })
}

/// A row of a diff table, by the line number it has on one side (a number
/// is unique among the rows of a file on its side).
pub struct RowRef {
    pub file: String,
    pub old: Option<u32>,
    pub new: Option<u32>,
}

/// A row that now belongs to (more) threads: their ids and color bars.
pub struct RowMark {
    pub row: RowRef,
    pub threads: String,
    pub bars: String,
}

/// What the page needs to show a new thread in the view it is in, without
/// drawing the view again: the card, where it goes, and its place in the
/// thread list.
pub struct ThreadPatch {
    pub place: PatchPlace,
    pub list_item: String,
    /// The thread the new entry goes in front of (`None`: at the end).
    pub list_before: Option<Ulid>,
    pub open: usize,
    pub all: usize,
}

pub enum PatchPlace {
    /// On lines of a diff table: the card as a table row to put after a row,
    /// and the rows its range now covers.
    Lines {
        card_row: String,
        after: RowRef,
        rows: Vec<RowMark>,
    },
    /// Outside the tables: a review-wide thread (`file` is `None`) or one of
    /// a file, as its card.
    Card { card: String, file: Option<String> },
}

/// The patch for thread `id` in view `revision`, or `None` if the thread is
/// drawn neither on lines nor as a review-wide or file card there (the whole
/// view is then drawn again instead).
pub fn thread_patch(
    loaded: &crate::bundle::Loaded,
    id: Ulid,
    revision: usize,
) -> Option<ThreadPatch> {
    let shown = shown_revisions(loaded).ok()?;
    let views = revision_views(&shown);
    let view = views.get(revision)?;
    let threads = build_threads(&loaded.events);
    let thread = threads.iter().find(|t| t.root_id == id)?;
    let blobs = loaded.blobs();
    let placed = place(&threads, view, &blobs, true);

    let ordered = ordered_threads(&threads, &placed.file_order, &placed.marks);
    let at = ordered.iter().position(|t| t.root_id == id)?;
    let finish = |place: PatchPlace| ThreadPatch {
        place,
        list_item: prefix_ids(&thread_list_item(thread, &placed.marks), revision),
        list_before: ordered.get(at + 1).map(|t| t.root_id),
        open: threads.iter().filter(|t| !t.resolved).count(),
        all: threads.len(),
    };
    // A review-wide or a file's thread is a card outside the tables.
    if placed.global.iter().any(|t| t.root_id == id) {
        return Some(finish(PatchPlace::Card {
            card: prefix_ids(&render_thread_html(thread, &placed.marks), revision),
            file: None,
        }));
    }
    if let Some((file, _)) = placed
        .by_file
        .iter()
        .find(|(_, ts)| ts.iter().any(|t| t.root_id == id))
    {
        return Some(finish(PatchPlace::Card {
            card: prefix_ids(&render_thread_html(thread, &placed.marks), revision),
            file: Some(file.clone()),
        }));
    }

    let ((file, side, line), _) = placed
        .by_line
        .iter()
        .find(|(_, ts)| ts.iter().any(|t| t.root_id == id))?;
    let row_ref = |file: &str, side: Side, line: u32| RowRef {
        file: file.to_string(),
        old: (side == Side::Old).then_some(line),
        new: (side == Side::New).then_some(line),
    };

    // Every row of the thread's range, with all the threads that now cover it.
    let mut rows = Vec::new();
    for ((f, s, l), ids) in &placed.highlighted {
        if !ids.contains(&id) {
            continue;
        }
        let Some(numbers) = row_numbers(&placed.diff, f, *s, *l) else {
            continue;
        };
        let mut covering: Vec<Ulid> = Vec::new();
        for (side, n) in [(Side::New, numbers.1), (Side::Old, numbers.0)] {
            if let Some(n) = n
                && let Some(ids) = placed.highlighted.get(&(f.clone(), side, n))
            {
                covering.extend(ids.iter().copied());
            }
        }
        covering.dedup();
        let (threads, bars) = bars_for(&covering, &placed.marks);
        let row = RowRef {
            file: f.clone(),
            old: numbers.0,
            new: numbers.1,
        };
        // A context row is in the map under both of its numbers.
        if rows
            .iter()
            .any(|m: &RowMark| (&m.row.file, m.row.old, m.row.new) == (&row.file, row.old, row.new))
        {
            continue;
        }
        rows.push(RowMark { row, threads, bars });
    }

    Some(finish(PatchPlace::Lines {
        card_row: prefix_ids(&thread_row(thread, &placed.marks), revision),
        after: row_ref(file, *side, *line),
        rows,
    }))
}

/// The (old, new) line numbers of the row of `file` that has line `line` on
/// `side`.
fn row_numbers(
    diff: &UnifiedDiff,
    file: &str,
    side: Side,
    line: u32,
) -> Option<(Option<u32>, Option<u32>)> {
    let file_diff = diff.files.iter().find(|f| file_key(f) == file)?;
    file_diff
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .find(|l| match side {
            Side::New => l.new_line == Some(line),
            Side::Old => l.old_line == Some(line),
        })
        .map(|l| (l.old_line, l.new_line))
}

/// The inside of revision `revision`'s section of the page (its title and
/// view), for when the page has to draw a view again.
pub fn render_view_inner(loaded: &crate::bundle::Loaded, revision: usize) -> Option<String> {
    let shown = shown_revisions(loaded).ok()?;
    let views = revision_views(&shown);
    let view = views.get(revision)?;
    let threads = build_threads(&loaded.events);
    let syntax_set = &*SYNTAXES;
    let theme = &THEMES.themes["InspiredGitHub"];
    let inner = prefix_ids(
        &render_view(&threads, view, &loaded.blobs(), syntax_set, theme, true),
        revision,
    );
    Some(format!(
        r#"<h2 class="diffnote-revision__title">{}</h2>{inner}"#,
        escape_html(&view.label)
    ))
}

/// What a thread written in view `index` is anchored against: the revision,
/// the diff as the page shows it (with the context that files the diff
/// doesn't touch appear in) and the digests of the files in it.
///
/// `file` is the file the thread is about: if the diff doesn't have it but the
/// revision stores it (the page can show any stored file), it is anchored to
/// its stored version, on both sides.
pub fn anchor_view(
    loaded: &crate::bundle::Loaded,
    index: usize,
    file: Option<&str>,
) -> Option<(
    crate::model::Revision,
    UnifiedDiff,
    Vec<crate::model::FileDigest>,
)> {
    let shown = shown_revisions(loaded).ok()?;
    let views = revision_views(&shown);
    let view = views.get(index)?;
    let threads = build_threads(&loaded.events);
    let placed = place(&threads, view, &loaded.blobs(), true);
    let mut diff = placed.diff;
    let mut files = view.files.to_vec();
    files.extend(placed.synthetic_files);
    if let Some(path) = file
        && !diff.files.iter().any(|f| file_key(f) == path)
        && let Some(entry) = loaded
            .manifest(shown[index].revision)
            .into_iter()
            .find(|e| e.path == path && loaded.blob(&e.digest).is_some())
    {
        diff.files.push(FileDiff {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            ..FileDiff::default()
        });
        files.push(crate::model::FileDigest {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            old: Some(entry.digest.clone()),
            new: Some(entry.digest),
        });
    }
    Some((shown[index].revision.clone(), diff, files))
}

// ---- other files: the stored tree, opened to look at ------------------------

/// Lines drawn at a time when a stored file is opened.
const OPEN_CHUNK: usize = 500;
/// Entries listed at most, in one directory or one search.
const TREE_LIMIT: usize = 500;

/// The files the revision stores that the view doesn't already have (as part
/// of the diff, or for a thread), by path.
fn other_files(
    loaded: &crate::bundle::Loaded,
    revision: usize,
) -> Option<Vec<crate::model::TreeFile>> {
    let shown = shown_revisions(loaded).ok()?;
    let views = revision_views(&shown);
    let view = views.get(revision)?;
    let threads = build_threads(&loaded.events);
    let placed = place(&threads, view, &loaded.blobs(), true);
    let mut files: Vec<crate::model::TreeFile> = loaded
        .manifest(shown[revision].revision)
        .into_iter()
        .filter(|f| !placed.file_order.contains(&f.path) && loaded.blob(&f.digest).is_some())
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Some(files)
}

fn tree_file_item(path: &str, label: &str) -> String {
    format!(
        r#"<li><button type="button" class="diffnote-tree__file" data-diffnote-open="{p}" title="{p}">{l}</button></li>"#,
        p = escape_html(path),
        l = escape_html(label),
    )
}

/// The list of stored files for the "other files" section of the page: the
/// entries of directory `dir` (sub-directories fold and are listed when
/// opened), or, with a `query`, the files whose path contains it.
pub fn tree_listing(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    dir: &str,
    query: &str,
) -> Option<String> {
    let files = other_files(loaded, revision)?;
    if files.is_empty() {
        return Some(
            r#"<p class="diffnote-tree__empty">ほかに保存されているファイルはありません</p>"#
                .to_string(),
        );
    }
    let mut items = Vec::new();
    let more;
    let query = query.trim().to_lowercase();
    if !query.is_empty() {
        let matching: Vec<&crate::model::TreeFile> = files
            .iter()
            .filter(|f| f.path.to_lowercase().contains(&query))
            .collect();
        more = matching.len().saturating_sub(TREE_LIMIT);
        for f in matching.into_iter().take(TREE_LIMIT) {
            items.push(tree_file_item(&f.path, &f.path));
        }
        if items.is_empty() {
            return Some(r#"<p class="diffnote-tree__empty">見つかりません</p>"#.to_string());
        }
    } else {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut dirs: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        let mut here: Vec<(&str, &str)> = Vec::new();
        for f in &files {
            let Some(rest) = f.path.strip_prefix(prefix.as_str()) else {
                continue;
            };
            match rest.split_once('/') {
                Some((name, _)) => *dirs.entry(name).or_default() += 1,
                None => here.push((rest, f.path.as_str())),
            }
        }
        let mut entries: Vec<String> = dirs
            .into_iter()
            .map(|(name, count)| {
                format!(
                    r#"<li><details class="diffnote-tree__dir" data-diffnote-dir="{p}"><summary>{n}/ <span class="diffnote-tree__count">{count}</span></summary><div data-diffnote-children></div></details></li>"#,
                    p = escape_html(&format!("{prefix}{name}")),
                    n = escape_html(name),
                )
            })
            .collect();
        entries.extend(
            here.into_iter()
                .map(|(name, path)| tree_file_item(path, name)),
        );
        more = entries.len().saturating_sub(TREE_LIMIT);
        items = entries.into_iter().take(TREE_LIMIT).collect();
    }
    let mut out = format!(r#"<ul class="diffnote-tree__list">{}</ul>"#, items.concat());
    if more > 0 {
        out.push_str(&format!(
            r#"<p class="diffnote-tree__empty">ほか {more} 件(検索で絞り込んでください)</p>"#
        ));
    }
    Some(out)
}

/// The text of a file the revision stores, or why it can't be shown.
fn stored_text(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    path: &str,
) -> Result<String, String> {
    let shown = shown_revisions(loaded).map_err(|e| e.to_string())?;
    let rev = shown
        .get(revision)
        .ok_or("そのリビジョンはありません")?
        .revision;
    let entry = loaded
        .manifest(rev)
        .into_iter()
        .find(|f| f.path == path)
        .ok_or("そのファイルはこのレビューに保存されていません")?;
    let bytes = loaded
        .blob(&entry.digest)
        .ok_or("そのファイルの内容が保存されていません")?;
    String::from_utf8(bytes.to_vec())
        .map_err(|_| "テキストファイルではないため、表示できません".to_string())
}

/// `OPEN_CHUNK` lines of `text` from line `from` (1-based) as a hunk of
/// unchanged lines, and where the next chunk starts (`None` at the end).
fn context_chunk(text: &str, from: usize) -> (Hunk, Option<usize>) {
    let from = from.max(1);
    let lines: Vec<crate::diff::DiffLine> = text
        .lines()
        .enumerate()
        .skip(from - 1)
        .take(OPEN_CHUNK)
        .map(|(i, l)| crate::diff::DiffLine {
            kind: LineKind::Context,
            content: l.to_string(),
            old_line: Some(i as u32 + 1),
            new_line: Some(i as u32 + 1),
            no_newline_at_eof: false,
        })
        .collect();
    let next = (from - 1 + lines.len() < text.lines().count()).then_some(from + OPEN_CHUNK);
    let count = lines.len() as u32;
    (
        Hunk {
            old_start: from as u32,
            old_lines: count,
            new_start: from as u32,
            new_lines: count,
            section_heading: None,
            lines,
        },
        next,
    )
}

fn more_row(path: &str, next: usize, total: usize) -> String {
    format!(
        r#"<tr class="diffnote-more-row"><td colspan="3"><button type="button" class="diffnote-button" data-diffnote-more data-path="{p}" data-from="{next}">続きを表示({next}〜 / 全 {total} 行)</button></td></tr>"#,
        p = escape_html(path),
    )
}

/// A stored file opened in view `revision`: its section of the page (as a
/// file of unchanged lines, the first chunk of it) and its entry for the file
/// list. Nothing is recorded: it is only looked at, until a comment is made.
pub struct OpenedFile {
    pub html: String,
    pub list_item: String,
}

pub fn open_file(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    path: &str,
) -> Result<OpenedFile, String> {
    let text = stored_text(loaded, revision, path)?;
    let total = text.lines().count();
    let (hunk, next) = context_chunk(&text, 1);
    let file_diff = FileDiff {
        old_path: Some(path.to_string()),
        new_path: Some(path.to_string()),
        hunks: vec![hunk],
        ..FileDiff::default()
    };
    let marks = Marks {
        interactive: true,
        ..Marks::default()
    };
    let syntax_set = &*SYNTAXES;
    let theme = &THEMES.themes["InspiredGitHub"];
    let mut html = render_file(
        Some(&file_diff),
        path,
        &[],
        &HashMap::new(),
        &HashMap::new(),
        &marks,
        &[],
        syntax_set,
        theme,
    );
    // Open, marked as only looked at, with a way to close it again.
    html = html
        .replacen("<details>", "<details open>", 1)
        .replacen(r#" id="file-"#, r#" data-diffnote-opened id="file-"#, 1)
        .replacen(
            "</summary>",
            r#"<button type="button" class="diffnote-mini" data-diffnote-close title="この表示を閉じる(記録には残りません)">閉じる</button></summary>"#,
            1,
        );
    if let Some(next) = next {
        html = html.replacen(
            "</table>",
            &format!("{}</table>", more_row(path, next, total)),
            1,
        );
    }
    let item = format!(
        r##"<li><a href="#file-{}">{}</a></li>"##,
        html_id(path),
        escape_html(path)
    );
    Ok(OpenedFile {
        html: prefix_ids(&html, revision),
        list_item: prefix_ids(&item, revision),
    })
}

/// The next chunk of an opened file, from line `from`, as table rows, and
/// where the chunk after it starts.
pub fn file_chunk(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    path: &str,
    from: usize,
) -> Result<(String, Option<usize>), String> {
    let text = stored_text(loaded, revision, path)?;
    let (hunk, next) = context_chunk(&text, from);
    let marks = Marks {
        interactive: true,
        ..Marks::default()
    };
    let syntax_set = &*SYNTAXES;
    let theme = &THEMES.themes["InspiredGitHub"];
    let rows = render_hunk(
        path,
        &hunk,
        guess_syntax(path, syntax_set),
        syntax_set,
        theme,
        &HashMap::new(),
        &HashMap::new(),
        &marks,
    );
    Ok((rows, next))
}

/// How many revision views the page has.
pub fn view_count(loaded: &crate::bundle::Loaded) -> usize {
    shown_revisions(loaded).map_or(0, |s| s.len())
}

/// Element ids must be unique across the views, so a view's ids carry its
/// number.
fn prefix_ids(html: &str, view: usize) -> String {
    html.replace(r#"id="file-"#, &format!(r#"id="r{view}-file-"#))
        .replace(r##"href="#file-"##, &format!(r##"href="#r{view}-file-"##))
        .replace(r#"id="thread-"#, &format!(r#"id="r{view}-thread-"#))
        .replace(
            r##"href="#thread-"##,
            &format!(r##"href="#r{view}-thread-"##),
        )
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
pub fn render(events: &[Event], views: &[RevisionView], blobs: &crate::digest::Blobs) -> String {
    render_with(events, views, blobs, false)
}

pub fn render_with(
    events: &[Event],
    views: &[RevisionView],
    blobs: &crate::digest::Blobs,
    interactive: bool,
) -> String {
    let threads = build_threads(events);
    let syntax_set = &*SYNTAXES;
    let theme = &THEMES.themes["InspiredGitHub"];

    let title = crate::review::title(events);
    let heading = escape_html(title.unwrap_or(DEFAULT_TITLE));
    let mut body = String::new();
    body.push_str(&format!(
        r#"<div class="diffnote-topbar"><header class="diffnote-summary"><h1>{heading}</h1>"#
    ));
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
    if interactive {
        body.push_str(
            r#"<button type="button" class="diffnote-button diffnote-topbar__quit" data-diffnote-shutdown title="サーバーを止めます">終了</button>"#,
        );
    }
    body.push_str("</div>\n");
    for (i, view) in views.iter().enumerate() {
        let current = if i + 1 == views.len() {
            " is-current"
        } else {
            ""
        };
        let inner = prefix_ids(
            &render_view(&threads, view, blobs, syntax_set, theme, interactive),
            i,
        );
        body.push_str(&format!(
            r#"<section class="diffnote-revision{current}" id="rev-{i}" data-diffnote-revision="{i}"><h2 class="diffnote-revision__title">{}</h2>{inner}</section>"#,
            escape_html(&view.label)
        ));
    }
    wrap_document(&body, title.unwrap_or(DEFAULT_TITLE), interactive)
}

/// Where every thread goes in one revision's view, worked out from its
/// anchor and the texts (never from what the diff shows).
struct Placed<'a> {
    /// The view's diff plus context around threads it doesn't show.
    diff: UnifiedDiff,
    /// The files that context adds (files the diff doesn't touch), as
    /// unchanged files.
    synthetic_files: Vec<crate::model::FileDigest>,
    global: Vec<&'a Thread>,
    by_file: HashMap<String, Vec<&'a Thread>>,
    /// Where a thread's card is drawn: keyed by the *last* line of its range.
    by_line: HashMap<(String, Side, u32), Vec<&'a Thread>>,
    /// Every line covered by a Span anchor's range, and *which* thread(s)
    /// cover it -- so overlapping range comments can each get their own
    /// color band instead of collapsing into one undifferentiated highlight.
    highlighted: HashMap<(String, Side, u32), Vec<Ulid>>,
    outdated: HashMap<String, Vec<&'a Thread>>,
    marks: Marks,
    file_order: Vec<String>,
}

fn place<'a>(
    threads: &'a [Thread],
    view: &RevisionView,
    blobs: &crate::digest::Blobs,
    interactive: bool,
) -> Placed<'a> {
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
    let (expanded, synthetic) = expand::expand(view.diff, &wants, &versions, blobs);
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
    let mut marks = Marks {
        interactive,
        ..Marks::default()
    };
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
            Placement::File(file) => {
                marks.files.insert(thread.root_id, file.clone());
                by_file.entry(file).or_default().push(thread)
            }
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
                marks.files.insert(thread.root_id, file.clone());
                marks.starts.insert(thread.root_id, line_start);
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
                marks.files.insert(thread.root_id, file.clone());
                marks.starts.insert(thread.root_id, before);
                by_line
                    .entry((file, Side::New, before.saturating_sub(1).max(1)))
                    .or_default()
                    .push(thread);
            }
            Placement::Unplaced { file } => {
                marks.files.insert(thread.root_id, file.clone());
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

    // What a thread on a file the diff doesn't touch is anchored to: that
    // file's one version, on both sides.
    let synthetic_files = synthetic
        .iter()
        .filter_map(|p| {
            let digest = versions.head(p)?.to_string();
            Some(crate::model::FileDigest {
                old_path: Some(p.clone()),
                new_path: Some(p.clone()),
                old: Some(digest.clone()),
                new: Some(digest),
            })
        })
        .collect();
    Placed {
        synthetic_files,
        diff: expanded,
        global,
        by_file,
        by_line,
        highlighted,
        outdated,
        marks,
        file_order,
    }
}

fn render_view(
    threads: &[Thread],
    view: &RevisionView,
    blobs: &crate::digest::Blobs,
    syntax_set: &SyntaxSet,
    theme: &Theme,
    interactive: bool,
) -> String {
    let Placed {
        diff,
        synthetic_files: _,
        global,
        by_file,
        by_line,
        highlighted,
        outdated,
        marks,
        file_order,
    } = place(threads, view, blobs, interactive);
    let diff = &diff;
    let mut body = String::new();
    body.push_str(r#"<aside class="diffnote-sidebar"><details class="diffnote-side" open><summary>ファイル</summary><nav class="diffnote-filelist"><ul>"#);
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
    body.push_str("</ul></nav></details>");
    body.push_str(&render_thread_list(threads, &file_order, &marks));
    if marks.interactive {
        // Any other stored file can be opened to look at (and comment on);
        // folded, and quiet, as it is not the usual way to review.
        body.push_str(r#"<details class="diffnote-side diffnote-side--quiet" data-diffnote-tree><summary>その他のファイル</summary><div class="diffnote-tree"><input type="search" class="diffnote-tree__search" placeholder="ファイルを検索" aria-label="ファイルを検索"><div data-diffnote-tree-list></div></div></details>"#);
    }
    body.push_str("</aside>\n");

    if marks.interactive {
        // Always there on the served page, for a review-wide comment to be
        // added to (its button, and where the cards go).
        body.push_str(r#"<section class="diffnote-global-comments" data-diffnote-global><div class="diffnote-add"><button type="button" class="diffnote-button" data-diffnote-add="global">レビュー全体にコメントする</button></div><div data-diffnote-cards>"#);
        for t in &global {
            body.push_str(&render_thread_html(t, &marks));
        }
        body.push_str("</div></section>\n");
    } else if !global.is_empty() {
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
    let comment_count =
        file_threads.len() + diff_file_comment_count(key, by_line) + outdated_threads.len();
    let open_attr = if comment_count > 0 { " open" } else { "" };
    let is_binary = file_diff.is_some_and(|f| f.is_binary);
    let is_rename = file_diff.is_some_and(|f| f.is_rename);

    // On the served page: which file the section is of, and a button to
    // comment on the file (one of the diff, so it has versions to point at).
    let file_attr = if marks.interactive {
        format!(r#" data-diffnote-file="{}""#, escape_html(key))
    } else {
        String::new()
    };
    let add = if marks.interactive && file_diff.is_some() {
        r#"<button type="button" class="diffnote-mini" data-diffnote-add="file" title="このファイルにコメントする">コメント</button>"#
    } else {
        ""
    };
    out.push_str(&format!(
        r#"<section class="diffnote-file" id="file-{id}"{file_attr}><details{open_attr}><summary><h2>{name}{binary}{rename}</h2>{copy}{add}</summary>"#,
        id = html_id(key),
        name = escape_html(key),
        copy = copy_button(key, "パスをコピー"),
        binary = if is_binary { " (バイナリ)" } else { "" },
        rename = if is_rename { " (名前変更)" } else { "" },
    ));

    if marks.interactive {
        out.push_str("<div data-diffnote-cards>");
    }
    for t in file_threads {
        out.push_str(&render_thread_html(t, marks));
    }
    if marks.interactive {
        out.push_str("</div>");
    }

    match file_diff {
        None => {
            out.push_str(
                r#"<p class="diffnote-file__missing">このファイルは指定したdiffに含まれていません（コメント作成時点と異なるdiffを指定している可能性があります）。</p>"#,
            );
        }
        Some(file_diff) if !file_diff.hunks.is_empty() => {
            let syntax = guess_syntax(key, syntax_set);
            // Which file a row is of, for selecting lines on the served page.
            let named = if marks.interactive {
                format!(r#" data-diffnote-file="{}""#, escape_html(key))
            } else {
                String::new()
            };
            out.push_str(&format!(
                r#"<div class="diffnote-diff-scroll"><table class="diffnote-diff"{named}>"#
            ));
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
    // The next line number on each side, before each row: with the row's own
    // numbers, enough to say which lines a selection of rows covers (a range
    // starts at these counters and ends at where they are after its last row).
    let (mut old_next, mut new_next) = (hunk.old_start, hunk.new_start);
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
        let (row_class, mut row_attrs) = commented_row_markup(class, &covering, marks);
        if marks.interactive {
            row_attrs.push_str(&format!(
                r#" data-diffnote-old="{}" data-diffnote-new="{}" data-diffnote-old-next="{old_next}" data-diffnote-new-next="{new_next}""#,
                line.old_line.map(|n| n.to_string()).unwrap_or_default(),
                line.new_line.map(|n| n.to_string()).unwrap_or_default(),
            ));
        }
        old_next += u32::from(line.old_line.is_some());
        new_next += u32::from(line.new_line.is_some());
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
fn commented_row_markup(base_class: &str, covering: &[Ulid], marks: &Marks) -> (String, String) {
    if covering.is_empty() {
        return (base_class.to_string(), String::new());
    }
    let class = format!("{base_class} diffnote-line--commented");
    let (ids, bars) = bars_for(covering, marks);
    let attrs = format!(
        r#" style="--diffnote-bars: {bars}" data-diffnote-threads="{ids}""#,
        bars = escape_html(&bars),
    );
    (class, attrs)
}

/// For a line covered by these threads: their ids (space-separated, as the
/// hover script reads them) and the stacked color bars, one per thread.
fn bars_for(covering: &[Ulid], marks: &Marks) -> (String, String) {
    let mut bars = Vec::new();
    let mut ids = Vec::new();
    for (i, id) in covering.iter().enumerate() {
        let color = marks
            .color_of
            .get(id)
            .map(|c| PALETTE[*c])
            .unwrap_or("#999");
        let offset = 3 + i as u32 * 4;
        bars.push(format!("inset {offset}px 0 0 0 {color}"));
        ids.push(id.to_string());
    }
    (ids.join(" "), bars.join(", "))
}

fn thread_row(t: &Thread, marks: &Marks) -> String {
    format!(
        r#"<tr class="diffnote-thread-row"><td colspan="3">{}</td></tr>"#,
        render_thread_html(t, marks)
    )
}

/// Where a thread is: `path`, `path:LINE` or `path:FIRST-LAST`, as it is in
/// this view (the form `edit --show` reads). Shown on the thread and copied by
/// its button, so the extent of a range is not left to color alone. `None` for
/// a review-wide thread.
fn location(marks: &Marks, id: Ulid) -> Option<String> {
    let file = marks.files.get(&id)?;
    Some(match marks.lines.get(&id) {
        Some(&(a, b)) if a == b => format!("{file}:{a}"),
        Some(&(a, b)) => format!("{file}:{a}-{b}"),
        None => file.clone(),
    })
}

/// A small button that copies `text` (see the script).
fn copy_button(text: &str, title: &str) -> String {
    format!(
        r#"<button type="button" class="diffnote-copy" data-diffnote-copy="{}" title="{}">コピー</button>"#,
        escape_html(text),
        escape_html(title),
    )
}

/// The first line of a comment, plain and short, to tell threads apart in the
/// list.
fn preview(body: &str) -> String {
    let line = body
        .lines()
        .map(|l| l.trim().trim_start_matches(['#', '>', '-', '*', ' ']))
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = line.chars().take(48).collect();
    if line.chars().count() > 48 {
        out.push('…');
    }
    out
}

/// One thread's entry in the thread list.
fn thread_list_item(t: &Thread, marks: &Marks) -> String {
    let full = location(marks, t.root_id);
    let short = match (marks.files.get(&t.root_id), marks.lines.get(&t.root_id)) {
        (None, _) => "全体".to_string(),
        (Some(file), lines) => {
            let name = file.rsplit('/').next().unwrap_or(file);
            match lines {
                Some(&(a, b)) if a == b => format!("{name}:{a}"),
                Some(&(a, b)) => format!("{name}:{a}-{b}"),
                None => name.to_string(),
            }
        }
    };
    let color = marks
        .color_of
        .get(&t.root_id)
        .map_or("#8b949e", |c| PALETTE[*c]);
    format!(
        r##"<li class="{state}"><a href="#thread-{id}" data-diffnote-jump="{id}" title="{title}"><span class="diffnote-thread__swatch" style="background:{color}"></span><span class="diffnote-threadlist__where">{short}</span>{resolved}<span class="diffnote-threadlist__preview">{preview}</span></a></li>"##,
        state = if t.resolved { "is-resolved" } else { "" },
        id = t.root_id,
        title = escape_html(full.as_deref().unwrap_or("差分全体")),
        short = escape_html(&short),
        resolved = if t.resolved {
            r#"<span class="diffnote-threadlist__state">解決済み</span>"#
        } else {
            ""
        },
        preview = escape_html(&preview(&t.body)),
    )
}

/// Every thread in reading order -- review-wide ones, then by file and line --
/// each a link to its card.
/// The threads in reading order: review-wide ones first, then by file and
/// line.
fn ordered_threads<'a>(
    threads: &'a [Thread],
    file_order: &[String],
    marks: &Marks,
) -> Vec<&'a Thread> {
    let mut items: Vec<(Option<usize>, u32, &Thread)> = threads
        .iter()
        .map(|t| {
            let file = marks
                .files
                .get(&t.root_id)
                .and_then(|f| file_order.iter().position(|o| o == f));
            (file, marks.starts.get(&t.root_id).copied().unwrap_or(0), t)
        })
        .collect();
    items.sort_by_key(|(file, start, t)| (*file, *start, t.created_at));
    items.into_iter().map(|(_, _, t)| t).collect()
}

fn render_thread_list(threads: &[Thread], file_order: &[String], marks: &Marks) -> String {
    // Where a thread added later goes, so the page has it even with none yet.
    if threads.is_empty() && !marks.interactive {
        return String::new();
    }
    let mut out = format!(
        r#"<details class="diffnote-side" open><summary>スレッド <span class="diffnote-badge" title="未解決 / 全部">{} / {}</span></summary><nav class="diffnote-threadlist"><ol>"#,
        threads.iter().filter(|t| !t.resolved).count(),
        threads.len(),
    );
    for t in ordered_threads(threads, file_order, marks) {
        out.push_str(&thread_list_item(t, marks));
    }
    out.push_str("</ol></nav></details>");
    out
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
    let location = location(marks, t.root_id);
    out.push_str(&format!(
        "<summary>{swatch}{status}{place}{absence}{copy}</summary>",
        status = if t.resolved {
            "解決済み"
        } else {
            "未解決"
        },
        place = location.as_deref().map_or(String::new(), |l| format!(
            r#" <span class="diffnote-thread__where">{}</span>"#,
            escape_html(l)
        )),
        absence = match marks.absent.get(&t.root_id) {
            Some(anchor::Absence::Deleted) => " (削除された行)",
            Some(anchor::Absence::NotYet) => " (この版にはまだない行)",
            Some(anchor::Absence::Unknown) => " (この版にない行)",
            None => "",
        },
        copy = location.as_deref().map_or(String::new(), |l| copy_button(
            l,
            "ファイルパスと行をコピー"
        )),
    ));
    if marks.absent.contains_key(&t.root_id) {
        out.push_str(&render_snippet(
            marks.was.get(&t.root_id),
            "diffnote-deleted__snippet",
        ));
    }
    out.push_str(&render_comment_article(&t.author, t.created_at, &t.body));
    for r in &t.replies {
        out.push_str(&render_comment_article(&r.author, r.created_at, &r.body));
    }
    if marks.interactive {
        out.push_str(&render_actions(t));
    }
    out.push_str("</details>");
    out
}

/// The buttons and reply box of a thread card on the interactive page.
fn render_actions(t: &Thread) -> String {
    let (action, label) = if t.resolved {
        ("reopen", "再開する")
    } else {
        ("resolve", "解決にする")
    };
    format!(
        r#"<div class="diffnote-thread__actions"><form class="diffnote-reply" data-diffnote-thread="{id}"><textarea rows="2" placeholder="返信を書く(Ctrl+Enter で送信)"></textarea><div class="diffnote-reply__buttons"><button type="submit" class="diffnote-button diffnote-button--primary">返信</button><button type="button" class="diffnote-button" data-diffnote-action="{action}" data-diffnote-thread="{id}">{label}</button></div></form></div>"#,
        id = t.root_id,
    )
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
    out.push_str(&render_snippet(
        marks.was.get(&t.root_id),
        "diffnote-outdated__snippet",
    ));
    out.push_str(&render_thread_html(t, marks));
    out.push_str("</div>");
    out
}

fn render_comment_article(author: &str, created_at: time::OffsetDateTime, body: &str) -> String {
    format!(
        r#"<article class="diffnote-comment"><p class="diffnote-comment__author">{}{}</p><div class="diffnote-comment__body">{}</div></article>"#,
        escape_html(author),
        time_tag(created_at),
        markdown_to_html(body),
    )
}

/// The moment a comment was written, small and easy to overlook. Drawn in UTC
/// so it is right without a script; the script rewrites it to the viewer's
/// local time, keeping the exact time as a tooltip.
fn time_tag(at: time::OffsetDateTime) -> String {
    use time::format_description::well_known::Rfc3339;
    let utc = at.to_offset(time::UtcOffset::UTC);
    format!(
        r#"<time class="diffnote-comment__time" datetime="{iso}" title="{iso}">{y:04}-{mo:02}-{d:02} {h:02}:{mi:02} UTC</time>"#,
        iso = utc.format(&Rfc3339).unwrap_or_default(),
        y = utc.year(),
        mo = u8::from(utc.month()),
        d = utc.day(),
        h = utc.hour(),
        mi = utc.minute(),
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

/// What the page is called when the review has no title.
const DEFAULT_TITLE: &str = "diffnote レビュー";

fn wrap_document(body: &str, title: &str, interactive: bool) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
{css}
</style>
</head>
<body{api}>
<article class="diffnote-review">
{body}
</article>
<script>
{js}
</script>
</body>
</html>
"##,
        title = escape_html(title),
        // The script acts on the server only where there is one.
        api = if interactive {
            r#" data-diffnote-api="1""#
        } else {
            ""
        },
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
.diffnote-sidebar { grid-column: 1; grid-row: 1 / span 200; position: sticky; top: calc(var(--diffnote-topbar-h) + 12px); max-height: calc(100vh - var(--diffnote-topbar-h) - 24px); overflow: auto; margin-top: 12px; border-right: 1px solid var(--diffnote-color-border); padding-right: 8px; font-size: 12.5px; }
.diffnote-side > summary { cursor: pointer; font-weight: 600; font-size: 12px; color: var(--diffnote-color-muted); padding: 4px 8px; user-select: none; }
.diffnote-side + .diffnote-side { margin-top: 8px; }
.diffnote-threadlist ol { list-style: none; margin: 0; padding: 0; }
.diffnote-threadlist a { display: block; padding: 4px 8px; color: var(--diffnote-color-fg); text-decoration: none; border-radius: 6px; line-height: 1.35; }
.diffnote-threadlist a:hover { background: var(--diffnote-color-gutter); }
.diffnote-threadlist .is-resolved a { color: var(--diffnote-color-muted); }
.diffnote-threadlist__where { font-family: var(--diffnote-font-mono); font-size: 12px; font-weight: 600; word-break: break-all; }
.diffnote-threadlist__state { margin-left: 6px; font-size: 11px; color: var(--diffnote-color-muted); border: 1px solid var(--diffnote-color-border); border-radius: 1em; padding: 0 6px; white-space: nowrap; }
.diffnote-threadlist__preview { display: block; margin-left: 16px; color: var(--diffnote-color-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.diffnote-side--quiet > summary { font-weight: 400; opacity: 0.8; }
.diffnote-tree { padding: 2px 8px 8px; }
.diffnote-tree__search { width: 100%; font: inherit; font-size: 12px; padding: 3px 6px; border: 1px solid var(--diffnote-color-border); border-radius: 6px; background: var(--diffnote-color-bg); }
.diffnote-tree__list { list-style: none; margin: 4px 0; padding: 0; }
[data-diffnote-children] .diffnote-tree__list { padding-left: 12px; }
.diffnote-tree__dir > summary { cursor: pointer; font-family: var(--diffnote-font-mono); font-size: 12px; padding: 2px 0; }
.diffnote-tree__file { display: block; width: 100%; text-align: left; font-family: var(--diffnote-font-mono); font-size: 12px; padding: 2px 4px; color: var(--diffnote-color-fg); background: none; border: 0; border-radius: 4px; cursor: pointer; word-break: break-all; }
.diffnote-tree__file:hover { background: var(--diffnote-color-gutter); }
.diffnote-tree__count { color: var(--diffnote-color-muted); font-size: 11px; }
.diffnote-tree__empty { margin: 4px 0; color: var(--diffnote-color-muted); font-size: 12px; }
.diffnote-more-row > td { padding: 6px 12px !important; background: var(--diffnote-color-gutter); text-align: center; }
.diffnote-add { padding: 8px 12px 0; }
.diffnote-compose-wrap { padding: 8px 12px; }
.diffnote-global-comments .diffnote-add { padding: 0; }
.diffnote-global-comments .diffnote-compose-wrap { padding: 8px 0 0; }
.diffnote-copy, .diffnote-mini { margin-left: 8px; padding: 0 7px; font: inherit; font-size: 11px; font-weight: 400; line-height: 18px; color: var(--diffnote-color-muted); background: var(--diffnote-color-bg); border: 1px solid var(--diffnote-color-border); border-radius: 4px; cursor: pointer; vertical-align: baseline; }
.diffnote-copy:hover, .diffnote-mini:hover { color: var(--diffnote-color-fg); border-color: var(--diffnote-color-muted); }
.diffnote-copy.is-done { color: #1a7f37; border-color: #1a7f37; }
body[data-diffnote-api] .diffnote-line__gutter-old, body[data-diffnote-api] .diffnote-line__gutter-new { cursor: pointer; }
body[data-diffnote-api] .diffnote-line__gutter-new { position: relative; }
body[data-diffnote-api] .diffnote-line__gutter-new::before { content: "+"; position: absolute; left: 4px; top: 2px; width: 16px; height: 16px; line-height: 16px; text-align: center; font-weight: 700; color: #fff; background: var(--diffnote-color-accent); border-radius: 4px; opacity: 0; }
body[data-diffnote-api] .diffnote-diff tr:hover .diffnote-line__gutter-new::before { opacity: 1; }
body.is-selecting { user-select: none; }
.diffnote-diff .diffnote-select > td { background-image: linear-gradient(rgba(9, 105, 218, 0.16), rgba(9, 105, 218, 0.16)); }
.diffnote-diff .diffnote-select > td:first-child { --dn-l: inset 4px 0 0 0 var(--diffnote-color-accent); }
.diffnote-diff .diffnote-select > td:last-child { --dn-r: inset -3px 0 0 0 var(--diffnote-color-accent); }
.diffnote-diff .diffnote-select-first > td { --dn-t: inset 0 3px 0 0 var(--diffnote-color-accent); }
.diffnote-diff .diffnote-select-last > td { --dn-b: inset 0 -3px 0 0 var(--diffnote-color-accent); }
.diffnote-composer-row > td { background: var(--diffnote-color-bg); padding: 6px 16px 6px 124px !important; white-space: normal; font-family: var(--diffnote-font); font-size: 14px; line-height: 1.5; }
.diffnote-compose { max-width: 960px; border: 1px solid var(--diffnote-color-accent); border-radius: 6px; padding: 8px 12px; background: var(--diffnote-color-thread-bg); }
.diffnote-compose__where { margin-bottom: 4px; font-family: var(--diffnote-font-mono); font-size: 12px; color: var(--diffnote-color-muted); }
.diffnote-compose textarea { display: block; width: 100%; font: inherit; font-size: 14px; padding: 6px 8px; resize: vertical; border: 1px solid var(--diffnote-color-border); border-radius: 6px; background: var(--diffnote-color-bg); }
.diffnote-compose textarea:focus { outline: 2px solid var(--diffnote-color-accent); outline-offset: -1px; }
.diffnote-topbar__quit { margin-left: auto; }
.diffnote-button { font: inherit; font-size: 12.5px; padding: 3px 12px; color: var(--diffnote-color-fg); background: var(--diffnote-color-bg); border: 1px solid var(--diffnote-color-border); border-radius: 6px; cursor: pointer; }
.diffnote-button:hover { background: var(--diffnote-color-gutter); }
.diffnote-button:disabled { opacity: 0.6; cursor: default; }
.diffnote-button--primary { color: #fff; background: var(--diffnote-color-accent); border-color: var(--diffnote-color-accent); }
.diffnote-button--primary:hover { background: #0860ca; }
.diffnote-thread__actions { margin-top: 6px; }
.diffnote-reply textarea { display: block; width: 100%; font: inherit; font-size: 14px; padding: 6px 8px; resize: vertical; border: 1px solid var(--diffnote-color-border); border-radius: 6px; background: var(--diffnote-color-bg); }
.diffnote-reply textarea:focus { outline: 2px solid var(--diffnote-color-accent); outline-offset: -1px; }
.diffnote-reply__buttons { display: flex; gap: 6px; margin-top: 6px; }
.diffnote-comment.is-pending { opacity: 0.55; }
.diffnote-error { margin: 6px 0 0; color: #cf222e; font-size: 12.5px; }
.diffnote-thread__where { font-family: var(--diffnote-font-mono); font-size: 12px; font-weight: 400; color: var(--diffnote-color-muted); }
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
.diffnote-comment__time { margin-left: 0.7em; font-weight: 400; font-size: 11px; color: #8b949e; white-space: nowrap; }
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
  .diffnote-sidebar { position: static; max-height: 40vh; margin: 8px 0; border-right: 0; }
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
  // While lines are being chosen by dragging (served page): the choice is
  // what is shown, so no comment's range is.
  var dragging = false;

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
    if (pinned || dragging) return;
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
  // A thread in the list: open its card, bring it to the middle of the
  // screen and keep its range shown.
  function jump(link) {
    var card = document.getElementById(link.getAttribute('href').slice(1));
    if (!card) return;
    for (var n = card; n; n = n.parentElement) {
      if (n.tagName === 'DETAILS') n.open = true;
    }
    card.scrollIntoView({ block: 'center' });
    pinned = card.getAttribute('data-diffnote-thread-id');
    activate(scopeOf(card), pinned);
  }

  // Copy buttons (file paths, thread locations). Handled before anything else
  // sees the click, since they sit inside <summary> elements.
  function copyText(text, button) {
    function done() {
      var before = button.textContent;
      button.textContent = 'コピーしました';
      button.classList.add('is-done');
      setTimeout(function () {
        button.textContent = before;
        button.classList.remove('is-done');
      }, 1400);
    }
    function fallback() {
      var ta = document.createElement('textarea');
      ta.value = text;
      ta.setAttribute('readonly', '');
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      try { document.execCommand('copy'); done(); } catch (err) { /* nothing to do */ }
      document.body.removeChild(ta);
    }
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, fallback);
    } else {
      fallback();
    }
  }
  document.addEventListener('click', function (e) {
    var button = e.target.closest ? e.target.closest('[data-diffnote-copy]') : null;
    if (!button) return;
    e.preventDefault();
    e.stopPropagation();
    copyText(button.getAttribute('data-diffnote-copy'), button);
  }, true);

  document.addEventListener('click', function (e) {
    var link = e.target.closest ? e.target.closest('a[data-diffnote-jump]') : null;
    if (link) { e.preventDefault(); jump(link); return; }
    // A line number on the served page starts a selection, not a pin.
    if (document.body.hasAttribute('data-diffnote-api') && e.target.closest &&
        e.target.closest('.diffnote-line__gutter-old, .diffnote-line__gutter-new')) return;
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
  // Set by the served page: draws a view again (see `refresh` below).
  var refreshView = null;
  function show(i) {
    pinned = null;
    clear();
    views.forEach(function (v, k) { v.classList.toggle('is-current', k === i); });
    links.forEach(function (a, k) { a.classList.toggle('is-current', k === i); });
    if (refreshView && views[i].hasAttribute('data-stale')) refreshView(i);
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

  // --- Times in the viewer's own time zone -------------------------------
  function two(n) { return (n < 10 ? '0' : '') + n; }
  slice.call(document.querySelectorAll('time[datetime]')).forEach(function (t) {
    var d = new Date(t.getAttribute('datetime'));
    if (isNaN(d.getTime())) return;
    t.textContent = d.getFullYear() + '-' + two(d.getMonth() + 1) + '-' + two(d.getDate()) +
      ' ' + two(d.getHours()) + ':' + two(d.getMinutes());
    t.title = d.toLocaleString();
  });

  // --- Changing threads from the served page ----------------------------
  // A change is sent to the server, which answers with the HTML of what
  // changed; that is swapped in, so the page is never reloaded. The change
  // also shows at once (a reply as a faded comment, a resolve as the new
  // state) and is corrected by the answer, or undone with a message if it
  // fails.
  if (document.body.hasAttribute('data-diffnote-api')) {
    var post = function (path, data) {
      return fetch(path, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Diffnote': '1' },
        credentials: 'same-origin',
        body: JSON.stringify(data || {})
      }).then(function (r) {
        return r.json().catch(function () { return { ok: false, error: '応答を読めませんでした' }; });
      }, function () {
        return { ok: false, error: 'サーバーに接続できませんでした' };
      });
    };
    var swap = function (el, html) {
      var tpl = document.createElement('template');
      tpl.innerHTML = html.trim();
      var fresh = tpl.content.firstElementChild;
      el.replaceWith(fresh);
      return fresh;
    };
    var apply = function (res) {
      var again = active && active.id === res.thread ? active.scope : null;
      if (again) clear();
      res.views.forEach(function (v) {
        var card = document.getElementById('r' + v.revision + '-thread-' + res.thread);
        if (card) swap(card, v.card);
        var section = document.getElementById('rev-' + v.revision);
        var link = section && section.querySelector('a[data-diffnote-jump=' + JSON.stringify(res.thread) + ']');
        if (link && link.parentElement) swap(link.parentElement, v.list_item);
      });
      slice.call(document.querySelectorAll('.diffnote-side .diffnote-badge[title]')).forEach(function (b) {
        b.textContent = res.open + ' / ' + res.all;
      });
      var summary = document.querySelector('.diffnote-summary p');
      if (summary) summary.textContent = 'スレッド ' + res.all + ' 件(解決済み ' + (res.all - res.open) + ' 件)';
      if (again) {
        var current = document.querySelector('.diffnote-revision.is-current') || document;
        activate(current, res.thread);
      }
    };
    var showError = function (where, message) {
      var old = where.querySelector('.diffnote-error');
      if (old) old.remove();
      var p = document.createElement('p');
      p.className = 'diffnote-error';
      p.textContent = message;
      where.appendChild(p);
    };
    var send = function (form) {
      var box = form.querySelector('textarea');
      var text = box.value.trim();
      if (!text || form.classList.contains('is-sending')) return;
      form.classList.add('is-sending');
      box.disabled = true;
      var pending = document.createElement('article');
      pending.className = 'diffnote-comment is-pending';
      var who = document.createElement('p');
      who.className = 'diffnote-comment__author';
      who.textContent = '保存中…';
      var what = document.createElement('div');
      what.className = 'diffnote-comment__body';
      what.textContent = text;
      pending.appendChild(who);
      pending.appendChild(what);
      var actions = form.closest('.diffnote-thread__actions');
      actions.parentNode.insertBefore(pending, actions);
      post('/api/threads/' + form.getAttribute('data-diffnote-thread') + '/replies', { body: text }).then(function (res) {
        pending.remove();
        form.classList.remove('is-sending');
        box.disabled = false;
        if (res.ok) {
          apply(res);
        } else {
          showError(form, res.error || '保存できませんでした');
          box.focus();
        }
      });
    };
    document.addEventListener('submit', function (e) {
      var form = e.target.closest ? e.target.closest('.diffnote-reply') : null;
      if (!form) return;
      e.preventDefault();
      send(form);
    });
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && e.target.closest && e.target.closest('.diffnote-reply')) {
        e.preventDefault();
        send(e.target.closest('.diffnote-reply'));
      }
    });
    document.addEventListener('click', function (e) {
      var quit = e.target.closest ? e.target.closest('[data-diffnote-shutdown]') : null;
      if (quit) {
        post('/api/shutdown').then(function () {
          document.body.innerHTML = '<p style="padding:24px;font:14px sans-serif">終了しました。このタブは閉じてかまいません。</p>';
        });
        return;
      }
      var button = e.target.closest ? e.target.closest('[data-diffnote-action]') : null;
      if (!button) return;
      var id = button.getAttribute('data-diffnote-thread');
      var action = button.getAttribute('data-diffnote-action');
      var card = button.closest('.diffnote-thread');
      var was = card.classList.contains('diffnote-thread--resolved');
      // At once: the state and the button show what will be.
      card.classList.toggle('diffnote-thread--resolved', action === 'resolve');
      button.disabled = true;
      post('/api/threads/' + id + '/' + action).then(function (res) {
        if (res.ok) {
          apply(res);
        } else {
          card.classList.toggle('diffnote-thread--resolved', was);
          button.disabled = false;
          showError(button.closest('.diffnote-thread__actions'), res.error || '保存できませんでした');
        }
      });
    });

    // --- A new thread on lines ---------------------------------------------
    // Press a line number, or drag over several (Shift+click extends the last
    // choice); a box opens under the last line. Sent as counters on each side:
    // where they stand before the first line, and after the last.
    var sel = null;       // { table, rows, from } while lines are chosen
    var composer = null;  // the open box's <tr>

    var diffRows = function (table) {
      return slice.call(table.querySelectorAll('tr[data-diffnote-old-next]'));
    };
    var rowOf = function (node) {
      var td = node.closest ? node.closest('.diffnote-line__gutter-old, .diffnote-line__gutter-new') : null;
      var tr = td && td.closest('tr[data-diffnote-old-next]');
      return tr && tr.closest('table[data-diffnote-file]') ? tr : null;
    };
    var num = function (tr, name) { return +tr.getAttribute('data-diffnote-' + name); };
    var has = function (tr, name) { return tr.getAttribute('data-diffnote-' + name) !== ''; };

    var unchoose = function () {
      if (sel) sel.rows.forEach(function (r) {
        r.classList.remove('diffnote-select', 'diffnote-select-first', 'diffnote-select-last');
      });
    };
    var choose = function (table, from, to) {
      unchoose();
      var rows = diffRows(table);
      var a = rows.indexOf(from), b = rows.indexOf(to);
      var picked = rows.slice(Math.min(a, b), Math.max(a, b) + 1);
      picked.forEach(function (r) { r.classList.add('diffnote-select'); });
      picked[0].classList.add('diffnote-select-first');
      picked[picked.length - 1].classList.add('diffnote-select-last');
      sel = { table: table, rows: picked, from: from };
    };

    // The lines the chosen rows cover, per side.
    var counters = function () {
      var first = sel.rows[0], last = sel.rows[sel.rows.length - 1];
      var span = function (side) {
        var start = num(first, side + '-next');
        var after = num(last, side + '-next') + (has(last, side) ? 1 : 0);
        return { start: start, len: after - start };
      };
      return { base: span('old'), head: span('new') };
    };
    var whereText = function () {
      var c = counters();
      var part = c.head.len > 0 ? c.head : c.base;
      var end = part.start + part.len - 1;
      return sel.table.getAttribute('data-diffnote-file') + ':' + (end > part.start ? part.start + '-' + end : part.start);
    };

    var closeComposer = function () {
      if (composer) composer.remove();
      composer = null;
      unchoose();
      sel = null;
      slice.call(document.querySelectorAll('.diffnote-compose-wrap')).forEach(function (w) { w.remove(); });
    };

    var openComposer = function () {
      var draft = composer ? composer.querySelector('textarea').value : '';
      if (composer) composer.remove();
      slice.call(document.querySelectorAll('.diffnote-compose-wrap')).forEach(function (w) { w.remove(); });
      var last = sel.rows[sel.rows.length - 1];
      var tr = document.createElement('tr');
      tr.className = 'diffnote-composer-row';
      var td = document.createElement('td');
      td.colSpan = 3;
      td.innerHTML = '<form class="diffnote-compose"><div class="diffnote-compose__where"></div>' +
        '<textarea rows="3" placeholder="コメントを書く(Ctrl+Enter で送信)"></textarea>' +
        '<div class="diffnote-reply__buttons"><button type="submit" class="diffnote-button diffnote-button--primary">コメントする</button>' +
        '<button type="button" class="diffnote-button" data-diffnote-cancel>キャンセル</button></div></form>';
      td.querySelector('.diffnote-compose__where').textContent = whereText();
      tr.appendChild(td);
      last.after(tr);
      composer = tr;
      var box = tr.querySelector('textarea');
      box.value = draft;
      box.focus();
    };

    document.addEventListener('mousedown', function (e) {
      if (e.button !== 0) return;
      var row = rowOf(e.target);
      if (!row) return;
      var table = row.closest('table');
      e.preventDefault();
      // Whatever comment's range was shown gives way to the choice.
      pinned = null;
      clear();
      if (e.shiftKey && sel && sel.table === table) {
        choose(table, sel.from, row);
      } else {
        choose(table, row, row);
      }
      dragging = true;
      document.body.classList.add('is-selecting');
    });
    document.addEventListener('mouseover', function (e) {
      if (!dragging) return;
      var row = rowOf(e.target);
      if (row && row.closest('table') === sel.table) choose(sel.table, sel.from, row);
    });
    document.addEventListener('mouseup', function () {
      if (!dragging) return;
      dragging = false;
      document.body.classList.remove('is-selecting');
      if (sel) openComposer();
    });

    var sendThread = function (form) {
      var box = form.querySelector('textarea');
      var text = box.value.trim();
      // A box for a file or the whole review says so; the others are for lines.
      var scope = form.getAttribute('data-diffnote-scope');
      if (!text || form.classList.contains('is-sending') || (!scope && !sel)) return;
      var request;
      if (scope) {
        request = {
          scope: scope, body: text,
          revision: +form.getAttribute('data-diffnote-revision'),
          file: form.getAttribute('data-diffnote-target') || undefined
        };
      } else {
        var c = counters();
        var table = sel.table;
        request = {
          revision: +table.closest('.diffnote-revision').getAttribute('data-diffnote-revision'),
          file: table.getAttribute('data-diffnote-file'),
          base: c.base, head: c.head, body: text
        };
      }
      form.classList.add('is-sending');
      box.disabled = true;
      var cell = form.parentNode;
      form.style.display = 'none';
      var pending = document.createElement('article');
      pending.className = 'diffnote-comment is-pending';
      pending.innerHTML = '<p class="diffnote-comment__author">保存中…</p><div class="diffnote-comment__body"></div>';
      pending.querySelector('.diffnote-comment__body').textContent = text;
      cell.appendChild(pending);
      post('/api/threads', request).then(function (res) {
        pending.remove();
        form.style.display = '';
        form.classList.remove('is-sending');
        box.disabled = false;
        if (!res.ok) {
          showError(form, res.error || '保存できませんでした');
          box.focus();
          return;
        }
        closeComposer();
        applyNewThread(res);
      });
    };

    var rowAt = function (table, ref) {
      if (ref.new !== null && ref.new !== undefined) return table.querySelector('tr[data-diffnote-new="' + ref.new + '"]');
      return table.querySelector('tr[data-diffnote-old="' + ref.old + '"]');
    };
    var tableOf = function (section, file) {
      return slice.call(section.querySelectorAll('table[data-diffnote-file]')).filter(function (t) {
        return t.getAttribute('data-diffnote-file') === file;
      })[0];
    };
    var parseRow = function (html) {
      var tpl = document.createElement('template');
      tpl.innerHTML = '<table><tbody>' + html + '</tbody></table>';
      return tpl.content.querySelector('tr');
    };
    var setCounts = function (open, all) {
      slice.call(document.querySelectorAll('.diffnote-side .diffnote-badge[title]')).forEach(function (b) {
        b.textContent = open + ' / ' + all;
      });
      var summary = document.querySelector('.diffnote-summary p');
      if (summary) summary.textContent = 'スレッド ' + all + ' 件(解決済み ' + (all - open) + ' 件)';
    };

    // The new thread is put into the view it was written in; the other views
    // are marked stale and drawn again when they are opened.
    var insertListItem = function (section, p) {
      var ol = section.querySelector('.diffnote-threadlist ol');
      if (!ol) return;
      var tpl = document.createElement('template');
      tpl.innerHTML = p.list_item.trim();
      var item = tpl.content.firstElementChild;
      var before = p.list_before && ol.querySelector('a[data-diffnote-jump=' + JSON.stringify(p.list_before) + ']');
      ol.insertBefore(item, before ? before.parentElement : null);
    };

    var applyNewThread = function (res) {
      var section = document.getElementById('rev-' + res.revision);
      views.forEach(function (v, k) { if (k !== res.revision) v.setAttribute('data-stale', ''); });
      setCounts(res.open, res.all);
      if (res.reload || !res.patch) { refreshView(res.revision); return; }
      if (active) clear();
      var p = res.patch;
      if (p.kind === 'card') {
        // A review-wide or a file's thread: a card at the end of its place.
        var owner = p.file === null || p.file === undefined
          ? section.querySelector('[data-diffnote-global]')
          : slice.call(section.querySelectorAll('section.diffnote-file')).filter(function (s) {
              return s.getAttribute('data-diffnote-file') === p.file;
            })[0];
        var cards = owner && owner.querySelector('[data-diffnote-cards]');
        if (!cards) { refreshView(res.revision); return; }
        var holder = document.createElement('template');
        holder.innerHTML = p.card.trim();
        var fresh = holder.content.firstElementChild;
        cards.appendChild(fresh);
        var details = fresh.closest('details.diffnote-file, .diffnote-file > details');
        if (details) details.open = true;
        insertListItem(section, p);
        fresh.scrollIntoView({ block: 'nearest' });
        return;
      }
      var table = tableOf(section, p.after.file);
      var after = table && rowAt(table, p.after);
      if (!after) { refreshView(res.revision); return; }
      p.rows.forEach(function (m) {
        var t = tableOf(section, m.row.file);
        var tr = t && rowAt(t, m.row);
        if (!tr) return;
        tr.classList.add('diffnote-line--commented');
        tr.setAttribute('data-diffnote-threads', m.threads);
        tr.style.setProperty('--diffnote-bars', m.bars);
      });
      var at = after.nextElementSibling;
      while (at && at.classList.contains('diffnote-thread-row')) { at = at.nextElementSibling; }
      var row = parseRow(p.card_row);
      after.parentNode.insertBefore(row, at);
      insertListItem(section, p);
      var card = row.querySelector('.diffnote-thread');
      if (card) {
        pinned = card.getAttribute('data-diffnote-thread-id');
        activate(section, pinned);
        card.scrollIntoView({ block: 'nearest' });
      }
    };

    document.addEventListener('submit', function (e) {
      var form = e.target.closest ? e.target.closest('.diffnote-compose') : null;
      if (!form) return;
      e.preventDefault();
      sendThread(form);
    });
    document.addEventListener('keydown', function (e) {
      var form = e.target.closest ? e.target.closest('.diffnote-compose') : null;
      if (form && e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); sendThread(form); }
      if (e.key === 'Escape' && (composer || document.querySelector('.diffnote-compose-wrap'))) closeComposer();
    });
    document.addEventListener('click', function (e) {
      if (e.target.closest && e.target.closest('[data-diffnote-cancel]')) closeComposer();
    });

    // "Comment on this file" / "on the whole review": a box at the top of the
    // file (opened if it was folded) or of the page.
    var openScopeComposer = function (button) {
      var scope = button.getAttribute('data-diffnote-add');
      var section = button.closest('.diffnote-revision');
      var owner = scope === 'global' ? button.closest('[data-diffnote-global]') : button.closest('section.diffnote-file');
      var file = scope === 'file' ? owner.getAttribute('data-diffnote-file') : '';
      closeComposer();
      var wrap = document.createElement('div');
      wrap.className = 'diffnote-compose-wrap';
      wrap.innerHTML = '<form class="diffnote-compose"><div class="diffnote-compose__where"></div>' +
        '<textarea rows="3" placeholder="コメントを書く(Ctrl+Enter で送信)"></textarea>' +
        '<div class="diffnote-reply__buttons"><button type="submit" class="diffnote-button diffnote-button--primary">コメントする</button>' +
        '<button type="button" class="diffnote-button" data-diffnote-cancel>キャンセル</button></div></form>';
      var form = wrap.querySelector('form');
      form.setAttribute('data-diffnote-scope', scope);
      form.setAttribute('data-diffnote-revision', section.getAttribute('data-diffnote-revision'));
      if (file) form.setAttribute('data-diffnote-target', file);
      wrap.querySelector('.diffnote-compose__where').textContent = scope === 'global' ? 'レビュー全体へのコメント' : file + ' へのコメント';
      if (scope === 'global') {
        button.parentElement.after(wrap);
      } else {
        var details = owner.querySelector('details');
        details.open = true;
        details.querySelector('summary').after(wrap);
      }
      wrap.querySelector('textarea').focus();
    };
    document.addEventListener('click', function (e) {
      var button = e.target.closest ? e.target.closest('[data-diffnote-add]') : null;
      if (!button) return;
      e.preventDefault();
      e.stopPropagation();
      openScopeComposer(button);
    }, true);

    // --- Other stored files: opened to look at, and to comment on ---------------
    // Nothing is recorded by opening one; a comment on it is what keeps it.
    var sectionOf = function (el) { return el.closest('.diffnote-revision'); };
    var revisionOf = function (el) { return sectionOf(el).getAttribute('data-diffnote-revision'); };
    var getJSON = function (url) {
      return fetch(url, { credentials: 'same-origin' }).then(function (r) { return r.json(); }, function () {
        return { ok: false, error: 'サーバーに接続できませんでした' };
      });
    };
    var loadTree = function (holder, revision, dir, query) {
      holder.textContent = '読み込み中…';
      return getJSON('/api/files/' + revision + '/tree?dir=' + encodeURIComponent(dir) + '&q=' + encodeURIComponent(query || '')).then(function (res) {
        if (!res.ok) { holder.textContent = res.error || '読み込めませんでした'; return; }
        holder.innerHTML = res.html;
      });
    };
    // Folded lists are read when first opened (a directory of thousands of
    // files costs nothing until then).
    document.addEventListener('toggle', function (e) {
      var d = e.target;
      if (!d.open || !d.matches) return;
      if (d.matches('[data-diffnote-tree]')) {
        var list = d.querySelector('[data-diffnote-tree-list]');
        if (!list.hasChildNodes()) loadTree(list, revisionOf(d), '', '');
      } else if (d.matches('[data-diffnote-dir]')) {
        var kids = d.querySelector('[data-diffnote-children]');
        if (!kids.hasChildNodes()) loadTree(kids, revisionOf(d), d.getAttribute('data-diffnote-dir'), '');
      }
    }, true);
    var searchTimer = null;
    document.addEventListener('input', function (e) {
      var box = e.target;
      if (!box.matches || !box.matches('.diffnote-tree__search')) return;
      clearTimeout(searchTimer);
      searchTimer = setTimeout(function () {
        loadTree(box.parentElement.querySelector('[data-diffnote-tree-list]'), revisionOf(box), '', box.value);
      }, 250);
    });
    var openFile = function (button) {
      var section = sectionOf(button);
      var path = button.getAttribute('data-diffnote-open');
      var existing = slice.call(section.querySelectorAll('section.diffnote-file')).filter(function (s) {
        return s.getAttribute('data-diffnote-file') === path;
      })[0];
      if (existing) { existing.querySelector('details').open = true; existing.scrollIntoView({ block: 'start' }); return; }
      getJSON('/api/files/' + revisionOf(button) + '/open?path=' + encodeURIComponent(path)).then(function (res) {
        if (!res.ok) { showError(button.closest('.diffnote-tree'), res.error || '開けませんでした'); return; }
        var tpl = document.createElement('template');
        tpl.innerHTML = res.html.trim();
        var fresh = tpl.content.firstElementChild;
        var files = section.querySelectorAll('section.diffnote-file');
        files[files.length - 1].after(fresh);
        var ul = section.querySelector('.diffnote-filelist ul');
        if (ul) {
          var li = document.createElement('template');
          li.innerHTML = res.list_item.trim();
          ul.appendChild(li.content.firstElementChild);
        }
        fresh.scrollIntoView({ block: 'start' });
      });
    };
    document.addEventListener('click', function (e) {
      var t = e.target.closest ? e.target : null;
      if (!t) return;
      var open = t.closest('[data-diffnote-open]');
      if (open) { openFile(open); return; }
      var close = t.closest('[data-diffnote-close]');
      if (close) {
        e.preventDefault();
        var sec = close.closest('section.diffnote-file');
        slice.call(sectionOf(sec).querySelectorAll('.diffnote-filelist a')).forEach(function (a) {
          if (a.getAttribute('href') === '#' + sec.id) a.parentElement.remove();
        });
        if (sec.contains(composer) || sec.querySelector('.diffnote-compose-wrap')) closeComposer();
        sec.remove();
        return;
      }
      var more = t.closest('[data-diffnote-more]');
      if (more) {
        var row = more.closest('tr');
        more.disabled = true;
        getJSON('/api/files/' + revisionOf(more) + '/more?path=' + encodeURIComponent(more.getAttribute('data-path')) + '&from=' + more.getAttribute('data-from')).then(function (res) {
          if (!res.ok) { more.disabled = false; return; }
          var tpl = document.createElement('template');
          tpl.innerHTML = '<table><tbody>' + res.html + '</tbody></table>';
          slice.call(tpl.content.querySelectorAll('tr')).forEach(function (tr) { row.parentNode.insertBefore(tr, row); });
          if (res.next) {
            more.setAttribute('data-from', res.next);
            more.textContent = '続きを表示(' + res.next + ' 行目から)';
            more.disabled = false;
          } else {
            row.remove();
          }
        });
      }
    });
  }

  // --- The file list follows what is on screen ---------------------------
  var watchers = [];
  function watchFiles(k) {
    var v = views[k];
    if (watchers[k]) watchers[k].disconnect();
    if (!('IntersectionObserver' in window)) return;
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
    watchers[k] = io;
  }
  views.forEach(function (v, k) { watchFiles(k); });
  if (document.body.hasAttribute('data-diffnote-api')) {
    // A view the page can't patch (or that another change made stale) is
    // fetched again and put in place of the old one.
    refreshView = function (k) {
      return fetch('/api/views/' + k, { credentials: 'same-origin' }).then(function (r) { return r.json(); }).then(function (res) {
        if (!res.ok) return;
        if (active) clear();
        views[k].innerHTML = res.html;
        views[k].removeAttribute('data-stale');
        watchFiles(k);
      });
    };
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
        assert_eq!(
            s.html
                .matches(r#"<section class="diffnote-revision"#)
                .count(),
            2
        );
        assert!(view(&s.html, 1).starts_with(r#"id="rev-1" data-diffnote-revision="1""#));
        assert!(
            s.html
                .contains(r#"class="diffnote-revision is-current" id="rev-1""#)
        );
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
        let from = view
            .find(&format!(r#"data-diffnote-thread-id="{id}""#))
            .unwrap();
        let rest = &view[from..];
        let start = rest.find("<summary>").unwrap();
        rest[start..rest.find("</summary>").unwrap()].to_string()
    }

    #[test]
    fn a_threads_location_is_where_it_is_in_this_view() {
        let s = scenario();
        // `B` is line 2 in revision 1 and line 3 in revision 2 (`top` came
        // first), whichever revision the thread was written on.
        assert!(summary_of(view(&s.html, 0), s.t1).contains(">f.txt:2</span>"));
        assert!(summary_of(view(&s.html, 1), s.t1).contains(">f.txt:3</span>"));
    }

    #[test]
    fn the_title_and_revision_tabs_share_one_bar() {
        let s = scenario();
        let bar = &s.html[s.html.find(r#"<div class="diffnote-topbar">"#).unwrap()..];
        let bar = &bar[..bar.find("</div>").unwrap()];
        assert!(
            bar.contains(r#"<header class="diffnote-summary">"#),
            "{bar}"
        );
        assert!(bar.contains(r#"<nav class="diffnote-revisions">"#), "{bar}");
    }

    #[test]
    fn a_thread_card_carries_its_color_for_the_range_highlight() {
        let s = scenario();
        let card = format!(
            r##"data-diffnote-thread-id="{}" data-diffnote-color="#"##,
            s.t1
        );
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
        assert!(
            s.html
                .contains(r#"style="--diffnote-bars: inset 3px 0 0 0 #"#)
        );
        assert!(!s.html.contains(r#"style="box-shadow"#));
    }

    /// One revision, one comment made at 2026-09-20 09:19:43 UTC, and a reply
    /// at 2026-09-21 00:05 UTC, plus the given extra events.
    fn dated(extra: Vec<Event>) -> String {
        let id = Ulid::new();
        let at = |secs| OffsetDateTime::from_unix_timestamp(secs).unwrap();
        let mut events = vec![
            Event::Comment {
                id,
                parent: None,
                author: "a@example.com".into(),
                created_at: at(1_789_895_983),
                anchor: Some(Anchor::Span {
                    base: Some(range(2, 1, R1_BASE)),
                    head: Some(range(2, 1, R1_HEAD)),
                }),
                body: "first".into(),
            },
            Event::Comment {
                id: Ulid::new(),
                parent: Some(id),
                author: "b@example.com".into(),
                created_at: at(1_789_949_100),
                anchor: None,
                body: "second".into(),
            },
        ];
        events.extend(extra);
        let (_dir, loaded) = bundle_of(&[(R1_BASE, R1_HEAD, files_source(None))], events);
        render_bundle(&loaded).unwrap()
    }

    fn title_event(title: &str) -> Event {
        Event::Title {
            title: title.into(),
            author: "a@example.com".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn without_a_title_the_page_has_the_default_heading() {
        let html = dated(Vec::new());
        assert!(html.contains("<h1>diffnote レビュー</h1>"), "{html}");
        assert!(html.contains("<title>diffnote レビュー</title>"));
    }

    #[test]
    fn the_title_takes_the_headings_place_and_is_escaped() {
        let html = dated(vec![title_event("ログイン改修 <v2> & co")]);
        assert!(
            html.contains("<h1>ログイン改修 &lt;v2&gt; &amp; co</h1>"),
            "{html}"
        );
        assert!(html.contains("<title>ログイン改修 &lt;v2&gt; &amp; co</title>"));
        assert!(!html.contains("<h1>diffnote レビュー</h1>"));
    }

    #[test]
    fn the_last_title_wins_and_an_empty_one_takes_it_away() {
        let html = dated(vec![title_event("first"), title_event("second")]);
        assert!(html.contains("<h1>second</h1>"));
        let html = dated(vec![title_event("first"), title_event("  ")]);
        assert!(html.contains("<h1>diffnote レビュー</h1>"));
    }

    #[test]
    fn every_comment_and_reply_shows_when_it_was_written_small() {
        let html = dated(Vec::new());
        // Root and reply each get one, in UTC (the script localizes them), with
        // the exact instant in `datetime`.
        assert!(
            html.contains(r#"<time class="diffnote-comment__time" datetime="2026-09-20T09:19:43Z" title="2026-09-20T09:19:43Z">2026-09-20 09:19 UTC</time>"#),
            "{html}"
        );
        assert!(html.contains("2026-09-21 00:05 UTC"), "{html}");
        assert_eq!(html.matches("<time ").count(), 2);
        // Quiet: a small, gray, unbold style.
        let style = &html[html.find(".diffnote-comment__time {").unwrap()..];
        let rule = &style[..style.find('}').unwrap()];
        assert!(
            rule.contains("font-size: 11px") && rule.contains("color: #8b949e"),
            "{rule}"
        );
    }

    /// The thread list of a view: its `<li>` items, in order.
    fn thread_list(view: &str) -> Vec<&str> {
        let from = view.find(r#"<nav class="diffnote-threadlist">"#).unwrap();
        let list = &view[from..];
        let list = &list[..list.find("</nav>").unwrap()];
        list.split("<li").skip(1).collect()
    }

    #[test]
    fn every_file_has_a_button_that_copies_its_path() {
        let s = scenario();
        for i in 0..2 {
            let v = view(&s.html, i);
            let head = &v[v.find(r#"<section class="diffnote-file""#).unwrap()..];
            let head = &head[..head.find("</summary>").unwrap()];
            assert!(head.contains("<h2>f.txt</h2>"), "{head}");
            assert!(head.contains(r#"data-diffnote-copy="f.txt""#), "{head}");
        }
    }

    #[test]
    fn a_thread_shows_and_copies_its_location() {
        let s = scenario();
        // The line thread: its place in each view, in the form `edit --show` reads.
        let card = summary_of(view(&s.html, 1), s.t1);
        assert!(
            card.contains(r#"<span class="diffnote-thread__where">f.txt:3</span>"#),
            "{card}"
        );
        assert!(card.contains(r#"data-diffnote-copy="f.txt:3""#), "{card}");
        // A file-level thread has just the path; a review-wide one no location.
        let file = summary_of(view(&s.html, 1), s.t3);
        assert!(file.contains(r#"data-diffnote-copy="f.txt""#), "{file}");
        let global = summary_of(view(&s.html, 1), s.global);
        assert!(!global.contains("data-diffnote-copy"), "{global}");
    }

    #[test]
    fn location_covers_a_range_a_line_a_file_and_the_whole_review() {
        let mut marks = Marks::default();
        let (range, line, file, global) = (Ulid::new(), Ulid::new(), Ulid::new(), Ulid::new());
        for id in [range, line, file] {
            marks.files.insert(id, "src/a.rs".to_string());
        }
        marks.lines.insert(range, (10, 13));
        marks.lines.insert(line, (7, 7));
        assert_eq!(location(&marks, range).as_deref(), Some("src/a.rs:10-13"));
        assert_eq!(location(&marks, line).as_deref(), Some("src/a.rs:7"));
        assert_eq!(location(&marks, file).as_deref(), Some("src/a.rs"));
        assert_eq!(location(&marks, global), None);
    }

    #[test]
    fn a_copy_buttons_text_is_escaped() {
        let button = copy_button(r#"a"b<c>&.rs:1"#, r#"t"itle"#);
        assert!(
            button.contains(r#"data-diffnote-copy="a&quot;b&lt;c&gt;&amp;.rs:1""#),
            "{button}"
        );
        assert!(button.contains(r#"title="t&quot;itle""#), "{button}");
    }

    #[test]
    fn the_thread_list_has_every_thread_review_wide_ones_first_then_by_line() {
        let s = scenario();
        for i in 0..2 {
            let v = view(&s.html, i);
            let items = thread_list(v);
            assert_eq!(items.len(), 5, "view {i}: {items:?}");
            for id in [s.t1, s.t2, s.t3, s.t4, s.global] {
                // Each links to its own view's card, which exists.
                let href = format!(r##"href="#r{i}-thread-{id}""##);
                assert_eq!(
                    items.iter().filter(|it| it.contains(&href)).count(),
                    1,
                    "{href}"
                );
                assert!(v.contains(&format!(r#"id="r{i}-thread-{id}""#)), "{href}");
            }
            assert!(
                items[0].contains(&s.global.to_string()),
                "review-wide first: {items:?}"
            );
        }
        // In revision 2 `B` (line 3) comes before `D` (line 5).
        let items = thread_list(view(&s.html, 1));
        let at = |id: Ulid| {
            items
                .iter()
                .position(|it| it.contains(&id.to_string()))
                .unwrap()
        };
        assert!(at(s.t1) < at(s.t2), "{items:?}");
    }

    #[test]
    fn a_thread_in_the_list_shows_its_place_and_the_start_of_what_it_says() {
        let s = scenario();
        let items = thread_list(view(&s.html, 1));
        let item = items
            .iter()
            .find(|it| it.contains(&s.t1.to_string()))
            .unwrap();
        assert!(
            item.contains(r#"<span class="diffnote-threadlist__where">f.txt:3</span>"#),
            "{item}"
        );
        assert!(
            item.contains(r#"<span class="diffnote-threadlist__preview">about B</span>"#),
            "{item}"
        );
        assert!(item.contains(r#"title="f.txt:3""#), "{item}");
        // The file thread was resolved.
        let file = items
            .iter()
            .find(|it| it.contains(&s.t3.to_string()))
            .unwrap();
        assert!(
            file.contains("is-resolved") && file.contains("解決済み"),
            "{file}"
        );
        let global = &items[0];
        assert!(global.contains(">全体</span>"), "{global}");
    }

    #[test]
    fn the_list_heading_counts_open_and_all_threads() {
        let s = scenario();
        assert!(
            s.html.contains(r#"title="未解決 / 全部">4 / 5</span>"#),
            "{}",
            &s.html[..200]
        );
    }

    #[test]
    fn a_preview_is_the_first_line_plain_and_short() {
        assert_eq!(preview("hello\nworld"), "hello");
        assert_eq!(preview("\n\n  ## Title here\nbody"), "Title here");
        assert_eq!(preview("> quoted"), "quoted");
        assert_eq!(preview("- item"), "item");
        assert_eq!(preview(""), "");
        let long = "あ".repeat(60);
        assert_eq!(preview(&long), format!("{}…", "あ".repeat(48)));
        assert_eq!(preview(&"あ".repeat(48)), "あ".repeat(48));
    }

    #[test]
    fn a_review_with_no_threads_has_no_thread_list() {
        let (_dir, loaded) = bundle_of(&[(R1_BASE, R1_HEAD, files_source(None))], Vec::new());
        let html = render_bundle(&loaded).unwrap();
        assert!(!html.contains(r#"<nav class="diffnote-threadlist">"#));
        assert!(html.contains(r#"<nav class="diffnote-filelist">"#));
    }

    #[test]
    fn the_page_scales_to_a_phones_width() {
        let s = scenario();
        assert!(
            s.html.contains(
                r#"<meta name="viewport" content="width=device-width, initial-scale=1">"#
            )
        );
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
        assert!(
            !card.contains("削除された行"),
            "not deleted: it isn't there yet"
        );
        assert!(
            card.contains(r#"<pre class="diffnote-deleted__snippet">top"#),
            "{card}"
        );
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
            assert_eq!(v.matches("<p>overall</p>").count(), 1, "view {i}");
            assert!(v.contains("diffnote-global-comments"));
        }
    }

    #[test]
    fn one_revision_has_no_switcher_and_is_shown() {
        let (_dir, loaded) = bundle_of(&[(R1_BASE, R1_HEAD, files_source(None))], Vec::new());
        let html = render_bundle(&loaded).unwrap();
        assert!(!html.contains(r#"<nav class="diffnote-revisions""#));
        assert_eq!(
            html.matches(r#"<section class="diffnote-revision"#).count(),
            1
        );
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
            &[
                (R1_BASE, R1_HEAD, git),
                (R1_HEAD, R2_HEAD, files_source(None)),
            ],
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
        assert_eq!(
            html.matches(r#"<section class="diffnote-revision"#).count(),
            1
        );
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
            assert!(
                section.contains(&format!(r#"diffnote-line__gutter-new">{n}<"#)),
                "line {n}"
            );
        }
        assert!(
            !section.contains(r#"diffnote-line__gutter-new">8<"#),
            "no more than 3 lines around"
        );
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
        // Once as the thread (the list only shows a short preview of it).
        assert_eq!(html.matches("<p>lost text</p>").count(), 1);
    }
}
