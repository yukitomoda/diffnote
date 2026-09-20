//! Turns the position a comment was written at in the editor (an
//! [`AnchorScope`], derived from the diff's line counters) into the
//! [`Anchor`] that is persisted: for each side, the file version (its
//! digest) and the line range in it. No text is kept -- the versions
//! themselves are in the bundle.

use crate::annotation::{AnchorScope, LineSpan};
use crate::diff::UnifiedDiff;
use crate::model::{Anchor, FileDigest, FileRef, LineRange, Side};

/// `revisions` are the (base, head) ids of the reviewed change; `files` the
/// digests of the files the diff touches.
pub fn build_anchor(
    scope: &AnchorScope,
    parsed_diff: &UnifiedDiff,
    files: &[FileDigest],
    revisions: &(Option<String>, Option<String>),
) -> anyhow::Result<Anchor> {
    match scope {
        AnchorScope::Global => Ok(Anchor::Global {
            base: revisions.0.clone(),
            head: revisions.1.clone(),
        }),
        AnchorScope::File { file } => {
            let entry = files.iter().find(|f| {
                f.new_path.as_deref() == Some(file) || f.old_path.as_deref() == Some(file)
            });
            let make = |path: Option<&String>, digest: Option<&String>| {
                path.map(|p| FileRef {
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
                    anyhow::anyhow!("内部エラー: ファイル '{file}' が解析済みの差分にありません")
                })?;
            let range = |which: Side, path: Option<&String>, span: &LineSpan| {
                path.map(|path| LineRange {
                    file: path.clone(),
                    digest: crate::anchor::digest_for(files, path, which)
                        .unwrap_or_default()
                        .to_string(),
                    start: span.start,
                    len: span.len,
                })
            };
            Ok(Anchor::Span {
                base: range(Side::Old, file_diff.old_path.as_ref(), base),
                head: range(Side::New, file_diff.new_path.as_ref(), head),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::{self, Item};
    use crate::digest::digest;
    use crate::files::{Tree, diff_trees};

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

    fn parse_with_comment(
        c: &Change,
        lines: Vec<String>,
    ) -> (crate::diff::UnifiedDiff, AnchorScope) {
        let text = lines.join("\n") + "\n";
        let parsed = annotation::parse(&text).unwrap();
        let Item::NewThread { scope, .. } = &parsed.items[0] else {
            panic!("expected a thread in:\n{text}\n({})", c.diff)
        };
        (parsed.diff.clone(), scope.clone())
    }

    /// The anchor of a comment written right after the diff line `index`
    /// (0-based, counting every line of `diff`).
    fn anchor_after_line(c: &Change, index: usize) -> Anchor {
        let mut lines: Vec<String> = c.diff.lines().map(str::to_string).collect();
        lines.insert(index + 1, "> comment".to_string());
        let (diff, scope) = parse_with_comment(c, lines);
        build_anchor(&scope, &diff, &c.files, &revisions()).unwrap()
    }

    /// The anchor for a comment after the first diff line equal to `line`.
    fn anchor_after(c: &Change, line: &str) -> Anchor {
        let index = c
            .diff
            .lines()
            .position(|l| l == line)
            .unwrap_or_else(|| panic!("no diff line {line:?} in:\n{}", c.diff));
        anchor_after_line(c, index)
    }

    fn ranges(a: &Anchor) -> (Option<&LineRange>, Option<&LineRange>) {
        let Anchor::Span { base, head } = a else {
            panic!("expected a span, got {a:?}")
        };
        (base.as_ref(), head.as_ref())
    }

    fn at(r: Option<&LineRange>) -> (u32, u32) {
        let r = r.expect("that side exists");
        (r.start, r.len)
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
    fn a_context_line_is_a_one_line_range_on_both_sides() {
        let a = anchor_after(&two_hunks(), " l5");
        let (base, head) = ranges(&a);
        assert_eq!(at(base), (5, 1));
        assert_eq!(at(head), (5, 1));
        assert_eq!(base.unwrap().file, "f.txt");
        assert_eq!(base.unwrap().digest, digest(numbered(20)));
        assert_eq!(
            head.unwrap().digest,
            digest(edited(&numbered(20), &[(2, Some("L2")), (19, Some("L19"))]))
        );
    }

    #[test]
    fn numbering_in_a_later_hunk_follows_the_diff_not_the_hunk() {
        let a = anchor_after(&two_hunks(), " l16");
        let (base, head) = ranges(&a);
        assert_eq!((at(base), at(head)), ((16, 1), (16, 1)));
    }

    #[test]
    fn an_added_line_is_an_insertion_point_on_the_base_side() {
        let c = change(
            Some("a\nb\nc\nd\ne\nf\ng\n"),
            Some("a\nb\nc\nX\nd\ne\nf\ng\n"),
        );
        let a = anchor_after(&c, "+X");
        let (base, head) = ranges(&a);
        assert_eq!(at(head), (4, 1));
        // Between base lines 3 and 4: it sits before line 4 (`d`).
        assert_eq!(at(base), (4, 0));
        assert!(base.unwrap().is_empty());
    }

    #[test]
    fn a_removed_line_is_an_insertion_point_on_the_head_side() {
        let c = change(Some("a\nb\nc\nd\ne\nf\ng\n"), Some("a\nb\nc\ne\nf\ng\n"));
        let a = anchor_after(&c, "-d");
        let (base, head) = ranges(&a);
        assert_eq!(at(base), (4, 1));
        assert_eq!(at(head), (4, 0));
    }

    #[test]
    fn a_range_covers_its_lines_on_each_side() {
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
        let (diff, scope) = parse_with_comment(&c, lines);
        let a = build_anchor(&scope, &diff, &c.files, &revisions()).unwrap();
        let (base, head) = ranges(&a);
        assert_eq!(at(base), (3, 3));
        assert_eq!(at(head), (3, 3));
    }

    #[test]
    fn a_range_of_only_added_lines_is_empty_on_the_base_side() {
        let c = change(Some("a\nb\n"), Some("a\nX\nY\nb\n"));
        let mut lines: Vec<String> = c.diff.lines().map(str::to_string).collect();
        let open = lines.iter().position(|l| l == "+X").unwrap();
        lines.insert(open, ">[r".to_string());
        let close = lines.iter().position(|l| l == "+Y").unwrap();
        lines.insert(close + 1, ">]r".to_string());
        lines.insert(close + 2, "> two new lines".to_string());
        let (diff, scope) = parse_with_comment(&c, lines);
        let a = build_anchor(&scope, &diff, &c.files, &revisions()).unwrap();
        let (base, head) = ranges(&a);
        assert_eq!(at(head), (2, 2));
        assert_eq!(at(base), (2, 0));
    }

    #[test]
    fn a_whole_hunk_is_a_span_over_both_of_its_ranges() {
        let old = numbered(12);
        let c = change(
            Some(&old),
            Some(&edited(&old, &[(6, Some("L6")), (7, None)])),
        );
        // A comment straight after the hunk header.
        let index = c.diff.lines().position(|l| l.starts_with("@@")).unwrap();
        let a = anchor_after_line(&c, index);
        let (base, head) = ranges(&a);
        // Old lines 3..=10, new lines 3..=9.
        assert_eq!(at(base), (3, 8));
        assert_eq!(at(head), (3, 7));
    }

    #[test]
    fn added_and_deleted_files_have_only_one_side() {
        let added = change(None, Some("a\nb\nc\n"));
        let a = anchor_after(&added, "+b");
        let (base, head) = ranges(&a);
        assert!(base.is_none());
        assert_eq!(at(head), (2, 1));
        assert_eq!(head.unwrap().digest, digest("a\nb\nc\n"));

        let deleted = change(Some("a\nb\nc\n"), None);
        let a = anchor_after(&deleted, "-b");
        let (base, head) = ranges(&a);
        assert!(head.is_none());
        assert_eq!(at(base), (2, 1));
        assert_eq!(base.unwrap().digest, digest("a\nb\nc\n"));
    }

    #[test]
    fn each_side_records_the_digest_of_its_own_version() {
        let c = change(Some("a\nb\nc\n"), Some("a\nB\nc\n"));
        let a = anchor_after(&c, " a");
        let (base, head) = ranges(&a);
        assert_eq!(base.unwrap().digest, digest("a\nb\nc\n"));
        assert_eq!(head.unwrap().digest, digest("a\nB\nc\n"));
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
        let a = build_anchor(scope, &parsed.diff, &files, &revisions()).unwrap();
        let (base, head) = ranges(&a);
        let (base, head) = (base.unwrap(), head.unwrap());
        assert_eq!(
            (base.file.as_str(), head.file.as_str()),
            ("old.txt", "new.txt")
        );
        assert_eq!(base.digest, digest("a\nb\nc\n"));
        assert_eq!(head.digest, digest("a\nB\nc\n"));
        // The comment is after the trailing context line ` c`.
        assert_eq!((at(Some(base)), at(Some(head))), ((3, 1), (3, 1)));
    }

    #[test]
    fn global_and_file_anchors_record_the_revisions_and_both_file_versions() {
        let c = change(Some("a\nb\n"), Some("a\nB\n"));
        let header: String = c.diff.lines().take(3).map(|l| format!("{l}\n")).collect();
        let parsed = annotation::parse(&format!("> global\n\n{header}> file\n")).unwrap();
        let scopes: Vec<_> = parsed
            .items
            .iter()
            .map(|i| match i {
                Item::NewThread { scope, .. } => scope.clone(),
                _ => panic!(),
            })
            .collect();
        let g = build_anchor(&scopes[0], &parsed.diff, &c.files, &revisions()).unwrap();
        assert_eq!(
            g,
            Anchor::Global {
                base: Some("base-rev".into()),
                head: Some("head-rev".into())
            }
        );
        let f = build_anchor(&scopes[1], &parsed.diff, &c.files, &revisions()).unwrap();
        let Anchor::File { base, head } = f else {
            panic!()
        };
        assert_eq!(base.unwrap().digest, digest("a\nb\n"));
        assert_eq!(head.unwrap().digest, digest("a\nB\n"));
    }

    /// For a comment after *every* line of a diff: each side's range is where
    /// the diff's line really is in that version of the file (and a side with
    /// no line there is an empty range).
    fn check_every_position(c: &Change) {
        let old_lines: Vec<&str> = c.old.as_deref().unwrap_or("").lines().collect();
        let new_lines: Vec<&str> = c.new.as_deref().unwrap_or("").lines().collect();
        let first_hunk = c.diff.lines().position(|l| l.starts_with("@@")).unwrap();
        for (i, line) in c.diff.lines().enumerate().skip(first_hunk + 1) {
            let (marker, content) = match line.chars().next() {
                Some(m @ (' ' | '+' | '-')) => (m, &line[1..]),
                _ => continue, // the next file's header etc.
            };
            let a = anchor_after_line(c, i);
            let (base, head) = ranges(&a);
            let (has_base, has_head) = (marker != '+', marker != '-');
            for (r, has, lines) in [(base, has_base, &old_lines), (head, has_head, &new_lines)] {
                let Some(r) = r else {
                    assert!(
                        lines.is_empty(),
                        "{line:?}: side missing but the file has lines"
                    );
                    continue;
                };
                if has {
                    assert_eq!(r.len, 1, "{line:?}");
                    assert_eq!(
                        lines[r.start as usize - 1],
                        content,
                        "{line:?} is not at line {} of its version",
                        r.start
                    );
                } else {
                    // A point: it sits between the line before and the one after.
                    assert_eq!(r.len, 0, "{line:?}");
                    assert!(
                        r.start >= 1 && r.start as usize <= lines.len() + 1,
                        "{line:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_comment_position_anchors_to_where_its_line_really_is() {
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
        check_every_position(&change(None, Some("only\nnew\n")));
        check_every_position(&change(Some("only\nold\n"), None));
    }
}
