//! Turns the position a comment was written at in the editor (an
//! [`AnchorScope`], derived from the diff's line counters) into the
//! [`Anchor`] that is persisted: both sides' ranges, the digests of the file
//! versions they refer to, and the surrounding text kept for re-anchoring.

use crate::anchor;
use crate::model::Anchor;

/// `revisions` are the (base, head) ids of the reviewed change; `blobs`
/// holds the file versions read this session. When the full text of a side is
/// held, its context is taken from it; otherwise (a `--snapshot diff`
/// review) from the lines the diff shows.
pub fn build_anchor(
    scope: &crate::annotation::AnchorScope,
    parsed_diff: &crate::diff::UnifiedDiff,
    files: &[crate::model::FileDigest],
    revisions: &(Option<String>, Option<String>),
    blobs: &crate::digest::Blobs,
    context_lines: u32,
) -> anyhow::Result<Anchor> {
    use crate::annotation::AnchorScope;
    let file_entry = |file: &str| {
        files
            .iter()
            .find(|f| f.new_path.as_deref() == Some(file) || f.old_path.as_deref() == Some(file))
    };
    match scope {
        AnchorScope::Global => Ok(Anchor::Global {
            base: revisions.0.clone(),
            head: revisions.1.clone(),
        }),
        AnchorScope::File { file } => {
            let entry = file_entry(file);
            let make = |path: Option<&String>, digest: Option<&String>| {
                path.map(|p| crate::model::FileRef {
                    file: p.clone(),
                    digest: digest.cloned().unwrap_or_default(),
                })
            };
            Ok(match entry {
                Some(e) => Anchor::File {
                    base: make(e.old_path.as_ref(), e.old.as_ref()),
                    head: make(e.new_path.as_ref(), e.new.as_ref()),
                },
                None => Anchor::File {
                    base: None,
                    head: make(Some(&file.clone()), None),
                },
            })
        }
        AnchorScope::Span { file, base, head } => {
            let file_diff = parsed_diff
                .files
                .iter()
                .find(|f| {
                    f.new_path.as_deref() == Some(file) || f.old_path.as_deref() == Some(file)
                })
                .ok_or_else(|| {
                    anyhow::anyhow!("internal error: file '{file}' not found in the parsed diff")
                })?;
            let side = |which: crate::model::Side,
                        path: Option<&String>,
                        span: &crate::annotation::LineSpan|
             -> anyhow::Result<Option<crate::model::SideAnchor>> {
                let Some(path) = path else { return Ok(None) };
                let digest = anchor::digest_for(files, path, which).unwrap_or_default();
                // The context comes from the whole file when it is held:
                // complete, contiguous lines around the range. Only a
                // diff-only review has to make do with what the hunks show.
                let corpus = match blobs.text(digest) {
                    Some(text) => anchor::corpus_from_file_text(text),
                    None => anchor::corpus_from_diff_hunks(file_diff, which),
                };
                let context =
                    anchor::context_for_span(&corpus, span.start, span.len, context_lines)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "internal error: could not locate {path}:{}+{} in the parsed diff",
                                span.start,
                                span.len
                            )
                        })?;
                Ok(Some(crate::model::SideAnchor {
                    file: path.clone(),
                    digest: digest.to_string(),
                    start: span.start,
                    context,
                }))
            };
            Ok(Anchor::Span {
                base: side(crate::model::Side::Old, file_diff.old_path.as_ref(), base)?,
                head: side(crate::model::Side::New, file_diff.new_path.as_ref(), head)?,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::{self, Item};
    use crate::digest::{Blobs, digest};
    use crate::files::{Tree, diff_trees};
    use crate::model::{FileDigest, SideAnchor};

    /// A one-file change: the unified diff between `old` and `new` (as the
    /// directory provider makes it) and the digests of both versions.
    struct Change {
        diff: String,
        files: Vec<FileDigest>,
        old: Option<String>,
        new: Option<String>,
    }

    fn change(old: Option<&str>, new: Option<&str>) -> Change {
        let tree = |t: Option<&str>| -> Tree {
            t.map(|t| ("f.txt".to_string(), t.as_bytes().to_vec()))
                .into_iter()
                .collect()
        };
        let (diff, files) = diff_trees(&tree(old), &tree(new));
        Change {
            diff,
            files,
            old: old.map(str::to_string),
            new: new.map(str::to_string),
        }
    }

    fn numbered(n: u32) -> String {
        (1..=n).map(|i| format!("l{i}\n")).collect()
    }

    /// `text` with the given 1-based lines replaced (`None` = removed).
    fn edited(text: &str, edits: &[(u32, Option<&str>)]) -> String {
        let mut out = String::new();
        for (i, line) in text.lines().enumerate() {
            match edits.iter().find(|(n, _)| *n == i as u32 + 1) {
                Some((_, Some(new))) => out.push_str(&format!("{new}\n")),
                Some((_, None)) => {}
                None => out.push_str(&format!("{line}\n")),
            }
        }
        out
    }

    fn revisions() -> (Option<String>, Option<String>) {
        (Some("base-rev".into()), Some("head-rev".into()))
    }

    /// The anchor of a comment written right after the diff line `index`
    /// (0-based, counting every line of `diff`), with the file texts held
    /// (`held`) or not (a diff-only review).
    fn anchor_after_line(c: &Change, index: usize, held: bool) -> Anchor {
        let mut lines: Vec<&str> = c.diff.lines().collect();
        lines.insert(index + 1, "> comment");
        let text = lines.join("\n") + "\n";
        let parsed = annotation::parse(&text).unwrap();
        let Item::NewThread { scope, .. } = &parsed.items[0] else {
            panic!("expected a thread");
        };
        let mut blobs = Blobs::default();
        if held {
            for t in [&c.old, &c.new].into_iter().flatten() {
                blobs.add(t.as_bytes());
            }
        }
        build_anchor(scope, &parsed.diff, &c.files, &revisions(), &blobs, 3).unwrap()
    }

    /// The anchor for a comment after the first diff line equal to `line`.
    fn anchor_after(c: &Change, line: &str, held: bool) -> Anchor {
        let index = c
            .diff
            .lines()
            .position(|l| l == line)
            .unwrap_or_else(|| panic!("no diff line {line:?} in:\n{}", c.diff));
        anchor_after_line(c, index, held)
    }

    fn sides(a: &Anchor) -> (Option<&SideAnchor>, Option<&SideAnchor>) {
        let Anchor::Span { base, head } = a else {
            panic!("expected a span, got {a:?}")
        };
        (base.as_ref(), head.as_ref())
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Two hunks with a gap between them: lines 2 and 19 of 20 changed.
    fn two_hunks() -> Change {
        let old = numbered(20);
        change(
            Some(&old),
            Some(&edited(&old, &[(2, Some("L2")), (19, Some("L19"))])),
        )
    }

    #[test]
    fn context_at_the_end_of_a_hunk_reaches_past_it_when_the_file_is_held() {
        let c = two_hunks();
        let a = anchor_after(&c, " l5", true);
        let (base, head) = sides(&a);
        let head = head.unwrap();
        assert_eq!((head.start, &head.context.target), (5, &strs(&["l5"])));
        assert_eq!(head.context.before, strs(&["L2", "l3", "l4"]));
        assert_eq!(head.context.after, strs(&["l6", "l7", "l8"]));
        let base = base.unwrap();
        assert_eq!(base.context.before, strs(&["l2", "l3", "l4"]));
        assert_eq!(base.context.after, strs(&["l6", "l7", "l8"]));
    }

    #[test]
    fn diff_only_context_stops_at_the_edge_of_the_hunk() {
        // Without the file, the lines after the hunk are simply unknown --
        // in particular, the *next* hunk's lines must not be taken for them.
        let c = two_hunks();
        let a = anchor_after(&c, " l5", false);
        let (base, head) = sides(&a);
        assert!(head.unwrap().context.after.is_empty());
        assert!(base.unwrap().context.after.is_empty());
        assert_eq!(head.unwrap().context.before, strs(&["L2", "l3", "l4"]));

        // ...and the same at the top of the second hunk.
        let a = anchor_after(&c, " l16", false);
        let (_, head) = sides(&a);
        let head = head.unwrap();
        assert_eq!(head.start, 16);
        assert!(head.context.before.is_empty(), "{:?}", head.context.before);
        assert_eq!(head.context.after, strs(&["l17", "l18", "L19"]));
    }

    #[test]
    fn context_at_the_top_of_a_second_hunk_reaches_back_when_the_file_is_held() {
        let a = anchor_after(&two_hunks(), " l16", true);
        let (_, head) = sides(&a);
        assert_eq!(head.unwrap().context.before, strs(&["l13", "l14", "l15"]));
    }

    #[test]
    fn context_is_short_at_the_start_and_end_of_the_file() {
        let c = change(Some("a\nb\nc\n"), Some("a\nB\nc\n"));
        for held in [true, false] {
            let a = anchor_after(&c, " a", held);
            let (_, head) = sides(&a);
            let head = head.unwrap();
            assert_eq!(head.start, 1);
            assert!(head.context.before.is_empty());
            assert_eq!(head.context.after, strs(&["B", "c"]));

            let a = anchor_after(&c, " c", held);
            let (_, head) = sides(&a);
            let head = head.unwrap();
            assert_eq!(head.start, 3);
            assert_eq!(head.context.before, strs(&["a", "B"]));
            assert!(head.context.after.is_empty());
        }
    }

    #[test]
    fn an_added_line_is_an_insertion_point_on_the_base_side() {
        let c = change(
            Some("a\nb\nc\nd\ne\nf\ng\n"),
            Some("a\nb\nc\nX\nd\ne\nf\ng\n"),
        );
        let a = anchor_after(&c, "+X", true);
        let (base, head) = sides(&a);
        let (base, head) = (base.unwrap(), head.unwrap());
        assert_eq!((head.start, &head.context.target), (4, &strs(&["X"])));
        assert_eq!(head.context.before, strs(&["a", "b", "c"]));
        assert_eq!(head.context.after, strs(&["d", "e", "f"]));
        // Before base line 4 (`d`): nothing of its own, just the neighbours.
        assert_eq!((base.start, base.len()), (4, 0));
        assert!(base.is_empty());
        assert_eq!(base.context.before, strs(&["a", "b", "c"]));
        assert_eq!(base.context.after, strs(&["d", "e", "f"]));
    }

    #[test]
    fn a_removed_line_is_an_insertion_point_on_the_head_side() {
        let c = change(
            Some("a\nb\nc\nd\ne\nf\ng\n"),
            Some("a\nb\nc\ne\nf\ng\n"),
        );
        for held in [true, false] {
            let a = anchor_after(&c, "-d", held);
            let (base, head) = sides(&a);
            let (base, head) = (base.unwrap(), head.unwrap());
            assert_eq!((base.start, &base.context.target), (4, &strs(&["d"])));
            assert_eq!((head.start, head.len()), (4, 0));
            assert_eq!(head.context.before, strs(&["a", "b", "c"]));
            assert_eq!(head.context.after.first().map(String::as_str), Some("e"));
        }
    }

    #[test]
    fn a_range_takes_its_whole_text_from_the_file() {
        let c = change(
            Some("a\nb\nc\nd\ne\nf\ng\nh\n"),
            Some("a\nb\nC\nD\nE\nf\ng\nh\n"),
        );
        let mut lines: Vec<String> = c.diff.lines().map(str::to_string).collect();
        let open = lines.iter().position(|l| l == "-c").unwrap();
        lines.insert(open, ">[r".to_string());
        let close = lines.iter().position(|l| l == "+E").unwrap();
        lines.insert(close + 1, ">]r".to_string());
        lines.insert(close + 2, "> range".to_string());
        let text = lines.join("\n") + "\n";
        let parsed = annotation::parse(&text).unwrap();
        let Item::NewThread { scope, .. } = &parsed.items[0] else {
            panic!()
        };
        let mut blobs = Blobs::default();
        for t in [&c.old, &c.new].into_iter().flatten() {
            blobs.add(t.as_bytes());
        }
        let a = build_anchor(scope, &parsed.diff, &c.files, &revisions(), &blobs, 3).unwrap();
        let (base, head) = sides(&a);
        let (base, head) = (base.unwrap(), head.unwrap());
        assert_eq!((head.start, &head.context.target), (3, &strs(&["C", "D", "E"])));
        assert_eq!(head.context.before, strs(&["a", "b"]));
        assert_eq!(head.context.after, strs(&["f", "g", "h"]));
        assert_eq!((base.start, &base.context.target), (3, &strs(&["c", "d", "e"])));
    }

    #[test]
    fn added_and_deleted_files_have_only_one_side() {
        let added = change(None, Some("a\nb\nc\n"));
        for held in [true, false] {
            let a = anchor_after(&added, "+b", held);
            let (base, head) = sides(&a);
            assert!(base.is_none());
            let head = head.unwrap();
            assert_eq!((head.start, &head.context.target), (2, &strs(&["b"])));
            assert_eq!(head.context.before, strs(&["a"]));
            assert_eq!(head.context.after, strs(&["c"]));
        }
        let deleted = change(Some("a\nb\nc\n"), None);
        for held in [true, false] {
            let a = anchor_after(&deleted, "-b", held);
            let (base, head) = sides(&a);
            assert!(head.is_none());
            assert_eq!(base.unwrap().context.target, strs(&["b"]));
        }
    }

    #[test]
    fn the_digests_are_those_of_the_file_versions_whether_or_not_they_are_held() {
        let c = change(Some("a\nb\nc\n"), Some("a\nB\nc\n"));
        for held in [true, false] {
            let a = anchor_after(&c, " a", held);
            let (base, head) = sides(&a);
            assert_eq!(base.unwrap().digest, digest("a\nb\nc\n"));
            assert_eq!(head.unwrap().digest, digest("a\nB\nc\n"));
        }
    }

    #[test]
    fn text_that_isnt_the_recorded_version_is_not_used() {
        // Same digests in `files`, but the blobs hold something else: the
        // context must come from the hunks rather than from a wrong file.
        let old = numbered(20);
        let c = change(Some(&old), Some(&edited(&old, &[(2, Some("L2"))])));
        let lines: Vec<&str> = c.diff.lines().collect();
        let index = lines.iter().position(|l| *l == " l5").unwrap();
        let mut with_comment = lines.clone();
        with_comment.insert(index + 1, "> comment");
        let parsed = annotation::parse(&(with_comment.join("\n") + "\n")).unwrap();
        let Item::NewThread { scope, .. } = &parsed.items[0] else {
            panic!()
        };
        let mut blobs = Blobs::default();
        blobs.add(b"something\nelse\nentirely\n");
        let a = build_anchor(scope, &parsed.diff, &c.files, &revisions(), &blobs, 3).unwrap();
        let (_, head) = sides(&a);
        // From the hunk (which ends at line 5), not from the wrong text.
        assert_eq!(head.unwrap().context.before, strs(&["L2", "l3", "l4"]));
        assert!(head.unwrap().context.after.is_empty());
    }

    #[test]
    fn a_whole_hunk_is_a_span_over_both_of_its_ranges() {
        let old = numbered(12);
        let c = change(Some(&old), Some(&edited(&old, &[(6, Some("L6")), (7, None)])));
        // A comment straight after the hunk header.
        let index = c.diff.lines().position(|l| l.starts_with("@@")).unwrap();
        let a = anchor_after_line(&c, index, true);
        let (base, head) = sides(&a);
        let (base, head) = (base.unwrap(), head.unwrap());
        assert_eq!((base.start, base.len()), (3, 8));
        assert_eq!((head.start, head.len()), (3, 7));
        assert_eq!(base.context.target.first().map(String::as_str), Some("l3"));
        assert_eq!(base.context.target.last().map(String::as_str), Some("l10"));
        assert_eq!(head.context.target.last().map(String::as_str), Some("l10"));
        assert_eq!(head.context.after, strs(&["l11", "l12"]));
        assert_eq!(head.context.before, strs(&["l1", "l2"]));
    }

    #[test]
    fn a_renamed_file_has_each_side_under_its_own_path_and_digest() {
        let diff = "\
diff --git a/old.txt b/new.txt
similarity index 90%
rename from old.txt
rename to new.txt
--- a/old.txt
+++ b/new.txt
@@ -1,3 +1,3 @@
 a
-b
+B
 c
> comment
";
        let parsed = annotation::parse(diff).unwrap();
        let Item::NewThread { scope, .. } = &parsed.items[0] else {
            panic!()
        };
        let files = vec![FileDigest {
            old_path: Some("old.txt".into()),
            new_path: Some("new.txt".into()),
            old: Some(digest("a\nb\nc\n")),
            new: Some(digest("a\nB\nc\n")),
        }];
        let mut blobs = Blobs::default();
        blobs.add(b"a\nb\nc\n");
        blobs.add(b"a\nB\nc\n");
        let a = build_anchor(scope, &parsed.diff, &files, &revisions(), &blobs, 3).unwrap();
        let (base, head) = sides(&a);
        let (base, head) = (base.unwrap(), head.unwrap());
        assert_eq!((base.file.as_str(), head.file.as_str()), ("old.txt", "new.txt"));
        assert_eq!(base.digest, digest("a\nb\nc\n"));
        assert_eq!(head.digest, digest("a\nB\nc\n"));
        // The comment is after the trailing context line ` c`.
        assert_eq!((base.start, head.start), (3, 3));
        assert_eq!(head.context.target, strs(&["c"]));
        assert_eq!(head.context.before, strs(&["a", "B"]));
        assert_eq!(base.context.before, strs(&["a", "b"]));
    }

    #[test]
    fn global_and_file_anchors_record_the_revisions_and_both_file_versions() {
        let c = change(Some("a\nb\n"), Some("a\nB\n"));
        let parsed = annotation::parse(&format!("> global\n\n{}> file\n", {
            let mut d = String::new();
            for l in c.diff.lines().take(3) {
                d.push_str(l);
                d.push('\n');
            }
            d
        }))
        .unwrap();
        let scopes: Vec<_> = parsed
            .items
            .iter()
            .map(|i| match i {
                Item::NewThread { scope, .. } => scope.clone(),
                _ => panic!(),
            })
            .collect();
        let blobs = Blobs::default();
        let g = build_anchor(&scopes[0], &parsed.diff, &c.files, &revisions(), &blobs, 3).unwrap();
        assert_eq!(
            g,
            Anchor::Global {
                base: Some("base-rev".into()),
                head: Some("head-rev".into())
            }
        );
        let f = build_anchor(&scopes[1], &parsed.diff, &c.files, &revisions(), &blobs, 3).unwrap();
        let Anchor::File { base, head } = f else {
            panic!()
        };
        assert_eq!(base.unwrap().digest, digest("a\nb\n"));
        assert_eq!(head.unwrap().digest, digest("a\nB\n"));
    }

    /// For a comment after *every* line of a diff: the anchor's ranges are
    /// where the text really is in the file, and the diff-only context is
    /// always a prefix/suffix of the full-file one (never something else).
    fn check_every_position(c: &Change) {
        let (old, new) = (c.old.as_deref().unwrap_or(""), c.new.as_deref().unwrap_or(""));
        let text_of = |t: &str, start: u32, len: u32| -> Vec<String> {
            t.lines()
                .skip(start as usize - 1)
                .take(len as usize)
                .map(str::to_string)
                .collect()
        };
        let first_hunk = c.diff.lines().position(|l| l.starts_with("@@")).unwrap();
        for (i, line) in c.diff.lines().enumerate().skip(first_hunk + 1) {
            if !(line.starts_with(' ') || line.starts_with('+') || line.starts_with('-')) {
                continue; // `diff --git` of the next file etc.
            }
            let full = anchor_after_line(c, i, true);
            let bare = anchor_after_line(c, i, false);
            let (fb, fh) = sides(&full);
            let (bb, bh) = sides(&bare);
            for (text, f, b) in [(old, fb, bb), (new, fh, bh)] {
                let (Some(f), Some(b)) = (f, b) else {
                    assert!(f.is_none() && b.is_none(), "line {i}: {line:?}");
                    continue;
                };
                assert_eq!((f.start, &f.context.target), (b.start, &b.context.target), "{line:?}");
                assert_eq!(
                    f.context.target,
                    text_of(text, f.start, f.len()),
                    "range text is not what the file has at those lines ({line:?})"
                );
                assert!(
                    f.context.before.ends_with(&b.context.before),
                    "{line:?}: {:?} vs {:?}",
                    f.context.before,
                    b.context.before
                );
                assert!(
                    f.context.after.starts_with(&b.context.after),
                    "{line:?}: {:?} vs {:?}",
                    f.context.after,
                    b.context.after
                );
                // The full context is exactly the (up to 3) lines around it.
                let (s, n) = (f.start as usize, f.len() as usize);
                let lines: Vec<&str> = text.lines().collect();
                let want_before: Vec<String> = lines[s.saturating_sub(4).min(s - 1)..s - 1]
                    .iter()
                    .map(|l| l.to_string())
                    .collect();
                let want_after: Vec<String> = lines[(s - 1 + n).min(lines.len())..]
                    .iter()
                    .take(3)
                    .map(|l| l.to_string())
                    .collect();
                assert_eq!(f.context.before, want_before, "{line:?}");
                assert_eq!(f.context.after, want_after, "{line:?}");
            }
        }
    }

    #[test]
    fn every_comment_position_anchors_to_where_its_text_really_is() {
        let old = numbered(30);
        let new = edited(
            &old,
            &[
                (2, Some("L2")),
                (3, None),
                (4, Some("L4a\nL4b")),
                (15, Some("L15")),
                (16, Some("L16")),
                (17, None),
                (28, Some("L28")),
            ],
        );
        check_every_position(&change(Some(&old), Some(&new)));
        check_every_position(&two_hunks());
        check_every_position(&change(Some("a\nb\nc\n"), Some("a\nb\nc\nd\n")));
        check_every_position(&change(Some("a\nb\nc\nd\n"), Some("d\n")));
    }
}
