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
//! - `>#[<ulid>` / `>#]<ulid>` decorative markers around an existing
//!   comment's lines (before its first row, and just before its `>#@`
//!   header, after its last), drawn only when it covers more than its own
//!   row, so the range can be seen; ignored on parse like any `>#` text.
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

use crate::anchor::{self, Absence, Placement};
use crate::diff::{
    self, DiffLine, FileDiff, Hunk, LineKind, ParseError as DiffParseError, UnifiedDiff,
};
use crate::expand;
use crate::model::Side;
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

/// A run of lines in one version of a file. `len == 0` is an insertion
/// point: `start` is the line the (absent) text would sit *before*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSpan {
    pub start: u32,
    pub len: u32,
}

impl LineSpan {
    fn new(start: u32, len: u32) -> Self {
        Self { start, len }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorScope {
    Global,
    File {
        file: String,
    },
    /// What the diff position covers on each side. A context line is a
    /// one-line span on both; an added line an empty `base` and a one-line
    /// `head`; a removed line the reverse; a hunk or `>[`..`>]` range the
    /// whole run on both sides. Never empty on both.
    Span {
        file: String,
        base: LineSpan,
        head: LineSpan,
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
    /// Things that parsed fine but probably aren't what the user meant --
    /// currently a closed range that no comment was attached to.
    pub warnings: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnnotationError {
    #[error("{line} 行目: {message}")]
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
        Err(err(line_no, format!("範囲の ID が不正です: {id:?}")))
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
            Err(err(line_no, "'reanchor' にはスレッド ID の引数が必要です"))
        }
        ("reanchor", Target::Reply, _) => Err(err(
            line_no,
            "'reanchor' は '>>!reanchor' ではなく '>!reanchor <id>' と書いてください",
        )),
        _ => Err(err(
            line_no,
            format!("ディレクティブが未知か、書式が不正です: {rest:?}"),
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
                    "'>>' の返信先になるスレッド(新規または表示済み)が直前にありません",
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
    let mut warnings: Vec<String> = Vec::new();
    // The range most recently closed (id, line of its `>]`) while no
    // comment has been attached to it yet. A range is only a scope for the
    // comment that follows, so one that never gets a comment records nothing.
    let mut unused_range: Option<(String, usize)> = None;

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
                        return Err(err(line_no, format!("範囲 '{id}' はすでに開いています")));
                    }
                    let file = current_file
                        .as_ref()
                        .map(file_label)
                        .ok_or_else(|| err(line_no, "ファイルの外に範囲マーカーがあります"))?;
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
                        .ok_or_else(|| err(line_no, format!("範囲 '{id}' は開かれていません")))?;
                    warn_unused_range(&mut unused_range, &mut warnings);
                    unused_range = Some((id.to_string(), line_no));
                    let base = LineSpan::new(start.start_old_line, old_no - start.start_old_line);
                    let head = LineSpan::new(start.start_new_line, new_no - start.start_new_line);
                    if base.len == 0 && head.len == 0 {
                        return Err(err(line_no, format!("範囲 '{id}' が空です")));
                    }
                    current_scope = AnchorScope::Span {
                        file: start.file,
                        base,
                        head,
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
                            format!("'>#@' ヘッダのスレッド ID が不正です: {id_token:?}"),
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
                            if target == Target::New {
                                unused_range = None;
                            }
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
                            if target == Target::New {
                                unused_range = None;
                            }
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
        // any pending comment block -- and the chance to comment on a range
        // that was just closed.
        warn_unused_range(&mut unused_range, &mut warnings);
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
                .ok_or_else(|| err(line_no, "ファイルヘッダの外に '+++' 行があります"))?;
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
                .ok_or_else(|| err(line_no, "ファイルの外にハンクヘッダがあります"))?;
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
            // A side with no lines has its `start` one *before* the point
            // (git's convention), unlike ours (the line it sits before).
            let side = |start: u32, len: u32| LineSpan::new(if len == 0 { start + 1 } else { start }, len);
            current_scope = AnchorScope::Span {
                file: file_lbl,
                base: side(old_start, old_lines),
                head: side(new_start, new_lines),
            };
            continue;
        }

        if raw_line.strip_prefix('\\').is_some() {
            let hunk = current_hunk
                .as_mut()
                .ok_or_else(|| err(line_no, "ハンクの外に '\\' マーカーがあります"))?;
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
            .ok_or_else(|| err(line_no, "ファイルの外に差分の内容があります"))?;
        let hunk = current_hunk
            .as_mut()
            .ok_or_else(|| err(line_no, "ハンクの外に差分の内容があります"))?;
        let line = match prefix_char {
            ' ' => {
                let l = DiffLine {
                    kind: LineKind::Context,
                    content: content.to_string(),
                    old_line: Some(old_no),
                    new_line: Some(new_no),
                    no_newline_at_eof: false,
                };
                current_scope = AnchorScope::Span {
                    file: file_lbl,
                    base: LineSpan::new(old_no, 1),
                    head: LineSpan::new(new_no, 1),
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
                current_scope = AnchorScope::Span {
                    file: file_lbl,
                    base: LineSpan::new(old_no, 0),
                    head: LineSpan::new(new_no, 1),
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
                current_scope = AnchorScope::Span {
                    file: file_lbl,
                    base: LineSpan::new(old_no, 1),
                    head: LineSpan::new(new_no, 0),
                };
                old_no += 1;
                l
            }
            other => {
                return Err(err(
                    line_no,
                    format!("差分の行頭が認識できません: {other:?}"),
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

    warn_unused_range(&mut unused_range, &mut warnings);
    if let Some(id) = open_ranges.into_keys().next() {
        return Err(err(
            last_line_no + 1,
            format!("範囲 '{id}' が閉じられていません"),
        ));
    }

    Ok(Parsed {
        diff: UnifiedDiff { files },
        items,
        warnings,
    })
}

fn warn_unused_range(unused: &mut Option<(String, usize)>, warnings: &mut Vec<String>) {
    if let Some((id, line)) = unused.take() {
        let name = if id.is_empty() {
            "範囲".to_string()
        } else {
            format!("範囲 '{id}'")
        };
        warnings.push(format!(
            "{line} 行目: {name} は閉じられていますが、コメントがないため何も記録されません"
        ));
    }
}

/// Renders `diff_text` back out verbatim (byte-for-byte, so the blank-line
/// message-separator convention and everything else stays exactly correct),
/// with existing `threads` interleaved as read-only `>#@`/`>#` blocks at
/// their resolved position -- same `anchor::resolve_placement` used by HTML
/// export. A thread whose lines the view doesn't have is drawn at the point
/// where they are or were, tagged `[削除済み]` (removed since), `[まだない]`
/// (only in a later version) or `[不在]`, with what they say quoted (`>#|`).
///
/// Known limitation: a comment body containing a line that happens to look
/// like a `>#@<ulid> ...` header (vanishingly unlikely in practice) would
/// be misread as a real header on the next parse. Body lines are otherwise
/// never interpreted, only displayed.
/// `Some(was)` = the thread's lines were removed; this is where they were.
type ByLine<'a> = HashMap<(String, Side, u32), Vec<(&'a Thread, Option<(Vec<String>, Absence)>)>>;

pub fn render_for_edit(
    diff_text: &str,
    diff: &UnifiedDiff,
    view: &anchor::ViewVersions,
    blobs: &crate::digest::Blobs,
    threads: &[Thread],
    extra: &[expand::Want],
) -> (String, Vec<crate::model::FileDigest>) {
    let placements: Vec<Placement> = threads
        .iter()
        .map(|t| anchor::resolve_placement(&t.anchor, diff, view, blobs))
        .collect();

    // The buffer is the diff plus context around whatever threads are about
    // that the diff doesn't show, and files it doesn't touch at all.
    let wants: Vec<expand::Want> = placements
        .iter()
        .filter_map(expand::want_of)
        .chain(extra.iter().cloned())
        .collect();
    let (expanded, synthetic) = expand::expand(diff, &wants, view, blobs);
    let (shown, text, synthetic) = match expand::to_text(diff_text, diff, &expanded) {
        Some(text) => (expanded, text, synthetic),
        None => (diff.clone(), diff_text.to_string(), Vec::new()),
    };
    let diff_text = text.as_str();
    // What a comment written in a file of pure context is anchored to.
    let synthetic_files: Vec<crate::model::FileDigest> = synthetic
        .iter()
        .filter_map(|p| {
            let digest = view.head(p)?.to_string();
            Some(crate::model::FileDigest {
                old_path: Some(p.clone()),
                new_path: Some(p.clone()),
                old: Some(digest.clone()),
                new: Some(digest),
            })
        })
        .collect();

    let mut global: Vec<&Thread> = Vec::new();
    let mut by_file: HashMap<String, Vec<&Thread>> = HashMap::new();
    let mut by_line: ByLine = HashMap::new();
    // Where a thread begins, so its `>#[` marker can be drawn before its first
    // row (the card itself is after the last). A thread on a single row gets
    // none: its card is right under that row.
    let mut starts: HashMap<(String, Side, u32), Vec<Ulid>> = HashMap::new();
    let mut outdated: HashMap<String, Vec<&Thread>> = HashMap::new();

    for (thread, placement) in threads.iter().zip(placements) {
        // A card goes after a row of the diff; without one the thread is
        // listed as unplaced.
        let placement = match expand::want_of(&placement) {
            Some(w) if !expand::has_row(&shown, &w) => Placement::Unplaced { file: w.file },
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
                starts
                    .entry((file.clone(), side, line_start))
                    .or_default()
                    .push(thread.root_id);
                // With base-side rows too, the range may begin on one.
                if let Some((old_start, _)) = old_range {
                    starts
                        .entry((file.clone(), Side::Old, old_start))
                        .or_default()
                        .push(thread.root_id);
                }
                by_line
                    .entry((file, side, line_end))
                    .or_default()
                    .push((thread, None));
            }
            Placement::Point {
                file,
                before,
                was,
                kind,
            } => {
                by_line
                    .entry((file, Side::New, before.saturating_sub(1).max(1)))
                    .or_default()
                    .push((thread, Some((was, kind))));
            }
            Placement::Unplaced { file } => outdated.entry(file).or_default().push(thread),
        }
    }

    let mut out = String::new();
    if !outdated.is_empty() {
        out.push_str(
            ">#--- 現在の差分に配置できなかった既存コメントです。>!reanchor <id> で位置を指定できます ---\n",
        );
        for ts in outdated.values() {
            for t in ts {
                render_thread_block(&mut out, t, None);
            }
        }
    }
    for t in &global {
        render_thread_block(&mut out, t, None);
    }

    let mut current_file: Option<String> = None;
    let mut file_has_hunks = false;
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;

    // Threads whose range start was reached (`seen`), and those that got a
    // `>#[` marker and so get its `>#]` closing pair too (`marked`).
    let mut marks = RangeMarks::default();

    for raw_line in diff_text.lines() {
        let row_start = out.len();
        out.push_str(raw_line);
        out.push('\n');

        if raw_line.starts_with("diff --git ") {
            current_file = None;
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
            file_has_hunks = false;
            if let Some(f) = &current_file
                && let Some(ts) = by_file.get(f)
            {
                for t in ts {
                    render_thread_block(&mut out, t, None);
                }
            }
            continue;
        }
        if raw_line.starts_with("--- ") {
            if current_file.is_none() || file_has_hunks {
                current_file = None;
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
                    render_thread_block(&mut out, t, None);
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
                let keys = [(Side::Old, old_no), (Side::New, new_no)];
                mark_starts(&mut out, row_start, &starts, &by_line, &mut marks, &file, &keys);
                emit_line_threads(&mut out, &by_line, &marks, &file, Side::New, new_no);
                emit_line_threads(&mut out, &by_line, &marks, &file, Side::Old, old_no);
                old_no += 1;
                new_no += 1;
            }
            Some('+') => {
                let keys = [(Side::New, new_no)];
                mark_starts(&mut out, row_start, &starts, &by_line, &mut marks, &file, &keys);
                emit_line_threads(&mut out, &by_line, &marks, &file, Side::New, new_no);
                new_no += 1;
            }
            Some('-') => {
                let keys = [(Side::Old, old_no)];
                mark_starts(&mut out, row_start, &starts, &by_line, &mut marks, &file, &keys);
                emit_line_threads(&mut out, &by_line, &marks, &file, Side::Old, old_no);
                old_no += 1;
            }
            _ => {}
        }
    }

    (out, synthetic_files)
}

/// Which threads have had their range's start reached, and which of those
/// got a `>#[` marker (and so get a `>#]` before their card).
#[derive(Default)]
struct RangeMarks {
    seen: std::collections::HashSet<Ulid>,
    drawn: std::collections::HashSet<Ulid>,
}

/// Draws `>#[<id>` at byte `at` (before the row just written) for each thread
/// that begins on this row, unless its card is drawn right under this same
/// row (it covers just this one). `keys` are the row's places: its line on
/// each side it has.
fn mark_starts(
    out: &mut String,
    at: usize,
    starts: &HashMap<(String, Side, u32), Vec<Ulid>>,
    by_line: &ByLine,
    marks: &mut RangeMarks,
    file: &str,
    keys: &[(Side, u32)],
) {
    let card_here = |id: &Ulid| {
        keys.iter().any(|&(side, line)| {
            by_line
                .get(&(file.to_string(), side, line))
                .is_some_and(|ts| ts.iter().any(|(t, _)| t.root_id == *id))
        })
    };
    let mut markers = String::new();
    for &(side, line) in keys {
        for id in starts.get(&(file.to_string(), side, line)).into_iter().flatten() {
            if marks.seen.insert(*id) && !card_here(id) {
                marks.drawn.insert(*id);
                markers.push_str(&format!(">#[{id}\n"));
            }
        }
    }
    out.insert_str(at, &markers);
}

fn emit_line_threads(
    out: &mut String,
    by_line: &ByLine,
    marks: &RangeMarks,
    file: &str,
    side: Side,
    line: u32,
) {
    if let Some(ts) = by_line.get(&(file.to_string(), side, line)) {
        for (t, absent) in ts {
            if marks.drawn.contains(&t.root_id) {
                out.push_str(&format!(">#]{}\n", t.root_id));
            }
            render_thread_block(out, t, absent.as_ref().map(|(w, k)| (w.as_slice(), *k)));
        }
    }
}

fn render_thread_block(out: &mut String, t: &Thread, absent: Option<(&[String], Absence)>) {
    let mut tags = String::new();
    if t.resolved {
        tags.push_str(" [解決済み]");
    }
    if let Some((_, kind)) = absent {
        tags.push_str(match kind {
            Absence::Deleted => " [削除済み]",
            Absence::NotYet => " [まだない]",
            Absence::Unknown => " [不在]",
        });
    }
    let quoted: Vec<String> = absent
        .map(|(was, _)| was)
        .unwrap_or_default()
        .iter()
        .map(|l| format!("| {l}"))
        .collect();
    render_rendered_entry(out, t.root_id, &t.author, t.created_at, &t.body, &tags, &quoted);
    for r in &t.replies {
        render_rendered_entry(out, t.root_id, &r.author, r.created_at, &r.body, "", &[]);
    }
}

fn render_rendered_entry(
    out: &mut String,
    thread_id: Ulid,
    author: &str,
    created_at: OffsetDateTime,
    body: &str,
    tags: &str,
    quoted: &[String],
) {
    let ts = created_at.format(&Rfc3339).unwrap_or_default();
    out.push_str(&format!(">#@{thread_id} {author} {ts}{tags}\n"));
    for line in quoted {
        out.push_str(">#");
        out.push_str(line);
        out.push('\n');
    }
    for line in body.lines() {
        out.push_str(">#");
        out.push_str(line);
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Anchor;

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
            AnchorScope::Span {
                file: "src/lib.rs".to_string(),
                base: LineSpan::new(13, 0),
                head: LineSpan::new(15, 1),
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
                AnchorScope::Span {
                file: "src/lib.rs".to_string(),
                base: LineSpan::new(13, 1),
                head: LineSpan::new(17, 1),
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
            AnchorScope::Span {
                file: "src/lib.rs".to_string(),
                base: LineSpan::new(13, 0),
                head: LineSpan::new(15, 2),
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
            AnchorScope::Span {
                file: "f.rs".to_string(),
                base: LineSpan::new(1, 10),
                head: LineSpan::new(1, 10),
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
            AnchorScope::Span {
                file: "f.rs".to_string(),
                base: LineSpan::new(1, 2),
                head: LineSpan::new(1, 1),
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
            AnchorScope::Span {
                file: "f.rs".to_string(),
                base: LineSpan::new(2, 2),
                head: LineSpan::new(2, 0),
            }
        );
        assert_eq!(body.as_deref(), Some("削除された範囲へのコメント"));
    }

    #[test]
    fn positional_anchor_levels_global_file_and_hunk_span() {
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
            AnchorScope::Span {
                file: "f.rs".to_string(),
                base: LineSpan::new(1, 1),
                head: LineSpan::new(1, 1),
            }
        );
    }

    #[test]
    fn reply_without_a_preceding_thread_is_an_error() {
        let text = format!("{BASE}>> どのスレッドへの返信？\n");
        assert!(parse(&text).is_err());
    }

    fn with_range(after_close: &str, id: &str) -> String {
        BASE.replacen(
            "+    fn baz(&self) -> i32 {\n+        self.value * 2\n+    }\n",
            &format!(
                "+    fn baz(&self) -> i32 {{\n>[{id}\n+        self.value * 2\n+    }}\n>]{id}\n{after_close}"
            ),
            1,
        )
    }

    #[test]
    fn a_closed_range_with_a_comment_produces_no_warning() {
        let parsed = parse(&with_range("> 範囲コメント\n", "a")).unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        // A blank line before the comment doesn't detach it from the range.
        let parsed = parse(&with_range("\n> 範囲コメント\n", "a")).unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    }

    #[test]
    fn a_closed_range_without_a_comment_warns_and_records_nothing() {
        // The next diff line arrives before any comment.
        let parsed = parse(&with_range("", "a")).unwrap();
        assert!(parsed.items.is_empty());
        assert_eq!(parsed.warnings.len(), 1);
        assert!(
            parsed.warnings[0].contains("範囲 'a'"),
            "{:?}",
            parsed.warnings
        );
        assert!(parsed.warnings[0].contains(" 行目: "));

        // ...and the same for an anonymous range.
        let parsed = parse(&with_range("", "")).unwrap();
        assert_eq!(parsed.warnings.len(), 1);
        assert!(
            parsed.warnings[0].contains("範囲 は閉じられて"),
            "{:?}",
            parsed.warnings
        );
    }

    #[test]
    fn a_reply_does_not_count_as_commenting_on_a_range() {
        let id = Ulid::new();
        let text = with_range(
            &format!(">#@{id} someone@example.com 2026-09-18T10:00:00Z\n>> 返信\n"),
            "a",
        );
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.warnings.len(), 1);
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

    fn range(start: u32, len: u32, text: &str) -> crate::model::LineRange {
        crate::model::LineRange {
            file: "src/lib.rs".to_string(),
            digest: crate::digest::digest(text),
            start,
            len,
        }
    }

    /// The file `BASE`'s hunk is an excerpt of: nine filler lines, then the
    /// hunk's old side (lines 10-13).
    fn old_text() -> String {
        let mut t: String = (1..=9).map(|n| format!("// filler {n}\n")).collect();
        t.push_str(
            "    fn bar(&self) -> i32 {\n        self.value\n    }\n}\n",
        );
        t
    }

    /// ...and its new side (lines 10-17).
    fn new_text() -> String {
        let mut t: String = (1..=9).map(|n| format!("// filler {n}\n")).collect();
        t.push_str(
            "    fn bar(&self) -> i32 {\n        self.value\n    }\n\n    fn baz(&self) -> i32 {\n        self.value * 2\n    }\n}\n",
        );
        t
    }

    /// What `render_for_edit` needs to know about BASE's revision.
    struct Fixture {
        files: Vec<crate::model::FileDigest>,
        texts: Vec<String>,
    }

    fn fixture() -> Fixture {
        Fixture {
            files: vec![crate::model::FileDigest {
                old_path: Some("src/lib.rs".to_string()),
                new_path: Some("src/lib.rs".to_string()),
                old: Some(crate::digest::digest(old_text())),
                new: Some(crate::digest::digest(new_text())),
            }],
            texts: vec![old_text(), new_text()],
        }
    }

    fn render(fx: &Fixture, extra: &[&str], threads: &[Thread]) -> String {
        render_with_files(fx, extra, threads).0
    }

    fn render_with_files(
        fx: &Fixture,
        extra: &[&str],
        threads: &[Thread],
    ) -> (String, Vec<crate::model::FileDigest>) {
        let diff = diff::parse(BASE).unwrap();
        let mut blobs = crate::digest::Blobs::default();
        for t in fx.texts.iter().map(String::as_str).chain(extra.iter().copied()) {
            blobs.add(t.as_bytes());
        }
        // The extra texts are older versions of the file, taken to today's.
        for t in extra {
            blobs.link(&crate::digest::digest(t), &crate::digest::digest(new_text()));
        }
        render_for_edit(
            BASE,
            &diff,
            &anchor::ViewVersions {
                files: &fx.files,
                tree: &[],
            },
            &blobs,
            threads,
            &[],
        )
    }

    /// A comment on the added line `self.value * 2` (new line 15).
    fn on_baz_line() -> Anchor {
        Anchor::Span {
            base: Some(range(13, 0, &old_text())),
            head: Some(range(15, 1, &new_text())),
        }
    }

    #[test]
    fn render_for_edit_places_global_file_hunk_and_line_threads_at_their_scope() {
        let (global_id, file_id, hunk_id, line_id) =
            (Ulid::new(), Ulid::new(), Ulid::new(), Ulid::new());
        let threads = vec![
            thread_with(
                global_id,
                Anchor::Global {
                    base: None,
                    head: Some("rev".to_string()),
                },
                "global comment",
            ),
            thread_with(
                file_id,
                Anchor::File {
                    base: None,
                    head: Some(crate::model::FileRef {
                        file: "src/lib.rs".to_string(),
                        digest: "d".to_string(),
                    }),
                },
                "file comment",
            ),
            // The whole hunk: old lines 10-13, new lines 10-17.
            thread_with(
                hunk_id,
                Anchor::Span {
                    base: Some(range(10, 4, &old_text())),
                    head: Some(range(10, 8, &new_text())),
                },
                "hunk comment",
            ),
            thread_with(line_id, on_baz_line(), "line comment"),
        ];

        let rendered = render(&fixture(), &[], &threads);
        assert!(!rendered.contains("[削除済み]"));

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

        // A hunk comment is a span over the hunk's lines, drawn after the
        // last of them like any range comment.
        let hunk_pos = rendered.find(&format!(">#@{hunk_id}")).unwrap();
        let last_line = rendered.rfind("\n }\n").unwrap();
        assert!(last_line < hunk_pos, "a span comment must render after its lines");

        let line_pos = rendered.find(&format!(">#@{line_id}")).unwrap();
        let target_line_pos = rendered.find("self.value * 2").unwrap();
        assert!(
            target_line_pos < line_pos,
            "line comment must render after its target line"
        );
    }

    /// A comment on new lines `start` to `start + len - 1` of the added block.
    fn on_new_lines(start: u32, len: u32) -> Anchor {
        Anchor::Span {
            base: Some(range(13, 0, &old_text())),
            head: Some(range(start, len, &new_text())),
        }
    }

    #[test]
    fn a_thread_over_several_rows_has_a_start_marker_before_its_first_row() {
        // `fn baz` (14) and `self.value * 2` (15).
        let id = Ulid::new();
        let rendered = render(
            &fixture(),
            &[],
            &[thread_with(id, on_new_lines(14, 2), "about baz")],
        );
        let (marker, card) = (format!(">#[{id}\n"), format!(">#]{id}\n>#@{id}"));
        let (m, c) = (rendered.find(&marker).unwrap(), rendered.find(&card).unwrap());
        let first = rendered.find("+    fn baz").unwrap();
        let last = rendered.find("        self.value * 2").unwrap();
        // Marker, then the first row, then the last row, then the card.
        assert!(m < first && first < last && last < c, "{rendered}");
        // The row before the range is not inside it.
        assert!(rendered.find("+\n").unwrap() < m, "{rendered}");
        assert_eq!(rendered.matches(&marker).count(), 1);
    }

    #[test]
    fn a_thread_on_one_row_has_no_start_marker() {
        let id = Ulid::new();
        let rendered = render(&fixture(), &[], &[thread_with(id, on_baz_line(), "one line")]);
        assert!(rendered.contains(&format!(">#@{id}")));
        assert!(!rendered.contains(">#["), "{rendered}");
    }

    #[test]
    fn a_hunk_thread_starts_before_the_hunks_first_row_and_each_marker_appears_once() {
        let (hunk, inner) = (Ulid::new(), Ulid::new());
        let rendered = render(
            &fixture(),
            &[],
            &[
                thread_with(
                    hunk,
                    Anchor::Span {
                        base: Some(range(10, 4, &old_text())),
                        head: Some(range(10, 8, &new_text())),
                    },
                    "the hunk",
                ),
                thread_with(inner, on_new_lines(13, 3), "inside it"),
            ],
        );
        let at = |needle: &str| rendered.find(needle).unwrap();
        let (hunk_marker, inner_marker) = (format!(">#[{hunk}\n"), format!(">#[{inner}\n"));
        assert!(at("@@ -10,4") < at(&hunk_marker) && at(&hunk_marker) < at("     fn bar"), "{rendered}");
        // The inner range starts at the added blank line: after the context
        // row `    }`, before that blank `+` row.
        assert!(at("     }\n") < at(&inner_marker), "{rendered}");
        assert!(rendered.contains(&format!("{inner_marker}+\n+    fn baz")), "{rendered}");
        assert_eq!(rendered.matches(&hunk_marker).count(), 1);
        assert_eq!(rendered.matches(&inner_marker).count(), 1);
    }

    #[test]
    fn a_replaced_block_starts_on_its_removed_row_and_a_context_row_has_no_marker() {
        let (old, new) = ("tags\nraw\nlong\nsemver\n", "tags\nraw\nshort\nsemver\n");
        let tree = |t: &str| -> crate::files::Tree {
            [("f.txt".to_string(), t.as_bytes().to_vec())].into()
        };
        let (text, files) = crate::files::diff_trees(&tree(old), &tree(new));
        let file_range = |text: &str, start, len| crate::model::LineRange {
            file: "f.txt".to_string(),
            ..range(start, len, text)
        };
        let (block, context) = (Ulid::new(), Ulid::new());
        let threads = vec![
            // `long` -> `short`: a removed row and an added row.
            thread_with(
                block,
                Anchor::Span {
                    base: Some(file_range(old, 3, 1)),
                    head: Some(file_range(new, 3, 1)),
                },
                "the change",
            ),
            // `semver`, unchanged: one context row, both sides.
            thread_with(
                context,
                Anchor::Span {
                    base: Some(file_range(old, 4, 1)),
                    head: Some(file_range(new, 4, 1)),
                },
                "just this line",
            ),
        ];
        let mut blobs = crate::digest::Blobs::default();
        blobs.add(old.as_bytes());
        blobs.add(new.as_bytes());
        let (rendered, _) = render_for_edit(
            &text,
            &diff::parse(&text).unwrap(),
            &anchor::ViewVersions {
                files: &files,
                tree: &[],
            },
            &blobs,
            &threads,
            &[],
        );
        assert!(
            rendered.contains(&format!(">#[{block}\n-long\n+short\n>#]{block}\n>#@{block}")),
            "{rendered}"
        );
        assert!(!rendered.contains(&format!(">#[{context}")), "{rendered}");
        assert!(rendered.contains(&format!(" semver\n>#@{context}")), "{rendered}");
    }

    #[test]
    fn a_thread_with_replies_has_one_opening_and_one_closing_marker() {
        let id = Ulid::new();
        let mut thread = thread_with(id, on_new_lines(14, 2), "about baz");
        for body in ["first reply", "second reply"] {
            thread.replies.push(crate::review::Reply {
                author: "other@example.com".to_string(),
                created_at: OffsetDateTime::UNIX_EPOCH,
                body: body.to_string(),
            });
        }
        let rendered = render(&fixture(), &[], &[thread]);
        // Three `>#@` headers (root and two replies), but the range is one.
        assert_eq!(rendered.matches(&format!(">#@{id}")).count(), 3, "{rendered}");
        assert_eq!(rendered.matches(&format!(">#[{id}\n")).count(), 1, "{rendered}");
        assert_eq!(rendered.matches(&format!(">#]{id}\n")).count(), 1, "{rendered}");
        // The closing marker sits right before the first header, not between.
        let close = rendered.find(&format!(">#]{id}\n")).unwrap();
        let first_header = rendered.find(&format!(">#@{id}")).unwrap();
        assert_eq!(first_header, close + format!(">#]{id}\n").len(), "{rendered}");
        assert!(parse(&rendered).unwrap().items.is_empty());
    }

    #[test]
    fn a_marker_that_was_drawn_is_ignored_when_the_buffer_is_read_back() {
        let id = Ulid::new();
        let rendered = render(
            &fixture(),
            &[],
            &[thread_with(id, on_new_lines(14, 2), "about baz")],
        );
        assert!(rendered.contains(">#["));
        let parsed = parse(&rendered).unwrap();
        assert!(parsed.items.is_empty() && parsed.warnings.is_empty(), "{parsed:?}");
    }

    #[test]
    fn a_thread_written_against_an_older_version_is_placed_by_following_the_lines() {
        // The same file three lines shorter at the top: `self.value * 2` was
        // line 12 then, and the position is worked out from the two texts.
        let older: String = new_text().lines().skip(3).map(|l| format!("{l}\n")).collect();
        let root_id = Ulid::new();
        let thread = thread_with(
            root_id,
            Anchor::Span {
                base: None,
                head: Some(range(12, 1, &older)),
            },
            "written earlier",
        );
        let rendered = render(&fixture(), &[&older], std::slice::from_ref(&thread));
        let header_pos = rendered.find(&format!(">#@{root_id}")).unwrap();
        let target_pos = rendered.find("self.value * 2").unwrap();
        let after = rendered.find("+    }\n").unwrap();
        assert!(target_pos < header_pos && header_pos < after);
        assert!(!rendered.contains("[削除済み]"));
    }

    #[test]
    fn a_thread_on_lines_that_were_removed_is_drawn_where_they_were_and_quotes_them() {
        // Written against a version that had an extra line between `}` (new
        // line 12) and the blank line; this diff's file doesn't have it.
        let mut older = new_text();
        older = older.replacen("    }\n\n", "    }\n    // gone soon\n\n", 1);
        let root_id = Ulid::new();
        let thread = thread_with(
            root_id,
            Anchor::Span {
                base: None,
                head: Some(range(13, 1, &older)),
            },
            "why is this here",
        );
        let rendered = render(&fixture(), &[&older], std::slice::from_ref(&thread));
        assert!(rendered.contains(" [削除済み]"), "{rendered}");
        assert!(rendered.contains(">#|     // gone soon"), "{rendered}");
        // Between the lines it was between: after `    }` (line 12), before
        // the added blank line.
        assert!(
            rendered.contains(&format!("     }}\n>#@{root_id}")),
            "{rendered}"
        );
        assert!(rendered.contains(&format!(">#@{root_id}")) && !rendered.contains("+\n>#@"));
    }

    #[test]
    fn render_for_edit_shows_unplaceable_threads_in_a_preamble_not_inline() {
        let root_id = Ulid::new();
        // A version that isn't in the bundle: nothing to follow from.
        let thread = thread_with(
            root_id,
            Anchor::Span {
                base: None,
                head: Some(range(1, 1, "a version nobody kept")),
            },
            "gone",
        );
        let rendered = render(&fixture(), &[], std::slice::from_ref(&thread));
        let header_pos = rendered.find(&format!(">#@{root_id}")).unwrap();
        let diff_git_pos = rendered.find("diff --git").unwrap();
        assert!(
            header_pos < diff_git_pos,
            "an unplaced thread must render in the preamble, not inline in the diff"
        );
    }

    #[test]
    fn a_thread_on_a_line_the_diff_does_not_show_is_drawn_inline_with_context() {
        let root_id = Ulid::new();
        // `// filler 2` (line 2) is far above the hunk that starts at line 10.
        let thread = thread_with(
            root_id,
            Anchor::Span {
                base: Some(range(2, 1, &old_text())),
                head: Some(range(2, 1, &new_text())),
            },
            "far above the hunk",
        );
        let rendered = render(&fixture(), &[], std::slice::from_ref(&thread));
        // A block of context (lines 1..=5) is added for it...
        let block = rendered.find("@@ -1,5 +1,5 @@").expect(&rendered);
        let header = rendered.find(&format!(">#@{root_id}")).unwrap();
        let line2 = rendered.find(" // filler 2\n").unwrap();
        assert!(block < line2 && line2 < header, "{rendered}");
        // ...before the real hunk, in the same file, and it is not in the
        // preamble.
        assert!(header > rendered.find("diff --git").unwrap());
        assert!(header < rendered.find("@@ -10,4").unwrap());
        // The buffer still parses, and its context hunk is context only.
        let parsed = parse(&rendered).expect("valid");
        assert_eq!(parsed.diff.files.len(), 1);
        let hunks = &parsed.diff.files[0].hunks;
        assert_eq!(hunks.len(), 2);
        assert!(hunks[0].lines.iter().all(|l| l.kind == crate::diff::LineKind::Context));
    }

    #[test]
    fn a_thread_on_a_file_the_diff_does_not_touch_gets_a_file_of_context() {
        let readme = "# title\nline 2\nline 3\nline 4\nline 5\nline 6\n";
        let root_id = Ulid::new();
        let anchor = Anchor::Span {
            base: None,
            head: Some(crate::model::LineRange {
                file: "README.md".to_string(),
                digest: crate::digest::digest(readme),
                start: 4,
                len: 1,
            }),
        };
        let thread = thread_with(root_id, anchor, "about the readme");
        let diff = diff::parse(BASE).unwrap();
        let fx = fixture();
        let mut blobs = crate::digest::Blobs::default();
        for t in fx.texts.iter().map(String::as_str).chain([readme]) {
            blobs.add(t.as_bytes());
        }
        let tree = [crate::model::TreeFile {
            path: "README.md".to_string(),
            digest: crate::digest::digest(readme),
        }];
        let (rendered, synthetic) = render_for_edit(
            BASE,
            &diff,
            &anchor::ViewVersions {
                files: &fx.files,
                tree: &tree,
            },
            &blobs,
            std::slice::from_ref(&thread),
            &[],
        );
        // The real diff is untouched and comes first; the README follows.
        assert!(rendered.starts_with(&format!("{BASE}diff --git a/README.md b/README.md\n")), "{rendered}");
        let header = rendered.find(&format!(">#@{root_id}")).unwrap();
        let line4 = rendered.find(" line 4\n").unwrap();
        assert!(line4 < header);
        // What a comment written there is anchored to: the file's version.
        assert_eq!(synthetic.len(), 1);
        assert_eq!(synthetic[0].new_path.as_deref(), Some("README.md"));
        assert_eq!(synthetic[0].new, Some(crate::digest::digest(readme)));
        // A new comment added on that context parses to that file, and the
        // whole buffer reads back.
        let annotated = rendered.replacen(" line 6\n", " line 6\n> new comment on the readme\n", 1);
        let parsed = parse(&annotated).unwrap();
        let [Item::NewThread { scope, .. }] = parsed.items.as_slice() else {
            panic!("{:?}", parsed.items);
        };
        assert_eq!(
            *scope,
            AnchorScope::Span {
                file: "README.md".to_string(),
                base: LineSpan::new(6, 1),
                head: LineSpan::new(6, 1),
            }
        );
        let anchor = crate::create::build_anchor(
            scope,
            &parsed.diff,
            &synthetic,
            &(None, None),
        )
        .unwrap();
        let Anchor::Span { head: Some(h), .. } = anchor else {
            panic!()
        };
        assert_eq!((h.start, h.len), (6, 1));
        assert_eq!(h.digest, crate::digest::digest(readme));
    }

    /// Runs `render_for_edit` with no threads and just these requests.
    fn render_showing(extra: &[expand::Want], readme: Option<&str>) -> (String, Vec<crate::model::FileDigest>) {
        let diff = diff::parse(BASE).unwrap();
        let fx = fixture();
        let mut blobs = crate::digest::Blobs::default();
        for t in fx.texts.iter().map(String::as_str).chain(readme) {
            blobs.add(t.as_bytes());
        }
        let tree: Vec<crate::model::TreeFile> = readme
            .map(|t| crate::model::TreeFile {
                path: "README.md".to_string(),
                digest: crate::digest::digest(t),
            })
            .into_iter()
            .collect();
        render_for_edit(
            BASE,
            &diff,
            &anchor::ViewVersions {
                files: &fx.files,
                tree: &tree,
            },
            &blobs,
            &[],
            extra,
        )
    }

    fn want(file: &str, start: u32, end: u32) -> expand::Want {
        expand::Want {
            file: file.to_string(),
            lines: Some((Side::New, start, end)),
            row: None,
        }
    }

    #[test]
    fn requested_lines_of_the_diffs_own_file_get_context_blocks() {
        let (text, synthetic) = render_showing(&[want("src/lib.rs", 2, 3)], None);
        assert!(text.contains("@@ -1,6 +1,6 @@") || text.contains("@@ -1,5 +1,5 @@"), "{text}");
        assert!(text.contains(" // filler 2\n") && text.contains(" // filler 3\n"));
        assert!(synthetic.is_empty(), "no new file: it is the diff's own");
        // It reads back, and the real hunk is still there.
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.diff.files.len(), 1);
        assert_eq!(parsed.diff.files[0].hunks.len(), 2);
    }

    #[test]
    fn requested_lines_the_diff_already_shows_change_nothing() {
        // Lines 10..=17 are the hunk itself.
        let (text, synthetic) = render_showing(&[want("src/lib.rs", 11, 14)], None);
        assert_eq!(text, BASE);
        assert!(synthetic.is_empty());
    }

    #[test]
    fn a_requested_file_the_diff_does_not_touch_is_appended_and_readable() {
        let readme = "# t\nline 2\nline 3\n";
        let (text, synthetic) = render_showing(&[want("README.md", 1, 3)], Some(readme));
        assert!(text.starts_with(BASE));
        assert!(text.contains("diff --git a/README.md b/README.md\n"));
        assert_eq!(synthetic.len(), 1);
        assert_eq!(synthetic[0].new, Some(crate::digest::digest(readme)));
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.diff.files.len(), 2);
        assert_eq!(parsed.diff.files[1].hunks[0].lines.len(), 3);
    }

    #[test]
    fn requests_and_threads_are_shown_together_without_repeating_lines() {
        let root_id = Ulid::new();
        let thread = thread_with(
            root_id,
            Anchor::Span {
                base: Some(range(2, 1, &old_text())),
                head: Some(range(2, 1, &new_text())),
            },
            "on line 2",
        );
        let diff = diff::parse(BASE).unwrap();
        let fx = fixture();
        let mut blobs = crate::digest::Blobs::default();
        for t in &fx.texts {
            blobs.add(t.as_bytes());
        }
        let (text, _) = render_for_edit(
            BASE,
            &diff,
            &anchor::ViewVersions {
                files: &fx.files,
                tree: &[],
            },
            &blobs,
            std::slice::from_ref(&thread),
            &[want("src/lib.rs", 3, 4)],
        );
        let parsed = parse(&text).unwrap();
        let lines: Vec<u32> = parsed.diff.files[0]
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().filter_map(|l| l.new_line))
            .collect();
        let mut sorted = lines.clone();
        sorted.dedup();
        assert_eq!(lines, sorted, "no line twice");
        assert!(text.contains(&format!(">#@{root_id}")));
    }

    #[test]
    fn a_thread_whose_context_cannot_be_read_is_unplaced_not_lost() {
        // README's version is recorded but its text isn't in the bundle.
        let root_id = Ulid::new();
        let anchor = Anchor::Span {
            base: None,
            head: Some(crate::model::LineRange {
                file: "README.md".to_string(),
                digest: crate::digest::digest("held nowhere"),
                start: 1,
                len: 1,
            }),
        };
        let thread = thread_with(root_id, anchor, "lost");
        let rendered = render(&fixture(), &[], std::slice::from_ref(&thread));
        let header = rendered.find(&format!(">#@{root_id}")).unwrap();
        assert!(header < rendered.find("diff --git").unwrap(), "in the preamble");
    }

    #[test]
    fn render_then_reparse_round_trips_a_reply_to_an_existing_thread() {
        let root_id = Ulid::new();
        let thread = thread_with(root_id, on_baz_line(), "original");
        let rendered = render(&fixture(), &[], std::slice::from_ref(&thread));
        let annotated = format!("{rendered}>> 承知しました\n");
        let parsed = parse(&annotated).expect("valid annotation");

        assert_eq!(parsed.items.len(), 1);
        let Item::Reply { target, body, .. } = &parsed.items[0] else {
            panic!("expected a reply");
        };
        assert_eq!(*target, ThreadRef::Existing(root_id));
        assert_eq!(body.as_deref(), Some("承知しました"));
    }

    #[test]
    fn a_deleted_marker_and_its_quote_do_not_disturb_reparsing() {
        let mut older = new_text();
        older = older.replacen("    }\n\n", "    }\n    // gone soon\n\n", 1);
        let thread = thread_with(
            Ulid::new(),
            Anchor::Span {
                base: None,
                head: Some(range(13, 1, &older)),
            },
            "why is this here",
        );
        let rendered = render(&fixture(), &[&older], std::slice::from_ref(&thread));
        let reparsed = parse(&rendered).expect("the rendered buffer parses");
        assert!(reparsed.items.is_empty(), "{:?}", reparsed.items);
        assert_eq!(reparsed.diff, diff::parse(BASE).unwrap());
    }

    #[test]
    fn reject_is_no_longer_a_directive() {
        let text = format!("{BASE}> text\n>> reply\n>>!reject\n");
        assert!(parse(&text).is_err());
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
                base: None,
                head: Some(crate::model::FileRef {
                    file: "img.bin".to_string(),
                    digest: "d".to_string(),
                }),
            },
            "about the image",
        )];
        let (rendered, _) = render_for_edit(
            WITH_BINARY,
            &diff,
            &anchor::ViewVersions {
                files: &[],
                tree: &[],
            },
            &Default::default(),
            &threads,
            &[],
        );
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
