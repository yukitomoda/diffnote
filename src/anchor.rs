//! Re-anchoring: relocating a comment's [`Anchor`] against a diff/file
//! version other than the one it was originally captured against.
//!
//! [`resolve`] is the core, corpus-agnostic matching algorithm. The caller
//! builds a [`CorpusLine`] search space with one of the two constructors
//! and picks between them per the design's two tiers:
//!
//! - **Tier 1** — [`corpus_from_file_text`]: the full text of the viewed
//!   file version is known (a bundle snapshot, or read at edit time), so all
//!   of it is searched -- and, when the version the anchor was written
//!   against is held too, a line diff between the two gives the position
//!   directly ([`resolve_with_texts`]). A match that lands outside every
//!   hunk is [`Placement::OutsideDiff`], not outdated.
//! - **Tier 2** — [`corpus_from_diff_hunks`]: only a bare new diff/patch is
//!   available, so the search is restricted to that diff's own visible
//!   context/added/removed lines for the relevant side.
//!
//! Matching, in order:
//! 1. If the file's current digest on the anchor's side matches the
//!    anchor's own `origin_file_digest`, skip matching entirely —
//!    [`Resolution::Current`].
//! 2. Exact, line-number-contiguous match of `context.target` in the
//!    corpus. Zero candidates falls through to fuzzy matching (3); exactly
//!    one is used directly; more than one is disambiguated using
//!    `context.before`/`context.after` (whichever candidate gains a
//!    strictly higher context-match score than every other), and a
//!    remaining tie is treated as unresolvable ([`Resolution::Outdated`]
//!    rather than guessing).
//! 3. A fixed-length (`target.len()`) sliding-window similarity search
//!    (via the `similar` crate's character-diff ratio) over the corpus. The
//!    best-scoring window is used if its ratio is at least
//!    [`DEFAULT_SIMILARITY_THRESHOLD`], as [`Resolution::Relocated`];
//!    otherwise [`Resolution::Outdated`]. Windows that aren't
//!    line-number-contiguous (e.g. would straddle a hunk gap in a Tier 2
//!    corpus) are never considered. A target whose surrounding lines
//!    shifted by a different number of lines than the target itself grew
//!    or shrank is a known limitation of this fixed-length window.
//!
//! A caller accepting a [`Resolution::Relocated`] anchor (silently, unless
//! rejected — see the annotation format's `>>!reject`/`>!reanchor`) should
//! persist it as the new authoritative anchor for future re-anchoring runs.

use crate::diff::{FileDiff, UnifiedDiff};
use crate::model::{Anchor, Context, FileDigest, Side, SideAnchor};

pub const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The stored context still matches exactly where expected (or the
    /// file's current digest already matched the anchor's own digest).
    Current,
    /// Found elsewhere with reasonable confidence; not yet human-confirmed.
    Relocated(SideAnchor),
    /// No confident match; the thread is shown frozen, not placed inline.
    Outdated,
}

/// One candidate line to search: `line` is its 1-based number on whichever
/// side the corpus was built for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusLine {
    pub line: u32,
    pub content: String,
}

/// Tier 1 corpus: every line of a file's current full text.
pub fn corpus_from_file_text(text: &str) -> Vec<CorpusLine> {
    text.lines()
        .enumerate()
        .map(|(i, content)| CorpusLine {
            line: (i + 1) as u32,
            content: content.to_string(),
        })
        .collect()
}

/// Tier 2 corpus: only the lines of `file` that are visible on `side` in
/// the diff currently being viewed (context lines are visible on both
/// sides; added lines only on `New`; removed lines only on `Old`).
pub fn corpus_from_diff_hunks(file: &FileDiff, side: Side) -> Vec<CorpusLine> {
    let mut out = Vec::new();
    for hunk in &file.hunks {
        for line in &hunk.lines {
            let line_no = match side {
                Side::New => line.new_line,
                Side::Old => line.old_line,
            };
            if let Some(line_no) = line_no {
                out.push(CorpusLine {
                    line: line_no,
                    content: line.content.clone(),
                });
            }
        }
    }
    out
}

/// Builds the `Context` for a freshly-created side anchor covering `len`
/// lines from `start` in `corpus`, with up to `context_lines` around it. A
/// `len` of 0 is an insertion point before line `start`: no target, just
/// the lines around it. Returns `None` if a non-empty range isn't present
/// (or isn't line-number-contiguous) in `corpus` -- callers get this from
/// the same parsed diff the comment was just written against, so that
/// should never happen in practice.
pub fn context_for_span(
    corpus: &[CorpusLine],
    start: u32,
    len: u32,
    context_lines: u32,
) -> Option<Context> {
    let n = context_lines as usize;
    if len == 0 {
        let idx = corpus
            .iter()
            .position(|l| l.line >= start)
            .unwrap_or(corpus.len());
        return Some(extract_context(corpus, idx, 0, n, n));
    }
    let start_idx = corpus.iter().position(|l| l.line == start)?;
    let len = len as usize;
    if start_idx + len > corpus.len()
        || !(0..len).all(|k| corpus[start_idx + k].line == start + k as u32)
    {
        return None;
    }
    Some(extract_context(corpus, start_idx, len, n, n))
}

/// The digest of `file` on `side` in the revision described by `files`, if
/// the file is one that revision's diff touches and has that side. `file`
/// may be spelled as either its old or its new path.
pub fn digest_for<'a>(files: &'a [FileDigest], file: &str, side: Side) -> Option<&'a str> {
    let entry = files
        .iter()
        .find(|f| f.new_path.as_deref() == Some(file) || f.old_path.as_deref() == Some(file))?;
    match side {
        Side::Old => entry.old.as_deref(),
        Side::New => entry.new.as_deref(),
    }
}

/// Re-finds one side of an anchor in `corpus`. `current_file_digest` is the
/// digest of that side's file in the version being viewed, when known;
/// `None` (unknown) never short-circuits, and a relocated anchor then
/// records an empty digest, so it is simply re-matched next time.
///
/// An insertion/deletion point (empty target) has no text of its own to
/// search for, so it is only ever `Current` (digest match) or `Outdated`
/// here; callers place such a change through the other side of the anchor.
pub fn resolve(
    side: &SideAnchor,
    current_file_digest: Option<&str>,
    corpus: &[CorpusLine],
) -> Resolution {
    if current_file_digest == Some(side.digest.as_str()) {
        return Resolution::Current;
    }

    let context = &side.context;
    let target = &context.target;
    if target.is_empty() {
        return Resolution::Outdated;
    }
    let exact = find_exact_matches(corpus, target);
    let (chosen, is_exact) = match exact.as_slice() {
        [] => (
            find_best_fuzzy_match(corpus, target, DEFAULT_SIMILARITY_THRESHOLD),
            false,
        ),
        [single] => (Some(*single), true),
        many => (
            disambiguate(corpus, many, target.len(), &context.before, &context.after),
            true,
        ),
    };

    let Some(start_idx) = chosen else {
        return Resolution::Outdated;
    };

    let start = corpus[start_idx].line;
    // Only an *exact* match at the original position means nothing at all
    // changed. A fuzzy match landing on the same line numbers still means
    // the content itself drifted, so it must still be reported (and its
    // context refreshed) as Relocated, not silently treated as Current.
    if is_exact && start == side.start {
        return Resolution::Current;
    }

    let new_context = extract_context(
        corpus,
        start_idx,
        target.len(),
        context.before.len(),
        context.after.len(),
    );
    Resolution::Relocated(SideAnchor {
        file: side.file.clone(),
        digest: current_file_digest.unwrap_or_default().to_string(),
        start,
        context: new_context,
    })
}

/// Like [`resolve`], but when the text of the version the anchor was
/// written against and of the version being viewed are both held, follows
/// the line diffs from one to the other -- through each intermediate version
/// some revision recorded, if there are any (see [`crate::digest::Blobs`]):
/// a range that survives every step is at a known new position, with no
/// searching or guessing. Anything else (a step edited the range beyond
/// recognition, or a text is missing) falls back to [`resolve`]'s search of
/// `corpus`, which must then be built from the current text.
pub fn resolve_with_texts(
    side: &SideAnchor,
    current_file_digest: Option<&str>,
    corpus: &[CorpusLine],
    blobs: &crate::digest::Blobs,
) -> Resolution {
    let Some(current_digest) = current_file_digest else {
        return resolve(side, None, corpus);
    };
    if current_digest == side.digest {
        return Resolution::Current;
    }
    if let Some(chain) = blobs.chain(&side.digest, current_digest)
        && let Some(start) = follow(&chain, side.start, side.len())
        && let Some(idx) = corpus.iter().position(|l| l.line == start)
        && idx + side.context.target.len() <= corpus.len()
    {
        let context = extract_context(
            corpus,
            idx,
            side.context.target.len(),
            side.context.before.len(),
            side.context.after.len(),
        );
        // Same place and same text: nothing moved. An in-place edit keeps
        // the position but not the text, so it is still reported (and its
        // context refreshed) as relocated.
        if start == side.start && context.target == side.context.target {
            return Resolution::Current;
        }
        return Resolution::Relocated(SideAnchor {
            file: side.file.clone(),
            digest: current_digest.to_string(),
            start,
            context,
        });
    }
    resolve(side, current_file_digest, corpus)
}

/// Follows a range through consecutive versions' texts; `None` as soon as
/// one step can't place it.
fn follow(texts: &[&str], start: u32, len: u32) -> Option<u32> {
    texts
        .windows(2)
        .try_fold(start, |at, pair| map_range(pair[0], pair[1], at, len))
}

/// Where the `len` lines starting at `start` (1-based) of `origin` sit in
/// `current`, if they all fall inside one unchanged run -- or inside a block
/// that was edited in place (replaced by the same number of lines that still
/// look like it, so line `k` of the old block is line `k` of the new one).
/// For an insertion point (`len == 0`) the line it sits before is followed.
fn map_range(origin: &str, current: &str, start: u32, len: u32) -> Option<u32> {
    let first = start.checked_sub(1)? as usize;
    let count = len.max(1) as usize;
    let diff = similar::TextDiff::from_lines(origin, current);
    diff.ops().iter().find_map(|op| match *op {
        similar::DiffOp::Equal {
            old_index,
            new_index,
            len,
        } if first >= old_index && first + count <= old_index + len => {
            Some((new_index + (first - old_index)) as u32 + 1)
        }
        similar::DiffOp::Replace {
            old_index,
            old_len,
            new_index,
            new_len,
        } if old_len == new_len && first >= old_index && first + count <= old_index + old_len => {
            let old_lines: String = diff.old_slices()[old_index..old_index + old_len].concat();
            let new_lines: String = diff.new_slices()[new_index..new_index + new_len].concat();
            let ratio = similar::TextDiff::from_chars(&old_lines, &new_lines).ratio();
            (ratio >= DEFAULT_SIMILARITY_THRESHOLD)
                .then(|| (new_index + (first - old_index)) as u32 + 1)
        }
        _ => None,
    })
}

/// Starting corpus indices where `corpus[i..i+target.len()]` is
/// content-equal to `target` line-by-line *and* line-number-contiguous
/// (so a Tier 2 corpus with a hunk gap can never produce a spurious match
/// straddling that gap).
fn find_exact_matches(corpus: &[CorpusLine], target: &[String]) -> Vec<usize> {
    if target.is_empty() || corpus.len() < target.len() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    'windows: for i in 0..=(corpus.len() - target.len()) {
        for (k, expected) in target.iter().enumerate() {
            if corpus[i + k].content != *expected {
                continue 'windows;
            }
            if k > 0 && corpus[i + k].line != corpus[i + k - 1].line + 1 {
                continue 'windows;
            }
        }
        matches.push(i);
    }
    matches
}

/// How many of `before`/`after` match, walking outward from the match and
/// stopping at the first mismatch (context is expected to be contiguous).
fn score_context(
    corpus: &[CorpusLine],
    match_start: usize,
    match_len: usize,
    before: &[String],
    after: &[String],
) -> usize {
    let mut score = 0;

    let mut idx = match_start;
    for b in before.iter().rev() {
        if idx == 0 {
            break;
        }
        let candidate = idx - 1;
        if corpus[candidate].content == *b && corpus[candidate].line + 1 == corpus[idx].line {
            score += 1;
            idx = candidate;
        } else {
            break;
        }
    }

    let mut idx = match_start + match_len;
    for a in after {
        if idx >= corpus.len() {
            break;
        }
        let prev_line = corpus[idx - 1].line;
        if corpus[idx].content == *a && corpus[idx].line == prev_line + 1 {
            score += 1;
            idx += 1;
        } else {
            break;
        }
    }

    score
}

/// Picks the exact-match candidate with a strictly-highest context score.
/// A tie is reported as unresolvable (`None`) rather than guessed.
fn disambiguate(
    corpus: &[CorpusLine],
    candidates: &[usize],
    match_len: usize,
    before: &[String],
    after: &[String],
) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    let mut unique = true;
    for &idx in candidates {
        let score = score_context(corpus, idx, match_len, before, after);
        match best {
            None => best = Some((idx, score)),
            Some((_, best_score)) if score > best_score => {
                best = Some((idx, score));
                unique = true;
            }
            Some((_, best_score)) if score == best_score => {
                unique = false;
            }
            _ => {}
        }
    }
    if unique {
        best.map(|(idx, _)| idx)
    } else {
        None
    }
}

/// Known v1 limitation: unlike `disambiguate`, this scores candidates on
/// `target` content alone and never consults `context.before`/`after`. A
/// line that drifted can lose to a *different*, unrelated line elsewhere in
/// the corpus that simply happens to be a closer character-level match
/// (e.g. a one-character `+`/`-` difference outscoring a renamed
/// identifier). Confirmed live with `anchor::resolve` chained across
/// several revisions of a real file — deliberately left as-is for now
/// rather than chasing precision; revisit if false relocations turn out to
/// be common in practice.
fn find_best_fuzzy_match(
    corpus: &[CorpusLine],
    target: &[String],
    threshold: f32,
) -> Option<usize> {
    if target.is_empty() || corpus.len() < target.len() {
        return None;
    }
    let target_text = target.join("\n");
    let mut best: Option<(usize, f32)> = None;
    for i in 0..=(corpus.len() - target.len()) {
        let contiguous =
            (1..target.len()).all(|k| corpus[i + k].line == corpus[i + k - 1].line + 1);
        if !contiguous {
            continue;
        }
        let window_text = corpus[i..i + target.len()]
            .iter()
            .map(|l| l.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        // Character-level, not line-level: a line-level diff treats two
        // differing single lines as 100%-different no matter how much text
        // they share, which makes it useless for exactly the case this is
        // for (single-line targets that were lightly edited).
        let ratio = similar::TextDiff::from_chars(&target_text, &window_text).ratio();
        if best.is_none_or(|(_, best_ratio)| ratio > best_ratio) {
            best = Some((i, ratio));
        }
    }
    best.filter(|(_, ratio)| *ratio >= threshold)
        .map(|(idx, _)| idx)
}

pub(crate) fn extract_context(
    corpus: &[CorpusLine],
    match_start: usize,
    match_len: usize,
    before_n: usize,
    after_n: usize,
) -> Context {
    let before_start = match_start.saturating_sub(before_n);
    let before = corpus[before_start..match_start]
        .iter()
        .map(|l| l.content.clone())
        .collect();
    let target = corpus[match_start..match_start + match_len]
        .iter()
        .map(|l| l.content.clone())
        .collect();
    let after_end = (match_start + match_len + after_n).min(corpus.len());
    let after = corpus[match_start + match_len..after_end]
        .iter()
        .map(|l| l.content.clone())
        .collect();
    Context {
        before,
        target,
        after,
    }
}

/// Where a thread ends up being drawn, after resolving its anchor against a
/// diff. Shared by the HTML exporter and round-trip annotation rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Global,
    File(String),
    Line {
        file: String,
        /// The side the card is drawn on: the head side when the anchor
        /// still has text there, else the base side (a deleted range).
        side: Side,
        line_start: u32,
        line_end: u32,
        /// The base-side range the anchor also covers when `side` is
        /// `New` (e.g. a replaced block), so both get highlighted.
        old_range: Option<(u32, u32)>,
        /// `Some(new_anchor)` if a side came from a `Relocated` guess (not
        /// yet human-confirmed); `None` if every side matched where it was
        /// recorded (nothing to accept/reject).
        relocated: Option<Anchor>,
    },
    /// Anchor is a `Span` but couldn't be confidently placed in `diff`.
    Outdated {
        file: String,
    },
    /// The text still exists, but not on a line the diff shows, so it can't
    /// be drawn inline. Distinct from `Outdated`; shown in the same
    /// unplaced section.
    OutsideDiff {
        file: String,
    },
}

pub fn find_file<'a>(diff: &'a UnifiedDiff, file: &str) -> Option<&'a FileDiff> {
    diff.files
        .iter()
        .find(|f| f.new_path.as_deref() == Some(file) || f.old_path.as_deref() == Some(file))
}

/// What became of one side of a `Span` anchor in the diff being viewed.
enum SideState {
    /// No such side, or an insertion/deletion point with no text to place.
    Absent,
    Placed {
        start: u32,
        end: u32,
        relocated: Option<SideAnchor>,
    },
    /// Found, but not on lines the diff shows.
    Outside,
    Outdated,
}

fn place_side(
    side: &SideAnchor,
    which: Side,
    file_diff: &FileDiff,
    current_files: &[FileDigest],
    blobs: &crate::digest::Blobs,
) -> SideState {
    if side.is_empty() {
        return SideState::Absent;
    }
    let current = digest_for(current_files, &side.file, which);
    // Tier 1 when the full text of the version being viewed is held.
    let full_text = current.and_then(|d| blobs.text(d));
    let visible = corpus_from_diff_hunks(file_diff, which);
    let corpus = match full_text {
        Some(text) => corpus_from_file_text(text),
        None => visible.clone(),
    };
    let (start, end, relocated) =
        match resolve_with_texts(side, current, &corpus, blobs) {
            Resolution::Outdated => return SideState::Outdated,
            Resolution::Current => (side.start, side.end(), None),
            Resolution::Relocated(a) => (a.start, a.end(), Some(a)),
        };
    if !(start..=end).all(|l| visible.iter().any(|v| v.line == l)) {
        return SideState::Outside;
    }
    SideState::Placed {
        start,
        end,
        relocated,
    }
}

/// `blobs` holds file versions by digest (the bundle's snapshots, plus
/// whatever this session read). A side whose current version is held is
/// searched against the whole file (Tier 1), and mapped from the version it
/// was written against by a line diff when that is held too; otherwise it
/// falls back to the diff's own visible lines (Tier 2).
///
/// A `Span` is drawn on its head side when that still resolves, else on its
/// base side. If a side moved, the returned `relocated` anchor carries the
/// new position of that side and keeps the other as recorded.
pub fn resolve_placement(
    anchor: &Anchor,
    diff: &UnifiedDiff,
    current_files: &[FileDigest],
    blobs: &crate::digest::Blobs,
) -> Placement {
    match anchor {
        Anchor::Global { .. } => Placement::Global,
        Anchor::File { base, head } => {
            Placement::File(head.as_ref().or(base.as_ref()).map(|f| f.file.clone()).unwrap_or_default())
        }
        Anchor::Span { base, head } => {
            let label = head.as_ref().or(base.as_ref()).map(|s| s.file.clone()).unwrap_or_default();
            let file_diff = [head, base]
                .into_iter()
                .flatten()
                .find_map(|s| find_file(diff, &s.file));
            let Some(file_diff) = file_diff else {
                return Placement::Outdated { file: label };
            };
            let head_state = head.as_ref().map_or(SideState::Absent, |s| {
                place_side(s, Side::New, file_diff, current_files, blobs)
            });
            let base_state = base.as_ref().map_or(SideState::Absent, |s| {
                place_side(s, Side::Old, file_diff, current_files, blobs)
            });

            let relocated = |h: &SideState, b: &SideState| {
                let moved = |st: &SideState| match st {
                    SideState::Placed { relocated, .. } => relocated.clone(),
                    _ => None,
                };
                let (rh, rb) = (moved(h), moved(b));
                if rh.is_none() && rb.is_none() {
                    return None;
                }
                Some(Anchor::Span {
                    base: rb.or_else(|| base.clone()),
                    head: rh.or_else(|| head.clone()),
                })
            };

            match (&head_state, &base_state) {
                (SideState::Placed { start, end, .. }, b) => Placement::Line {
                    file: head.as_ref().map(|s| s.file.clone()).unwrap_or(label),
                    side: Side::New,
                    line_start: *start,
                    line_end: *end,
                    old_range: match b {
                        SideState::Placed { start, end, .. } => Some((*start, *end)),
                        _ => None,
                    },
                    relocated: relocated(&head_state, &base_state),
                },
                (_, SideState::Placed { start, end, .. }) => Placement::Line {
                    file: base.as_ref().map(|s| s.file.clone()).unwrap_or(label),
                    side: Side::Old,
                    line_start: *start,
                    line_end: *end,
                    old_range: None,
                    relocated: relocated(&head_state, &base_state),
                },
                (SideState::Outside, _) | (_, SideState::Outside) => {
                    Placement::OutsideDiff { file: label }
                }
                _ => Placement::Outdated { file: label },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn corpus(lines: &[(u32, &str)]) -> Vec<CorpusLine> {
        lines
            .iter()
            .map(|(n, c)| CorpusLine {
                line: *n,
                content: c.to_string(),
            })
            .collect()
    }

    fn anchor(line_start: u32, line_end: u32, target: &[&str]) -> SideAnchor {
        anchor_with(
            line_start,
            line_end,
            Vec::new(),
            target,
            Vec::new(),
            "old-digest",
        )
    }

    fn anchor_with(
        line_start: u32,
        line_end: u32,
        before: Vec<String>,
        target: &[&str],
        after: Vec<String>,
        digest: &str,
    ) -> SideAnchor {
        let _ = line_end; // the range's length is `target.len()`
        SideAnchor {
            file: "src/lib.rs".to_string(),
            digest: digest.to_string(),
            start: line_start,
            context: Context {
                before,
                target: target.iter().map(|s| s.to_string()).collect(),
                after,
            },
        }
    }

    fn span(head: SideAnchor) -> Anchor {
        Anchor::Span {
            base: None,
            head: Some(head),
        }
    }

    #[test]
    fn same_digest_is_current_without_searching() {
        let a = anchor_with(
            1,
            1,
            Vec::new(),
            &["whatever, never looked at"],
            Vec::new(),
            "same",
        );
        // An empty corpus would make any real search fail; this proves the
        // fast path really does skip matching.
        assert_eq!(resolve(&a, Some("same"), &[]), Resolution::Current);
    }

    #[test]
    fn exact_unique_match_at_the_same_position_is_current() {
        let c = corpus(&[(10, "fn bar() {"), (11, "  self.value"), (12, "}")]);
        let a = anchor(11, 11, &["  self.value"]);
        assert_eq!(resolve(&a, Some("new-digest"), &c), Resolution::Current);
    }

    #[test]
    fn exact_unique_match_at_a_new_position_is_relocated() {
        let c = corpus(&[
            (1, "// unrelated preamble"),
            (2, "// more preamble"),
            (10, "fn bar() {"),
            (11, "  self.value"),
            (12, "}"),
        ]);
        // Originally at line 5, the content now sits at line 11.
        let a = anchor(5, 5, &["  self.value"]);
        let resolved = resolve(&a, Some("new-digest"), &c);
        let Resolution::Relocated(SideAnchor {
            start,
            digest,
            context,
            ..
        }) = resolved
        else {
            panic!("expected Relocated, got something else");
        };
        assert_eq!(start, 11);
        assert_eq!(digest, "new-digest");
        assert_eq!(context.target, vec!["  self.value".to_string()]);
    }

    #[test]
    fn ambiguous_exact_matches_are_disambiguated_by_surrounding_context() {
        // "return x;" appears twice; only the second occurrence is preceded
        // by "fn b() {" as recorded in the anchor's `before` context.
        let c = corpus(&[
            (1, "fn a() {"),
            (2, "return x;"),
            (3, "}"),
            (4, "fn b() {"),
            (5, "return x;"),
            (6, "}"),
        ]);
        let a = anchor_with(
            99,
            99,
            vec!["fn b() {".to_string()],
            &["return x;"],
            Vec::new(),
            "old-digest",
        );
        let Resolution::Relocated(SideAnchor { start, .. }) =
            resolve(&a, Some("new-digest"), &c)
        else {
            panic!("expected Relocated");
        };
        assert_eq!(start, 5);
    }

    #[test]
    fn ambiguous_exact_matches_with_no_disambiguation_are_outdated() {
        let c = corpus(&[(1, "return x;"), (2, "return x;")]);
        let a = anchor(99, 99, &["return x;"]);
        assert_eq!(resolve(&a, Some("new-digest"), &c), Resolution::Outdated);
    }

    #[test]
    fn fuzzy_match_above_threshold_is_relocated() {
        // The commented line picked up an extra parameter since the
        // original comment was made — still clearly "the same line" by
        // content, just no longer an exact match.
        let c = corpus(&[
            (1, "impl Calc {"),
            (2, "    fn baz(&self, factor: i32) -> i32 {"),
            (3, "        self.value * factor"),
            (4, "    }"),
        ]);
        let a = anchor(2, 2, &["    fn baz(&self) -> i32 {"]);
        let Resolution::Relocated(SideAnchor { start, context, .. }) =
            resolve(&a, Some("new-digest"), &c)
        else {
            panic!("expected Relocated");
        };
        assert_eq!(start, 2);
        assert_eq!(
            context.target,
            vec!["    fn baz(&self, factor: i32) -> i32 {".to_string()]
        );
    }

    #[test]
    fn fuzzy_match_below_threshold_is_outdated() {
        let c = corpus(&[
            (1, "totally unrelated content"),
            (2, "nothing like the original"),
        ]);
        let a = anchor(5, 5, &["    fn baz(&self) -> i32 {"]);
        assert_eq!(resolve(&a, Some("new-digest"), &c), Resolution::Outdated);
    }

    #[test]
    fn an_insertion_point_is_current_by_digest_but_never_searched() {
        let mut a = anchor(3, 3, &[]);
        a.context.before = vec!["x".to_string()];
        let c = corpus(&[(1, "x"), (2, "y")]);
        assert_eq!(resolve(&a, Some("old-digest"), &c), Resolution::Current);
        assert_eq!(resolve(&a, Some("new-digest"), &c), Resolution::Outdated);
    }

    #[test]
    fn global_and_file_anchors_are_placed_without_searching() {
        let diff = crate::diff::parse(TIER1_DIFF).unwrap();
        let global = Anchor::Global {
            base: None,
            head: Some("rev".into()),
        };
        assert_eq!(
            resolve_placement(&global, &diff, &[], &Default::default()),
            Placement::Global
        );
        let file = Anchor::File {
            base: None,
            head: Some(crate::model::FileRef {
                file: "gone.rs".into(),
                digest: "d".into(),
            }),
        };
        assert_eq!(
            resolve_placement(&file, &diff, &[], &Default::default()),
            Placement::File("gone.rs".into())
        );
    }

    #[test]
    fn target_longer_than_corpus_is_outdated() {
        let c = corpus(&[(1, "only one line")]);
        let a = anchor(1, 2, &["line one", "line two"]);
        assert_eq!(resolve(&a, Some("new-digest"), &c), Resolution::Outdated);
    }

    #[test]
    fn tier2_corpus_never_matches_across_a_hunk_gap() {
        // Lines 11 and 20 come from two different hunks of the same file
        // (line 11 ends one hunk's visible range, line 20 starts another);
        // nothing actually connects them in the real file. The target is an
        // exact content match for that (11, 20) pair, but must still be
        // rejected for not being line-number-contiguous, and no other
        // window in the corpus is textually similar enough to match instead.
        let c = corpus(&[
            (10, "1234567890"),
            (11, "QWERTYUIOP1"),
            (20, "ASDFGHJKL2"),
            (21, "0987654321"),
        ]);
        let a = anchor(1, 2, &["QWERTYUIOP1", "ASDFGHJKL2"]);
        assert_eq!(resolve(&a, Some("new-digest"), &c), Resolution::Outdated);
    }

    #[test]
    fn corpus_from_diff_hunks_picks_the_requested_side() {
        let diff_text = "\
--- a/f.rs
+++ b/f.rs
@@ -1,2 +1,3 @@
 context1
-removed
+added1
+added2
";
        let parsed = crate::diff::parse(diff_text).expect("valid diff");
        let file = &parsed.files[0];

        let new_side = corpus_from_diff_hunks(file, Side::New);
        assert_eq!(
            new_side
                .iter()
                .map(|l| l.content.as_str())
                .collect::<Vec<_>>(),
            vec!["context1", "added1", "added2"]
        );
        assert_eq!(
            new_side.iter().map(|l| l.line).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        let old_side = corpus_from_diff_hunks(file, Side::Old);
        assert_eq!(
            old_side
                .iter()
                .map(|l| l.content.as_str())
                .collect::<Vec<_>>(),
            vec!["context1", "removed"]
        );
        assert_eq!(
            old_side.iter().map(|l| l.line).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn corpus_from_file_text_is_one_based() {
        let c = corpus_from_file_text("a\nb\nc\n");
        assert_eq!(c.iter().map(|l| l.line).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(
            c.iter().map(|l| l.content.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn context_for_span_grabs_surrounding_lines() {
        let c = corpus_from_file_text("a\nb\nc\nd\ne\nf\ng\n");
        let ctx = context_for_span(&c, 4, 2, 2).expect("range should be found");
        assert_eq!(ctx.before, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(ctx.target, vec!["d".to_string(), "e".to_string()]);
        assert_eq!(ctx.after, vec!["f".to_string(), "g".to_string()]);
    }

    #[test]
    fn context_for_span_of_an_insertion_point_has_only_the_lines_around_it() {
        let c = corpus_from_file_text("a\nb\nc\nd\n");
        let ctx = context_for_span(&c, 3, 0, 1).unwrap();
        assert_eq!(ctx.before, vec!["b".to_string()]);
        assert!(ctx.target.is_empty());
        assert_eq!(ctx.after, vec!["c".to_string()]);
    }

    #[test]
    fn context_for_span_is_none_when_the_range_is_not_present() {
        let c = corpus(&[(1, "a"), (2, "b"), (10, "c")]);
        // Lines 2 and 10 exist individually but aren't contiguous.
        assert_eq!(context_for_span(&c, 2, 9, 1), None);
        assert_eq!(context_for_span(&c, 5, 2, 1), None);
    }

    #[test]
    fn digest_for_finds_a_file_by_either_path_and_side() {
        let files = vec![FileDigest {
            old_path: Some("old.rs".to_string()),
            new_path: Some("new.rs".to_string()),
            old: Some("sha256:o".to_string()),
            new: Some("sha256:n".to_string()),
        }];
        assert_eq!(digest_for(&files, "new.rs", Side::New), Some("sha256:n"));
        assert_eq!(digest_for(&files, "old.rs", Side::Old), Some("sha256:o"));
        assert_eq!(digest_for(&files, "other.rs", Side::New), None);

        let added = vec![FileDigest {
            old_path: None,
            new_path: Some("a.rs".to_string()),
            old: None,
            new: Some("sha256:n".to_string()),
        }];
        assert_eq!(digest_for(&added, "a.rs", Side::Old), None);
    }

    #[test]
    fn unknown_current_digest_never_short_circuits() {
        let a = anchor(3, 3, &["x"]);
        let c = corpus(&[(1, "a"), (2, "b"), (3, "x")]);
        // Matched by search instead; at the same place, so still Current --
        // but through the search path, not the digest shortcut.
        assert_eq!(resolve(&a, None, &c), Resolution::Current);
    }
    const TIER1_DIFF: &str = "diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 l1
+l2
 l3
 l4
";

    const TIER1_TEXT: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";

    fn tier1_digests() -> Vec<FileDigest> {
        vec![FileDigest {
            old_path: Some("src/lib.rs".into()),
            new_path: Some("src/lib.rs".into()),
            old: None,
            new: Some(crate::digest::digest(TIER1_TEXT)),
        }]
    }

    fn tier1_blobs() -> crate::digest::Blobs<'static> {
        let mut blobs = crate::digest::Blobs::default();
        blobs.add(TIER1_TEXT.as_bytes());
        blobs
    }

    #[test]
    fn full_text_finds_a_line_the_diff_shows_only_partially() {
        let diff = crate::diff::parse(TIER1_DIFF).unwrap();
        // `l3` used to be line 2; the new hunk shows it at line 3.
        let a = span(anchor(2, 2, &["l3"]));
        for (files, blobs) in [
            (tier1_digests(), tier1_blobs()),
            (Vec::new(), Default::default()),
        ] {
            let p = resolve_placement(&a, &diff, &files, &blobs);
            let Placement::Line {
                line_start,
                relocated,
                ..
            } = p
            else {
                panic!("{p:?}")
            };
            assert_eq!(line_start, 3);
            assert!(relocated.is_some());
        }
    }

    #[test]
    fn text_present_only_outside_the_hunks_is_outside_the_diff_not_outdated() {
        let diff = crate::diff::parse(TIER1_DIFF).unwrap();
        let a = span(anchor(8, 8, &["l8"]));
        // The diff alone can't see line 8: nothing to say but "outdated".
        assert_eq!(
            resolve_placement(&a, &diff, &[], &Default::default()),
            Placement::Outdated {
                file: "src/lib.rs".into()
            }
        );
        // With the full file it is found, but there is no hunk to draw it in.
        assert_eq!(
            resolve_placement(&a, &diff, &tier1_digests(), &tier1_blobs()),
            Placement::OutsideDiff {
                file: "src/lib.rs".into()
            }
        );
        // Genuinely gone stays outdated even with the full file.
        let gone = span(anchor(8, 8, &["completely different"]));
        assert!(matches!(
            resolve_placement(&gone, &diff, &tier1_digests(), &tier1_blobs()),
            Placement::Outdated { .. }
        ));
    }

    /// Blobs holding `texts`, with each consecutive pair linked as a step.
    fn blobs_of<'a>(texts: &'a [&'a str], linked: bool) -> crate::digest::Blobs<'a> {
        let mut blobs = crate::digest::Blobs::default();
        for t in texts {
            blobs.add(t.as_bytes());
        }
        if linked {
            for pair in texts.windows(2) {
                blobs.link(
                    &crate::digest::digest(pair[0]),
                    &crate::digest::digest(pair[1]),
                );
            }
        }
        blobs
    }

    fn resolve_through(
        a: &SideAnchor,
        texts: &[&str],
        linked: bool,
    ) -> Resolution {
        let current = texts.last().unwrap();
        let mut a = a.clone();
        a.digest = crate::digest::digest(texts[0]);
        resolve_with_texts(
            &a,
            Some(&crate::digest::digest(current)),
            &corpus_from_file_text(current),
            &blobs_of(texts, linked),
        )
    }

    #[test]
    fn a_line_diff_between_the_two_versions_gives_the_position_directly() {
        // Identical-looking lines would defeat a text search; the diff
        // between the versions still knows which one the range was.
        let texts = ["x\nsame\nsame\ny\n", "new\nx\nsame\nsame\ny\n"];
        let a = anchor(3, 3, &["same"]);
        let Resolution::Relocated(moved) = resolve_through(&a, &texts, false) else {
            panic!("expected Relocated");
        };
        assert_eq!(moved.start, 4);
        // Without the origin text this is ambiguous, so it can't be placed.
        let corpus = corpus_from_file_text(texts[1]);
        assert_eq!(
            resolve_with_texts(
                &a,
                Some(&crate::digest::digest(texts[1])),
                &corpus,
                &blobs_of(&texts[1..], false)
            ),
            Resolution::Outdated
        );
    }

    #[test]
    fn a_range_is_followed_through_intermediate_versions() {
        // Step 1 edits the line in place, step 2 pushes it down.
        let texts = [
            "x\nreturn value\ny\n",
            "x\nreturn values\ny\n",
            "p\nq\nx\nreturn values\ny\n",
        ];
        let a = anchor(2, 2, &["return value"]);
        for linked in [true, false] {
            let Resolution::Relocated(moved) = resolve_through(&a, &texts, linked) else {
                panic!("expected Relocated (linked: {linked})");
            };
            assert_eq!(moved.start, 4);
            assert_eq!(moved.context.target, vec!["return values".to_string()]);
        }
    }

    #[test]
    fn a_line_edited_in_place_is_followed_only_while_it_still_looks_alike() {
        let a = anchor(2, 2, &["return value"]);
        let close = ["x\nreturn value\ny\n", "x\nreturn values\ny\n"];
        assert!(matches!(
            resolve_through(&a, &close, false),
            Resolution::Relocated(_)
        ));
        // Same shape, unrelated text: the search fallback finds nothing either.
        let far = ["x\nreturn value\ny\n", "x\nzzzzzzzzzzzz\ny\n"];
        assert_eq!(resolve_through(&a, &far, false), Resolution::Outdated);
    }

    #[test]
    fn chain_goes_through_linked_versions_and_falls_back_to_a_direct_pair() {
        let texts = ["one\n", "two\n", "three\n"];
        let linked = blobs_of(&texts, true);
        let d = |t: &str| crate::digest::digest(t);
        assert_eq!(
            linked.chain(&d(texts[0]), &d(texts[2])).unwrap(),
            vec!["one\n", "two\n", "three\n"]
        );
        let unlinked = blobs_of(&texts, false);
        assert_eq!(
            unlinked.chain(&d(texts[0]), &d(texts[2])).unwrap(),
            vec!["one\n", "three\n"]
        );
        assert!(linked.chain("sha256:missing", &d(texts[2])).is_none());
    }

    #[test]
    fn an_insertion_point_follows_the_line_it_sits_before() {
        let texts = ["a\nb\n", "z\na\nb\n"];
        let mut a = anchor(2, 2, &[]);
        a.context.before = vec!["a".to_string()];
        a.context.after = vec!["b".to_string()];
        let Resolution::Relocated(moved) = resolve_through(&a, &texts, false) else {
            panic!("expected Relocated");
        };
        assert_eq!(moved.start, 3);
        assert!(moved.context.target.is_empty());
    }
}
