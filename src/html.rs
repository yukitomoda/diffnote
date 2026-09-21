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

/// The scripts of the client-side page, in the order they are put in it: the
/// libraries (plain-script builds, so the page works from a file), then the
/// page's own. Each adds to `window.Diffnote`.
const CLIENT_LIBS: [&str; 5] = [
    include_str!("../ui/vendor/preact.min.js"),
    include_str!("../ui/vendor/hooks.umd.js"),
    include_str!("../ui/vendor/htm.js"),
    include_str!("../ui/client/lib.js"),
    include_str!("../ui/client/interact.js"),
];
const CLIENT_APP: &str = include_str!("../ui/client/app.js");
/// Only the served page talks to the server (an exported page makes no
/// requests), so only it has this.
const CLIENT_API: &str = include_str!("../ui/client/api.js");

/// The page `diffnote export` writes: one self-contained HTML file, drawn in
/// the browser by a client-side app from the data of [`view_model`] embedded in
/// it. It needs JavaScript, and nothing else: no requests, no modules, so it
/// opens from a file.
pub fn render_export(loaded: &crate::bundle::Loaded) -> anyhow::Result<String> {
    client_page(loaded, false)
}

/// The same app for `diffnote serve`: it can change the review, through the
/// server that serves it.
pub fn render_served_page(loaded: &crate::bundle::Loaded) -> anyhow::Result<String> {
    client_page(loaded, true)
}

fn client_page(loaded: &crate::bundle::Loaded, interactive: bool) -> anyhow::Result<String> {
    let data = if interactive {
        served_model_json(loaded)?
    } else {
        view_model_json(loaded)?
    };
    let title = crate::review::title(&loaded.events).unwrap_or(DEFAULT_TITLE);
    let mut scripts: Vec<&str> = CLIENT_LIBS.to_vec();
    if interactive {
        scripts.push(CLIENT_API);
    }
    scripts.push(CLIENT_APP);
    let scripts: String = scripts
        .iter()
        .map(|s| format!("<script>\n{s}\n</script>\n"))
        .collect();
    Ok(format!(
        r#"<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>

{css}
</style>
</head>
<body>
<div id="app"></div>
<script type="application/json" id="diffnote-data">{data}</script>
{scripts}<script>Diffnote.start();</script>
</body>
</html>
"#,
        title = escape_html(title),
        css = STYLE,
    ))
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
    git: Option<&dyn CommitFiles>,
) -> Option<AnchorView> {
    let shown = shown_revisions(loaded).ok()?;
    let views = revision_views(&shown);
    let view = views.get(index)?;
    let threads = build_threads(&loaded.events);
    let placed = place(&threads, view, &loaded.blobs(), true);
    let mut diff = placed.diff;
    let mut files = view.files.to_vec();
    files.extend(placed.synthetic_files);
    let mut store = None;
    if let Some(path) = file
        && !diff.files.iter().any(|f| file_key(f) == path)
        && let Ok(content) = file_content(loaded, index, path, git)
        && std::str::from_utf8(&content.bytes).is_ok()
    {
        let digest = crate::digest::digest(&content.bytes);
        // From the commit: the comment is what stores it.
        if !content.stored {
            store = Some((
                crate::model::TreeFile {
                    path: path.to_string(),
                    digest: digest.clone(),
                },
                content.bytes,
            ));
        }
        diff.files.push(FileDiff {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            ..FileDiff::default()
        });
        files.push(crate::model::FileDigest {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            old: Some(digest.clone()),
            new: Some(digest),
        });
    }
    Some(AnchorView {
        revision: shown[index].revision.clone(),
        diff,
        files,
        store,
    })
}

/// What [`anchor_view`] gives: what a thread is anchored against.
pub struct AnchorView {
    pub revision: crate::model::Revision,
    pub diff: UnifiedDiff,
    pub files: Vec<crate::model::FileDigest>,
    /// A file of the commit that the bundle doesn't store yet and that the
    /// thread is about: to be stored (its entry for the revision and its
    /// content) together with the thread.
    pub store: Option<(crate::model::TreeFile, Vec<u8>)>,
}

/// Files of a git commit, for a page served next to the repository the review
/// was made from: the ones the bundle doesn't store can still be looked at
/// (and are stored when a comment is made on them).
pub trait CommitFiles {
    /// Every file of the commit's tree, or `None` if the repository doesn't
    /// have the commit.
    fn tree(&self, commit: &str) -> Option<std::sync::Arc<Vec<crate::git::TreeEntry>>>;
    /// The content of one of those files.
    fn read(&self, entry: &crate::git::TreeEntry) -> Result<Vec<u8>, String>;
}

/// Files bigger than this are not opened (or stored by a comment).
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// The commit a revision's head is, for a review made from git.
fn head_commit(revision: &crate::model::Revision) -> Option<&str> {
    match &revision.source {
        crate::model::Source::Git(g) => Some(g.head.as_str()),
        crate::model::Source::Files { .. } => None,
    }
}

// ---- other files: the stored tree, opened to look at ------------------------

/// Lines drawn at a time when a stored file is opened.
const OPEN_CHUNK: usize = 500;
/// Entries listed at most, in one directory or one search.
const TREE_LIMIT: usize = 500;

/// The files the revision has that the view doesn't already show (as part of
/// the diff, or for a thread): those it stores and, next to the repository,
/// the rest of its head commit. By path.
fn other_files(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    git: Option<&dyn CommitFiles>,
) -> Option<Vec<String>> {
    let shown = shown_revisions(loaded).ok()?;
    let views = revision_views(&shown);
    let view = views.get(revision)?;
    let threads = build_threads(&loaded.events);
    let placed = place(&threads, view, &loaded.blobs(), true);
    let rev = shown[revision].revision;
    let manifest = loaded.manifest(rev);
    let mut files: Vec<String> = manifest
        .iter()
        .filter(|f| !placed.file_order.contains(&f.path) && loaded.blob(&f.digest).is_some())
        .map(|f| f.path.clone())
        .collect();
    if let Some(tree) = head_commit(rev).and_then(|c| git?.tree(c)) {
        files.extend(
            tree.iter()
                .map(|e| &e.path)
                .filter(|p| {
                    !placed.file_order.contains(p) && !manifest.iter().any(|f| &f.path == *p)
                })
                .cloned(),
        );
    }
    files.sort();
    files.dedup();
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
    git: Option<&dyn CommitFiles>,
) -> Option<String> {
    let files = other_files(loaded, revision, git)?;
    // A review made from git, with its repository out of reach.
    let note = {
        let shown = shown_revisions(loaded).ok()?;
        let rev = shown.get(revision)?.revision;
        match head_commit(rev) {
            Some(c) if git.and_then(|g| g.tree(c)).is_none() => {
                r#"<p class="diffnote-tree__empty">このバンドルの git リポジトリが見つからないため、保存済みのファイルだけを表示しています</p>"#
            }
            _ => "",
        }
    };
    if files.is_empty() {
        return Some(format!(
            r#"<p class="diffnote-tree__empty">ほかに開けるファイルはありません</p>{note}"#
        ));
    }
    let mut items = Vec::new();
    let more;
    let query = query.trim().to_lowercase();
    if !query.is_empty() {
        let matching: Vec<&String> = files
            .iter()
            .filter(|f| f.to_lowercase().contains(&query))
            .collect();
        more = matching.len().saturating_sub(TREE_LIMIT);
        for f in matching.into_iter().take(TREE_LIMIT) {
            items.push(tree_file_item(f, f));
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
            let Some(rest) = f.strip_prefix(prefix.as_str()) else {
                continue;
            };
            match rest.split_once('/') {
                Some((name, _)) => *dirs.entry(name).or_default() += 1,
                None => here.push((rest, f.as_str())),
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
    if dir.is_empty() && query.is_empty() {
        out.push_str(note);
    }
    if more > 0 {
        out.push_str(&format!(
            r#"<p class="diffnote-tree__empty">ほか {more} 件(検索で絞り込んでください)</p>"#
        ));
    }
    Some(out)
}

/// A file of a revision to look at or comment on: what the bundle stores of
/// it, or else (next to the repository) what its head commit has.
struct FileContent {
    bytes: Vec<u8>,
    /// Whether the bundle already stores it (a comment on it need store nothing).
    stored: bool,
}

fn file_content(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    path: &str,
    git: Option<&dyn CommitFiles>,
) -> Result<FileContent, String> {
    let shown = shown_revisions(loaded).map_err(|e| e.to_string())?;
    let rev = shown
        .get(revision)
        .ok_or("そのリビジョンはありません")?
        .revision;
    if let Some(entry) = loaded.manifest(rev).into_iter().find(|f| f.path == path)
        && let Some(bytes) = loaded.blob(&entry.digest)
    {
        return Ok(FileContent {
            bytes: bytes.to_vec(),
            stored: true,
        });
    }
    // Not stored: from the commit, if the repository is there and has it.
    let git = git.ok_or("そのファイルはこのレビューに保存されていません")?;
    let commit = head_commit(rev).ok_or("そのファイルはこのレビューに保存されていません")?;
    let tree = git
        .tree(commit)
        .ok_or("このバンドルの git リポジトリが見つからないため、このファイルは開けません")?;
    let entry = tree
        .iter()
        .find(|e| e.path == path)
        .ok_or("そのファイルはこのコミットにありません")?;
    if entry.size > MAX_FILE_BYTES {
        return Err(format!(
            "大きすぎるため開けません({:.1} MB。上限は {} MB)",
            entry.size as f64 / (1024.0 * 1024.0),
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    Ok(FileContent {
        bytes: git.read(entry)?,
        stored: false,
    })
}

/// The text of a file of the revision, or why it can't be shown.
fn stored_text(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    path: &str,
    git: Option<&dyn CommitFiles>,
) -> Result<String, String> {
    String::from_utf8(file_content(loaded, revision, path, git)?.bytes)
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
    git: Option<&dyn CommitFiles>,
) -> Result<OpenedFile, String> {
    let text = stored_text(loaded, revision, path, git)?;
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
    git: Option<&dyn CommitFiles>,
) -> Result<(String, Option<usize>), String> {
    let text = stored_text(loaded, revision, path, git)?;
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
    // Where there is something to hide (on the served page there may be, soon).
    if interactive || threads.iter().any(|t| t.resolved) {
        body.push_str(
            r#"<label class="diffnote-toggle"><input type="checkbox" data-diffnote-hide-resolved> 解決済みを隠す<span class="diffnote-toggle__count" data-diffnote-resolved-count></span></label>"#,
        );
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
    /// Where each thread is, in the order of the threads.
    placements: Vec<Placement>,
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
    // Where each thread ended up (what the page is drawn from), in thread order.
    let mut placed_at: Vec<Placement> = Vec::new();

    for (thread, placement) in threads.iter().zip(placements) {
        // A card is drawn after a row of the diff; without one (the text to
        // build it from isn't held) the thread is listed as unplaced.
        let placement = match expand::want_of(&placement) {
            Some(w) if !expand::has_row(diff, &w) => Placement::Unplaced { file: w.file },
            _ => placement,
        };
        placed_at.push(placement.clone());
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
        placements: placed_at,
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
        placements: _,
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

const STYLE: &str = include_str!("../ui/style.css");

/// Purely local DOM interaction: no fetch, no network, no storage -- safe
/// under a bare `file://` URL. Switches revisions, shows the range of the
/// comment under the mouse (or pinned by a click) on its lines and outlines
/// its card -- the precise way to tell overlapping ranges apart -- and marks
/// which files are on screen in the file list.
const SCRIPT: &str = include_str!("../ui/app.js");

pub(crate) mod viewmodel;
pub use viewmodel::{
    ViewModel, served_model_json, thread_json, view_model, view_model_for, view_model_json,
};

#[cfg(test)]
mod tests {
    use super::viewmodel::{PlacementData, RevisionData};
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
    /// The scenario's review, and the ids of its threads (`t1`, `t2`, `t3`, `t4`, `global`).
    fn scenario_parts() -> (tempfile::TempDir, bundle::Loaded, [Ulid; 5]) {
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
        (dir, loaded, [t1, t2, t3, t4, global])
    }

    fn scenario() -> Scenario {
        let (dir, loaded, [t1, t2, t3, t4, global]) = scenario_parts();
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
    fn the_export_has_a_box_to_hide_resolved_threads_only_when_there_are_some() {
        // The scenario has a resolved thread.
        let s = scenario();
        assert!(s.html.contains(r#"<label class="diffnote-toggle"><input type="checkbox" data-diffnote-hide-resolved> 解決済みを隠す"#), "{}", &s.html[..600]);
        // The counter the script fills in.
        assert!(s.html.contains("data-diffnote-resolved-count"));
        // Nothing resolved: nothing to hide, so no box.
        let html = dated(Vec::new());
        assert!(
            !html.contains(r#"<input type="checkbox" data-diffnote-hide-resolved>"#),
            "{html}"
        );
    }

    #[test]
    fn the_served_page_always_has_the_box_for_threads_resolved_later() {
        let (_dir, loaded) = bundle_of(&[(R1_BASE, R1_HEAD, files_source(None))], Vec::new());
        let html = render_bundle_interactive(&loaded).unwrap();
        assert!(html.contains(r#"<input type="checkbox" data-diffnote-hide-resolved>"#));
        // Before the quit button, which stays at the right end.
        let toggle = html.find("data-diffnote-hide-resolved>").unwrap();
        assert!(toggle < html.find("data-diffnote-shutdown").unwrap());
    }

    #[test]
    fn hiding_resolved_threads_covers_cards_rows_list_entries_and_line_marks() {
        // The rules the box switches on (the class is on the body).
        for rule in [
            ".diffnote-hide-resolved .diffnote-thread--resolved { display: none; }",
            ".diffnote-hide-resolved .diffnote-thread-row:has(> td > .diffnote-thread--resolved) { display: none; }",
            ".diffnote-hide-resolved .diffnote-outdated__entry:has(> .diffnote-thread--resolved) { display: none; }",
            ".diffnote-hide-resolved .diffnote-threadlist .is-resolved { display: none; }",
            ".diffnote-hide-resolved .diffnote-line--resolved-only > td:first-child { --dn-l: 0 0 0 0 transparent; }",
        ] {
            assert!(STYLE.contains(rule), "{rule}");
        }
    }

    // ---- the view model ---------------------------------------------------

    fn model_of_scenario() -> (ViewModel, [Ulid; 5], tempfile::TempDir, bundle::Loaded) {
        let (dir, loaded, ids) = scenario_parts();
        (view_model(&loaded).unwrap(), ids, dir, loaded)
    }

    fn placement_of(rev: &RevisionData, id: Ulid) -> &PlacementData {
        &rev.placements[&id.to_string()]
    }

    #[test]
    fn the_model_has_the_threads_their_comments_and_a_revision_per_view() {
        let (m, [t1, ..], _dir, _loaded) = model_of_scenario();
        assert_eq!(m.version, 1);
        assert_eq!(m.title, None);
        assert_eq!(m.revisions.len(), 2);
        assert_eq!(m.threads.len(), 5);
        let thread = m.threads.iter().find(|t| t.id == t1.to_string()).unwrap();
        // The first comment, then its reply; text as HTML; times in UTC.
        assert_eq!(thread.comments.len(), 2);
        assert_eq!(thread.comments[0].html, "<p>about B</p>\n");
        assert_eq!(thread.comments[1].html, "<p>a reply</p>\n");
        assert_eq!(thread.comments[0].author, "r@example.com");
        assert_eq!(thread.comments[0].at, "1970-01-01T00:00:00Z");
        assert!(!thread.resolved);
        let file_thread = m
            .threads
            .iter()
            .find(|t| t.comments[0].html.contains("file thread"))
            .unwrap();
        assert!(file_thread.resolved);
        assert!(
            m.revisions[1].label.starts_with("#2 "),
            "{}",
            m.revisions[1].label
        );
    }

    #[test]
    fn a_title_is_carried() {
        let (_dir, loaded) = bundle_of(
            &[(R1_BASE, R1_HEAD, files_source(None))],
            vec![title_event("ログイン改修")],
        );
        assert_eq!(
            view_model(&loaded).unwrap().title.as_deref(),
            Some("ログイン改修")
        );
    }

    #[test]
    fn rows_have_a_kind_line_numbers_and_colored_text() {
        let (m, _, _dir, _loaded) = model_of_scenario();
        let file = &m.revisions[0].files[0];
        assert_eq!((file.path.as_str(), file.status), ("f.txt", "modified"));
        assert_eq!(file.old_path, None);
        assert_eq!(file.hunks.len(), 1);
        let rows: Vec<(&str, Option<u32>, Option<u32>)> =
            file.hunks[0].rows.iter().map(|r| (r.k, r.o, r.n)).collect();
        // `b` became `B`: a removed row, an added row, unchanged around them.
        assert_eq!(
            rows,
            [
                ("c", Some(1), Some(1)),
                ("d", Some(2), None),
                ("a", None, Some(2)),
                ("c", Some(3), Some(3)),
                ("c", Some(4), Some(4)),
            ]
        );
        assert!(file.hunks[0].rows[1].h.contains('b') && file.hunks[0].rows[2].h.contains('B'));
        assert!(
            file.hunks[0].header.starts_with("@@ -1,4 +1,4 @@"),
            "{}",
            file.hunks[0].header
        );
    }

    #[test]
    fn placements_are_where_the_page_draws_each_thread_in_each_revision() {
        let (m, [t1, t2, t3, t4, global], _dir, loaded) = model_of_scenario();
        let (r0, r1) = (&m.revisions[0], &m.revisions[1]);
        // A thread on lines follows them: `B` is line 2 in revision 1, line 3 in 2.
        let PlacementData::Line {
            file,
            side,
            start,
            end,
            color,
            ..
        } = placement_of(r0, t1)
        else {
            panic!("a line");
        };
        assert_eq!((file.as_str(), *side, *start, *end), ("f.txt", "new", 2, 2));
        let _ = color;
        let PlacementData::Line { start, end, .. } = placement_of(r1, t1) else {
            panic!("a line");
        };
        assert_eq!((*start, *end), (3, 3));
        // The whole-file and the review-wide threads.
        assert!(matches!(placement_of(r1, t3), PlacementData::File { file } if file == "f.txt"));
        assert!(matches!(placement_of(r1, global), PlacementData::Global));
        // A thread about lines an earlier view doesn't have yet.
        let PlacementData::Point { absence, was, .. } = placement_of(r0, t4) else {
            panic!("a point");
        };
        assert_eq!(*absence, "not-yet");
        assert_eq!(was, &vec!["top".to_string()]);
        // Where the page's own rows say it is: the model and the page agree.
        let html = render_bundle(&loaded).unwrap();
        let rows = rows_of(view(&html, 1), t2);
        let PlacementData::Line { start, end, .. } = placement_of(r1, t2) else {
            panic!("a line");
        };
        assert_eq!(rows.last().unwrap().1, end.to_string());
        assert!(rows.iter().any(|(_, n)| *n == start.to_string()));
    }

    #[test]
    fn the_order_is_the_order_of_the_pages_thread_list() {
        let (m, ids, _dir, loaded) = model_of_scenario();
        let html = render_bundle(&loaded).unwrap();
        for (i, rev) in m.revisions.iter().enumerate() {
            let in_page: Vec<String> = thread_list(view(&html, i))
                .iter()
                .map(|item| {
                    ids.iter()
                        .find(|id| item.contains(&id.to_string()))
                        .unwrap()
                        .to_string()
                })
                .collect();
            assert_eq!(rev.order, in_page, "revision {i}");
            assert_eq!(rev.order.len(), 5);
        }
        assert_eq!(
            m.revisions[1].order[0],
            ids[4].to_string(),
            "review-wide first"
        );
    }

    #[test]
    fn a_file_the_diff_never_touches_comes_in_as_context_with_its_lines() {
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
                "about line 4",
            )],
        );
        let m = view_model(&loaded).unwrap();
        let files = &m.revisions[0].files;
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["f.txt", "README.md"]
        );
        let readme_file = &files[1];
        assert_eq!(readme_file.status, "context");
        let rows: Vec<(&str, Option<u32>, Option<u32>)> = readme_file
            .hunks
            .iter()
            .flat_map(|h| &h.rows)
            .map(|r| (r.k, r.o, r.n))
            .collect();
        // Line 4 with three lines around it, all unchanged.
        assert_eq!(
            rows.iter().map(|r| r.2.unwrap()).collect::<Vec<_>>(),
            (1..=7).collect::<Vec<_>>()
        );
        assert!(rows.iter().all(|r| r.0 == "c" && r.1 == r.2));
        let PlacementData::Line {
            file, start, end, ..
        } = &m.revisions[0].placements[&id.to_string()]
        else {
            panic!("a line");
        };
        assert_eq!((file.as_str(), *start, *end), ("README.md", 4, 4));
    }

    #[test]
    fn a_thread_whose_versions_are_not_held_is_unplaced_with_its_file() {
        let id = Ulid::new();
        let (_dir, loaded) = bundle_with_readme(
            "# x\n",
            vec![comment(
                id,
                Anchor::Span {
                    base: None,
                    head: Some(range_of("README.md", "held nowhere\n", 1, 1)),
                },
                "lost text",
            )],
        );
        let m = view_model(&loaded).unwrap();
        assert!(matches!(&m.revisions[0].placements[&id.to_string()],
            PlacementData::Unplaced { file, .. } if file == "README.md"));
        assert!(m.revisions[0].order.contains(&id.to_string()));
    }

    #[test]
    fn the_json_has_no_less_than_sign_so_it_can_sit_in_a_script_element() {
        let id = Ulid::new();
        let (_dir, loaded) = bundle_of(
            &[(R1_BASE, R1_HEAD, files_source(None))],
            vec![comment(
                id,
                Anchor::Span {
                    base: Some(range(2, 1, R1_BASE)),
                    head: Some(range(2, 1, R1_HEAD)),
                },
                "</script><!-- <b>bold</b> -->",
            )],
        );
        let json = view_model_json(&loaded).unwrap();
        assert!(!json.contains('<'), "{json}");
        // It is still the same data.
        let back: serde_json::Value = serde_json::from_str(&json).unwrap();
        let html = back["threads"][0]["comments"][0]["html"].as_str().unwrap();
        assert!(
            html.contains("<b>bold</b>") || html.contains("&lt;b&gt;"),
            "{html}"
        );
        assert!(html.contains("script"), "{html}");
    }

    #[test]
    fn the_status_of_a_file_says_what_happened_to_it() {
        let file = |old: Option<&str>, new: Option<&str>, rename: bool, binary: bool| FileDiff {
            old_path: old.map(str::to_string),
            new_path: new.map(str::to_string),
            is_rename: rename,
            is_binary: binary,
            hunks: Vec::new(),
        };
        use viewmodel::file_status;
        assert_eq!(
            file_status(Some(&file(Some("a"), Some("a"), false, false)), true),
            "modified"
        );
        assert_eq!(
            file_status(Some(&file(None, Some("a"), false, false)), true),
            "added"
        );
        assert_eq!(
            file_status(Some(&file(Some("a"), None, false, false)), true),
            "deleted"
        );
        assert_eq!(
            file_status(Some(&file(Some("a"), Some("b"), true, false)), true),
            "renamed"
        );
        assert_eq!(
            file_status(Some(&file(Some("a"), Some("a"), false, true)), true),
            "binary"
        );
        assert_eq!(
            file_status(Some(&file(Some("a"), Some("a"), false, false)), false),
            "context"
        );
        assert_eq!(file_status(None, false), "context");
    }

    #[test]
    fn the_export_is_one_page_with_its_data_and_scripts_and_nothing_to_fetch() {
        let (_dir, loaded, _ids) = scenario_parts();
        let page = render_export(&loaded).unwrap();
        assert!(page.contains(r#"<script type="application/json" id="diffnote-data">"#));
        assert!(page.contains("Diffnote.start();"));
        assert!(page.contains(r#"<div id="app"></div>"#));
        // Nothing that would need a request, a module or a worker (which a
        // page opened from a file can't have).
        for s in CLIENT_LIBS.iter().chain(&[CLIENT_APP]) {
            assert!(
                !s.contains("</script"),
                "a script that would end its element"
            );
            assert!(!s.contains("import("), "dynamic import");
            assert!(!s.contains("fetch("), "fetch");
            assert!(!s.contains("XMLHttpRequest"), "XMLHttpRequest");
            assert!(!s.contains("new Worker"), "workers");
            assert!(!s.contains("serviceWorker"), "service workers");
        }
        assert!(!page.contains(r#"type="module""#));
        // Only the served page has what talks to the server.
        assert!(
            !page.contains("D.api = "),
            "an exported page makes no requests"
        );
        assert!(render_served_page(&loaded).unwrap().contains("D.api = "));
        assert!(!page.contains("src="), "no external file");
        assert!(!page.contains("<link"), "no external style");
    }

    #[test]
    fn the_export_page_carries_the_title() {
        let (_dir, loaded) = bundle_of(
            &[(R1_BASE, R1_HEAD, files_source(None))],
            vec![title_event("ログイン改修 <v2> & co")],
        );
        let page = render_export(&loaded).unwrap();
        assert!(
            page.contains("<title>ログイン改修 &lt;v2&gt; &amp; co</title>"),
            "{}",
            &page[..400]
        );
        let (_dir, loaded) = bundle_of(&[(R1_BASE, R1_HEAD, files_source(None))], Vec::new());
        assert!(
            render_export(&loaded)
                .unwrap()
                .contains("<title>diffnote レビュー</title>")
        );
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
