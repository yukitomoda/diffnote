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
//! and new side (absent where the line isn't there), `h` the text, colored, as
//! HTML.

use super::*;
use serde::Serialize;
use std::collections::BTreeMap;

/// The version of this format, for the page to check.
pub const VERSION: u32 = 1;

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
    /// The text, as HTML.
    pub html: String,
    /// The text as written (served page only: to edit it).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
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
    pub hunks: Vec<HunkData>,
}

#[derive(Serialize)]
pub struct HunkData {
    pub header: String,
    pub rows: Vec<RowData>,
}

#[derive(Serialize)]
pub struct RowData {
    pub k: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub o: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    pub h: String,
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
    let shown = shown_revisions(loaded)?;
    let views = revision_views(&shown);
    let threads = build_threads(&loaded.events);
    let blobs = loaded.blobs();

    let revisions = views
        .iter()
        .zip(&shown)
        .map(|(view, s)| revision_data(&threads, view, &s.label, &blobs))
        .collect();
    Ok(ViewModel {
        version: VERSION,
        stamp: stamp(loaded),
        editable: Vec::new(),
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
pub fn view_model_json(loaded: &crate::bundle::Loaded) -> anyhow::Result<String> {
    model_json(&view_model(loaded)?)
}

/// The same for the served page, which may change the review.
pub fn served_model_json(
    loaded: &crate::bundle::Loaded,
    editable: Vec<String>,
) -> anyhow::Result<String> {
    let mut model = view_model_for(loaded, true)?;
    model.editable = editable;
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
        html: markdown_to_html(body),
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
) -> RevisionData {
    let placed = place(threads, view, blobs);
    let in_diff: std::collections::HashSet<String> = view.diff.files.iter().map(file_key).collect();
    let syntax_set = &*SYNTAXES;
    let theme = &THEMES.themes["InspiredGitHub"];

    let files = placed
        .file_order
        .iter()
        .map(|key| {
            let file_diff = anchor::find_file(&placed.diff, key);
            let syntax = guess_syntax(key, syntax_set);
            let hunks = file_diff
                .map(|f| {
                    f.hunks
                        .iter()
                        .map(|hunk| hunk_data(hunk, syntax, syntax_set, theme))
                        .collect()
                })
                .unwrap_or_default();
            FileData {
                path: key.clone(),
                old_path: file_diff
                    .filter(|f| f.is_rename)
                    .and_then(|f| f.old_path.clone()),
                status: file_status(file_diff, in_diff.contains(key)),
                hunks,
            }
        })
        .collect();

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

pub(super) fn file_status(file: Option<&FileDiff>, in_diff: bool) -> &'static str {
    let Some(f) = file.filter(|_| in_diff) else {
        return "context";
    };
    if f.is_binary {
        "binary"
    } else if f.new_path.is_none() {
        "deleted"
    } else if f.old_path.is_none() {
        "added"
    } else if f.is_rename {
        "renamed"
    } else {
        "modified"
    }
}

fn hunk_data(
    hunk: &Hunk,
    syntax: &SyntaxReference,
    syntax_set: &SyntaxSet,
    theme: &Theme,
) -> HunkData {
    // Each hunk is colored from its start, as the page always has.
    let mut highlighter = HighlightLines::new(syntax, theme);
    let rows = hunk
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
            h: highlight_line(&mut highlighter, &line.content, syntax_set),
        })
        .collect();
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
    let theme = &THEMES.themes["InspiredGitHub"];
    let syntax = guess_syntax(path, syntax_set);
    (hunk_data(&hunk, syntax, syntax_set, theme), next)
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
