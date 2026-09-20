//! Showing what threads are about even where the diff says nothing.
//!
//! A thread is placed by where its lines are in the file (see `anchor`), not
//! by whether the diff happens to show them. So a view (the HTML export, the
//! `edit` buffer) draws its diff *plus* a few lines of context around every
//! placed thread that the diff doesn't show, taken from the full text of the
//! version being viewed -- and, for a file the diff doesn't touch at all, a
//! file that is nothing but that context.
//!
//! [`expand`] does that to a parsed diff; [`to_text`] writes the result back
//! out as unified diff text (for the `edit` buffer, which the annotation
//! parser reads again).

use crate::anchor::{Placement, ViewVersions};
use crate::diff::{DiffLine, FileDiff, Hunk, LineKind, UnifiedDiff};
use crate::digest::Blobs;
use crate::model::Side;
use std::collections::BTreeSet;

/// Lines of context drawn on each side of a thread's lines.
pub const CONTEXT_RADIUS: u32 = 3;

/// What a placed thread needs the view to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    pub file: String,
    /// The lines (inclusive) on `side`, or `None` for the file itself.
    pub lines: Option<(Side, u32, u32)>,
    /// The row the thread's card is drawn after, when it has lines.
    pub row: Option<(Side, u32)>,
}

/// What `placement` needs shown, if it is drawn on the view at all.
pub fn want_of(placement: &Placement) -> Option<Want> {
    match placement {
        Placement::Line {
            file,
            side,
            line_start,
            line_end,
            ..
        } => Some(Want {
            file: file.clone(),
            lines: Some((*side, *line_start, *line_end)),
            row: Some((*side, *line_end)),
        }),
        // The lines on each side of the point; drawn after the one before it
        // (after the first line, for a point at the very top).
        Placement::Point { file, before, .. } => Some(Want {
            file: file.clone(),
            lines: Some((Side::New, before.saturating_sub(1).max(1), *before)),
            row: Some((Side::New, before.saturating_sub(1).max(1))),
        }),
        Placement::File(file) => Some(Want {
            file: file.clone(),
            lines: None,
            row: None,
        }),
        Placement::Global | Placement::Unplaced { .. } => None,
    }
}

impl Hunk {
    /// First line of the hunk on the old side (1-based); for a hunk with no
    /// old lines, the line an insertion there sits before.
    fn old_first(&self) -> u32 {
        if self.old_lines == 0 {
            self.old_start + 1
        } else {
            self.old_start
        }
    }

    fn new_first(&self) -> u32 {
        if self.new_lines == 0 {
            self.new_start + 1
        } else {
            self.new_start
        }
    }

    fn old_next(&self) -> u32 {
        self.old_first() + self.old_lines
    }

    fn new_next(&self) -> u32 {
        self.new_first() + self.new_lines
    }

    /// A hunk made of `lines`, whose first old/new lines are `old_first` /
    /// `new_first`.
    fn from_lines(
        lines: Vec<DiffLine>,
        old_first: u32,
        new_first: u32,
        section_heading: Option<String>,
    ) -> Hunk {
        let old_lines = lines.iter().filter(|l| l.kind != LineKind::Added).count() as u32;
        let new_lines = lines.iter().filter(|l| l.kind != LineKind::Removed).count() as u32;
        Hunk {
            old_start: if old_lines == 0 {
                old_first - 1
            } else {
                old_first
            },
            old_lines,
            new_start: if new_lines == 0 {
                new_first - 1
            } else {
                new_first
            },
            new_lines,
            section_heading,
            lines,
        }
    }
}

/// The old line the (unchanged) new line `new` is, given the hunks.
fn new_to_old(hunks: &[Hunk], new: u32) -> u32 {
    let shift: i64 = hunks
        .iter()
        .filter(|h| h.new_next() <= new)
        .map(|h| h.new_lines as i64 - h.old_lines as i64)
        .sum();
    (new as i64 - shift) as u32
}

/// The new line the (unchanged) old line `old` is, or `None` if a hunk
/// covers it (then it is shown already).
fn old_to_new(hunks: &[Hunk], old: u32) -> Option<u32> {
    if hunks
        .iter()
        .any(|h| old >= h.old_first() && old < h.old_next())
    {
        return None;
    }
    let shift: i64 = hunks
        .iter()
        .filter(|h| h.old_next() <= old)
        .map(|h| h.new_lines as i64 - h.old_lines as i64)
        .sum();
    Some((old as i64 + shift) as u32)
}

fn context_line(text: &[&str], hunks: &[Hunk], new: u32) -> DiffLine {
    DiffLine {
        kind: LineKind::Context,
        content: text[new as usize - 1].to_string(),
        old_line: Some(new_to_old(hunks, new)),
        new_line: Some(new),
        no_newline_at_eof: false,
    }
}

/// `file`'s hunks with context added around the wanted new-side lines.
fn expand_file(hunks: &[Hunk], wanted: &[(Side, u32, u32)], text: &str) -> Vec<Hunk> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len() as u32;
    if wanted.is_empty() || total == 0 {
        return hunks.to_vec();
    }

    let shown: BTreeSet<u32> = hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter_map(|l| l.new_line)
        .collect();

    // Context goes around the wanted lines the diff doesn't show. Lines it
    // already shows have their context already, so they add nothing.
    let mut need: BTreeSet<u32> = BTreeSet::new();
    for &(side, start, end) in wanted {
        let lines: Vec<u32> = match side {
            Side::New => (start..=end).collect(),
            // Old lines a hunk covers are shown already; the rest are
            // unchanged, so they have new-side numbers.
            Side::Old => (start..=end).filter_map(|o| old_to_new(hunks, o)).collect(),
        };
        for n in lines
            .into_iter()
            .filter(|n| *n >= 1 && *n <= total && !shown.contains(n))
        {
            for m in n.saturating_sub(CONTEXT_RADIUS).max(1)..=(n + CONTEXT_RADIUS).min(total) {
                need.insert(m);
            }
        }
    }
    need.retain(|n| !shown.contains(n));

    // Runs of consecutive unseen lines become context-only hunks.
    let mut added: Vec<Hunk> = Vec::new();
    let mut run: Vec<u32> = Vec::new();
    let flush = |run: &mut Vec<u32>, added: &mut Vec<Hunk>| {
        if let (Some(&first), false) = (run.first(), run.is_empty()) {
            let body: Vec<DiffLine> = run
                .iter()
                .map(|&n| context_line(&lines, hunks, n))
                .collect();
            added.push(Hunk::from_lines(
                body,
                new_to_old(hunks, first),
                first,
                None,
            ));
            run.clear();
        }
    };
    for n in need {
        if run.last().is_some_and(|&last| last + 1 != n) {
            flush(&mut run, &mut added);
        }
        run.push(n);
    }
    flush(&mut run, &mut added);

    // In file order, with hunks that now touch merged into one.
    let mut all: Vec<Hunk> = hunks.iter().cloned().chain(added).collect();
    all.sort_by_key(|h| (h.old_first(), h.new_first()));
    let mut out: Vec<Hunk> = Vec::new();
    for h in all {
        match out.last_mut() {
            Some(prev) if prev.old_next() == h.old_first() && prev.new_next() == h.new_first() => {
                let (of, nf) = (prev.old_first(), prev.new_first());
                let heading = prev.section_heading.clone().or(h.section_heading.clone());
                let mut lines = std::mem::take(&mut prev.lines);
                lines.extend(h.lines);
                *prev = Hunk::from_lines(lines, of, nf, heading);
            }
            _ => out.push(h),
        }
    }
    out
}

/// `diff` with context added around what `wants` ask to see, read from the
/// full texts of the version being viewed. Files the diff doesn't have are
/// appended (in `wants` order) as files of context only; the second value
/// lists them. A want whose text isn't held is left as it was.
pub fn expand(
    diff: &UnifiedDiff,
    wants: &[Want],
    view: &ViewVersions,
    blobs: &Blobs,
) -> (UnifiedDiff, Vec<String>) {
    let mut out = diff.clone();
    let mut synthetic: Vec<String> = Vec::new();
    let mut files_wanted: Vec<&str> = Vec::new();
    for w in wants {
        if !files_wanted.contains(&w.file.as_str()) {
            files_wanted.push(&w.file);
        }
    }
    for file in files_wanted {
        let wanted: Vec<(Side, u32, u32)> = wants
            .iter()
            .filter(|w| w.file == file)
            .filter_map(|w| w.lines)
            .collect();
        let existing = out.files.iter().position(|f| {
            f.new_path.as_deref() == Some(file) || f.old_path.as_deref() == Some(file)
        });
        match existing {
            Some(i) => {
                let f = &out.files[i];
                let Some(text) = f
                    .new_path
                    .as_deref()
                    .and_then(|p| view.head(p))
                    .and_then(|d| blobs.text(d))
                else {
                    continue;
                };
                out.files[i].hunks = expand_file(&f.hunks, &wanted, text);
            }
            None => {
                let Some(text) = view.head(file).and_then(|d| blobs.text(d)) else {
                    continue;
                };
                out.files.push(FileDiff {
                    old_path: Some(file.to_string()),
                    new_path: Some(file.to_string()),
                    hunks: expand_file(&[], &wanted, text),
                    ..FileDiff::default()
                });
                synthetic.push(file.to_string());
            }
        }
    }
    (out, synthetic)
}

/// Whether `diff` has what to draw a thread at: its file, and the row after
/// which its card goes.
pub fn has_row(diff: &UnifiedDiff, want: &Want) -> bool {
    let Some(file) = diff.files.iter().find(|f| {
        f.new_path.as_deref() == Some(want.file.as_str())
            || f.old_path.as_deref() == Some(want.file.as_str())
    }) else {
        return false;
    };
    let Some((side, line)) = want.row else {
        return true;
    };
    file.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .any(|l| match side {
            Side::New => l.new_line == Some(line),
            Side::Old => l.old_line == Some(line),
        })
}

fn hunk_text(h: &Hunk) -> String {
    let mut out = format!(
        "@@ -{},{} +{},{} @@",
        h.old_start, h.old_lines, h.new_start, h.new_lines
    );
    if let Some(heading) = &h.section_heading {
        out.push(' ');
        out.push_str(heading);
    }
    out.push('\n');
    for l in &h.lines {
        out.push(match l.kind {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
        });
        out.push_str(&l.content);
        out.push('\n');
        if l.no_newline_at_eof {
            out.push_str("\\ No newline at end of file\n");
        }
    }
    out
}

/// `expanded` written as unified diff text, keeping `original_text` (which
/// `original` was parsed from) verbatim wherever the diff wasn't changed and
/// each changed file's own header lines. `None` if the text doesn't split
/// into the files `original` has.
pub fn to_text(
    original_text: &str,
    original: &UnifiedDiff,
    expanded: &UnifiedDiff,
) -> Option<String> {
    if expanded == original {
        return Some(original_text.to_string());
    }
    let mut sections: Vec<Vec<&str>> = Vec::new();
    for line in original_text.lines() {
        if line.starts_with("diff --git ") {
            sections.push(Vec::new());
        }
        sections.last_mut()?.push(line);
    }
    if sections.len() != original.files.len() {
        return None;
    }
    let mut out = String::new();
    for (i, section) in sections.iter().enumerate() {
        let (was, now) = (&original.files[i], &expanded.files[i]);
        let header_end = if was.hunks == now.hunks {
            section.len()
        } else {
            section
                .iter()
                .position(|l| l.starts_with("@@ ") || *l == "@@")
                .unwrap_or(section.len())
        };
        for l in &section[..header_end] {
            out.push_str(l);
            out.push('\n');
        }
        if was.hunks != now.hunks {
            for h in &now.hunks {
                out.push_str(&hunk_text(h));
            }
        }
    }
    for f in &expanded.files[original.files.len()..] {
        let path = f.new_path.as_deref().or(f.old_path.as_deref())?;
        out.push_str(&format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n"
        ));
        for h in &f.hunks {
            out.push_str(&hunk_text(h));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::Placement;
    use crate::diff;
    use crate::digest::digest;
    use crate::files::{Tree, diff_trees};
    use crate::model::{FileDigest, TreeFile};

    fn numbered(n: u32) -> String {
        (1..=n).map(|i| format!("l{i}\n")).collect()
    }

    fn tree(files: &[(&str, &str)]) -> Tree {
        files
            .iter()
            .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
            .collect()
    }

    /// A view between two trees: the diff text, its parse, the per-file
    /// digests, and every text held.
    struct Scene {
        text: String,
        diff: UnifiedDiff,
        files: Vec<FileDigest>,
        held: Vec<String>,
        tree: Vec<TreeFile>,
    }

    fn scene(old: &[(&str, &str)], new: &[(&str, &str)]) -> Scene {
        let (text, files) = diff_trees(&tree(old), &tree(new));
        let held = old.iter().chain(new).map(|(_, t)| t.to_string()).collect();
        // Files present in `new` but not touched are the revision's recorded
        // (untouched) tree.
        let touched: Vec<&str> = files.iter().filter_map(|f| f.new_path.as_deref()).collect();
        let tree = new
            .iter()
            .filter(|(p, _)| !touched.contains(p))
            .map(|(p, t)| TreeFile {
                path: p.to_string(),
                digest: digest(t),
            })
            .collect();
        Scene {
            diff: diff::parse(&text).unwrap(),
            text,
            files,
            held,
            tree,
        }
    }

    fn one(old: &str, new: &str) -> Scene {
        scene(&[("f.txt", old)], &[("f.txt", new)])
    }

    fn run(sc: &Scene, wants: &[Want]) -> (UnifiedDiff, Vec<String>) {
        let mut blobs = Blobs::default();
        for t in &sc.held {
            blobs.add(t.as_bytes());
        }
        expand(
            &sc.diff,
            wants,
            &ViewVersions {
                files: &sc.files,
                tree: &sc.tree,
            },
            &blobs,
        )
    }

    fn want(file: &str, side: Side, start: u32, end: u32) -> Want {
        Want {
            file: file.into(),
            lines: Some((side, start, end)),
            row: Some((side, end)),
        }
    }

    /// (old_start,old_lines,new_start,new_lines) of each hunk of file 0.
    fn shape(d: &UnifiedDiff) -> Vec<(u32, u32, u32, u32)> {
        d.files[0]
            .hunks
            .iter()
            .map(|h| (h.old_start, h.old_lines, h.new_start, h.new_lines))
            .collect()
    }

    fn new_lines_of(h: &Hunk) -> Vec<u32> {
        h.lines.iter().filter_map(|l| l.new_line).collect()
    }

    /// `old` (30 lines) with lines 3 and 25 edited: hunks at 1..=6 and 22..=28.
    fn two_hunks() -> Scene {
        let old = numbered(30);
        let new = old.replace("l3\n", "L3\n").replace("l25\n", "L25\n");
        one(&old, &new)
    }

    // ---- what is added ----------------------------------------------------

    #[test]
    fn lines_the_diff_already_shows_change_nothing() {
        let sc = two_hunks();
        for (side, a, b) in [
            (Side::New, 2, 5),
            (Side::New, 3, 3),
            (Side::Old, 1, 6),
            (Side::New, 22, 28),
        ] {
            let (d, synthetic) = run(&sc, &[want("f.txt", side, a, b)]);
            assert_eq!(d, sc.diff, "{side:?} {a}..={b}");
            assert!(synthetic.is_empty());
        }
    }

    #[test]
    fn an_unseen_line_gets_a_hunk_of_context_around_it() {
        let sc = two_hunks();
        assert_eq!(shape(&sc.diff), [(1, 6, 1, 6), (22, 7, 22, 7)]);
        let (d, _) = run(&sc, &[want("f.txt", Side::New, 12, 12)]);
        // Lines 9..=15 in the gap between the two hunks.
        assert_eq!(shape(&d), [(1, 6, 1, 6), (9, 7, 9, 7), (22, 7, 22, 7)]);
        let added = &d.files[0].hunks[1];
        assert_eq!(new_lines_of(added), (9..=15).collect::<Vec<_>>());
        assert!(added.lines.iter().all(|l| l.kind == LineKind::Context));
        assert_eq!(added.lines[3].content, "l12");
        assert_eq!(added.lines[3].old_line, Some(12));
    }

    #[test]
    fn a_range_gets_context_around_the_lines_that_were_unseen() {
        let sc = two_hunks();
        let (d, _) = run(&sc, &[want("f.txt", Side::New, 12, 14)]);
        assert_eq!(
            new_lines_of(&d.files[0].hunks[1]),
            (9..=17).collect::<Vec<_>>()
        );
    }

    #[test]
    fn context_that_touches_a_hunk_becomes_part_of_it() {
        let sc = two_hunks();
        // Line 9's window (6..=12) starts right where the first hunk ends.
        let (d, _) = run(&sc, &[want("f.txt", Side::New, 9, 9)]);
        assert_eq!(shape(&d), [(1, 12, 1, 12), (22, 7, 22, 7)]);
        assert_eq!(
            new_lines_of(&d.files[0].hunks[0]),
            (1..=12).collect::<Vec<_>>()
        );
        // Contents of the original hunk's changed lines are untouched.
        assert_eq!(d.files[0].hunks[0].lines[2].content, "l3");
        assert_eq!(d.files[0].hunks[0].lines[2].kind, LineKind::Removed);
    }

    #[test]
    fn context_that_bridges_two_hunks_joins_them() {
        let sc = two_hunks();
        let (d, _) = run(
            &sc,
            &[
                want("f.txt", Side::New, 10, 10),
                want("f.txt", Side::New, 18, 18),
            ],
        );
        // 7..=13 and 15..=21 with the hunks at 1..=6 and 22..=28: one hunk
        // apart from line 14, which stays a gap... no: 13 and 15 leave 14.
        assert_eq!(d.files[0].hunks.len(), 2, "{:?}", shape(&d));
        let (d, _) = run(
            &sc,
            &[
                want("f.txt", Side::New, 10, 10),
                want("f.txt", Side::New, 14, 14),
                want("f.txt", Side::New, 18, 18),
            ],
        );
        assert_eq!(
            shape(&d),
            [(1, 28, 1, 28)],
            "everything from 1 to 28 is now shown"
        );
    }

    #[test]
    fn context_is_cut_at_the_ends_of_the_file() {
        let old = numbered(10);
        let sc = one(&old, &old.replace("l5\n", "L5\n"));
        // The diff shows 2..=8; ask for the first and last lines.
        let (d, _) = run(&sc, &[want("f.txt", Side::New, 1, 1)]);
        assert_eq!(new_lines_of(&d.files[0].hunks[0]).first(), Some(&1));
        let (d, _) = run(&sc, &[want("f.txt", Side::New, 10, 10)]);
        let last = d.files[0].hunks.last().unwrap();
        assert_eq!(new_lines_of(last).last(), Some(&10));
        assert!(new_lines_of(last).iter().all(|n| *n <= 10));
    }

    #[test]
    fn a_line_past_the_end_of_the_file_is_ignored() {
        let sc = two_hunks();
        let (d, _) = run(&sc, &[want("f.txt", Side::New, 99, 99)]);
        assert_eq!(d, sc.diff);
    }

    // ---- numbering across changed lines -------------------------------------

    #[test]
    fn lines_after_added_lines_keep_their_old_numbers() {
        // Two lines added at the top: every unchanged line moves down by 2.
        let old = numbered(20);
        let sc = one(&old, &format!("A\nB\n{old}"));
        let (d, _) = run(&sc, &[want("f.txt", Side::New, 15, 15)]);
        let h = d.files[0].hunks.last().unwrap();
        let line = h.lines.iter().find(|l| l.new_line == Some(15)).unwrap();
        assert_eq!((line.content.as_str(), line.old_line), ("l13", Some(13)));
        assert_eq!((h.old_start, h.new_start), (h.old_start, h.old_start + 2));
    }

    #[test]
    fn lines_after_removed_lines_keep_their_old_numbers() {
        let old = numbered(20);
        let sc = one(&old, &old.replace("l1\nl2\nl3\n", ""));
        // New line 12 is old line 15.
        let (d, _) = run(&sc, &[want("f.txt", Side::New, 12, 12)]);
        let h = d.files[0].hunks.last().unwrap();
        let line = h.lines.iter().find(|l| l.new_line == Some(12)).unwrap();
        assert_eq!((line.content.as_str(), line.old_line), ("l15", Some(15)));
    }

    #[test]
    fn a_want_on_the_old_side_is_found_through_the_shift() {
        let old = numbered(20);
        let sc = one(&old, &format!("A\nB\n{old}")); // + 2 lines at the top
        // Old line 15 is new line 17.
        let (d, _) = run(&sc, &[want("f.txt", Side::Old, 15, 15)]);
        let h = d.files[0].hunks.last().unwrap();
        let line = h.lines.iter().find(|l| l.old_line == Some(15)).unwrap();
        assert_eq!((line.content.as_str(), line.new_line), ("l15", Some(17)));
        // Old lines a hunk covers (removed lines) are shown already.
        let sc = one(&old, &old.replace("l5\n", ""));
        let (d, _) = run(&sc, &[want("f.txt", Side::Old, 5, 5)]);
        assert_eq!(d, sc.diff);
    }

    #[test]
    fn hunks_without_lines_on_one_side_number_their_neighbours_right() {
        // A pure addition at the very top (`@@ -0,0 +1,2 @@`-style) and a
        // pure removal at the very end.
        let old = numbered(12);
        let new = format!("X\nY\n{}", old.replace("l12\n", ""));
        let sc = one(&old, &new);
        let (d, _) = run(&sc, &[want("f.txt", Side::New, 8, 8)]);
        for h in &d.files[0].hunks {
            for l in &h.lines {
                let old_text = old.lines().nth(l.old_line.map_or(0, |n| n as usize - 1));
                if l.kind == LineKind::Context {
                    assert_eq!(Some(l.content.as_str()), old_text, "{l:?}");
                }
            }
        }
        assert!(
            d.files[0]
                .hunks
                .iter()
                .flat_map(|h| &h.lines)
                .any(|l| l.new_line == Some(8))
        );
    }

    // ---- files the diff doesn't have -----------------------------------------

    #[test]
    fn a_file_the_diff_never_mentions_is_added_as_pure_context() {
        let readme = "# title\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\n";
        let sc = scene(
            &[("f.txt", "a\n"), ("README.md", readme)],
            &[("f.txt", "b\n"), ("README.md", readme)],
        );
        assert_eq!(sc.diff.files.len(), 1);
        let (d, synthetic) = run(&sc, &[want("README.md", Side::New, 5, 5)]);
        assert_eq!(synthetic, ["README.md"]);
        assert_eq!(d.files.len(), 2);
        let f = &d.files[1];
        assert_eq!(
            (f.old_path.as_deref(), f.new_path.as_deref()),
            (Some("README.md"), Some("README.md"))
        );
        assert_eq!(f.hunks.len(), 1);
        assert_eq!(new_lines_of(&f.hunks[0]), (2..=8).collect::<Vec<_>>());
        assert!(
            f.hunks[0]
                .lines
                .iter()
                .all(|l| l.kind == LineKind::Context && l.old_line == l.new_line)
        );
        // The original file is untouched.
        assert_eq!(d.files[0], sc.diff.files[0]);
    }

    #[test]
    fn a_thread_on_a_whole_untouched_file_adds_the_file_without_hunks() {
        let sc = scene(
            &[("f.txt", "a\n"), ("README.md", "x\n")],
            &[("f.txt", "b\n"), ("README.md", "x\n")],
        );
        let (d, synthetic) = run(
            &sc,
            &[Want {
                file: "README.md".into(),
                lines: None,
                row: None,
            }],
        );
        assert_eq!(synthetic, ["README.md"]);
        assert!(d.files[1].hunks.is_empty());
    }

    #[test]
    fn several_wants_on_one_untouched_file_share_one_added_file() {
        let readme = numbered(40);
        let sc = scene(
            &[("f.txt", "a\n"), ("README.md", &readme)],
            &[("f.txt", "b\n"), ("README.md", &readme)],
        );
        let (d, synthetic) = run(
            &sc,
            &[
                want("README.md", Side::New, 3, 3),
                want("README.md", Side::New, 4, 4),
                want("README.md", Side::New, 30, 30),
            ],
        );
        assert_eq!(synthetic, ["README.md"], "once");
        let f = &d.files[1];
        assert_eq!(f.hunks.len(), 2, "lines 1..=7 and 27..=33");
        assert_eq!(new_lines_of(&f.hunks[0]), (1..=7).collect::<Vec<_>>());
        assert_eq!(new_lines_of(&f.hunks[1]), (27..=33).collect::<Vec<_>>());
    }

    #[test]
    fn a_want_whose_text_is_not_held_is_left_alone() {
        let sc = scene(
            &[("f.txt", "a\n"), ("README.md", "x\n")],
            &[("f.txt", "b\n"), ("README.md", "x\n")],
        );
        // README's version is in the tree but its text isn't held.
        let mut blobs = Blobs::default();
        blobs.add(b"a\n");
        blobs.add(b"b\n");
        let (d, synthetic) = expand(
            &sc.diff,
            &[want("README.md", Side::New, 1, 1)],
            &ViewVersions {
                files: &sc.files,
                tree: &sc.tree,
            },
            &blobs,
        );
        assert_eq!(d, sc.diff);
        assert!(synthetic.is_empty());
        // Nor is a file the revision has no version of.
        let (d, _) = run(&sc, &[want("elsewhere.txt", Side::New, 1, 1)]);
        assert_eq!(d, sc.diff);
    }

    #[test]
    fn a_deleted_file_is_shown_whole_already() {
        let sc = scene(&[("f.txt", "a\nb\nc\n")], &[]);
        let (d, synthetic) = run(&sc, &[want("f.txt", Side::Old, 2, 2)]);
        assert_eq!(d, sc.diff);
        assert!(synthetic.is_empty());
    }

    // ---- wants from placements, and rows ---------------------------------------

    #[test]
    fn placements_say_what_to_show_and_where_the_card_goes() {
        let line = Placement::Line {
            file: "f".into(),
            side: Side::Old,
            line_start: 4,
            line_end: 6,
            old_range: None,
        };
        assert_eq!(
            want_of(&line),
            Some(Want {
                file: "f".into(),
                lines: Some((Side::Old, 4, 6)),
                row: Some((Side::Old, 6))
            })
        );
        // A point shows the lines on both sides and is drawn after the first.
        let point = Placement::Point {
            file: "f".into(),
            before: 9,
            was: Vec::new(),
            kind: crate::anchor::Absence::Deleted,
        };
        let w = want_of(&point).unwrap();
        assert_eq!(
            (w.lines, w.row),
            (Some((Side::New, 8, 9)), Some((Side::New, 8)))
        );
        let top = Placement::Point {
            file: "f".into(),
            before: 1,
            was: Vec::new(),
            kind: crate::anchor::Absence::Deleted,
        };
        assert_eq!(want_of(&top).unwrap().row, Some((Side::New, 1)));
        assert_eq!(want_of(&Placement::File("f".into())).unwrap().row, None);
        assert_eq!(want_of(&Placement::Global), None);
        assert_eq!(want_of(&Placement::Unplaced { file: "f".into() }), None);
    }

    #[test]
    fn has_row_needs_the_file_and_the_row() {
        let sc = two_hunks();
        assert!(has_row(&sc.diff, &want("f.txt", Side::New, 3, 3)));
        assert!(
            has_row(&sc.diff, &want("f.txt", Side::Old, 3, 3)),
            "the removed row"
        );
        assert!(!has_row(&sc.diff, &want("f.txt", Side::New, 12, 12)));
        assert!(!has_row(&sc.diff, &want("nope.txt", Side::New, 3, 3)));
        let file_only = Want {
            file: "f.txt".into(),
            lines: None,
            row: None,
        };
        assert!(has_row(&sc.diff, &file_only));
        // Rows are on the side asked for: added lines have no old row.
        let sc = one("a\n", "a\nb\n");
        assert!(has_row(&sc.diff, &want("f.txt", Side::New, 2, 2)));
        assert!(!has_row(&sc.diff, &want("f.txt", Side::Old, 2, 2)));
    }

    // ---- writing the diff back out ----------------------------------------------

    #[test]
    fn unchanged_diff_is_written_back_verbatim() {
        let sc = two_hunks();
        assert_eq!(to_text(&sc.text, &sc.diff, &sc.diff).unwrap(), sc.text);
    }

    #[test]
    fn expanded_text_reparses_to_the_expanded_diff() {
        let sc = two_hunks();
        for wants in [
            vec![want("f.txt", Side::New, 12, 12)],
            vec![want("f.txt", Side::New, 9, 9)],
            vec![
                want("f.txt", Side::New, 10, 10),
                want("f.txt", Side::New, 14, 14),
                want("f.txt", Side::New, 18, 18),
            ],
            vec![
                want("f.txt", Side::New, 30, 30),
                want("f.txt", Side::New, 1, 1),
            ],
        ] {
            let (d, _) = run(&sc, &wants);
            let text = to_text(&sc.text, &sc.diff, &d).unwrap();
            assert_eq!(diff::parse(&text).unwrap(), d, "{text}");
        }
    }

    #[test]
    fn a_file_added_for_context_is_written_with_its_own_header() {
        let readme = numbered(9);
        let sc = scene(
            &[("f.txt", "a\n"), ("README.md", &readme)],
            &[("f.txt", "b\n"), ("README.md", &readme)],
        );
        let (d, synthetic) = run(&sc, &[want("README.md", Side::New, 5, 5)]);
        assert_eq!(synthetic, ["README.md"]);
        let text = to_text(&sc.text, &sc.diff, &d).unwrap();
        assert!(
            text.starts_with(&sc.text),
            "the real diff comes first, verbatim"
        );
        assert!(text.contains("diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -2,7 +2,7 @@\n"), "{text}");
        assert_eq!(diff::parse(&text).unwrap(), d);
    }

    #[test]
    fn a_changed_files_own_header_lines_survive_and_others_stay_verbatim() {
        // A rename with an edit, an untouched-hunk file, and a mode line.
        let text = "\
diff --git a/old.txt b/new.txt
similarity index 80%
rename from old.txt
rename to new.txt
index 111..222 100644
--- a/old.txt
+++ b/new.txt
@@ -1,3 +1,3 @@ fn heading
 a
-b
+B
 c
diff --git a/other.txt b/other.txt
index 333..444 100644
--- a/other.txt
+++ b/other.txt
@@ -1 +1 @@
-x
+y
";
        let original = diff::parse(text).unwrap();
        let big: String = (1..=12).map(|n| format!("n{n}\n")).collect();
        let head = format!("a\nB\nc\n{big}");
        let view_files = vec![FileDigest {
            old_path: Some("old.txt".into()),
            new_path: Some("new.txt".into()),
            old: Some(digest("a\nb\nc\n")),
            new: Some(digest(&head)),
        }];
        let mut blobs = Blobs::default();
        blobs.add(head.as_bytes());
        let (d, _) = expand(
            &original,
            &[want("new.txt", Side::New, 10, 10)],
            &ViewVersions {
                files: &view_files,
                tree: &[],
            },
            &blobs,
        );
        assert_ne!(d, original);
        let out = to_text(text, &original, &d).unwrap();
        // Header lines of the changed file are kept; the other file is verbatim.
        assert!(out.starts_with("diff --git a/old.txt b/new.txt\nsimilarity index 80%\nrename from old.txt\nrename to new.txt\nindex 111..222 100644\n--- a/old.txt\n+++ b/new.txt\n@@ "), "{out}");
        assert!(out.ends_with("diff --git a/other.txt b/other.txt\nindex 333..444 100644\n--- a/other.txt\n+++ b/other.txt\n@@ -1 +1 @@\n-x\n+y\n"), "{out}");
        assert!(out.contains("fn heading"), "the section heading is kept");
        assert_eq!(diff::parse(&out).unwrap(), d);
    }

    #[test]
    fn a_missing_newline_marker_survives_expansion() {
        let text = "\
diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -10,3 +10,3 @@
 l10
 l11
-l12
+L12
\\ No newline at end of file
";
        let original = diff::parse(text).unwrap();
        assert!(
            original.files[0].hunks[0]
                .lines
                .iter()
                .any(|l| l.no_newline_at_eof)
        );
        let head: String = (1..=11).map(|n| format!("l{n}\n")).collect::<String>() + "L12";
        let files = vec![FileDigest {
            old_path: Some("f.txt".into()),
            new_path: Some("f.txt".into()),
            old: Some(digest("old")),
            new: Some(digest(&head)),
        }];
        let mut blobs = Blobs::default();
        blobs.add(head.as_bytes());
        let (d, _) = expand(
            &original,
            &[want("f.txt", Side::New, 5, 5)],
            &ViewVersions {
                files: &files,
                tree: &[],
            },
            &blobs,
        );
        // Lines 2..=8 were added in a hunk of their own (the diff's hunk
        // starts at 10, so 9 stays out).
        assert_eq!(d.files[0].hunks.len(), 2);
        let out = to_text(text, &original, &d).unwrap();
        assert!(
            out.contains("+L12\n\\ No newline at end of file\n"),
            "{out}"
        );
        assert_eq!(diff::parse(&out).unwrap(), d);
    }

    #[test]
    fn hunks_that_touch_are_merged_and_their_lines_kept_in_order() {
        // Two hunks of the diff plus context that meets both.
        let sc = two_hunks();
        let (d, _) = run(
            &sc,
            &[
                want("f.txt", Side::New, 9, 9),
                want("f.txt", Side::New, 19, 19),
            ],
        );
        // 6..=12 touches hunk 1 (..=6); 16..=22 touches hunk 2 (22..).
        assert_eq!(d.files[0].hunks.len(), 2, "{:?}", shape(&d));
        let all: Vec<u32> = d.files[0].hunks.iter().flat_map(new_lines_of).collect();
        assert!(
            all.windows(2).all(|w| w[0] < w[1]),
            "in order, no repeats: {all:?}"
        );
    }

    #[test]
    fn text_that_does_not_split_into_the_diffs_files_is_refused() {
        let sc = two_hunks();
        let (d, _) = run(&sc, &[want("f.txt", Side::New, 12, 12)]);
        // Same diff, but text with a different number of files.
        assert!(to_text("not a diff at all\n", &sc.diff, &d).is_none());
        let twice = format!("{}{}", sc.text, sc.text);
        assert!(to_text(&twice, &sc.diff, &d).is_none());
    }

    // ---- randomized: invariants of the result -----------------------------------

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, below: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % below.max(1)
        }
    }

    #[test]
    fn whatever_is_asked_the_result_is_a_consistent_diff() {
        let mut rng = Lcg(0x5EED);
        for round in 0..300 {
            let old_len = 1 + rng.next(40);
            let old: String = (0..old_len).map(|i| format!("o{i}\n")).collect();
            // A random edit: some lines dropped, some replaced, some added.
            let mut new = String::new();
            for i in 0..old_len {
                match rng.next(8) {
                    0 => {}
                    1 => new.push_str(&format!("edited{i}\n")),
                    2 => new.push_str(&format!("o{i}\nadded{i}\n")),
                    _ => new.push_str(&format!("o{i}\n")),
                }
            }
            if new.is_empty() {
                new.push_str("only\n");
            }
            let sc = one(&old, &new);
            if sc.diff.files.is_empty() {
                continue;
            }
            let new_len = new.lines().count() as u32;
            let mut wants = Vec::new();
            for _ in 0..1 + rng.next(4) {
                let a = 1 + rng.next(new_len as usize) as u32;
                let b = (a + rng.next(4) as u32).min(new_len);
                wants.push(want("f.txt", Side::New, a, b));
            }
            let (d, _) = run(&sc, &wants);
            let ctx = format!("round {round}: {old:?} -> {new:?} wants {wants:?}");

            let hunks = &d.files[0].hunks;
            let old_text: Vec<&str> = old.lines().collect();
            let new_text: Vec<&str> = new.lines().collect();
            let mut shown_new: BTreeSet<u32> = BTreeSet::new();
            let (mut last_old, mut last_new) = (0u32, 0u32);
            for h in hunks {
                // In order, without overlap.
                assert!(h.old_first() > last_old || last_old == 0, "{ctx}");
                assert!(h.new_first() > last_new || last_new == 0, "{ctx}");
                assert!(
                    h.old_first() >= last_old && h.new_first() >= last_new,
                    "{ctx}"
                );
                last_old = h.old_next();
                last_new = h.new_next();
                // The header agrees with the lines.
                let (o, n) = h.lines.iter().fold((0, 0), |(o, n), l| match l.kind {
                    LineKind::Context => (o + 1, n + 1),
                    LineKind::Removed => (o + 1, n),
                    LineKind::Added => (o, n + 1),
                });
                assert_eq!((o, n), (h.old_lines, h.new_lines), "{ctx}");
                // Every line says what the files say.
                for l in &h.lines {
                    if let Some(k) = l.new_line {
                        assert_eq!(new_text[k as usize - 1], l.content, "{ctx}");
                        shown_new.insert(k);
                    }
                    if let Some(k) = l.old_line {
                        assert_eq!(old_text[k as usize - 1], l.content, "{ctx}");
                    }
                }
            }
            // Everything asked for is shown, and nothing of the original is lost.
            for w in &wants {
                let (_, a, b) = w.lines.unwrap();
                for k in a..=b {
                    assert!(shown_new.contains(&k), "line {k} not shown: {ctx}");
                }
            }
            for l in sc.diff.files[0].hunks.iter().flat_map(|h| &h.lines) {
                assert!(
                    hunks.iter().flat_map(|h| &h.lines).any(|m| m == l),
                    "{l:?} lost: {ctx}"
                );
            }
            // And it can be written out and read back.
            let text = to_text(&sc.text, &sc.diff, &d).unwrap();
            assert_eq!(diff::parse(&text).unwrap(), d, "{ctx}");
        }
    }
}
