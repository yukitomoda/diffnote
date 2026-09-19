//! The `$EDITOR` annotation format: a temp file that starts from raw unified
//! diff text (see the `diff` module) and lets a reviewer interleave
//! `>`-prefixed comment lines by hand.
//!
//! Sigil grammar (column 0 of the line):
//!
//! - `>`        new comment, target = a brand-new thread at this position
//! - `>>`       new comment, target = reply to the nearest preceding thread
//! - `>#@<ulid> <author> <timestamp>`  header line for a rendering of an
//!   existing/persisted comment or reply (read-only, produced by `render`
//!   below); sets "nearest preceding thread" to that existing thread by id,
//!   so a following `>>`/`>>!<dir>` targets it same as a same-session `>`
//!   thread would. Always carries the *thread root's* id, even for a
//!   rendered reply, since threading is flat.
//! - `>#<text>` decorative body line for a rendered comment/reply above —
//!   purely for human reading, ignored on parse (never affects `last_thread`
//!   beyond what the `>#@` header already set).
//! - `>!<dir>`  directive on a brand-new thread (pairs with `>`)
//! - `>>!<dir>` directive on the nearest preceding thread (pairs with `>>`)
//! - `>[<id>` / `>]<id>`  opens/closes a (possibly cross-hunk, possibly
//!   overlapping) range comment; bare `>[`/`>]` is the anonymous shorthand
//! - `\`        general escape for the next character, so any sigil above
//!   can be written as literal text
//!
//! A comment's anchor (whole-diff / file / hunk / single-line / range) is
//! inferred purely from where it is placed: `current_scope` below always
//! reflects "whatever diff-structural thing was most recently passed",
//! which is exactly the anchor a following comment block picks up.
//!
//! Range side selection: a range's `line_start`/`line_end` are tracked on
//! the new side if anything inside it advanced the new side
//! (context/added lines), falling back to the old side only when the
//! whole range is removed lines with nothing else in it. A range that
//! advances *both* sides (removed lines followed by context/added ones)
//! keeps its old-side span too, in `old_range` -- purely for display, not
//! authoritative (re-anchoring only ever searches the new side).

use crate::anchor::{self, Placement};
use crate::diff::{
    self, DiffLine, FileDiff, Hunk, LineKind, ParseError as DiffParseError, UnifiedDiff,
};
use crate::model::{Anchor, Side};
use crate::review::Thread;
use std::collections::HashMap;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use ulid::Ulid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadId(pub usize);

/// What a `>>`/`>>!<dir>` block targets: a thread created earlier in this
/// same editing session, or an existing thread rendered via `>#@`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRef {
    New(ThreadId),
    Existing(Ulid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorScope {
    Global,
    File {
        file: String,
    },
    Hunk {
        file: String,
        hunk_index: usize,
    },
    Line {
        file: String,
        side: Side,
        line: u32,
    },
    Range {
        file: String,
        side: Side,
        line_start: u32,
        line_end: u32,
        /// The old-side sub-range this range also covered, if it wrapped
        /// both removed and new-side (context/added) lines -- see the
        /// matching field on `model::Anchor::Span`.
        old_range: Option<(u32, u32)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `>` — a brand-new thread anchored at this position.
    New,
    /// `>>` — the nearest preceding thread context.
    Reply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Directive {
    Resolve,
    Reopen,
    /// `reanchor <thread-id>` always targets an explicit thread by id,
    /// regardless of `Target`.
    Reanchor(String),
    /// `reject` — only meaningful with `Target::Reply`.
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    NewThread {
        id: ThreadId,
        scope: AnchorScope,
        body: Option<String>,
        directives: Vec<Directive>,
    },
    Reply {
        target: ThreadRef,
        body: Option<String>,
        directives: Vec<Directive>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub diff: UnifiedDiff,
    pub items: Vec<Item>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnnotationError {
    #[error("line {line}: {message}")]
    Malformed { line: usize, message: String },
}

fn err(line: usize, message: impl Into<String>) -> AnnotationError {
    AnnotationError::Malformed {
        line,
        message: message.into(),
    }
}

fn from_diff_err(e: DiffParseError) -> AnnotationError {
    err(e.line(), e.message().to_string())
}

/// `\X` anywhere in comment text means literal `X` (so `\\` is a literal
/// backslash, and a trailing lone `\` is kept as-is). Applied only to
/// assembled comment/reply bodies, never to directive names/arguments.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(next) => out.push(next),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn strip_one_leading_space(s: &str) -> &str {
    s.strip_prefix(' ').unwrap_or(s)
}

fn file_label(file: &FileDiff) -> String {
    file.new_path
        .clone()
        .or_else(|| file.old_path.clone())
        .unwrap_or_else(|| "?".to_string())
}

#[derive(Debug, PartialEq, Eq)]
enum Sigil<'a> {
    Comment(Target, &'a str),
    Directive(Target, &'a str),
    RangeOpen(&'a str),
    RangeClose(&'a str),
    RenderedHeader(&'a str),
    RenderedBody(&'a str),
}

/// `rest_after_gt` is everything after the line's leading `>`. Recognition
/// happens on this raw text *before* unescaping, so `\[`/`\]`/`\>`/`\!`/`\#`
/// never match a sigil and fall through to plain comment text instead.
fn classify_comment(rest_after_gt: &str) -> Sigil<'_> {
    if let Some(id) = rest_after_gt.strip_prefix('[') {
        return Sigil::RangeOpen(id);
    }
    if let Some(id) = rest_after_gt.strip_prefix(']') {
        return Sigil::RangeClose(id);
    }
    if let Some(rest) = rest_after_gt.strip_prefix('#') {
        return match rest.strip_prefix('@') {
            Some(header) => Sigil::RenderedHeader(header),
            None => Sigil::RenderedBody(rest),
        };
    }
    if let Some(rest2) = rest_after_gt.strip_prefix('>') {
        if let Some(rest3) = rest2.strip_prefix('!') {
            return Sigil::Directive(Target::Reply, rest3);
        }
        return Sigil::Comment(Target::Reply, rest2);
    }
    if let Some(rest) = rest_after_gt.strip_prefix('!') {
        return Sigil::Directive(Target::New, rest);
    }
    Sigil::Comment(Target::New, rest_after_gt)
}

fn validate_range_id(id: &str, line_no: usize) -> Result<(), AnnotationError> {
    if id.is_empty()
        || id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        Err(err(line_no, format!("invalid range id {id:?}")))
    }
}

fn parse_directive(
    target: Target,
    rest: &str,
    line_no: usize,
) -> Result<Directive, AnnotationError> {
    let rest = rest.trim();
    let (name, arg) = match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n, Some(a.trim())),
        None => (rest, None),
    };
    match (name, target, arg) {
        ("resolve", _, None) => Ok(Directive::Resolve),
        ("reopen", _, None) => Ok(Directive::Reopen),
        ("reanchor", Target::New, Some(id)) if !id.is_empty() => {
            Ok(Directive::Reanchor(id.to_string()))
        }
        ("reanchor", Target::New, None) => {
            Err(err(line_no, "'reanchor' requires a thread id argument"))
        }
        ("reanchor", Target::Reply, _) => Err(err(
            line_no,
            "'reanchor' must be written as '>!reanchor <id>', not '>>!reanchor'",
        )),
        ("reject", Target::Reply, None) => Ok(Directive::Reject),
        ("reject", Target::New, _) => Err(err(
            line_no,
            "'reject' must be written as '>>!reject', not '>!reject'",
        )),
        _ => Err(err(
            line_no,
            format!("unknown or malformed directive {rest:?}"),
        )),
    }
}

struct PendingBlock {
    target: Target,
    scope: AnchorScope,
    lines: Vec<String>,
    directives: Vec<Directive>,
    start_line: usize,
}

struct RangeStart {
    file: String,
    start_old_line: u32,
    start_new_line: u32,
}

fn flush_pending(
    pending: &mut Option<PendingBlock>,
    items: &mut Vec<Item>,
    next_thread_id: &mut usize,
    last_thread: &mut Option<ThreadRef>,
) -> Result<(), AnnotationError> {
    let Some(p) = pending.take() else {
        return Ok(());
    };
    let body = if p.lines.is_empty() {
        None
    } else {
        Some(unescape(&p.lines.join("\n")))
    };
    match p.target {
        Target::New => {
            let id = ThreadId(*next_thread_id);
            *next_thread_id += 1;
            items.push(Item::NewThread {
                id,
                scope: p.scope,
                body,
                directives: p.directives,
            });
            *last_thread = Some(ThreadRef::New(id));
        }
        Target::Reply => {
            let target = last_thread.ok_or_else(|| {
                err(
                    p.start_line,
                    "'>>' reply with no preceding thread (new or rendered) to reply to",
                )
            })?;
            items.push(Item::Reply {
                target,
                body,
                directives: p.directives,
            });
        }
    }
    Ok(())
}

pub fn parse(text: &str) -> Result<Parsed, AnnotationError> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current_file: Option<FileDiff> = None;
    let mut current_hunk: Option<Hunk> = None;
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;

    let mut current_scope = AnchorScope::Global;
    let mut open_ranges: HashMap<String, RangeStart> = HashMap::new();
    let mut next_thread_id: usize = 0;
    let mut last_thread: Option<ThreadRef> = None;
    let mut items: Vec<Item> = Vec::new();
    let mut pending: Option<PendingBlock> = None;
    let mut last_line_no: usize = 0;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        last_line_no = line_no;

        if raw_line.is_empty() {
            flush_pending(
                &mut pending,
                &mut items,
                &mut next_thread_id,
                &mut last_thread,
            )?;
            continue;
        }

        if let Some(rest_after_gt) = raw_line.strip_prefix('>') {
            match classify_comment(rest_after_gt) {
                Sigil::RangeOpen(id) => {
                    flush_pending(
                        &mut pending,
                        &mut items,
                        &mut next_thread_id,
                        &mut last_thread,
                    )?;
                    validate_range_id(id, line_no)?;
                    if open_ranges.contains_key(id) {
                        return Err(err(line_no, format!("range '{id}' is already open")));
                    }
                    let file = current_file
                        .as_ref()
                        .map(file_label)
                        .ok_or_else(|| err(line_no, "range marker outside of a file"))?;
                    open_ranges.insert(
                        id.to_string(),
                        RangeStart {
                            file,
                            start_old_line: old_no,
                            start_new_line: new_no,
                        },
                    );
                }
                Sigil::RangeClose(id) => {
                    flush_pending(
                        &mut pending,
                        &mut items,
                        &mut next_thread_id,
                        &mut last_thread,
                    )?;
                    let start = open_ranges
                        .remove(id)
                        .ok_or_else(|| err(line_no, format!("range '{id}' was never opened")))?;
                    // Prefer the new side (matches every other scope's
                    // preference), but a range spanning only removed lines
                    // never advances `new_no` -- fall back to the old side
                    // rather than erroring, since the range isn't actually
                    // empty, just old-side-only. A range that advances
                    // *both* (removed lines followed by context/added ones)
                    // keeps its old-side span too, so it isn't silently
                    // dropped from rendering/highlighting.
                    let new_advanced = new_no > start.start_new_line;
                    let old_advanced = old_no > start.start_old_line;
                    current_scope = if new_advanced {
                        AnchorScope::Range {
                            file: start.file,
                            side: Side::New,
                            line_start: start.start_new_line,
                            line_end: new_no - 1,
                            old_range: old_advanced.then(|| (start.start_old_line, old_no - 1)),
                        }
                    } else if old_advanced {
                        AnchorScope::Range {
                            file: start.file,
                            side: Side::Old,
                            line_start: start.start_old_line,
                            line_end: old_no - 1,
                            old_range: None,
                        }
                    } else {
                        return Err(err(line_no, format!("range '{id}' is empty")));
                    };
                }
                Sigil::RenderedHeader(header) => {
                    flush_pending(
                        &mut pending,
                        &mut items,
                        &mut next_thread_id,
                        &mut last_thread,
                    )?;
                    let id_token = header.split_whitespace().next().unwrap_or("");
                    let ulid = Ulid::from_string(id_token).map_err(|_| {
                        err(
                            line_no,
                            format!("'>#@' header has an invalid thread id {id_token:?}"),
                        )
                    })?;
                    last_thread = Some(ThreadRef::Existing(ulid));
                }
                Sigil::RenderedBody(_) => {
                    // Purely decorative; doesn't touch `last_thread` beyond
                    // what the block's `>#@` header already set.
                    flush_pending(
                        &mut pending,
                        &mut items,
                        &mut next_thread_id,
                        &mut last_thread,
                    )?;
                }
                Sigil::Directive(target, rest) => {
                    let directive = parse_directive(target, rest, line_no)?;
                    match &mut pending {
                        Some(p) if p.target == target => {
                            p.directives.push(directive);
                        }
                        _ => {
                            flush_pending(
                                &mut pending,
                                &mut items,
                                &mut next_thread_id,
                                &mut last_thread,
                            )?;
                            pending = Some(PendingBlock {
                                target,
                                scope: current_scope.clone(),
                                lines: Vec::new(),
                                directives: vec![directive],
                                start_line: line_no,
                            });
                        }
                    }
                }
                Sigil::Comment(target, text) => {
                    let text = strip_one_leading_space(text).to_string();
                    match &mut pending {
                        Some(p) if p.target == target => {
                            p.lines.push(text);
                        }
                        _ => {
                            flush_pending(
                                &mut pending,
                                &mut items,
                                &mut next_thread_id,
                                &mut last_thread,
                            )?;
                            pending = Some(PendingBlock {
                                target,
                                scope: current_scope.clone(),
                                lines: vec![text],
                                directives: Vec::new(),
                                start_line: line_no,
                            });
                        }
                    }
                }
            }
            continue;
        }

        // Everything below is a diff-structural line, which always ends
        // any pending comment block.
        flush_pending(
            &mut pending,
            &mut items,
            &mut next_thread_id,
            &mut last_thread,
        )?;

        if raw_line.strip_prefix("diff --git ").is_some() {
            diff::finish_file(&mut files, &mut current_file, &mut current_hunk);
            current_file = Some(FileDiff::default());
            continue;
        }

        if raw_line.starts_with("index ")
            || raw_line.starts_with("old mode ")
            || raw_line.starts_with("new mode ")
            || raw_line.starts_with("deleted file mode ")
            || raw_line.starts_with("new file mode ")
            || raw_line.starts_with("similarity index ")
        {
            continue;
        }

        if let Some(rest) = raw_line.strip_prefix("rename from ") {
            let file = current_file.get_or_insert_with(FileDiff::default);
            file.is_rename = true;
            file.old_path = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = raw_line.strip_prefix("rename to ") {
            let file = current_file.get_or_insert_with(FileDiff::default);
            file.is_rename = true;
            file.new_path = Some(rest.trim().to_string());
            continue;
        }

        if let Some((old_path, new_path)) = diff::parse_binary_line(raw_line) {
            let file_already_has_content = current_hunk.is_some()
                || current_file
                    .as_ref()
                    .is_some_and(|f| !f.hunks.is_empty() || f.is_binary);
            if file_already_has_content {
                diff::finish_file(&mut files, &mut current_file, &mut current_hunk);
            }
            let file = current_file.get_or_insert_with(FileDiff::default);
            file.is_binary = true;
            file.old_path = old_path;
            file.new_path = new_path;
            // Nothing follows a binary file's header to hang a comment on
            // but the file itself, so comments here are file-level.
            current_scope = AnchorScope::File {
                file: file_label(file),
            };
            continue;
        }

        if let Some(rest) = raw_line.strip_prefix("--- ") {
            // See the matching comment in diff::parse: a hunk still being
            // accumulated (not yet flushed into `.hunks`) must count too,
            // or two files' hunks can get silently merged into one.
            let file_already_has_content = current_hunk.is_some()
                || current_file
                    .as_ref()
                    .is_some_and(|f| !f.hunks.is_empty() || f.is_binary);
            if current_file.is_none() || file_already_has_content {
                diff::finish_file(&mut files, &mut current_file, &mut current_hunk);
                current_file = Some(FileDiff::default());
            }
            let file = current_file.as_mut().expect("just ensured");
            file.old_path = diff::parse_path(rest);
            continue;
        }

        if let Some(rest) = raw_line.strip_prefix("+++ ") {
            let file = current_file
                .as_mut()
                .ok_or_else(|| err(line_no, "'+++' line outside of a file header"))?;
            file.new_path = diff::parse_path(rest);
            current_scope = AnchorScope::File {
                file: file_label(file),
            };
            continue;
        }

        if raw_line.starts_with("@@ ") || raw_line == "@@" {
            diff::finish_hunk(&mut current_file, &mut current_hunk);
            let file = current_file
                .as_ref()
                .ok_or_else(|| err(line_no, "hunk header outside of a file"))?;
            let hunk_index = file.hunks.len();
            let file_lbl = file_label(file);
            let (old_start, old_lines, new_start, new_lines, section_heading) =
                diff::parse_hunk_header(raw_line, line_no).map_err(from_diff_err)?;
            old_no = old_start;
            new_no = new_start;
            current_hunk = Some(Hunk {
                old_start,
                old_lines,
                new_start,
                new_lines,
                section_heading,
                lines: Vec::new(),
            });
            current_scope = AnchorScope::Hunk {
                file: file_lbl,
                hunk_index,
            };
            continue;
        }

        if raw_line.strip_prefix('\\').is_some() {
            let hunk = current_hunk
                .as_mut()
                .ok_or_else(|| err(line_no, "'\\' marker outside of a hunk"))?;
            if let Some(last) = hunk.lines.last_mut() {
                last.no_newline_at_eof = true;
            }
            continue;
        }

        let mut chars = raw_line.chars();
        let prefix_char = chars.next().expect("checked not empty above");
        let content = chars.as_str();
        let file_lbl = current_file
            .as_ref()
            .map(file_label)
            .ok_or_else(|| err(line_no, "diff content outside of a file"))?;
        let hunk = current_hunk
            .as_mut()
            .ok_or_else(|| err(line_no, "diff content outside of a hunk"))?;
        let line = match prefix_char {
            ' ' => {
                let l = DiffLine {
                    kind: LineKind::Context,
                    content: content.to_string(),
                    old_line: Some(old_no),
                    new_line: Some(new_no),
                    no_newline_at_eof: false,
                };
                current_scope = AnchorScope::Line {
                    file: file_lbl,
                    side: Side::New,
                    line: new_no,
                };
                old_no += 1;
                new_no += 1;
                l
            }
            '+' => {
                let l = DiffLine {
                    kind: LineKind::Added,
                    content: content.to_string(),
                    old_line: None,
                    new_line: Some(new_no),
                    no_newline_at_eof: false,
                };
                current_scope = AnchorScope::Line {
                    file: file_lbl,
                    side: Side::New,
                    line: new_no,
                };
                new_no += 1;
                l
            }
            '-' => {
                let l = DiffLine {
                    kind: LineKind::Removed,
                    content: content.to_string(),
                    old_line: Some(old_no),
                    new_line: None,
                    no_newline_at_eof: false,
                };
                current_scope = AnchorScope::Line {
                    file: file_lbl,
                    side: Side::Old,
                    line: old_no,
                };
                old_no += 1;
                l
            }
            other => {
                return Err(err(
                    line_no,
                    format!("unrecognized diff line prefix {other:?}"),
                ));
            }
        };
        hunk.lines.push(line);
    }

    diff::finish_file(&mut files, &mut current_file, &mut current_hunk);
    flush_pending(
        &mut pending,
        &mut items,
        &mut next_thread_id,
        &mut last_thread,
    )?;

    if let Some(id) = open_ranges.into_keys().next() {
        return Err(err(
            last_line_no + 1,
            format!("range '{id}' was never closed"),
        ));
    }

    Ok(Parsed {
        diff: UnifiedDiff { files },
        items,
    })
}

/// Renders `diff_text` back out verbatim (byte-for-byte, so the blank-line
/// message-separator convention and everything else stays exactly correct),
/// with existing `threads` interleaved as read-only `>#@`/`>#` blocks at
/// their resolved position -- same `anchor::resolve_placement` used for
/// re-anchoring in `diffnote edit`'s write path and HTML export.
///
/// Returns the rendered text plus every thread whose position came from an
/// unconfirmed `Relocated` guess (shown with a `[moved]` tag) -- the
/// caller uses this list for the "silence = accept" bookkeeping: any of
/// these not explicitly overridden (`>!reanchor <id>`) or rejected
/// (`>>!reject`) in the user's edit should be auto-committed as a new
/// `reanchor` event on save.
///
/// Known limitation: a comment body containing a line that happens to look
/// like a `>#@<ulid> ...` header (vanishingly unlikely in practice) would
/// be misread as a real header on the next parse. Body lines are otherwise
/// never interpreted, only displayed.
/// `bool` = whether this thread's placement here came from a `Relocated`
/// (unconfirmed "[moved]") guess rather than an exact match.
type ByLine<'a> = HashMap<(String, Side, u32), Vec<(&'a Thread, bool)>>;

pub fn render_for_edit(
    diff_text: &str,
    diff: &UnifiedDiff,
    current_files: &[crate::model::FileDigest],
    threads: &[Thread],
) -> (String, Vec<(Ulid, Anchor)>) {
    let mut global: Vec<&Thread> = Vec::new();
    let mut by_file: HashMap<String, Vec<&Thread>> = HashMap::new();
    let mut by_hunk: HashMap<(String, usize), Vec<&Thread>> = HashMap::new();
    let mut by_line: ByLine = HashMap::new();
    let mut outdated: HashMap<String, Vec<&Thread>> = HashMap::new();
    let mut auto_relocated: Vec<(Ulid, Anchor)> = Vec::new();

    for thread in threads {
        match anchor::resolve_placement(&thread.anchor, diff, current_files) {
            Placement::Global => global.push(thread),
            Placement::File(file) => by_file.entry(file).or_default().push(thread),
            Placement::Hunk(file, idx) => by_hunk.entry((file, idx)).or_default().push(thread),
            Placement::Line {
                file,
                side,
                line_end,
                relocated,
                ..
            } => {
                let moved = relocated.is_some();
                if let Some(new_anchor) = relocated {
                    auto_relocated.push((thread.root_id, new_anchor));
                }
                by_line
                    .entry((file, side, line_end))
                    .or_default()
                    .push((thread, moved));
            }
            Placement::Outdated { file } => outdated.entry(file).or_default().push(thread),
        }
    }

    let mut out = String::new();
    if !outdated.is_empty() {
        out.push_str(
            ">#--- 現在のdiffに配置できなかった既存コメントです。>!reanchor <id> で位置を指定できます ---\n",
        );
        for ts in outdated.values() {
            for t in ts {
                render_thread_block(&mut out, t, false);
            }
        }
    }
    for t in &global {
        render_thread_block(&mut out, t, false);
    }

    let mut current_file: Option<String> = None;
    let mut hunk_idx: usize = 0;
    let mut file_has_hunks = false;
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;

    for raw_line in diff_text.lines() {
        out.push_str(raw_line);
        out.push('\n');

        if raw_line.starts_with("diff --git ") {
            current_file = None;
            hunk_idx = 0;
            file_has_hunks = false;
            continue;
        }
        if raw_line.starts_with("index ")
            || raw_line.starts_with("old mode ")
            || raw_line.starts_with("new mode ")
            || raw_line.starts_with("deleted file mode ")
            || raw_line.starts_with("new file mode ")
            || raw_line.starts_with("similarity index ")
            || raw_line.starts_with("rename from ")
            || raw_line.starts_with("rename to ")
        {
            continue;
        }
        if let Some((old_path, new_path)) = diff::parse_binary_line(raw_line) {
            current_file = new_path.or(old_path);
            hunk_idx = 0;
            file_has_hunks = false;
            if let Some(f) = &current_file
                && let Some(ts) = by_file.get(f)
            {
                for t in ts {
                    render_thread_block(&mut out, t, false);
                }
            }
            continue;
        }
        if raw_line.starts_with("--- ") {
            if current_file.is_none() || file_has_hunks {
                current_file = None;
                hunk_idx = 0;
                file_has_hunks = false;
            }
            continue;
        }
        if let Some(rest) = raw_line.strip_prefix("+++ ") {
            current_file = diff::parse_path(rest);
            if let Some(f) = &current_file
                && let Some(ts) = by_file.get(f)
            {
                for t in ts {
                    render_thread_block(&mut out, t, false);
                }
            }
            continue;
        }
        if raw_line.starts_with("@@ ") || raw_line == "@@" {
            if let Ok((old_start, _, new_start, _, _)) = diff::parse_hunk_header(raw_line, 0) {
                old_no = old_start;
                new_no = new_start;
            }
            file_has_hunks = true;
            if let Some(f) = &current_file
                && let Some(ts) = by_hunk.get(&(f.clone(), hunk_idx))
            {
                for t in ts {
                    render_thread_block(&mut out, t, false);
                }
            }
            hunk_idx += 1;
            continue;
        }
        if raw_line.starts_with('\\') {
            continue;
        }
        let Some(file) = current_file.clone() else {
            continue;
        };
        match raw_line.chars().next() {
            Some(' ') => {
                emit_line_threads(&mut out, &by_line, &file, Side::New, new_no);
                emit_line_threads(&mut out, &by_line, &file, Side::Old, old_no);
                old_no += 1;
                new_no += 1;
            }
            Some('+') => {
                emit_line_threads(&mut out, &by_line, &file, Side::New, new_no);
                new_no += 1;
            }
            Some('-') => {
                emit_line_threads(&mut out, &by_line, &file, Side::Old, old_no);
                old_no += 1;
            }
            _ => {}
        }
    }

    (out, auto_relocated)
}

fn emit_line_threads(out: &mut String, by_line: &ByLine, file: &str, side: Side, line: u32) {
    if let Some(ts) = by_line.get(&(file.to_string(), side, line)) {
        for (t, moved) in ts {
            render_thread_block(out, t, *moved);
        }
    }
}

fn render_thread_block(out: &mut String, t: &Thread, moved: bool) {
    let mut tags = String::new();
    if t.resolved {
        tags.push_str(" [resolved]");
    }
    if moved {
        tags.push_str(" [moved]");
    }
    render_rendered_entry(out, t.root_id, &t.author, t.created_at, &t.body, &tags);
    for r in &t.replies {
        render_rendered_entry(out, t.root_id, &r.author, r.created_at, &r.body, "");
    }
}

fn render_rendered_entry(
    out: &mut String,
    thread_id: Ulid,
    author: &str,
    created_at: OffsetDateTime,
    body: &str,
    tags: &str,
) {
    let ts = created_at.format(&Rfc3339).unwrap_or_default();
    out.push_str(&format!(">#@{thread_id} {author} {ts}{tags}\n"));
    for line in body.lines() {
        out.push_str(">#");
        out.push_str(line);
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Context, SourceHint};

    const BASE: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 83db48f..bf269c9 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -10,4 +10,8 @@ impl Foo {
     fn bar(&self) -> i32 {
         self.value
     }
+
+    fn baz(&self) -> i32 {
+        self.value * 2
+    }
 }
";

    #[test]
    fn single_line_comment_anchors_to_the_preceding_line() {
        // Insert the comment right after "self.value * 2" (new line 15).
        let text = BASE.replacen(
            "        self.value * 2\n+    }\n",
            "        self.value * 2\n> baz、ゼロ除算のケース考慮されてます？\n+    }\n",
            1,
        );
        let parsed = parse(&text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 1);
        let Item::NewThread {
            scope,
            body,
            directives,
            ..
        } = &parsed.items[0]
        else {
            panic!("expected a new thread");
        };
        assert_eq!(
            *scope,
            AnchorScope::Line {
                file: "src/lib.rs".to_string(),
                side: Side::New,
                line: 15,
            }
        );
        assert_eq!(
            body.as_deref(),
            Some("baz、ゼロ除算のケース考慮されてます？")
        );
        assert!(directives.is_empty());
    }

    #[test]
    fn blank_line_splits_into_independent_sibling_threads() {
        let text = format!(
            "{BASE}> まず命名を再考した方が良いと思います\n\n> 境界値のテストも足りていません\n"
        );
        let parsed = parse(&text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 2);
        for item in &parsed.items {
            let Item::NewThread { scope, .. } = item else {
                panic!("expected new threads");
            };
            // BASE's last line is the trailing context "}" (new line 17), so
            // both sibling threads anchor there.
            assert_eq!(
                *scope,
                AnchorScope::Line {
                    file: "src/lib.rs".to_string(),
                    side: Side::New,
                    line: 17,
                }
            );
        }
    }

    #[test]
    fn reply_targets_the_last_thread_and_directive_resolves_it() {
        let text = format!(
            "{BASE}> このブロックの計算、まとめられませんか？\n>> 対応しました\n>>!resolve\n"
        );
        let parsed = parse(&text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 2);
        let Item::NewThread { id: thread_id, .. } = &parsed.items[0] else {
            panic!("expected a new thread first");
        };
        let Item::Reply {
            target,
            body,
            directives,
        } = &parsed.items[1]
        else {
            panic!("expected a reply second");
        };
        assert_eq!(*target, ThreadRef::New(*thread_id));
        assert_eq!(body.as_deref(), Some("対応しました"));
        assert_eq!(directives, &vec![Directive::Resolve]);
    }

    #[test]
    fn new_thread_can_be_resolved_immediately_without_a_blank_line() {
        let text = format!("{BASE}> このコメントは確認のみです、対応不要です\n>!resolve\n");
        let parsed = parse(&text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 1);
        let Item::NewThread {
            body, directives, ..
        } = &parsed.items[0]
        else {
            panic!("expected a new thread");
        };
        assert_eq!(
            body.as_deref(),
            Some("このコメントは確認のみです、対応不要です")
        );
        assert_eq!(directives, &vec![Directive::Resolve]);
    }

    #[test]
    fn escaped_sigils_are_treated_as_literal_text() {
        let text = format!("{BASE}>\\[not a range marker\\] and \\\\ literal backslash\n");
        let parsed = parse(&text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 1);
        let Item::NewThread { body, .. } = &parsed.items[0] else {
            panic!("expected a new thread");
        };
        assert_eq!(
            body.as_deref(),
            Some("[not a range marker] and \\ literal backslash")
        );
    }

    #[test]
    fn range_comment_spans_from_open_to_close() {
        // Open the range right before "self.value * 2" (new line 15), close
        // it right after the following "}" (new line 16).
        let text = BASE.replacen(
            "+    fn baz(&self) -> i32 {\n+        self.value * 2\n+    }\n",
            "+    fn baz(&self) -> i32 {\n>[a\n+        self.value * 2\n+    }\n>]a\n> 範囲コメント\n",
            1,
        );
        let parsed = parse(&text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 1);
        let Item::NewThread { scope, body, .. } = &parsed.items[0] else {
            panic!("expected a new thread");
        };
        assert_eq!(
            *scope,
            AnchorScope::Range {
                file: "src/lib.rs".to_string(),
                side: Side::New,
                line_start: 15,
                line_end: 16,
                old_range: None,
            }
        );
        assert_eq!(body.as_deref(), Some("範囲コメント"));
    }

    #[test]
    fn range_can_cross_a_hunk_boundary() {
        let text = "\
diff --git a/f.rs b/f.rs
--- a/f.rs
+++ b/f.rs
@@ -1,2 +1,2 @@
>[x
-old1
+new1
@@ -10,2 +10,2 @@
-old2
+new2
>]x
> クロスhunkの範囲コメント
";
        let parsed = parse(text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 1);
        let Item::NewThread { scope, .. } = &parsed.items[0] else {
            panic!("expected a new thread");
        };
        assert_eq!(
            *scope,
            AnchorScope::Range {
                file: "f.rs".to_string(),
                side: Side::New,
                line_start: 1,
                line_end: 10,
                old_range: Some((1, 10)),
            }
        );
    }

    #[test]
    fn range_over_both_removed_and_new_lines_keeps_the_old_side_too() {
        // Removed lines followed by an added line, all inside one range --
        // the old side must not be silently dropped just because the new
        // side is the primary/authoritative one.
        let text = "\
diff --git a/f.rs b/f.rs
--- a/f.rs
+++ b/f.rs
@@ -1,3 +1,2 @@
>[m
-removed1
-removed2
+kept
>]m
> 削除と追加をまたぐ範囲コメント
";
        let parsed = parse(text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 1);
        let Item::NewThread { scope, body, .. } = &parsed.items[0] else {
            panic!("expected a new thread");
        };
        assert_eq!(
            *scope,
            AnchorScope::Range {
                file: "f.rs".to_string(),
                side: Side::New,
                line_start: 1,
                line_end: 1,
                old_range: Some((1, 2)),
            }
        );
        assert_eq!(body.as_deref(), Some("削除と追加をまたぐ範囲コメント"));
    }

    #[test]
    fn range_over_only_removed_lines_anchors_to_the_old_side() {
        // No `+`/context line between open and close, so `new_no` never
        // advances -- this used to be rejected as "range is empty" even
        // though the old side clearly spans two real lines.
        let text = "\
diff --git a/f.rs b/f.rs
--- a/f.rs
+++ b/f.rs
@@ -1,3 +1,1 @@
 context
>[r
-removed1
-removed2
>]r
> 削除された範囲へのコメント
 trailing
";
        let parsed = parse(text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 1);
        let Item::NewThread { scope, body, .. } = &parsed.items[0] else {
            panic!("expected a new thread");
        };
        assert_eq!(
            *scope,
            AnchorScope::Range {
                file: "f.rs".to_string(),
                side: Side::Old,
                line_start: 2,
                line_end: 3,
                old_range: None,
            }
        );
        assert_eq!(body.as_deref(), Some("削除された範囲へのコメント"));
    }

    #[test]
    fn positional_anchor_levels_global_file_and_hunk() {
        let text = "\
> diff全体へのコメント

diff --git a/f.rs b/f.rs
--- a/f.rs
+++ b/f.rs
> ファイル全体へのコメント
@@ -1,1 +1,1 @@
> hunk全体へのコメント
-old
+new
";
        let parsed = parse(text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 3);

        let Item::NewThread { scope, .. } = &parsed.items[0] else {
            panic!("expected a new thread");
        };
        assert_eq!(*scope, AnchorScope::Global);

        let Item::NewThread { scope, .. } = &parsed.items[1] else {
            panic!("expected a new thread");
        };
        assert_eq!(
            *scope,
            AnchorScope::File {
                file: "f.rs".to_string()
            }
        );

        let Item::NewThread { scope, .. } = &parsed.items[2] else {
            panic!("expected a new thread");
        };
        assert_eq!(
            *scope,
            AnchorScope::Hunk {
                file: "f.rs".to_string(),
                hunk_index: 0
            }
        );
    }

    #[test]
    fn reply_without_a_preceding_thread_is_an_error() {
        let text = format!("{BASE}>> どのスレッドへの返信？\n");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn unclosed_range_is_an_error() {
        let text = format!("{BASE}>[unclosed\n");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn rendered_header_sets_reply_target_to_the_existing_thread() {
        let existing_id = Ulid::new();
        let text = format!(
            "{BASE}>#@{existing_id} someone@example.com 2026-09-18T10:00:00Z\n>#元のコメント本文\n>> 対応しました\n>>!resolve\n"
        );
        let parsed = parse(&text).expect("valid annotation");
        assert_eq!(parsed.items.len(), 1);
        let Item::Reply {
            target,
            body,
            directives,
        } = &parsed.items[0]
        else {
            panic!("expected a reply");
        };
        assert_eq!(*target, ThreadRef::Existing(existing_id));
        assert_eq!(body.as_deref(), Some("対応しました"));
        assert_eq!(directives, &vec![Directive::Resolve]);
    }

    #[test]
    fn rendered_header_with_an_invalid_id_is_an_error() {
        let text = format!("{BASE}>#@not-a-ulid someone@example.com 2026-09-18T10:00:00Z\n");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn rendered_body_lines_are_purely_decorative() {
        // A `>#` body line with no preceding `>#@` header is just ignored,
        // not an error -- and doesn't itself set a reply target.
        let text = format!("{BASE}>#dangling body line, no header before it\n>> orphan reply\n");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn reanchor_directive_requires_new_target_and_an_id() {
        let text = format!("{BASE}> 位置がズレていたので修正しました\n>!reanchor 01J8Z\n");
        let parsed = parse(&text).expect("valid annotation");
        let Item::NewThread { directives, .. } = &parsed.items[0] else {
            panic!("expected a new thread");
        };
        assert_eq!(directives, &vec![Directive::Reanchor("01J8Z".to_string())]);

        let bad = format!("{BASE}> text\n>>!reanchor 01J8Z\n");
        assert!(parse(&bad).is_err());
    }

    fn thread_with(root_id: Ulid, anchor: Anchor, body: &str) -> Thread {
        Thread {
            root_id,
            author: "reviewer@example.com".to_string(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            body: body.to_string(),
            anchor,
            resolved: false,
            replies: Vec::new(),
        }
    }

    fn span(line_start: u32, line_end: u32, target: &str, origin_file_digest: &str) -> Anchor {
        Anchor::Span {
            file: "src/lib.rs".to_string(),
            side: Side::New,
            line_start,
            line_end,
            context: Context {
                before: Vec::new(),
                target: vec![target.to_string()],
                after: Vec::new(),
            },
            origin_file_digest: origin_file_digest.to_string(),
            source_hint: SourceHint::default(),
            old_range: None,
        }
    }

    /// The current per-file digests where `src/lib.rs` is at `digest`.
    fn files_at(digest: &str) -> Vec<crate::model::FileDigest> {
        vec![crate::model::FileDigest {
            old_path: Some("src/lib.rs".to_string()),
            new_path: Some("src/lib.rs".to_string()),
            old: None,
            new: Some(digest.to_string()),
        }]
    }

    #[test]
    fn render_for_edit_places_global_file_hunk_and_line_threads_at_their_scope() {
        let diff = diff::parse(BASE).unwrap();
        let digest = "same-digest";
        let global_id = Ulid::new();
        let file_id = Ulid::new();
        let hunk_id = Ulid::new();
        let line_id = Ulid::new();

        let threads = vec![
            thread_with(global_id, Anchor::Global, "global comment"),
            thread_with(
                file_id,
                Anchor::File {
                    file: "src/lib.rs".to_string(),
                },
                "file comment",
            ),
            thread_with(
                hunk_id,
                Anchor::Hunk {
                    file: "src/lib.rs".to_string(),
                    hunk_index: 0,
                },
                "hunk comment",
            ),
            thread_with(
                line_id,
                span(15, 15, "        self.value * 2", digest),
                "line comment",
            ),
        ];

        let (rendered, auto_relocated) = render_for_edit(BASE, &diff, &files_at(digest), &threads);
        assert!(auto_relocated.is_empty());
        assert!(!rendered.contains("[moved]"));

        let global_pos = rendered.find(&format!(">#@{global_id}")).unwrap();
        let diff_git_pos = rendered.find("diff --git").unwrap();
        assert!(
            global_pos < diff_git_pos,
            "global comment must render before the first file"
        );

        let file_pos = rendered.find(&format!(">#@{file_id}")).unwrap();
        let plusplus_pos = rendered.find("+++ b/src/lib.rs").unwrap();
        let at_pos = rendered.find("@@ -10,4").unwrap();
        assert!(
            plusplus_pos < file_pos && file_pos < at_pos,
            "file comment must render between +++ and the first hunk"
        );

        let hunk_pos = rendered.find(&format!(">#@{hunk_id}")).unwrap();
        let bar_pos = rendered.find("fn bar(&self)").unwrap();
        assert!(
            at_pos < hunk_pos && hunk_pos < bar_pos,
            "hunk comment must render between the hunk header and its first content line"
        );

        let line_pos = rendered.find(&format!(">#@{line_id}")).unwrap();
        let target_line_pos = rendered.find("self.value * 2").unwrap();
        assert!(
            target_line_pos < line_pos,
            "line comment must render after its target line"
        );
    }

    #[test]
    fn render_for_edit_tags_drifted_threads_as_moved_and_returns_them_for_auto_accept() {
        let diff = diff::parse(BASE).unwrap();
        let root_id = Ulid::new();
        // Recorded at a stale line number; the target text is unique in
        // BASE at line 15, so resolve() should find it there via exact
        // match and report it as Relocated (drifted, not confirmed).
        let thread = thread_with(
            root_id,
            span(99, 99, "        self.value * 2", "stale-digest"),
            "drifted",
        );

        let (rendered, auto_relocated) = render_for_edit(
            BASE,
            &diff,
            &files_at("current-digest"),
            std::slice::from_ref(&thread),
        );

        assert_eq!(auto_relocated.len(), 1);
        assert_eq!(auto_relocated[0].0, root_id);
        assert!(rendered.contains("[moved]"));
        let header_pos = rendered.find(&format!(">#@{root_id}")).unwrap();
        let target_pos = rendered.find("self.value * 2").unwrap();
        assert!(target_pos < header_pos);
    }

    #[test]
    fn render_for_edit_shows_outdated_threads_in_a_preamble_not_inline() {
        let diff = diff::parse(BASE).unwrap();
        let root_id = Ulid::new();
        let thread = thread_with(
            root_id,
            span(
                1,
                1,
                "content that matches nothing in BASE at all",
                "stale-digest",
            ),
            "gone",
        );

        let (rendered, auto_relocated) = render_for_edit(
            BASE,
            &diff,
            &files_at("current-digest"),
            std::slice::from_ref(&thread),
        );

        assert!(auto_relocated.is_empty());
        let header_pos = rendered.find(&format!(">#@{root_id}")).unwrap();
        let diff_git_pos = rendered.find("diff --git").unwrap();
        assert!(
            header_pos < diff_git_pos,
            "an outdated thread must render in the preamble, not inline in the diff"
        );
    }

    #[test]
    fn render_then_reparse_round_trips_a_reply_to_an_existing_thread() {
        let diff = diff::parse(BASE).unwrap();
        let root_id = Ulid::new();
        let thread = thread_with(
            root_id,
            span(15, 15, "        self.value * 2", "same-digest"),
            "original",
        );

        let (rendered, _) = render_for_edit(
            BASE,
            &diff,
            &files_at("same-digest"),
            std::slice::from_ref(&thread),
        );
        let annotated = format!("{rendered}>> 承知しました\n");
        let parsed = parse(&annotated).expect("valid annotation");

        assert_eq!(parsed.items.len(), 1);
        let Item::Reply { target, body, .. } = &parsed.items[0] else {
            panic!("expected a reply");
        };
        assert_eq!(*target, ThreadRef::Existing(root_id));
        assert_eq!(body.as_deref(), Some("承知しました"));
    }

    const WITH_BINARY: &str = "\
diff --git a/img.bin b/img.bin
Binary files a/img.bin and b/img.bin differ
diff --git a/t.txt b/t.txt
--- a/t.txt
+++ b/t.txt
@@ -1 +1 @@
-a
+b
";

    #[test]
    fn a_comment_after_a_binary_files_line_is_file_level() {
        let text = WITH_BINARY.replace("differ\n", "differ\n> about the image\n");
        let parsed = parse(&text).unwrap();
        let [Item::NewThread { scope, body, .. }] = parsed.items.as_slice() else {
            panic!("expected one new thread, got {:?}", parsed.items);
        };
        assert_eq!(
            *scope,
            AnchorScope::File {
                file: "img.bin".to_string()
            }
        );
        assert_eq!(body.as_deref(), Some("about the image"));
        assert_eq!(parsed.diff.files.len(), 2);
        assert!(parsed.diff.files[0].is_binary);
    }

    #[test]
    fn render_for_edit_places_a_binary_files_file_thread_after_its_line() {
        let diff = diff::parse(WITH_BINARY).unwrap();
        let id = Ulid::new();
        let threads = vec![thread_with(
            id,
            Anchor::File {
                file: "img.bin".to_string(),
            },
            "about the image",
        )];
        let (rendered, _) = render_for_edit(WITH_BINARY, &diff, &[], &threads);
        let lines: Vec<&str> = rendered.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.starts_with("Binary files"))
            .unwrap();
        assert!(lines[at + 1].starts_with(">#@"), "got {:?}", lines[at + 1]);
        assert!(lines[at + 2].contains("about the image"));
        // ...and the rendered text parses back to the same diff.
        let reparsed = parse(&rendered).unwrap();
        assert_eq!(reparsed.diff, diff);
    }
}
