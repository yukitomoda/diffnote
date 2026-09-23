//! Self-contained static HTML export, so a review can be viewed without
//! installing diffnote.
//!
//! The page is a client-side app (see `ui/`), drawn in the browser from the
//! data this works out (`viewmodel`) and embeds in it as JSON, with the app's
//! own bundle and the style embedded beside it. So the output is one file,
//! safe to open by a bare `file://` address: nothing to fetch, no modules, no
//! secure-context-only APIs. It does need JavaScript.
//!
//! What stays here is everything about the review itself: where each thread
//! sits in each revision, the order of the thread list, each line's pieces
//! with the colors of their syntax, and a comment's text parsed into elements.
//! The page lays that out; it works nothing out about the review.
//!
//! Class names are stable and BEM-style, and theme colors are `:root` CSS
//! custom properties, so a future custom-CSS feature can mostly work via
//! variable overrides.
//!
//! Placing threads is read-only: nothing is ever written back, a
//! `Relocated` result just changes where a thread is drawn for this one
//! page.

use crate::anchor::{self, Placement};
use crate::diff::{FileDiff, Hunk, LineKind, UnifiedDiff};
use crate::expand;
use crate::messages::{m, mf};
use crate::model::Side;
use crate::review::{Thread, build_threads};
use std::collections::HashMap;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use ulid::Ulid;

/// The syntax and color definitions, built once: each syntax compiles its
/// patterns the first time it highlights something, which costs far more than
/// drawing the lines, so a page (and a server) must not start over each time.
///
/// The set is the one `bat` uses (through `two-face`): syntect's own has no
/// TypeScript, Dockerfile, TOML and many more.
static SYNTAXES: std::sync::LazyLock<SyntaxSet> =
    std::sync::LazyLock::new(two_face::syntax::extra_newlines);

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
}

/// The revisions that have a diff to show, each with its label, diff and
/// tree (a fresh `init` snapshot has no diff, so is left out).
struct Shown<'a> {
    label: String,
    /// When it was recorded (RFC 3339, UTC): the page says it in the reader's
    /// own time, so the text of it is not made here.
    at: String,
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
            // The commit, not what it was called (`HEAD` and branches move).
            // The base is the same for every revision, and is said apart.
            crate::model::Source::Git(g) => g.head.chars().take(7).collect(),
            crate::model::Source::Files { .. } => m("html.dir_label").to_string(),
        };
        let label = format!("#{} {source}", shown.len() + 1);
        shown.push(Shown {
            label,
            at: viewmodel::rfc3339(revision.created_at),
            diff,
            revision,
            tree: loaded.manifest(revision),
        });
    }
    if shown.is_empty() {
        anyhow::bail!(m("html.no_diff_recorded"));
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

/// The page's script: one bundle, built from `ui/src` into `ui/dist` by
/// `mise run build` (see ui/README.md; build.rs says so if it is missing or
/// older than what it was built from). It is put in the page as a plain script,
/// so that the page works from a file, and it sets `window.Diffnote`.
///
/// There are two, and what tells them apart is what they were built from: only
/// the served page talks to the server, so the exported page's bundle holds
/// none of the code that would (`ui/src/entry-export.ts`).
const CLIENT_EXPORT: &str = include_str!("../ui/dist/export.js");
const CLIENT_SERVE: &str = include_str!("../ui/dist/serve.js");

/// The page `diffnote export` writes: one self-contained HTML file, drawn in
/// the browser by a client-side app from the data of [`view_model`] embedded in
/// it. It needs JavaScript, and nothing else: no requests, no modules, so it
/// opens from a file.
pub fn render_export(loaded: &crate::bundle::Loaded) -> anyhow::Result<String> {
    render_export_with(loaded, ExpandLimit::default())
}

/// The same, saying how many of the lines a diff leaves out the page carries
/// (so they can be shown from the file).
pub fn render_export_with(
    loaded: &crate::bundle::Loaded,
    limit: ExpandLimit,
) -> anyhow::Result<String> {
    client_page(loaded, None, limit)
}

/// The same app for `diffnote review` / `open`: it can change the review, through the
/// server that serves it. `editable` are the comments it may edit and delete.
pub fn render_served_page(
    loaded: &crate::bundle::Loaded,
    editable: Vec<String>,
    changed: Vec<String>,
    author: String,
    refreshable: bool,
    bundle_size: u64,
) -> anyhow::Result<String> {
    client_page(
        loaded,
        Some((editable, changed, author, refreshable, bundle_size)),
        ExpandLimit::Lines(0),
    )
}

/// What the served page is given: the comments it may change, those of them
/// changed in this session, the author, whether it can pull, the bundle size.
type Served = (Vec<String>, Vec<String>, String, bool, u64);

fn client_page(
    loaded: &crate::bundle::Loaded,
    served: Option<Served>,
    limit: ExpandLimit,
) -> anyhow::Result<String> {
    let interactive = served.is_some();
    let data = if let Some((editable, changed, author, refreshable, size)) = served {
        served_model_json(loaded, editable, changed, author, refreshable, size)?
    } else {
        view_model_json(loaded, limit)?
    };
    let title = crate::review::title(&loaded.settings).unwrap_or(m("html.default_title"));
    let script = if interactive {
        CLIENT_SERVE
    } else {
        CLIENT_EXPORT
    };
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
<script type="application/json" id="diffnote-messages">{messages}</script>
<script>
{script}
</script>
<script>Diffnote.start();</script>
</body>
</html>
"#,
        title = escape_html(title),
        css = STYLE,
        messages = crate::messages::as_json(),
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
    let placed = place(&threads, view, &loaded.blobs());
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
    let placed = place(&threads, view, &loaded.blobs());
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

/// One entry of the list of other files.
pub enum TreeItem {
    /// A directory (its own entries are listed when it is opened), and how
    /// many files are under it.
    Dir {
        path: String,
        name: String,
        count: usize,
    },
    File {
        path: String,
        label: String,
    },
}

/// What the list of other files says: entries, or why there are none, and a
/// note about the repository.
pub struct TreeData {
    pub items: Vec<TreeItem>,
    pub message: Option<&'static str>,
    pub note: Option<&'static str>,
    /// How many more there are than are listed.
    pub more: usize,
}

/// The files for the "other files" section of the page: the entries of
/// directory `dir` (sub-directories are listed when opened), or, with a
/// `query`, the files whose path contains it.
pub fn tree_data(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    dir: &str,
    query: &str,
    git: Option<&dyn CommitFiles>,
) -> Option<TreeData> {
    let files = other_files(loaded, revision, git)?;
    // A review made from git, with its repository out of reach.
    let note = {
        let shown = shown_revisions(loaded).ok()?;
        let rev = shown.get(revision)?.revision;
        match head_commit(rev) {
            Some(c) if git.and_then(|g| g.tree(c)).is_none() => Some(m("html.no_repo_notice")),
            _ => None,
        }
    };
    let message = |text| TreeData {
        items: Vec::new(),
        message: Some(text),
        note: None,
        more: 0,
    };
    if files.is_empty() {
        return Some(TreeData {
            note,
            ..message(m("html.nothing_else_openable"))
        });
    }
    let mut items = Vec::new();
    let query = query.trim().to_lowercase();
    if !query.is_empty() {
        let matching: Vec<&String> = files
            .iter()
            .filter(|f| f.to_lowercase().contains(&query))
            .collect();
        if matching.is_empty() {
            return Some(message(m("html.not_found")));
        }
        let more = matching.len().saturating_sub(TREE_LIMIT);
        for f in matching.into_iter().take(TREE_LIMIT) {
            items.push(TreeItem::File {
                path: f.clone(),
                label: f.clone(),
            });
        }
        return Some(TreeData {
            items,
            message: None,
            note: None,
            more,
        });
    }
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
    let mut entries: Vec<TreeItem> = dirs
        .into_iter()
        .map(|(name, count)| TreeItem::Dir {
            path: format!("{prefix}{name}"),
            name: name.to_string(),
            count,
        })
        .collect();
    entries.extend(here.into_iter().map(|(name, path)| TreeItem::File {
        path: path.to_string(),
        label: name.to_string(),
    }));
    let more = entries.len().saturating_sub(TREE_LIMIT);
    entries.truncate(TREE_LIMIT);
    Some(TreeData {
        items: entries,
        message: None,
        note: if dir.is_empty() { note } else { None },
        more,
    })
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
        .ok_or_else(|| m("html.revision_missing").to_string())?
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
    let git = git.ok_or_else(|| m("html.file_not_stored").to_string())?;
    let commit = head_commit(rev).ok_or_else(|| m("html.file_not_stored").to_string())?;
    let tree = git
        .tree(commit)
        .ok_or_else(|| m("html.no_repo_for_file").to_string())?;
    let entry = tree
        .iter()
        .find(|e| e.path == path)
        .ok_or_else(|| m("html.not_in_commit").to_string())?;
    if entry.size > MAX_FILE_BYTES {
        return Err(mf(
            "html.too_big",
            &[
                (
                    "size",
                    &format!("{:.1}", entry.size as f64 / (1024.0 * 1024.0)),
                ),
                ("limit", &(MAX_FILE_BYTES / (1024 * 1024)).to_string()),
            ],
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
        .map_err(|_| m("html.not_text").to_string())
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

/// One revision of the review to show: its diff and per-file digests, and a
/// short label for the switcher.
pub struct RevisionView<'a> {
    pub label: String,
    pub diff: &'a UnifiedDiff,
    pub files: &'a [crate::model::FileDigest],
    /// The head tree the revision recorded (for files its diff leaves alone).
    pub tree: &'a [crate::model::TreeFile],
}

/// Where every thread goes in one revision's view, worked out from its
/// anchor and the texts (never from what the diff shows).
struct Placed {
    /// Where each thread is, in the order of the threads.
    placements: Vec<Placement>,
    /// The view's diff plus context around threads it doesn't show.
    diff: UnifiedDiff,
    /// The files that context adds (files the diff doesn't touch), as
    /// unchanged files.
    synthetic_files: Vec<crate::model::FileDigest>,
    marks: Marks,
    file_order: Vec<String>,
}

fn place(threads: &[Thread], view: &RevisionView, blobs: &crate::digest::Blobs) -> Placed {
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
    // A stable color (index into PALETTE) per thread that ends up as a Line
    // placement, assigned in document order so it's deterministic run to run.
    let mut marks = Marks::default();
    // The files threads are about, in the order they are first met.
    let mut mentioned: Vec<String> = Vec::new();
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
            Placement::Global => {}
            Placement::File(file) => {
                marks.files.insert(thread.root_id, file.clone());
                mentioned.push(file);
            }
            Placement::Line {
                file,
                line_start,
                line_end,
                ..
            } => {
                let color = marks.color_of.len() % PALETTE.len();
                marks.color_of.insert(thread.root_id, color);
                marks.lines.insert(thread.root_id, (line_start, line_end));
                marks.files.insert(thread.root_id, file.clone());
                marks.starts.insert(thread.root_id, line_start);
                mentioned.push(file);
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
                mentioned.push(file);
            }
            Placement::Unplaced { file } => {
                marks.files.insert(thread.root_id, file.clone());
                marks
                    .was
                    .insert(thread.root_id, anchor::original_text(&thread.anchor, blobs));
                mentioned.push(file);
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
    for key in &mentioned {
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
        marks,
        file_order,
    }
}

fn file_key(file_diff: &FileDiff) -> String {
    file_diff
        .new_path
        .clone()
        .or_else(|| file_diff.old_path.clone())
        .unwrap_or_else(|| "?".to_string())
}

/// Colors assigned round-robin to threads, avoiding red/green (already used
/// for the added/removed diff backgrounds).
const PALETTE: [&str; 8] = [
    "#1f77b4", "#ff7f0e", "#9467bd", "#8c564b", "#e377c2", "#17becf", "#bcbd22", "#7f7f7f",
];

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

fn guess_syntax<'a>(file: &str, syntax_set: &'a SyntaxSet) -> &'a SyntaxReference {
    let path = std::path::Path::new(file);
    // By the whole name first (`Dockerfile`, `Makefile`), then by extension.
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| syntax_set.find_syntax_by_extension(name))
        .or_else(|| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .and_then(|ext| syntax_set.find_syntax_by_extension(ext))
        })
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
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

const STYLE: &str = include_str!("../ui/dist/style.css");

mod markdown;
pub(crate) mod tokens;
mod viewmodel;
mod words;
pub use viewmodel::{
    ExpandLimit, OpenedData, ViewModel, bundle_info, chunk_data, compare_data, lines_json,
    opened_data, served_model_json, stamp, thread_json, tree_json, view_model, view_model_for,
    view_model_json, view_model_with,
};

#[cfg(test)]
mod tests {
    use super::viewmodel::PlacementData;
    use super::*;
    use crate::bundle::{self, Additions};
    use crate::digest::digest;
    use crate::files::{Tree, diff_trees};
    use crate::model::{Anchor, Event, FileRef, LineRange, Revision, SnapshotMode, Source};
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
                commits: Vec::new(),
            }));
            let additions = Additions {
                diff: Some((key, diff_text)),
                blobs: vec![head.as_bytes().to_vec(), base.as_bytes().to_vec()],
                commits: Vec::new(),
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

    fn title_event(title: &str) -> Event {
        Event::Title {
            title: title.into(),
            author: "a@example.com".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn hiding_resolved_threads_covers_cards_rows_list_entries_and_line_marks() {
        // The rules the box switches on (the class is on the body). The style is
        // built (`mise run build`), so it is read without its spacing.
        let style: String = STYLE.chars().filter(|c| !c.is_whitespace()).collect();
        for rule in [
            ".diffnote-hide-resolved .diffnote-thread--resolved { display: none }",
            ".diffnote-hide-resolved .diffnote-thread-row:has(> td > .diffnote-thread--resolved) { display: none }",
            ".diffnote-hide-resolved .diffnote-outdated__entry:has(> .diffnote-thread--resolved) { display: none }",
            ".diffnote-hide-resolved .diffnote-threadlist .is-resolved { display: none }",
            ".diffnote-hide-resolved .diffnote-line--resolved-only > td:first-child { --dn-l: 0 0 0 0 transparent }",
        ] {
            let want: String = rule.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(style.contains(&want), "{rule}");
        }
    }

    // ---- the view model ---------------------------------------------------

    fn model_of_scenario() -> (ViewModel, [Ulid; 5], tempfile::TempDir, bundle::Loaded) {
        let (dir, loaded, ids) = scenario_parts();
        (view_model(&loaded).unwrap(), ids, dir, loaded)
    }

    #[test]
    fn the_model_has_the_threads_their_comments_and_a_revision_per_view() {
        let (m, [t1, ..], _dir, _loaded) = model_of_scenario();
        assert_eq!(m.version, 5);
        assert_eq!(m.title, None);
        assert_eq!(m.revisions.len(), 2);
        assert_eq!(m.threads.len(), 5);
        let thread = m.threads.iter().find(|t| t.id == t1.to_string()).unwrap();
        // The first comment, then its reply; text as a tree; times in UTC.
        assert_eq!(thread.comments.len(), 2);
        let doc = |i: usize| serde_json::to_string(&thread.comments[i].doc).unwrap();
        assert_eq!(doc(0), r#"[{"c":["about B"],"t":"p"}]"#);
        assert_eq!(doc(1), r#"[{"c":["a reply"],"t":"p"}]"#);
        assert_eq!(thread.comments[0].author, "r@example.com");
        assert_eq!(thread.comments[0].at, "1970-01-01T00:00:00Z");
        assert!(!thread.resolved);
        let file_thread = m
            .threads
            .iter()
            .find(|t| {
                serde_json::to_string(&t.comments[0].doc)
                    .unwrap()
                    .contains("file thread")
            })
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
        let text =
            |i: usize| -> String { file.hunks[0].rows[i].t.iter().map(|t| t.text()).collect() };
        assert!(text(1).contains('b') && text(2).contains('B'));
        assert!(
            file.hunks[0].header.starts_with("@@ -1,4 +1,4 @@"),
            "{}",
            file.hunks[0].header
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
        let json = view_model_json(&loaded, ExpandLimit::default()).unwrap();
        assert!(!json.contains('<'), "{json}");
        // It is still the same data.
        let back: serde_json::Value = serde_json::from_str(&json).unwrap();
        let doc = back["threads"][0]["comments"][0]["doc"].to_string();
        // The comment's own `<b>`, `</script>` and `<!--` are text in the tree.
        assert!(doc.contains("<b>bold</b>"), "{doc}");
        assert!(doc.contains("script"), "{doc}");
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

    /// The page without what is inside its script elements: the bundle is built
    /// minified, so a line like `full.src = image.src` is in it as `full.src=`,
    /// which says nothing about what the page loads. What it loads is in the
    /// markup, and `ui/src/test/sources.test.js` keeps elements out of the
    /// scripts' own text.
    fn without_scripts(page: &str) -> String {
        let mut out = String::new();
        let mut rest = page;
        while let Some(start) = rest.find("<script") {
            out.push_str(&rest[..start]);
            rest = &rest[start..];
            let end = rest.find("</script>").map_or(rest.len(), |i| i + 9);
            rest = &rest[end..];
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn the_export_is_one_page_with_its_data_and_scripts_and_nothing_to_fetch() {
        let (_dir, loaded, _ids) = scenario_parts();
        let page = render_export(&loaded).unwrap();
        assert!(page.contains(r#"<script type="application/json" id="diffnote-data">"#));
        assert!(page.contains("Diffnote.start();"));
        assert!(page.contains(r#"<div id="app"></div>"#));
        // Nothing that would need a request, a module or a worker (which a
        // page opened from a file can't have). What the bundle is built from is
        // held to more than this by `ui/src/test/sources.test.js`.
        assert!(
            !CLIENT_EXPORT.contains("</script"),
            "a script that would end its element"
        );
        assert!(!CLIENT_EXPORT.contains("import("), "dynamic import");
        assert!(!CLIENT_EXPORT.contains("XMLHttpRequest"), "XMLHttpRequest");
        assert!(!CLIENT_EXPORT.contains("new Worker"), "workers");
        assert!(!CLIENT_EXPORT.contains("serviceWorker"), "service workers");
        assert!(!page.contains(r#"type="module""#));
        // Only the served page has what talks to the server: it is left out of
        // the exported page's bundle, not turned off in it.
        assert!(
            !CLIENT_EXPORT.contains("fetch("),
            "an exported page makes no requests"
        );
        assert!(CLIENT_SERVE.contains("fetch("));
        let markup = without_scripts(&page);
        assert!(!markup.contains("src="), "no external file");
        assert!(!markup.contains("<link"), "no external style");
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
            commits: Vec::new(),
        }));
        events.extend(extra);
        let additions = Additions {
            diff: Some((digest("r"), diff_text)),
            blobs: vec![
                R1_BASE.as_bytes().to_vec(),
                R1_HEAD.as_bytes().to_vec(),
                readme.as_bytes().to_vec(),
            ],
            commits: Vec::new(),
        };
        bundle::save(&path, &bundle::load(&path).unwrap(), &events, &additions).unwrap();
        (dir, bundle::load(&path).unwrap())
    }

    fn range_of(file: &str, text: &str, start: u32, len: u32) -> LineRange {
        LineRange {
            file: file.to_string(),
            digest: digest(text),
            start,
            len,
        }
    }
}
