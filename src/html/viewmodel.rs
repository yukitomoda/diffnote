//! The data the page is drawn from: everything the page needs to know, worked
//! out here (where each thread is in each revision -- the re-anchoring --, the
//! order of the thread list, the lines of each file with their colors, the
//! comments as HTML), and nothing about how it is laid out.
//!
//! The page is a client-side app (see `ui/`): it gets this as JSON, embedded in
//! the HTML. Layouts (unified, side by side) are made from the rows there.
//!
//! Keys of a row are short, since a big diff has many of them: `k` kind
//! (`c` unchanged, `a` added, `d` removed), `o`/`n` the line number on the old
//! and new side (absent where the line isn't there), `t` the text as pieces
//! (`"text"`, or `[kind, "text"]` for a piece of a kind: see `tokens`). The
//! page draws the pieces; there is no HTML in the data.

use super::tokens::{Token, Tokenizer};
use super::*;
use serde::Serialize;
use std::collections::BTreeMap;

/// The version of this format, for the page to check.
pub const VERSION: u32 = 5;

#[derive(Serialize)]
pub struct ViewModel {
    pub version: u32,
    /// A stamp of the review's log when this was made (its digest): what a page
    /// compares to see whether the review has changed under it.
    pub stamp: String,
    /// The comments the page may edit or delete (served page only): those
    /// added since the server started.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub editable: Vec<String>,
    /// The name comments are written under (served page only; the page can
    /// change it for the rest of the session).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Whether the page may ask the server to take in what was added to the
    /// target since it started (served page only).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub refreshable: bool,
    /// What every revision is compared with: the review's first snapshot.
    pub base: Option<BaseData>,
    /// Whether differences that are only in white space are hidden.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ignore_whitespace: bool,
    /// Whether the page can change the review (the served one).
    pub interactive: bool,
    pub title: Option<String>,
    /// Every thread, whatever the revision (where it is is per revision).
    pub threads: Vec<ThreadData>,
    /// Oldest first; the last is the one shown first.
    pub revisions: Vec<RevisionData>,
}

#[derive(Serialize)]
pub struct ThreadData {
    pub id: String,
    pub resolved: bool,
    /// The first comment, then the replies.
    pub comments: Vec<CommentData>,
}

#[derive(Serialize)]
pub struct CommentData {
    pub id: String,
    pub author: String,
    /// When it was written (RFC 3339, UTC).
    pub at: String,
    /// The text as a tree (see `markdown`), not HTML.
    pub doc: Vec<serde_json::Value>,
    /// The text as written (served page only: to edit it).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
}

/// The base of a review: a commit (its short id), or, for a directory, when
/// the snapshot was taken (RFC 3339, UTC).
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BaseData {
    Git { id: String },
    Files { at: String },
}

#[derive(Serialize)]
pub struct RevisionData {
    pub label: String,
    /// In the order to show them: the diff's files, then those that only
    /// threads bring in.
    pub files: Vec<FileData>,
    /// Where each thread is in this revision, by thread id.
    pub placements: BTreeMap<String, PlacementData>,
    /// The thread ids in the order of the thread list (review-wide threads
    /// first, then by file and line).
    pub order: Vec<String>,
}

#[derive(Serialize)]
pub struct FileData {
    pub path: String,
    /// The old path of a renamed file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// `modified`, `added`, `deleted`, `renamed`, `binary`, or `context` (not
    /// in the diff: shown for the threads that are on it).
    pub status: &'static str,
    /// What was done to a `binary` file (there are no lines to tell it by):
    /// `added`, `deleted`, `renamed` or `modified`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<&'static str>,
    /// What the file is in this revision (its two versions' digests): the page
    /// takes a file marked as looked at for a new one if this changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
    pub hunks: Vec<HunkData>,
    /// The lines the diff leaves out: one place before the first hunk, one
    /// between each two, one after the last (`null` where nothing is left out).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<Option<GapData>>,
}

/// A run of lines the diff doesn't show (they are the same on both sides).
#[derive(Serialize)]
pub struct GapData {
    /// How many lines.
    pub n: u32,
    /// The old and the new line number of the first of them.
    pub o: u32,
    pub w: u32,
    /// The lines themselves, as a row's pieces, when the page has them (the
    /// exported page carries some; the served page asks for them).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<Vec<Vec<Token>>>,
    /// Whether they can be had at all: the text of the file is known.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub x: bool,
}

/// How many of the lines a diff leaves out an exported page carries (so that
/// they can be shown without a server), over all its files and revisions. The
/// smaller places come first, so more of them can be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandLimit {
    Lines(usize),
    All,
}

impl Default for ExpandLimit {
    fn default() -> Self {
        ExpandLimit::Lines(5000)
    }
}

#[derive(Serialize, Clone)]
pub struct HunkData {
    pub header: String,
    pub rows: Vec<RowData>,
}

#[derive(Serialize, Clone)]
pub struct RowData {
    pub k: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub o: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// The text, as pieces with their kinds (see `tokens`).
    pub t: Vec<Token>,
    /// The words that changed, as `[start, end)` in UTF-16 units of the text
    /// (a removed line and the added line that replaces it; see `words`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub w: Vec<[u32; 2]>,
}

/// Where a thread is in one revision.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PlacementData {
    /// On lines: `start..=end` on `side` (`new` or `old`), and, on a `new`
    /// placement, the old-side lines it also covers (a replaced block).
    /// `color` numbers the thread's color (the page has the palette).
    Line {
        file: String,
        side: &'static str,
        start: u32,
        end: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        old_range: Option<[u32; 2]>,
        color: usize,
    },
    File {
        file: String,
    },
    Global,
    /// The lines the thread is about are not in this revision: it sits at the
    /// point before line `before` of the new side, with what they said (`was`).
    /// `absence` is `deleted` (removed since), `not-yet` (only in a later
    /// version) or `unknown`.
    Point {
        file: String,
        before: u32,
        absence: &'static str,
        was: Vec<String>,
    },
    /// The versions needed to place it are not held: listed with its file.
    Unplaced {
        file: String,
        was: Vec<String>,
    },
}

/// The model of a whole review, for a page that only shows it.
pub fn view_model(loaded: &crate::bundle::Loaded) -> anyhow::Result<ViewModel> {
    view_model_for(loaded, false)
}

/// The model of a whole review; `interactive` says the page may change it.
pub fn view_model_for(
    loaded: &crate::bundle::Loaded,
    interactive: bool,
) -> anyhow::Result<ViewModel> {
    // The served page asks for the lines a diff leaves out when it wants them.
    let limit = if interactive {
        ExpandLimit::Lines(0)
    } else {
        ExpandLimit::default()
    };
    view_model_with(loaded, interactive, limit)
}

/// The same, with a say in how many of the left-out lines it carries.
pub fn view_model_with(
    loaded: &crate::bundle::Loaded,
    interactive: bool,
    limit: ExpandLimit,
) -> anyhow::Result<ViewModel> {
    let shown = shown_revisions(loaded)?;
    let views = revision_views(&shown);
    let threads = build_threads(&loaded.events);
    let blobs = loaded.blobs();

    // The latest revision (the one first looked at) gets the lines first.
    let mut budget = match limit {
        ExpandLimit::Lines(n) => n,
        ExpandLimit::All => usize::MAX,
    };
    let mut revisions: Vec<RevisionData> = views
        .iter()
        .zip(&shown)
        .rev()
        .map(|(view, s)| revision_data(&threads, view, &s.label, &blobs, &mut budget))
        .collect();
    revisions.reverse();
    Ok(ViewModel {
        version: VERSION,
        stamp: stamp(loaded),
        editable: Vec::new(),
        author: None,
        refreshable: false,
        base: loaded.revisions().next().map(|r| match &r.source {
            crate::model::Source::Git(g) => BaseData::Git {
                id: g.base.chars().take(7).collect(),
            },
            crate::model::Source::Files { .. } => BaseData::Files {
                at: rfc3339(r.created_at),
            },
        }),
        ignore_whitespace: crate::review::ignore_whitespace(&loaded.events),
        interactive,
        title: crate::review::title(&loaded.events).map(str::to_string),
        threads: {
            let ids = comment_ids(&loaded.events);
            threads
                .iter()
                .map(|t| thread_data(t, &ids, interactive))
                .collect()
        },
        revisions,
    })
}

/// The model as JSON that is safe to put inside a `<script>` element: no `<`
/// (so no `</script>` or `<!--`), which a JSON string can spell `<`.
pub fn view_model_json(
    loaded: &crate::bundle::Loaded,
    limit: ExpandLimit,
) -> anyhow::Result<String> {
    model_json(&view_model_with(loaded, false, limit)?)
}

/// The same for the served page, which may change the review.
pub fn served_model_json(
    loaded: &crate::bundle::Loaded,
    editable: Vec<String>,
    author: String,
    refreshable: bool,
) -> anyhow::Result<String> {
    let mut model = view_model_for(loaded, true)?;
    model.editable = editable;
    model.author = Some(author);
    model.refreshable = refreshable;
    model_json(&model)
}

/// A stamp of the review's log: it differs whenever the log does (a comment
/// added, edited or deleted).
pub fn stamp(loaded: &crate::bundle::Loaded) -> String {
    crate::digest::digest(serde_json::to_vec(&loaded.events).unwrap_or_default())
}

/// The ids of each thread's replies, in order (a thread's replies have no id
/// of their own in [`Thread`]).
fn comment_ids(events: &[crate::model::Event]) -> BTreeMap<Ulid, Vec<Ulid>> {
    let mut ids: BTreeMap<Ulid, Vec<Ulid>> = BTreeMap::new();
    for event in events {
        if let crate::model::Event::Comment {
            id,
            parent: Some(parent),
            ..
        } = event
        {
            ids.entry(*parent).or_default().push(*id);
        }
    }
    ids
}

fn model_json(model: &ViewModel) -> anyhow::Result<String> {
    let json = serde_json::to_string(model)?;
    Ok(json.replace('<', "\\u003c"))
}

/// One thread as the page has it (after a reply, or a resolve).
pub fn thread_json(loaded: &crate::bundle::Loaded, id: Ulid) -> Option<ThreadData> {
    let ids = comment_ids(&loaded.events);
    build_threads(&loaded.events)
        .iter()
        .find(|t| t.root_id == id)
        .map(|t| thread_data(t, &ids, true))
}

fn thread_data(
    thread: &Thread,
    reply_ids: &BTreeMap<Ulid, Vec<Ulid>>,
    with_body: bool,
) -> ThreadData {
    let comment = |id: Ulid, author: &str, at: time::OffsetDateTime, body: &str| CommentData {
        id: id.to_string(),
        author: author.to_string(),
        at: rfc3339(at),
        doc: super::markdown::tree(body),
        body: if with_body {
            body.to_string()
        } else {
            String::new()
        },
    };
    let mut comments = vec![comment(
        thread.root_id,
        &thread.author,
        thread.created_at,
        &thread.body,
    )];
    let ids = reply_ids
        .get(&thread.root_id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    comments.extend(
        thread
            .replies
            .iter()
            .zip(ids)
            .map(|(r, id)| comment(*id, &r.author, r.created_at, &r.body)),
    );
    ThreadData {
        id: thread.root_id.to_string(),
        resolved: thread.resolved,
        comments,
    }
}

fn rfc3339(at: time::OffsetDateTime) -> String {
    use time::format_description::well_known::Rfc3339;
    at.to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn revision_data(
    threads: &[Thread],
    view: &RevisionView,
    label: &str,
    blobs: &crate::digest::Blobs,
    budget: &mut usize,
) -> RevisionData {
    let placed = place(threads, view, blobs);
    let in_diff: std::collections::HashSet<String> = view.diff.files.iter().map(file_key).collect();
    let syntax_set = &*SYNTAXES;

    let mut files: Vec<FileData> = placed
        .file_order
        .iter()
        .map(|key| {
            let file_diff = anchor::find_file(&placed.diff, key);
            let syntax = guess_syntax(key, syntax_set);
            let hunks = file_diff
                .map(|f| {
                    f.hunks
                        .iter()
                        .map(|hunk| hunk_data(hunk, syntax, syntax_set))
                        .collect()
                })
                .unwrap_or_default();
            let gaps = match file_diff {
                Some(f) if !f.is_binary => gaps_of(&f.hunks, new_text(view, blobs, key)),
                _ => Vec::new(),
            };
            FileData {
                path: key.clone(),
                old_path: file_diff
                    .filter(|f| f.is_rename)
                    .and_then(|f| f.old_path.clone()),
                status: file_status(file_diff, in_diff.contains(key)),
                change: file_diff
                    .filter(|f| f.is_binary && in_diff.contains(key))
                    .map(binary_change),
                sig: file_sig(view.files, key),
                hunks,
                gaps,
            }
        })
        .collect();
    carry_gap_lines(&mut files, view, blobs, syntax_set, budget);

    let mut placements = BTreeMap::new();
    for (thread, placement) in threads.iter().zip(&placed.placements) {
        let color = placed.marks.color_of.get(&thread.root_id).copied();
        placements.insert(
            thread.root_id.to_string(),
            placement_data(placement, color, placed.marks.was.get(&thread.root_id)),
        );
    }
    let order = ordered_threads(threads, &placed.file_order, &placed.marks)
        .iter()
        .map(|t| t.root_id.to_string())
        .collect();
    RevisionData {
        label: label.to_string(),
        files,
        placements,
        order,
    }
}

/// The digests of a file's two versions, as one string (`None` for a file that
/// is not in the diff).
fn file_sig(files: &[crate::model::FileDigest], path: &str) -> Option<String> {
    let old = anchor::digest_for(files, path, Side::Old);
    let new = anchor::digest_for(files, path, Side::New);
    if old.is_none() && new.is_none() {
        return None;
    }
    Some(format!("{}|{}", old.unwrap_or(""), new.unwrap_or("")))
}

/// What a change to a file did to it, whatever it is that is in it.
fn binary_change(f: &FileDiff) -> &'static str {
    if f.new_path.is_none() {
        "deleted"
    } else if f.old_path.is_none() {
        "added"
    } else if f.is_rename {
        "renamed"
    } else {
        "modified"
    }
}

pub(super) fn file_status(file: Option<&FileDiff>, in_diff: bool) -> &'static str {
    let Some(f) = file.filter(|_| in_diff) else {
        return "context";
    };
    if f.is_binary {
        "binary"
    } else {
        binary_change(f)
    }
}

/// The hunks that were worked out (coloring a file is by far the most of the
/// work of the model, and what is reviewed doesn't change: a comment added
/// leaves the code as it was), by what a hunk is made of.
static HUNKS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, HunkData>>> =
    std::sync::LazyLock::new(Default::default);

/// How many hunks are kept: past it they are let go of (and worked out again
/// when they are asked for).
const KEPT_HUNKS: usize = 20_000;

fn hunk_data(hunk: &Hunk, syntax: &SyntaxReference, syntax_set: &SyntaxSet) -> HunkData {
    // Everything the result depends on: the syntax, and the hunk's header and lines.
    let mut key = Vec::new();
    key.extend_from_slice(syntax.name.as_bytes());
    key.push(0);
    key.extend_from_slice(
        format!(
            "{} {} {} {} {:?}",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines, hunk.section_heading
        )
        .as_bytes(),
    );
    for line in &hunk.lines {
        key.push(0);
        key.extend_from_slice(
            format!("{:?} {:?} {:?} ", line.kind, line.old_line, line.new_line).as_bytes(),
        );
        key.extend_from_slice(line.content.as_bytes());
    }
    let key = crate::digest::digest(&key);
    if let Some(done) = HUNKS.lock().ok().and_then(|kept| kept.get(&key).cloned()) {
        return done;
    }
    let done = make_hunk_data(hunk, syntax, syntax_set);
    if let Ok(mut kept) = HUNKS.lock() {
        if kept.len() >= KEPT_HUNKS {
            kept.clear();
        }
        kept.insert(key, done.clone());
    }
    done
}

fn make_hunk_data(hunk: &Hunk, syntax: &SyntaxReference, syntax_set: &SyntaxSet) -> HunkData {
    // Each hunk is read from its start.
    let mut tokenizer = Tokenizer::new(syntax, syntax_set);
    let mut rows: Vec<RowData> = hunk
        .lines
        .iter()
        .map(|line| RowData {
            k: match line.kind {
                LineKind::Context => "c",
                LineKind::Added => "a",
                LineKind::Removed => "d",
            },
            o: line.old_line,
            n: line.new_line,
            t: tokenizer.line(&line.content),
            w: Vec::new(),
        })
        .collect();
    mark_words(hunk, &mut rows);
    HunkData {
        header: format!(
            "@@ -{},{} +{},{} @@ {}",
            hunk.old_start,
            hunk.old_lines,
            hunk.new_start,
            hunk.new_lines,
            hunk.section_heading.as_deref().unwrap_or("")
        ),
        rows,
    }
}

/// Marks the words that changed in each removed line and the added line that
/// takes its place: a run of removed lines is paired, in order, with the run of
/// added lines after it (as the side by side layout puts them next to each
/// other).
fn mark_words(hunk: &Hunk, rows: &mut [RowData]) {
    let lines = &hunk.lines;
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != LineKind::Removed {
            i += 1;
            continue;
        }
        let removed_start = i;
        while i < lines.len() && lines[i].kind == LineKind::Removed {
            i += 1;
        }
        let added_start = i;
        while i < lines.len() && lines[i].kind == LineKind::Added {
            i += 1;
        }
        for k in 0..(added_start - removed_start).min(i - added_start) {
            let (r, a) = (removed_start + k, added_start + k);
            if let Some((old, new)) = super::words::changed(&lines[r].content, &lines[a].content) {
                rows[r].w = old;
                rows[a].w = new;
            }
        }
    }
}

/// The text of a file as this revision has it (its new side), if the bundle
/// holds it.
fn new_text<'a>(
    view: &RevisionView,
    blobs: &crate::digest::Blobs<'a>,
    path: &str,
) -> Option<&'a str> {
    let digest = view
        .files
        .iter()
        .find(|f| f.new_path.as_deref() == Some(path))?
        .new
        .as_deref()?;
    blobs.text(digest)
}

/// The number of a hunk's first and last line on a side, and of the line
/// after it: a hunk with no lines on a side (`,0`) sits after the line its
/// number names.
fn after(start: u32, len: u32) -> u32 {
    if len == 0 { start + 1 } else { start + len }
}

fn first_of(start: u32, len: u32) -> u32 {
    if len == 0 { start + 1 } else { start }
}

/// The places the hunks leave out: before the first, between, after the last
/// (which needs the text, to know where the file ends).
fn gaps_of(hunks: &[Hunk], text: Option<&str>) -> Vec<Option<GapData>> {
    if hunks.is_empty() {
        return Vec::new();
    }
    let total = text.map(|t| t.lines().count() as u32);
    let gap = |old: u32, new: u32, n: u32| {
        (n > 0).then_some(GapData {
            n,
            o: old,
            w: new,
            t: None,
            x: text.is_some(),
        })
    };
    let mut out = Vec::with_capacity(hunks.len() + 1);
    let first = &hunks[0];
    out.push(gap(1, 1, first_of(first.new_start, first.new_lines) - 1));
    for pair in hunks.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let (old, new) = (
            after(a.old_start, a.old_lines),
            after(a.new_start, a.new_lines),
        );
        let n = first_of(b.new_start, b.new_lines).saturating_sub(new);
        out.push(gap(old, new, n));
    }
    let last = &hunks[hunks.len() - 1];
    let (old, new) = (
        after(last.old_start, last.old_lines),
        after(last.new_start, last.new_lines),
    );
    out.push(total.and_then(|t| gap(old, new, (t + 1).saturating_sub(new))));
    if out.iter().all(Option::is_none) {
        return Vec::new();
    }
    out
}

/// Puts the lines of the smallest places into the files, while the budget
/// lasts.
fn carry_gap_lines(
    files: &mut [FileData],
    view: &RevisionView,
    blobs: &crate::digest::Blobs,
    syntax_set: &SyntaxSet,
    budget: &mut usize,
) {
    if *budget == 0 {
        return;
    }
    let mut places: Vec<(u32, usize, usize)> = Vec::new();
    for (f, file) in files.iter().enumerate() {
        for (g, gap) in file.gaps.iter().enumerate() {
            if let Some(gap) = gap.as_ref().filter(|g| g.x) {
                places.push((gap.n, f, g));
            }
        }
    }
    places.sort();
    for (n, f, g) in places {
        if n as usize > *budget {
            break;
        }
        let path = files[f].path.clone();
        let Some(text) = new_text(view, blobs, &path) else {
            continue;
        };
        let Some(gap) = files[f].gaps[g].as_mut() else {
            continue;
        };
        let lines: Vec<&str> = text
            .lines()
            .skip(gap.w as usize - 1)
            .take(n as usize)
            .collect();
        let mut tokenizer = Tokenizer::new(guess_syntax(&path, syntax_set), syntax_set);
        gap.t = Some(lines.iter().map(|l| tokenizer.line(l)).collect());
        *budget = budget.saturating_sub(n as usize);
    }
}

/// `count` lines of the file from line `from` (1-based) as the pieces of each,
/// for the served page to show what a diff leaves out.
pub fn lines_json(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    path: &str,
    from: usize,
    count: usize,
    git: Option<&dyn CommitFiles>,
) -> Result<Vec<Vec<Token>>, String> {
    let text = stored_text(loaded, revision, path, git)?;
    let syntax_set = &*SYNTAXES;
    let mut tokenizer = Tokenizer::new(guess_syntax(path, syntax_set), syntax_set);
    Ok(text
        .lines()
        .skip(from.max(1) - 1)
        .take(count.min(2000))
        .map(|l| tokenizer.line(l))
        .collect())
}

fn placement_data(
    placement: &Placement,
    color: Option<usize>,
    was: Option<&Vec<String>>,
) -> PlacementData {
    match placement {
        Placement::Global => PlacementData::Global,
        Placement::File(file) => PlacementData::File { file: file.clone() },
        Placement::Line {
            file,
            side,
            line_start,
            line_end,
            old_range,
        } => PlacementData::Line {
            file: file.clone(),
            side: if *side == Side::Old { "old" } else { "new" },
            start: *line_start,
            end: *line_end,
            old_range: old_range.map(|(a, b)| [a, b]),
            color: color.unwrap_or(0),
        },
        Placement::Point {
            file,
            before,
            was,
            kind,
        } => PlacementData::Point {
            file: file.clone(),
            before: *before,
            absence: match kind {
                anchor::Absence::Deleted => "deleted",
                anchor::Absence::NotYet => "not-yet",
                anchor::Absence::Unknown => "unknown",
            },
            was: was.clone(),
        },
        Placement::Unplaced { file } => PlacementData::Unplaced {
            file: file.clone(),
            was: was.cloned().unwrap_or_default(),
        },
    }
}

// ---- other files, for the client page ----------------------------------------

/// The list of other files as JSON: `entries` (`{dir, path, name, count}` or
/// `{file, path, name}`), `message`, `note`, `more`.
pub fn tree_json(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    dir: &str,
    query: &str,
    git: Option<&dyn CommitFiles>,
) -> Option<serde_json::Value> {
    let data = tree_data(loaded, revision, dir, query, git)?;
    let entries: Vec<serde_json::Value> = data
        .items
        .iter()
        .map(|item| match item {
            TreeItem::Dir { path, name, count } => {
                serde_json::json!({ "kind": "dir", "path": path, "name": name, "count": count })
            }
            TreeItem::File { path, label } => {
                serde_json::json!({ "kind": "file", "path": path, "name": label })
            }
        })
        .collect();
    Some(serde_json::json!({
        "entries": entries,
        "message": data.message,
        "note": data.note,
        "more": data.more,
    }))
}

/// A file opened to look at, as the client page draws one: its first lines
/// as a hunk of unchanged lines, how many lines it has, and where the next
/// lines start (`null` at the end).
#[derive(Serialize)]
pub struct OpenedData {
    pub path: String,
    pub total: usize,
    pub hunks: Vec<HunkData>,
    pub next: Option<usize>,
}

fn context_hunk_data(path: &str, text: &str, from: usize) -> (HunkData, Option<usize>) {
    let (hunk, next) = context_chunk(text, from);
    let syntax_set = &*SYNTAXES;
    let syntax = guess_syntax(path, syntax_set);
    (hunk_data(&hunk, syntax, syntax_set), next)
}

pub fn opened_data(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    path: &str,
    git: Option<&dyn CommitFiles>,
) -> Result<OpenedData, String> {
    let text = stored_text(loaded, revision, path, git)?;
    let (hunk, next) = context_hunk_data(path, &text, 1);
    Ok(OpenedData {
        path: path.to_string(),
        total: text.lines().count(),
        hunks: vec![hunk],
        next,
    })
}

/// The next lines of an opened file, from line `from`.
pub fn chunk_data(
    loaded: &crate::bundle::Loaded,
    revision: usize,
    path: &str,
    from: usize,
    git: Option<&dyn CommitFiles>,
) -> Result<(HunkData, Option<usize>), String> {
    let text = stored_text(loaded, revision, path, git)?;
    Ok(context_hunk_data(path, &text, from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::DiffLine;

    fn line(kind: LineKind, content: &str, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            content: content.into(),
            old_line: old,
            new_line: new,
            no_newline_at_eof: false,
        }
    }

    fn hunk(lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 0,
            section_heading: None,
            lines,
        }
    }

    fn data(h: &Hunk) -> HunkData {
        let syntax_set = &*SYNTAXES;
        hunk_data(h, guess_syntax("a.txt", syntax_set), syntax_set)
    }

    #[test]
    fn a_removed_line_and_the_added_line_after_it_say_which_words_changed() {
        let h = hunk(vec![
            line(LineKind::Context, "same", Some(1), Some(1)),
            line(LineKind::Removed, "type=sha,format=long", Some(2), None),
            line(LineKind::Added, "type=sha,format=short", None, Some(2)),
            line(LineKind::Context, "same", Some(3), Some(3)),
        ]);
        let d = data(&h);
        assert!(d.rows[0].w.is_empty() && d.rows[3].w.is_empty());
        assert_eq!(d.rows[1].w, vec![[16, 20]]);
        assert_eq!(d.rows[2].w, vec![[16, 21]]);
    }

    #[test]
    fn runs_are_paired_in_order_and_an_unpaired_line_has_no_words() {
        let h = hunk(vec![
            line(LineKind::Removed, "call(alpha, one)", Some(1), None),
            line(LineKind::Removed, "call(beta, two)", Some(2), None),
            line(LineKind::Removed, "gone entirely", Some(3), None),
            line(LineKind::Added, "call(alpha, uno)", None, Some(1)),
            line(LineKind::Added, "call(beta, dos)", None, Some(2)),
        ]);
        let d = data(&h);
        assert_eq!(d.rows[0].w, vec![[12, 15]]);
        assert_eq!(d.rows[3].w, vec![[12, 15]]);
        assert_eq!(d.rows[1].w, vec![[11, 14]]);
        assert!(d.rows[2].w.is_empty(), "no line to compare with");
        // Lines that are only added, or only removed, have nothing to compare.
        let h = hunk(vec![line(LineKind::Added, "new", None, Some(1))]);
        assert!(data(&h).rows[0].w.is_empty());
    }

    #[test]
    fn a_binary_file_says_whether_it_was_added_deleted_renamed_or_changed() {
        let diff = crate::diff::parse(
            "diff --git a/new.bin b/new.bin\nBinary files /dev/null and b/new.bin differ\n\
             diff --git a/gone.bin b/gone.bin\nBinary files a/gone.bin and /dev/null differ\n\
             diff --git a/same.bin b/same.bin\nBinary files a/same.bin and b/same.bin differ\n",
        )
        .unwrap();
        let changes: Vec<_> = diff.files.iter().map(binary_change).collect();
        assert_eq!(changes, ["added", "deleted", "modified"]);
        assert!(
            diff.files
                .iter()
                .all(|f| file_status(Some(f), true) == "binary")
        );
    }

    #[test]
    fn a_hunk_that_was_worked_out_is_kept_and_only_the_same_hunk_gets_it_back() {
        let json = |h: &HunkData| serde_json::to_string(h).unwrap();
        let syntax_set = &*SYNTAXES;
        let fresh =
            |h: &Hunk, name: &str| make_hunk_data(h, guess_syntax(name, syntax_set), syntax_set);
        let colored =
            |h: &Hunk, name: &str| hunk_data(h, guess_syntax(name, syntax_set), syntax_set);
        let a = hunk(vec![line(
            LineKind::Added,
            "let kept = 1; // one",
            None,
            Some(1),
        )]);
        for _ in 0..2 {
            assert_eq!(json(&colored(&a, "k.rs")), json(&fresh(&a, "k.rs")));
        }
        // Another text, another syntax or another position is another hunk.
        let b = hunk(vec![line(
            LineKind::Added,
            "let kept = 2; // one",
            None,
            Some(1),
        )]);
        assert_eq!(json(&colored(&b, "k.rs")), json(&fresh(&b, "k.rs")));
        assert_ne!(json(&colored(&a, "k.rs")), json(&colored(&b, "k.rs")));
        assert_eq!(json(&colored(&a, "k.txt")), json(&fresh(&a, "k.txt")));
        assert_ne!(json(&colored(&a, "k.rs")), json(&colored(&a, "k.txt")));
        let mut moved = a.clone();
        moved.lines[0].new_line = Some(7);
        assert_eq!(json(&colored(&moved, "k.rs")), json(&fresh(&moved, "k.rs")));
    }
}
