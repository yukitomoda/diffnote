//! Following an anchor -- a line range of one immutable file version -- into
//! another version of the file, and deciding where a thread is drawn.
//!
//! Nothing here guesses. The bundle holds every version an anchor refers to,
//! so where a range went is a matter of the two versions' texts: the line
//! diff between them (through the intermediate versions some revision
//! recorded, when there are any) says which lines survived, which were
//! edited, and which were removed. [`locate`] follows a range that way;
//! [`resolve_placement`] uses it to place a thread in a view.

use crate::diff::{FileDiff, UnifiedDiff};
use crate::digest::Blobs;
use crate::model::{Anchor, FileDigest, LineRange, Side, TreeFile};

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

/// Where a range is in another version of its file. `len == 0` is a point
/// (see [`LineRange`]): either it was one already, or every line of the range
/// was removed and this is where they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Located {
    pub start: u32,
    pub len: u32,
}

/// Follows `range` to the version of its file with digest `target`. `None`
/// when a needed version isn't held (a damaged bundle).
pub fn locate(range: &LineRange, target: &str, blobs: &Blobs) -> Option<Located> {
    if range.digest == target {
        return Some(Located {
            start: range.start,
            len: range.len,
        });
    }
    let path = blobs.path(&range.digest, target)?;
    let (start, len) = path
        .windows(2)
        .try_fold((range.start, range.len), |(at, len), pair| {
            Some(map_range(&blobs.steps(&pair[0], &pair[1])?, at, len))
        })?;
    Some(Located { start, len })
}

/// Where the `len` lines starting at `start` (1-based) of `origin` are in
/// `current`, from the line diff between them:
/// - an unchanged line goes to its counterpart;
/// - an edited line goes to what replaced it (line for line when the edit
///   kept the number of lines, else the whole replacing block);
/// - a removed line goes nowhere.
///
/// The range becomes the span from the first to the last line any of its
/// lines went to, so lines added inside it are inside it. If none went
/// anywhere, the result is the point where they were (`len == 0`). A point
/// (`len == 0`) stays before the line it was before.
fn map_range(ops: &[similar::DiffOp], start: u32, len: u32) -> (u32, u32) {
    use similar::DiffOp;
    let first = start.saturating_sub(1) as usize;

    if len > 0 {
        let end = first + len as usize;
        let (mut lo, mut hi): (Option<usize>, Option<usize>) = (None, None);
        let mut cover = |from: usize, to: usize| {
            lo = Some(lo.map_or(from, |l| l.min(from)));
            hi = Some(hi.map_or(to, |h| h.max(to)));
        };
        for op in ops.iter() {
            let old = op.old_range();
            let (a, b) = (old.start.max(first), old.end.min(end));
            if a >= b {
                continue;
            }
            match *op {
                DiffOp::Equal { new_index, .. } => {
                    cover(new_index + (a - old.start), new_index + (b - old.start) - 1);
                }
                DiffOp::Replace {
                    new_index, new_len, ..
                } => {
                    if old.len() == new_len {
                        cover(new_index + (a - old.start), new_index + (b - old.start) - 1);
                    } else {
                        cover(new_index, new_index + new_len - 1);
                    }
                }
                DiffOp::Delete { .. } | DiffOp::Insert { .. } => {}
            }
        }
        if let (Some(lo), Some(hi)) = (lo, hi) {
            return (lo as u32 + 1, (hi - lo) as u32 + 1);
        }
    }
    (point_before(ops, first) as u32 + 1, 0)
}

/// The (0-based) new line the point before old line `first` sits before.
fn point_before(ops: &[similar::DiffOp], first: usize) -> usize {
    use similar::DiffOp;
    for op in ops {
        if !op.old_range().contains(&first) {
            continue;
        }
        return match *op {
            DiffOp::Equal {
                old_index,
                new_index,
                ..
            } => new_index + (first - old_index),
            // Inside an edited or removed block: where the block is now.
            DiffOp::Replace { new_index, .. } | DiffOp::Delete { new_index, .. } => new_index,
            DiffOp::Insert { new_index, .. } => new_index,
        };
    }
    // Past the last old line: right after wherever the last one went.
    ops.iter()
        .rev()
        .find(|op| !op.old_range().is_empty())
        .map_or(0, |op| op.new_range().end)
}

/// The file versions a view (a revision's diff) is between: the files its
/// diff touches with both digests, and the head tree the revision recorded
/// for the others.
pub struct ViewVersions<'a> {
    pub files: &'a [FileDigest],
    pub tree: &'a [TreeFile],
}

impl ViewVersions<'_> {
    fn entry(&self, file: &str) -> Option<&FileDigest> {
        self.files
            .iter()
            .find(|f| f.new_path.as_deref() == Some(file) || f.old_path.as_deref() == Some(file))
    }

    fn recorded(&self, file: &str) -> Option<&str> {
        self.tree
            .iter()
            .find(|f| f.path == file)
            .map(|f| f.digest.as_str())
    }

    /// The digest of `file` at the view's head. A file the diff doesn't touch
    /// is the same on both sides, and is in the revision's recorded tree.
    pub fn head(&self, file: &str) -> Option<&str> {
        match self.entry(file) {
            Some(e) => e.new.as_deref(),
            None => self.recorded(file),
        }
    }

    /// The digest of `file` at the view's base.
    pub fn base(&self, file: &str) -> Option<&str> {
        match self.entry(file) {
            Some(e) => e.old.as_deref(),
            None => self.recorded(file),
        }
    }
}

/// Why a view's head has none of the lines a thread is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Absence {
    /// The lines existed in an earlier version and were removed since.
    Deleted,
    /// The lines only appear in a later version: the view is from before them.
    NotYet,
    /// The recorded revisions don't say which came first.
    Unknown,
}

/// Where a thread is drawn in a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Global,
    File(String),
    /// On lines of the view's diff.
    Line {
        file: String,
        /// The side the card is drawn on: the head side while the anchor's
        /// lines still exist there, else the base side (they were removed by
        /// this diff).
        side: Side,
        line_start: u32,
        line_end: u32,
        /// The base-side lines the anchor also covers when `side` is `New`
        /// (e.g. a replaced block), so both get highlighted.
        old_range: Option<(u32, u32)>,
    },
    /// The view's head has none of the lines the thread is about: the thread
    /// sits at the point where they are (or were), before head line
    /// `before`, and `was` is what they say.
    Point {
        file: String,
        before: u32,
        was: Vec<String>,
        /// Why the head doesn't have them.
        kind: Absence,
    },
    /// The versions needed to place it aren't in the bundle.
    Unplaced { file: String },
}

pub fn find_file<'a>(diff: &'a UnifiedDiff, file: &str) -> Option<&'a FileDiff> {
    diff.files
        .iter()
        .find(|f| f.new_path.as_deref() == Some(file) || f.old_path.as_deref() == Some(file))
}

/// The text of the range's lines, from the version it is a range of.
pub fn text_of(range: &LineRange, blobs: &Blobs) -> Vec<String> {
    let Some(text) = blobs.text(&range.digest) else {
        return Vec::new();
    };
    text.lines()
        .skip(range.start as usize - 1)
        .take(range.len as usize)
        .map(str::to_string)
        .collect()
}

/// The lines a thread was written about, as they were then: the first
/// non-empty range of its anchor (head side first).
pub fn original_text(anchor: &Anchor, blobs: &Blobs) -> Vec<String> {
    match anchor {
        Anchor::Span { base, head } => head
            .iter()
            .chain(base.iter())
            .find(|r| !r.is_empty())
            .map(|r| text_of(r, blobs))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Places `anchor` in the view described by `diff` and `view`.
///
/// A range is followed into both versions of the view. It is drawn on the
/// head side if it still has lines there (looking first at the anchor's own
/// head range, then its base range), else on the base side (the diff removed
/// them: they show as removed lines), else -- everything it covered was
/// removed at some point -- as a point where they were.
pub fn resolve_placement(
    anchor: &Anchor,
    diff: &UnifiedDiff,
    view: &ViewVersions,
    blobs: &Blobs,
) -> Placement {
    match anchor {
        Anchor::Global { .. } => Placement::Global,
        Anchor::File { base, head } => Placement::File(
            head.as_ref()
                .or(base.as_ref())
                .map(|f| f.file.clone())
                .unwrap_or_default(),
        ),
        Anchor::Span { base, head } => {
            let ranges: Vec<&LineRange> = head.iter().chain(base.iter()).collect();
            let label = ranges.first().map(|r| r.file.clone()).unwrap_or_default();
            let file_diff = ranges.iter().find_map(|r| find_file(diff, &r.file));
            let (head_path, base_path) = match file_diff {
                Some(f) => (f.new_path.as_deref(), f.old_path.as_deref()),
                None => (Some(label.as_str()), Some(label.as_str())),
            };
            let head_digest = head_path.and_then(|p| view.head(p));
            let base_digest = base_path.and_then(|p| view.base(p));

            let non_empty = |rs: &[Option<&LineRange>]| -> Vec<LineRange> {
                rs.iter()
                    .flatten()
                    .filter(|r| !r.is_empty())
                    .map(|r| (*r).clone())
                    .collect()
            };
            let head_first = non_empty(&[head.as_ref(), base.as_ref()]);
            let base_first = non_empty(&[base.as_ref(), head.as_ref()]);

            let into = |ranges: &[LineRange], digest: Option<&str>| -> (Option<Located>, Option<Located>) {
                let mut lines = None;
                let mut point = None;
                for r in ranges {
                    let Some(l) = digest.and_then(|d| locate(r, d, blobs)) else {
                        continue;
                    };
                    if l.len > 0 {
                        lines = Some(l);
                        break;
                    }
                    point.get_or_insert(l);
                }
                (lines, point)
            };
            let (new_lines, new_point) = into(&head_first, head_digest);
            let (old_lines, _) = into(&base_first, base_digest);

            // Lines the diff doesn't show are still lines: they get drawn with
            // context around them (see `expand`), not dropped.
            let head_file = |fallback: String| {
                file_diff.and_then(|f| f.new_path.clone()).unwrap_or(fallback)
            };
            let base_file = |fallback: String| {
                file_diff.and_then(|f| f.old_path.clone()).unwrap_or(fallback)
            };
            let old_range = old_lines.map(|l| (l.start, l.start + l.len - 1));
            if let Some(l) = new_lines {
                return Placement::Line {
                    file: head_file(label),
                    side: Side::New,
                    line_start: l.start,
                    line_end: l.start + l.len - 1,
                    old_range,
                };
            }
            if let Some((start, end)) = old_range {
                return Placement::Line {
                    file: base_file(label),
                    side: Side::Old,
                    line_start: start,
                    line_end: end,
                    old_range: None,
                };
            }
            if let Some(p) = new_point {
                let origin = head_first.first().map(|r| r.digest.as_str());
                let kind = match (origin, head_digest) {
                    (Some(o), Some(h)) if blobs.precedes(o, h) => Absence::Deleted,
                    (Some(o), Some(h)) if blobs.precedes(h, o) => Absence::NotYet,
                    _ => Absence::Unknown,
                };
                return Placement::Point {
                    file: head_file(label),
                    before: p.start,
                    was: original_text(anchor, blobs),
                    kind,
                };
            }
            Placement::Unplaced { file: label }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::digest;

    /// Lines of `text`, numbered from 1, as a tuple for terse assertions.
    fn map(origin: &str, current: &str, start: u32, len: u32) -> (u32, u32) {
        map_range(&crate::linediff::line_diff(origin, current), start, len)
    }

    // ---- unchanged and shifted lines --------------------------------------

    #[test]
    fn identical_texts_keep_every_range() {
        let t = "a\nb\nc\nd\n";
        for (start, len) in [(1, 1), (2, 2), (1, 4), (4, 1)] {
            assert_eq!(map(t, t, start, len), (start, len));
        }
    }

    #[test]
    fn lines_added_above_shift_a_range_down() {
        assert_eq!(map("a\nb\nc\n", "x\ny\na\nb\nc\n", 2, 2), (4, 2));
    }

    #[test]
    fn lines_added_below_leave_a_range_alone() {
        assert_eq!(map("a\nb\nc\n", "a\nb\nc\nx\ny\n", 2, 1), (2, 1));
    }

    #[test]
    fn lines_added_inside_a_range_grow_it() {
        let (old, new) = ("a\nb\nc\nd\ne\n", "a\nb\nX\nY\nc\nd\ne\n");
        assert_eq!(map(old, new, 2, 3), (2, 5), "b..d now spans b X Y c d");
    }

    #[test]
    fn lines_removed_inside_a_range_shrink_it() {
        assert_eq!(map("a\nb\nc\nd\ne\n", "a\nb\nd\ne\n", 2, 3), (2, 2));
    }

    #[test]
    fn lines_removed_at_the_ends_of_a_range_shrink_it_to_what_is_left() {
        let old = "a\nb\nc\nd\ne\n";
        assert_eq!(map(old, "a\nc\nd\ne\n", 2, 3), (2, 2), "the first line went");
        assert_eq!(map(old, "a\nb\nc\ne\n", 2, 3), (2, 2), "the last line went");
        assert_eq!(map(old, "a\nd\ne\n", 2, 3), (2, 1), "only d is left");
    }

    // ---- edited lines ------------------------------------------------------

    #[test]
    fn an_edited_line_goes_to_what_replaced_it() {
        assert_eq!(map("a\nb\nc\n", "a\nB\nc\n", 2, 1), (2, 1));
        // ...even when the new text is nothing like the old.
        assert_eq!(map("a\nb\nc\n", "a\n1234567890\nc\n", 2, 1), (2, 1));
    }

    #[test]
    fn an_edit_that_kept_the_number_of_lines_maps_line_for_line() {
        let (old, new) = ("a\nb1\nb2\nb3\nz\n", "a\nB1\nB2\nB3\nz\n");
        assert_eq!(map(old, new, 3, 1), (3, 1), "b2 -> B2");
        assert_eq!(map(old, new, 3, 2), (3, 2), "b2..b3 -> B2..B3");
    }

    #[test]
    fn an_edit_that_changed_the_number_of_lines_maps_to_the_whole_new_block() {
        let (old, new) = ("a\nb1\nb2\nz\n", "a\nX\nY\nW\nz\n");
        assert_eq!(map(old, new, 2, 1), (2, 3), "b1 -> the block X Y W");
        assert_eq!(map(old, new, 3, 1), (2, 3), "b2 -> the block X Y W");
    }

    #[test]
    fn a_range_reaching_from_unchanged_lines_into_an_edit_covers_both() {
        let (old, new) = ("a\nb\nc\nd\n", "a\nb\nCC\nDD\nEE\n");
        // b (unchanged) .. c (edited into the 3-line block) -> b..EE.
        assert_eq!(map(old, new, 2, 2), (2, 4));
    }

    #[test]
    fn a_range_whose_lines_were_all_removed_becomes_the_point_where_they_were() {
        let (old, new) = ("a\nb\nc\nd\n", "a\nd\n");
        // b..c gone: the point between a and d, i.e. before line 2.
        assert_eq!(map(old, new, 2, 2), (2, 0));
        // Removal at the top and at the bottom.
        assert_eq!(map("a\nb\nc\n", "c\n", 1, 2), (1, 0));
        assert_eq!(map("a\nb\nc\n", "a\nb\n", 3, 1), (3, 0), "before the line past the end");
    }

    #[test]
    fn removed_lines_are_placed_where_the_script_really_is() {
        // similar reports the first removal of this diff at new line 1; the
        // point is before new line 1 (line 0 of the new text has nothing).
        assert_eq!(map("v2\nv2\nv1\nv2\n", "v1\nv1\n", 1, 2), (1, 0));
    }

    #[test]
    fn a_whole_file_replaced_maps_to_the_whole_new_file() {
        assert_eq!(map("a\nb\n", "x\ny\nz\n", 1, 2), (1, 3));
        assert_eq!(map("a\nb\n", "", 1, 2), (1, 0), "or to nothing");
        assert_eq!(map("", "a\nb\n", 1, 0), (1, 0), "an empty file's only point");
    }

    // ---- points ------------------------------------------------------------

    #[test]
    fn a_point_stays_before_the_line_it_was_before() {
        let old = "a\nb\nc\n";
        assert_eq!(map(old, "x\na\nb\nc\n", 2, 0), (3, 0), "shifted with `b`");
        assert_eq!(map(old, "a\nb\nc\nx\n", 2, 0), (2, 0));
        assert_eq!(map(old, "a\nx\ny\nb\nc\n", 2, 0), (4, 0), "after what was inserted at it");
    }

    #[test]
    fn a_point_whose_next_line_was_edited_or_removed_goes_where_that_block_is() {
        let old = "a\nb\nc\nd\n";
        assert_eq!(map(old, "a\nB\nc\nd\n", 2, 0), (2, 0), "b edited in place");
        assert_eq!(map(old, "a\nc\nd\n", 2, 0), (2, 0), "b removed: before c");
        assert_eq!(map(old, "a\nd\n", 2, 0), (2, 0), "b, c removed: before d");
    }

    #[test]
    fn a_point_at_the_end_of_the_file_follows_the_last_line() {
        // Before "line 4", which doesn't exist: after `c`.
        assert_eq!(map("a\nb\nc\n", "a\nb\nc\n", 4, 0), (4, 0));
        assert_eq!(map("a\nb\nc\n", "x\na\nb\nc\n", 4, 0), (5, 0));
        assert_eq!(map("a\nb\nc\n", "a\nb\nq\n", 4, 0), (4, 0), "c was edited");
        assert_eq!(map("a\nb\nc\n", "a\nb\n", 4, 0), (3, 0), "c was removed");
    }

    // ---- shape of the answer ----------------------------------------------

    #[test]
    fn a_line_without_a_trailing_newline_is_still_a_line() {
        assert_eq!(map("a\nb", "a\nb", 2, 1), (2, 1));
        assert_eq!(map("a\nb", "x\na\nb", 2, 1), (3, 1));
        assert_eq!(map("a\nb\n", "a\nb", 2, 1), (2, 1), "adding or losing the final newline");
    }

    // ---- randomized: against an oracle -----------------------------------

    /// A tiny deterministic generator, so a failure is reproducible.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, below: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % below.max(1)
        }
    }

    /// Old lines have unique text `o<i>`; an edit is applied to a list of
    /// `Option<usize>` (the old line an entry is, or `None` for an inserted
    /// one), so where every surviving old line went is known exactly.
    fn render(entries: &[Option<usize>]) -> String {
        entries
            .iter()
            .enumerate()
            .map(|(i, e)| match e {
                Some(old) => format!("o{old}\n"),
                None => format!("new{i}\n"),
            })
            .collect()
    }

    #[test]
    fn insertions_and_removals_apart_from_each_other_follow_an_exact_oracle() {
        let mut rng = Lcg(0xD1FF);
        for round in 0..400 {
            let old_len = 1 + rng.next(12);
            let old: String = (0..old_len).map(|i| format!("o{i}\n")).collect();
            // Build the new version: keep, drop (only where the neighbours
            // are kept, so no removal touches an insertion), or insert.
            let mut entries: Vec<Option<usize>> = Vec::new();
            let mut last_touched = false;
            for i in 0..old_len {
                let touched_now = match rng.next(6) {
                    0 if !last_touched => true, // drop this line
                    _ => false,
                };
                if rng.next(4) == 0 && !touched_now && !last_touched {
                    for _ in 0..1 + rng.next(3) {
                        entries.push(None);
                    }
                }
                if !touched_now {
                    entries.push(Some(i));
                }
                last_touched = touched_now;
            }
            let new = render(&entries);

            let start = 1 + rng.next(old_len) as u32;
            let len = 1 + rng.next((old_len as u32 - start + 1) as usize) as u32;
            let got = map(&old, &new, start, len);

            // Oracle: where the surviving lines of the range are.
            let positions: Vec<usize> = entries
                .iter()
                .enumerate()
                .filter_map(|(at, e)| match e {
                    Some(o) if *o + 1 >= start as usize && *o + 1 < (start + len) as usize => {
                        Some(at)
                    }
                    _ => None,
                })
                .collect();
            let context = format!("round {round}: {old:?} -> {new:?}, range {start}+{len}");
            if let (Some(lo), Some(hi)) = (positions.first(), positions.last()) {
                assert_eq!(got, (*lo as u32 + 1, (hi - lo) as u32 + 1), "{context}");
            } else {
                // Everything in the range was removed: a point, and it sits
                // between the last surviving line before the range and the
                // first surviving line after it (inserted lines between are
                // fine either side).
                assert_eq!(got.1, 0, "{context}");
                let before = entries
                    .iter()
                    .rposition(|e| matches!(e, Some(o) if *o + 1 < start as usize));
                let after = entries
                    .iter()
                    .position(|e| matches!(e, Some(o) if *o + 1 >= (start + len) as usize));
                let lo = before.map_or(1, |b| b as u32 + 2);
                let hi = after.map_or(entries.len() as u32 + 1, |a| a as u32 + 1);
                assert!((lo..=hi).contains(&got.0), "{context}: point {} not in {lo}..={hi}", got.0);
            }
        }
    }

    #[test]
    fn a_range_is_always_inside_the_new_text() {
        // Whatever the edit, the answer is a real place in the new file.
        let mut rng = Lcg(0xBEEF);
        for _ in 0..500 {
            let mk = |rng: &mut Lcg| -> String {
                (0..rng.next(10))
                    .map(|_| format!("v{}\n", rng.next(5)))
                    .collect()
            };
            let (old, new) = (mk(&mut rng), mk(&mut rng));
            let old_len = old.lines().count() as u32;
            let new_len = new.lines().count() as u32;
            let start = 1 + rng.next(old_len as usize + 1) as u32;
            let len = rng.next((old_len + 2 - start) as usize) as u32;
            let (s, l) = map(&old, &new, start, len);
            assert!(s >= 1, "{old:?} -> {new:?} {start}+{len}: start {s}");
            assert!(
                s + l <= new_len + 1,
                "{old:?} -> {new:?} {start}+{len}: ({s},{l}) runs past {new_len} lines"
            );
        }
    }

    // ---- locate: chains, digests, missing versions --------------------------

    fn range_in(text: &str, start: u32, len: u32) -> LineRange {
        LineRange {
            file: "f.txt".into(),
            digest: digest(text),
            start,
            len,
        }
    }

    fn blobs_of<'a>(texts: &[&'a str], linked: bool) -> Blobs<'a> {
        let mut blobs = Blobs::default();
        for t in texts {
            blobs.add(t.as_bytes());
        }
        if linked {
            for pair in texts.windows(2) {
                blobs.link(&digest(pair[0]), &digest(pair[1]));
            }
        }
        blobs
    }

    #[test]
    fn locating_in_the_same_version_is_the_identity_and_needs_no_text() {
        let r = range_in("a\nb\n", 2, 1);
        assert_eq!(
            locate(&r, &digest("a\nb\n"), &Blobs::default()),
            Some(Located { start: 2, len: 1 })
        );
    }

    #[test]
    fn locating_needs_both_versions() {
        let r = range_in("a\nb\n", 2, 1);
        assert_eq!(locate(&r, &digest("x\n"), &blobs_of(&["a\nb\n"], false)), None);
        assert_eq!(locate(&r, &digest("x\n"), &blobs_of(&["x\n"], false)), None);
    }

    #[test]
    fn locating_follows_the_recorded_steps_and_each_can_change_the_length() {
        let texts = [
            "a\nb\nc\nd\n",
            "a\nb\nX\nc\nd\n",      // a line inside b..c: grows to b..c (3)
            "top\na\nb\nX\nc\nd\n",  // pushed down by one
            "top\na\nb\nc\nd\n",     // X removed: shrinks back
        ];
        let r = range_in(texts[0], 2, 2);
        let last = digest(texts[3]);
        for linked in [true, false] {
            assert_eq!(
                locate(&r, &last, &blobs_of(&texts, linked)),
                Some(Located { start: 3, len: 2 }),
                "linked: {linked}"
            );
        }
    }

    #[test]
    fn locating_can_go_backwards_to_an_older_version() {
        let (old, new) = ("a\nb\nc\n", "x\na\nb\nc\n");
        // Written against the newer version, viewed at the older.
        assert_eq!(
            locate(&range_in(new, 3, 1), &digest(old), &blobs_of(&[old, new], true)),
            Some(Located { start: 2, len: 1 })
        );
        // A line the older version doesn't have: where it would be.
        assert_eq!(
            locate(&range_in(new, 1, 1), &digest(old), &blobs_of(&[old, new], true)),
            Some(Located { start: 1, len: 0 })
        );
    }

    #[test]
    fn a_range_removed_along_the_way_stays_a_point_after_later_steps() {
        let texts = ["a\nb\nc\n", "a\nc\n", "z\na\nc\n"];
        let r = range_in(texts[0], 2, 1);
        assert_eq!(
            locate(&r, &digest(texts[2]), &blobs_of(&texts, true)),
            Some(Located { start: 3, len: 0 })
        );
    }

    // ---- placement in a view -------------------------------------------------

    /// One view: the diff between `old` and `new` of a single file `f.txt`,
    /// and every text given is held.
    struct View {
        diff: UnifiedDiff,
        files: Vec<FileDigest>,
        held: Vec<String>,
    }

    fn view(old: &str, new: &str, also_held: &[&str]) -> View {
        let tree = |t: &str| -> crate::files::Tree {
            [("f.txt".to_string(), t.as_bytes().to_vec())].into()
        };
        let (diff_text, files) = crate::files::diff_trees(&tree(old), &tree(new));
        View {
            diff: crate::diff::parse(&diff_text).unwrap(),
            files,
            held: [old, new]
                .iter()
                .chain(also_held)
                .map(|s| s.to_string())
                .collect(),
        }
    }

    fn place(v: &View, anchor: &Anchor) -> Placement {
        let mut blobs = Blobs::default();
        for t in &v.held {
            blobs.add(t.as_bytes());
        }
        // Every held text is a step from the previous one, in the order given.
        for pair in v.held.windows(2) {
            blobs.link(&digest(&pair[0]), &digest(&pair[1]));
        }
        resolve_placement(
            anchor,
            &v.diff,
            &ViewVersions {
                files: &v.files,
                tree: &[],
            },
            &blobs,
        )
    }

    fn span(base: Option<LineRange>, head: Option<LineRange>) -> Anchor {
        Anchor::Span { base, head }
    }

    fn line(side: Side, start: u32, end: u32, old: Option<(u32, u32)>) -> Placement {
        Placement::Line {
            file: "f.txt".into(),
            side,
            line_start: start,
            line_end: end,
            old_range: old,
        }
    }

    const OLD: &str = "a\nb\nc\nd\ne\n";
    const NEW: &str = "a\nB\nc\nd\nE\ne2\n";

    #[test]
    fn a_thread_is_drawn_on_the_head_lines_where_the_anchor_is_current() {
        let v = view(OLD, NEW, &[]);
        // `B` replaced `b`: both sides.
        let a = span(Some(range_in(OLD, 2, 1)), Some(range_in(NEW, 2, 1)));
        assert_eq!(place(&v, &a), line(Side::New, 2, 2, Some((2, 2))));
        // `e` was replaced by `E` and `e2`: a comment on the extra line is on
        // the whole replacing block, so it is tied to `e` on the old side.
        let a = span(Some(range_in(OLD, 5, 0)), Some(range_in(NEW, 6, 1)));
        assert_eq!(place(&v, &a), line(Side::New, 6, 6, Some((5, 5))));
    }

    #[test]
    fn a_purely_added_line_has_no_base_lines_to_highlight() {
        let (old, new) = ("a\nb\nc\n", "a\nb\nc\nd\n");
        let v = view(old, new, &[]);
        let a = span(Some(range_in(old, 4, 0)), Some(range_in(new, 4, 1)));
        assert_eq!(place(&v, &a), line(Side::New, 4, 4, None));
    }

    #[test]
    fn the_anchors_head_range_decides_the_head_placement_before_its_base_range() {
        // The comment was made on `b` -> `B` (revision v0 -> v1). The view is
        // v1 -> v2, where a line `X` was added right above `B`. The head
        // range (`B`) is at line 3 of v2; the base range (`b`), followed to
        // v2, is the whole block X B (lines 2-3). The head range wins.
        let (v0, v1, v2) = ("a\nb\nc\n", "a\nB\nc\n", "a\nX\nB\nc\n");
        // (v0 is only linked to v2, so its range is followed in one jump.)
        let view = View {
            held: vec![v0.into(), v2.into(), v1.into()],
            ..view(v1, v2, &[])
        };
        let a = span(Some(range_in(v0, 2, 1)), Some(range_in(v1, 2, 1)));
        assert_eq!(place(&view, &a), line(Side::New, 3, 3, Some((2, 2))));
    }

    #[test]
    fn a_thread_on_removed_lines_is_drawn_on_the_base_side_of_the_view() {
        let old = "a\nb\nc\nd\n";
        let new = "a\nd\n";
        let v = view(old, new, &[]);
        let a = span(Some(range_in(old, 2, 2)), Some(range_in(new, 2, 0)));
        assert_eq!(place(&v, &a), line(Side::Old, 2, 3, None));
    }

    #[test]
    fn a_thread_on_the_base_side_of_a_later_change_shows_on_the_head_side_of_an_earlier_view() {
        // Revision 1: c1 -> c2 adds `b`. Revision 2: c2 -> c3 removes it,
        // and the reviewer commented on that removed line.
        let (c1, c2, c3) = ("a\nc\n", "a\nb\nc\n", "a\nc\n");
        let comment_on_removed = span(Some(range_in(c2, 2, 1)), Some(range_in(c3, 2, 0)));
        // In revision 2's own view: the removed row.
        let v2 = view(c2, c3, &[]);
        assert_eq!(place(&v2, &comment_on_removed), line(Side::Old, 2, 2, None));
        // In revision 1's view, `b` is an added line -- on the *head* side.
        let v1 = view(c1, c2, &[c3]);
        assert_eq!(place(&v1, &comment_on_removed), line(Side::New, 2, 2, None));
    }

    #[test]
    fn a_thread_whose_lines_are_gone_from_the_head_is_a_point_with_what_it_said() {
        // The comment was on `b` in c2; c3 (the view's head) has no `b`.
        let (c2, c3, c4) = ("a\nb\nc\n", "a\nc\n", "a\nc\nd\n");
        let anchor = span(None, Some(range_in(c2, 2, 1)));
        let v = view(c3, c4, &[c2]);
        // c2 is an older version linked before c3 in `held` order c3,c4,c2? Put
        // the chain in order so c2 -> c3 -> c4 exists.
        let v = View {
            held: vec![c2.into(), c3.into(), c4.into()],
            ..v
        };
        assert_eq!(
            place(&v, &anchor),
            Placement::Point {
                file: "f.txt".into(),
                before: 2,
                was: vec!["b".to_string()],
                kind: Absence::Deleted,
            }
        );
    }

    #[test]
    fn the_same_point_is_not_yet_in_an_earlier_view_and_unknown_when_unrelated() {
        let (c1, c2) = ("a\nc\n", "a\nb\nc\n");
        // A comment on `b`, which c2 added; the view's head is c1 (a file the
        // view's diff doesn't touch, known from the revision's tree).
        let anchor = span(None, Some(range_in(c2, 2, 1)));
        let tree = [TreeFile {
            path: "f.txt".into(),
            digest: digest(c1),
        }];
        let placed = |linked: bool| {
            let mut blobs = Blobs::default();
            blobs.add(c1.as_bytes());
            blobs.add(c2.as_bytes());
            if linked {
                blobs.link(&digest(c1), &digest(c2));
            }
            resolve_placement(
                &anchor,
                &UnifiedDiff::default(),
                &ViewVersions {
                    files: &[],
                    tree: &tree,
                },
                &blobs,
            )
        };
        let Placement::Point { kind, was, before, .. } = placed(true) else {
            panic!("expected a point")
        };
        assert_eq!((kind, was, before), (Absence::NotYet, vec!["b".to_string()], 2));
        // With no recorded step between the versions, all that is known is
        // that the lines aren't in this one.
        assert!(matches!(placed(false), Placement::Point { kind: Absence::Unknown, .. }));
    }

    #[test]
    fn a_thread_on_lines_the_diff_does_not_show_is_placed_all_the_same() {
        // Line 3 is nowhere near the only change (line 30): the diff says
        // nothing about it, and that is no reason not to place it.
        let old: String = (1..=30).map(|n| format!("l{n}\n")).collect();
        let new = old.replace("l30\n", "L30\n");
        let v = view(&old, &new, &[]);
        let a = span(Some(range_in(&old, 3, 1)), Some(range_in(&new, 3, 1)));
        assert_eq!(place(&v, &a), line(Side::New, 3, 3, Some((3, 3))));
    }

    #[test]
    fn a_thread_whose_version_is_missing_is_unplaced() {
        let v = view(OLD, NEW, &[]);
        let a = span(None, Some(range_in("something else entirely\n", 1, 1)));
        assert_eq!(place(&v, &a), Placement::Unplaced { file: "f.txt".into() });
    }

    #[test]
    fn a_file_the_view_deletes_keeps_its_threads_on_the_removed_lines() {
        let v = view("a\nb\n", "", &[]);
        let a = span(Some(range_in("a\nb\n", 2, 1)), None);
        assert_eq!(place(&v, &a), line(Side::Old, 2, 2, None));
    }

    #[test]
    fn a_file_the_view_adds_places_head_threads() {
        let v = view("", "a\nb\n", &[]);
        let a = span(None, Some(range_in("a\nb\n", 2, 1)));
        assert_eq!(place(&v, &a), line(Side::New, 2, 2, None));
    }

    #[test]
    fn file_level_and_global_anchors_are_placed_without_looking_at_any_text() {
        let v = view(OLD, NEW, &[]);
        let global = Anchor::Global {
            base: None,
            head: Some("rev".into()),
        };
        assert_eq!(place(&v, &global), Placement::Global);
        let file = Anchor::File {
            base: None,
            head: Some(crate::model::FileRef {
                file: "gone.rs".into(),
                digest: "d".into(),
            }),
        };
        assert_eq!(place(&v, &file), Placement::File("gone.rs".into()));
    }

    #[test]
    fn untouched_files_are_looked_up_in_the_revisions_recorded_tree() {
        let readme = "# title\nbody\n";
        let v = view(OLD, NEW, &[readme]);
        let mut blobs = Blobs::default();
        for t in &v.held {
            blobs.add(t.as_bytes());
        }
        let tree = [TreeFile {
            path: "README.md".into(),
            digest: digest(readme),
        }];
        let a = Anchor::Span {
            base: None,
            head: Some(LineRange {
                file: "README.md".into(),
                digest: digest(readme),
                start: 2,
                len: 1,
            }),
        };
        let versions = ViewVersions {
            files: &v.files,
            tree: &tree,
        };
        // Found by its recorded digest, though the diff never mentions it.
        assert_eq!(
            resolve_placement(&a, &v.diff, &versions, &blobs),
            Placement::Line {
                file: "README.md".into(),
                side: Side::New,
                line_start: 2,
                line_end: 2,
                // Unchanged, so the same lines on the base side too.
                old_range: Some((2, 2)),
            }
        );
        // Without the tree entry the version at this revision is unknown.
        let none = ViewVersions {
            files: &v.files,
            tree: &[],
        };
        assert_eq!(
            resolve_placement(&a, &v.diff, &none, &blobs),
            Placement::Unplaced {
                file: "README.md".into()
            }
        );
    }

    // ---- small helpers -----------------------------------------------------

    #[test]
    fn digest_for_finds_a_file_by_either_path_and_side() {
        let files = vec![FileDigest {
            old_path: Some("old.rs".to_string()),
            new_path: Some("new.rs".to_string()),
            old: Some("sha256:old".to_string()),
            new: Some("sha256:new".to_string()),
        }];
        assert_eq!(digest_for(&files, "new.rs", Side::New), Some("sha256:new"));
        assert_eq!(digest_for(&files, "old.rs", Side::Old), Some("sha256:old"));
        assert_eq!(digest_for(&files, "old.rs", Side::New), Some("sha256:new"));
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
    fn original_text_is_what_the_first_non_empty_range_covered() {
        let t = "a\nb\nc\nd\n";
        let blobs = blobs_of(&[t], false);
        let head = span(None, Some(range_in(t, 2, 2)));
        assert_eq!(original_text(&head, &blobs), ["b", "c"]);
        // Head empty: the base range.
        let removed = span(Some(range_in(t, 3, 1)), Some(range_in("x\n", 1, 0)));
        assert_eq!(original_text(&removed, &blobs), ["c"]);
        // Not held: nothing to show.
        assert!(original_text(&span(None, Some(range_in("lost\n", 1, 1))), &blobs).is_empty());
        assert!(original_text(&Anchor::Global { base: None, head: None }, &blobs).is_empty());
    }

    #[test]
    fn view_versions_prefer_the_diffs_digests_over_the_recorded_tree() {
        let files = vec![FileDigest {
            old_path: Some("a".into()),
            new_path: Some("a".into()),
            old: Some("sha256:a-old".into()),
            new: Some("sha256:a-new".into()),
        }];
        let tree = [
            TreeFile {
                path: "a".into(),
                digest: "sha256:ignored".into(),
            },
            TreeFile {
                path: "b".into(),
                digest: "sha256:b".into(),
            },
        ];
        let v = ViewVersions {
            files: &files,
            tree: &tree,
        };
        assert_eq!(v.head("a"), Some("sha256:a-new"));
        assert_eq!(v.base("a"), Some("sha256:a-old"));
        // Untouched: the same on both sides.
        assert_eq!((v.head("b"), v.base("b")), (Some("sha256:b"), Some("sha256:b")));
        assert_eq!(v.head("c"), None);
    }
}
