//! Re-anchoring: relocating a comment's [`Anchor`] against a diff/file
//! version other than the one it was originally captured against.
//!
//! [`resolve`] is the core, corpus-agnostic matching algorithm. The caller
//! builds a [`CorpusLine`] search space with one of the two constructors
//! and picks between them per the design's two tiers:
//!
//! - **Tier 1** — [`corpus_from_file_text`]: the full new-side text of the
//!   file is known (a bundle snapshot, or read at edit time), so all of it
//!   is searched. Only for `Side::New` anchors: snapshots hold the new side
//!   only, so `Side::Old` anchors always use Tier 2 below. A match that
//!   lands outside every hunk is [`Placement::OutsideDiff`], not outdated.
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
use crate::model::{Anchor, Context, FileDigest, Side};

pub const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The stored context still matches exactly where expected (or the
    /// file's current digest already matched the anchor's own digest).
    Current,
    /// Found elsewhere with reasonable confidence; not yet human-confirmed.
    Relocated(Anchor),
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

/// Builds a `Context` snapshot for a freshly-created `Span` anchor: finds
/// `line_start..=line_end` in `corpus` and grabs up to `context_lines` on
/// each side. Returns `None` if the range isn't present (or isn't
/// line-number-contiguous) in `corpus` -- callers get this from the same
/// parsed diff the comment was just written against, so that should never
/// happen in practice.
pub fn context_for_line_range(
    corpus: &[CorpusLine],
    line_start: u32,
    line_end: u32,
    context_lines: u32,
) -> Option<Context> {
    let start_idx = corpus.iter().position(|l| l.line == line_start)?;
    let end_idx = corpus.iter().position(|l| l.line == line_end)?;
    if end_idx < start_idx {
        return None;
    }
    let match_len = end_idx - start_idx + 1;
    if !(0..match_len).all(|k| corpus[start_idx + k].line == line_start + k as u32) {
        return None;
    }
    Some(extract_context(
        corpus,
        start_idx,
        match_len,
        context_lines as usize,
        context_lines as usize,
    ))
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

/// `current_file_digest` is the digest of the anchor's file on its side in
/// the version being viewed, when known. `None` (unknown) never
/// short-circuits, and a relocated anchor then records an empty origin
/// digest, so it is simply re-matched next time.
pub fn resolve(
    anchor: &Anchor,
    current_file_digest: Option<&str>,
    corpus: &[CorpusLine],
) -> Resolution {
    // Global/File/Hunk anchors are positional only -- v1 doesn't re-verify
    // that the file/hunk they name still exists, so they're always current.
    let Anchor::Span {
        file,
        side,
        line_start: anchor_line_start,
        line_end: anchor_line_end,
        context,
        origin_file_digest,
        source_hint,
        old_range: _,
    } = anchor
    else {
        return Resolution::Current;
    };

    if current_file_digest == Some(origin_file_digest.as_str()) {
        return Resolution::Current;
    }

    let target = &context.target;
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

    let line_start = corpus[start_idx].line;
    let line_end = corpus[start_idx + target.len() - 1].line;
    // Only an *exact* match at the original position means nothing at all
    // changed. A fuzzy match landing on the same line numbers still means
    // the content itself drifted, so it must still be reported (and its
    // context refreshed) as Relocated, not silently treated as Current.
    if is_exact && line_start == *anchor_line_start && line_end == *anchor_line_end {
        return Resolution::Current;
    }

    let new_context = extract_context(
        corpus,
        start_idx,
        target.len(),
        context.before.len(),
        context.after.len(),
    );
    Resolution::Relocated(Anchor::Span {
        file: file.clone(),
        side: *side,
        line_start,
        line_end,
        context: new_context,
        origin_file_digest: current_file_digest.unwrap_or_default().to_string(),
        source_hint: source_hint.clone(),
        // The old side has nothing to search for in the new diff -- removed
        // content is gone by definition, so a relocation can't tell where
        // it "moved" to. Dropped rather than carried through stale.
        old_range: None,
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
    Hunk(String, usize),
    Line {
        file: String,
        side: Side,
        line_start: u32,
        line_end: u32,
        /// The old-side sub-range this range also covers, if any -- see
        /// `Anchor::Span::old_range`. Always `None` when `relocated` is
        /// `Some`, since a relocation can't carry it through.
        old_range: Option<(u32, u32)>,
        /// `Some(new_anchor)` if this position came from a `Relocated`
        /// guess (not yet human-confirmed); `None` if the anchor's
        /// original position matched exactly (nothing to accept/reject).
        relocated: Option<Anchor>,
    },
    /// Anchor is a `Span` but couldn't be confidently placed in `diff`.
    Outdated {
        file: String,
    },
    /// Tier 1 found the anchor's text in the file, but not on a line the
    /// diff shows, so it can't be drawn inline. Distinct from `Outdated`
    /// (the text still exists); shown in the same unplaced section.
    OutsideDiff {
        file: String,
    },
}

pub fn find_file<'a>(diff: &'a UnifiedDiff, file: &str) -> Option<&'a FileDiff> {
    diff.files
        .iter()
        .find(|f| f.new_path.as_deref() == Some(file) || f.old_path.as_deref() == Some(file))
}

/// `new_files` holds the full new-side content of the version being viewed
/// (from the bundle's snapshot, or read at edit time), keyed by path. A
/// `Side::New` anchor whose file is in it is searched against the whole
/// file (Tier 1); everything else falls back to the diff's own visible
/// lines (Tier 2).
pub fn resolve_placement(
    anchor: &Anchor,
    diff: &UnifiedDiff,
    current_files: &[FileDigest],
    new_files: &crate::files::Tree,
) -> Placement {
    match anchor {
        Anchor::Global => Placement::Global,
        Anchor::File { file } => Placement::File(file.clone()),
        Anchor::Hunk { file, hunk_index } => Placement::Hunk(file.clone(), *hunk_index),
        Anchor::Span { file, side, .. } => {
            let Some(file_diff) = find_file(diff, file) else {
                return Placement::Outdated { file: file.clone() };
            };
            let full_text = match side {
                Side::New => file_diff
                    .new_path
                    .as_deref()
                    .and_then(|p| new_files.get(p))
                    .and_then(|b| std::str::from_utf8(b).ok()),
                Side::Old => None,
            };
            let corpus = match full_text {
                Some(text) => corpus_from_file_text(text),
                None => corpus_from_diff_hunks(file_diff, *side),
            };
            let current = digest_for(current_files, file, *side);
            let resolution = resolve(anchor, current, &corpus);
            if full_text.is_some() {
                let placed = match &resolution {
                    Resolution::Current => Some(anchor),
                    Resolution::Relocated(a) => Some(a),
                    Resolution::Outdated => None,
                };
                if let Some(Anchor::Span {
                    line_start,
                    line_end,
                    ..
                }) = placed
                {
                    let visible = corpus_from_diff_hunks(file_diff, Side::New);
                    if !(*line_start..=*line_end).all(|l| visible.iter().any(|v| v.line == l)) {
                        return Placement::OutsideDiff { file: file.clone() };
                    }
                }
            }
            match resolution {
                Resolution::Outdated => Placement::Outdated { file: file.clone() },
                Resolution::Current => {
                    let Anchor::Span {
                        file,
                        side,
                        line_start,
                        line_end,
                        old_range,
                        ..
                    } = anchor
                    else {
                        unreachable!()
                    };
                    Placement::Line {
                        file: file.clone(),
                        side: *side,
                        line_start: *line_start,
                        line_end: *line_end,
                        old_range: *old_range,
                        relocated: None,
                    }
                }
                Resolution::Relocated(new_anchor) => {
                    let Anchor::Span {
                        ref file,
                        side,
                        line_start,
                        line_end,
                        ..
                    } = new_anchor
                    else {
                        unreachable!("resolve() only relocates Span anchors")
                    };
                    Placement::Line {
                        file: file.clone(),
                        side,
                        line_start,
                        line_end,
                        old_range: None,
                        relocated: Some(new_anchor.clone()),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceHint;

    fn corpus(lines: &[(u32, &str)]) -> Vec<CorpusLine> {
        lines
            .iter()
            .map(|(n, c)| CorpusLine {
                line: *n,
                content: c.to_string(),
            })
            .collect()
    }

    fn anchor(line_start: u32, line_end: u32, target: &[&str]) -> Anchor {
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
    ) -> Anchor {
        Anchor::Span {
            file: "src/lib.rs".to_string(),
            side: Side::New,
            line_start,
            line_end,
            context: Context {
                before,
                target: target.iter().map(|s| s.to_string()).collect(),
                after,
            },
            origin_file_digest: digest.to_string(),
            source_hint: SourceHint::default(),
            old_range: None,
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
        let Resolution::Relocated(Anchor::Span {
            line_start,
            line_end,
            origin_file_digest,
            context,
            ..
        }) = resolved
        else {
            panic!("expected Relocated, got something else");
        };
        assert_eq!(line_start, 11);
        assert_eq!(line_end, 11);
        assert_eq!(origin_file_digest, "new-digest");
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
        let Resolution::Relocated(Anchor::Span { line_start, .. }) =
            resolve(&a, Some("new-digest"), &c)
        else {
            panic!("expected Relocated");
        };
        assert_eq!(line_start, 5);
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
        let Resolution::Relocated(Anchor::Span {
            line_start,
            context,
            ..
        }) = resolve(&a, Some("new-digest"), &c)
        else {
            panic!("expected Relocated");
        };
        assert_eq!(line_start, 2);
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
    fn global_file_and_hunk_anchors_are_always_current() {
        // v1 doesn't re-verify that a named file/hunk still exists; an
        // empty corpus proves resolve() isn't even trying to search.
        assert_eq!(
            resolve(&Anchor::Global, Some("any-digest"), &[]),
            Resolution::Current
        );
        assert_eq!(
            resolve(
                &Anchor::File {
                    file: "gone.rs".to_string()
                },
                Some("any-digest"),
                &[]
            ),
            Resolution::Current
        );
        assert_eq!(
            resolve(
                &Anchor::Hunk {
                    file: "gone.rs".to_string(),
                    hunk_index: 99
                },
                Some("any-digest"),
                &[]
            ),
            Resolution::Current
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
    fn context_for_line_range_grabs_surrounding_lines() {
        let c = corpus_from_file_text("a\nb\nc\nd\ne\nf\ng\n");
        let ctx = context_for_line_range(&c, 4, 5, 2).expect("range should be found");
        assert_eq!(ctx.before, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(ctx.target, vec!["d".to_string(), "e".to_string()]);
        assert_eq!(ctx.after, vec!["f".to_string(), "g".to_string()]);
    }

    #[test]
    fn context_for_line_range_is_none_when_the_range_is_not_present() {
        let c = corpus(&[(1, "a"), (2, "b"), (10, "c")]);
        // Lines 2 and 10 exist individually but aren't contiguous.
        assert_eq!(context_for_line_range(&c, 2, 10, 1), None);
        assert_eq!(context_for_line_range(&c, 5, 6, 1), None);
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

    fn tier1_files() -> crate::files::Tree {
        let text: String = (1..=10).map(|n| format!("l{n}\n")).collect();
        [("src/lib.rs".to_string(), text.into_bytes())].into()
    }

    #[test]
    fn full_text_finds_a_line_the_diff_shows_only_partially() {
        let diff = crate::diff::parse(TIER1_DIFF).unwrap();
        // `l3` used to be line 2; the new hunk shows it at line 3.
        let a = anchor(2, 2, &["l3"]);
        for files in [tier1_files(), Default::default()] {
            let p = resolve_placement(&a, &diff, &[], &files);
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
        let a = anchor(8, 8, &["l8"]);
        // The diff alone can't see line 8: nothing to say but "outdated".
        assert_eq!(
            resolve_placement(&a, &diff, &[], &Default::default()),
            Placement::Outdated {
                file: "src/lib.rs".into()
            }
        );
        // With the full file it is found, but there is no hunk to draw it in.
        assert_eq!(
            resolve_placement(&a, &diff, &[], &tier1_files()),
            Placement::OutsideDiff {
                file: "src/lib.rs".into()
            }
        );
        // Genuinely gone stays outdated even with the full file.
        let gone = anchor(8, 8, &["completely different"]);
        assert!(matches!(
            resolve_placement(&gone, &diff, &[], &tier1_files()),
            Placement::Outdated { .. }
        ));
    }
}
