//! Turns the position a comment was placed at (an [`AnchorScope`]: the
//! lines it covers on each side of the diff) into the [`Anchor`] that is
//! persisted: for each side, the file version (its digest) and the line
//! range in it. No text is kept -- the versions themselves are in the
//! bundle.

use crate::diff::UnifiedDiff;
use crate::messages::mf;
use crate::model::{Anchor, FileDigest, FileRef, LineRange, Side};

/// A run of lines in one version of a file. `len == 0` is an insertion
/// point: `start` is the line the (absent) text would sit *before*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSpan {
    pub start: u32,
    pub len: u32,
}

/// Where a new comment is placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorScope {
    Global,
    File {
        file: String,
    },
    /// What the comment covers on each side. A context line is a one-line
    /// span on both; an added line an empty `base` and a one-line `head`; a
    /// removed line the reverse; a run of lines the whole run on both
    /// sides. Never empty on both.
    Span {
        file: String,
        base: LineSpan,
        head: LineSpan,
    },
}

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
                    anyhow::anyhow!(mf("create.internal_file_missing", &[("file", file)]))
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
    use crate::diff;
    use crate::digest::digest;
    use crate::files::{Tree, diff_trees};

    /// A one-file change: the unified diff between `old` and `new` (as the
    /// directory provider makes it) and the digests of both versions.
    struct Change {
        diff: UnifiedDiff,
        files: Vec<FileDigest>,
    }

    fn change(old: Option<&str>, new: Option<&str>) -> Change {
        let tree = |t: Option<&str>| -> Tree {
            t.map(|t| ("f.txt".to_string(), t.as_bytes().to_vec()))
                .into_iter()
                .collect()
        };
        let (text, files) = diff_trees(&tree(old), &tree(new));
        Change {
            diff: diff::parse(&text).unwrap(),
            files,
        }
    }

    fn revisions() -> (Option<String>, Option<String>) {
        (Some("base-rev".into()), Some("head-rev".into()))
    }

    /// The anchor of a comment on `f.txt` covering `base` and `head`
    /// (`(start, len)` each).
    fn span(c: &Change, base: (u32, u32), head: (u32, u32)) -> Anchor {
        let scope = AnchorScope::Span {
            file: "f.txt".into(),
            base: LineSpan {
                start: base.0,
                len: base.1,
            },
            head: LineSpan {
                start: head.0,
                len: head.1,
            },
        };
        build_anchor(&scope, &c.diff, &c.files, &revisions()).unwrap()
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

    #[test]
    fn each_side_keeps_its_lines_and_the_digest_of_its_own_version() {
        let c = change(Some("a\nb\nc\n"), Some("a\nB\nc\nd\n"));
        let a = span(&c, (2, 1), (2, 2));
        let (base, head) = ranges(&a);
        assert_eq!((at(base), at(head)), ((2, 1), (2, 2)));
        assert_eq!(base.unwrap().file, "f.txt");
        assert_eq!(base.unwrap().digest, digest("a\nb\nc\n"));
        assert_eq!(head.unwrap().digest, digest("a\nB\nc\nd\n"));
    }

    #[test]
    fn an_insertion_point_is_kept_as_an_empty_range() {
        let c = change(Some("a\nb\n"), Some("a\nX\nb\n"));
        let a = span(&c, (2, 0), (2, 1));
        let (base, head) = ranges(&a);
        assert_eq!(at(base), (2, 0));
        assert!(base.unwrap().is_empty());
        assert_eq!(at(head), (2, 1));
    }

    #[test]
    fn added_and_deleted_files_have_only_one_side() {
        let added = change(None, Some("a\nb\nc\n"));
        let a = span(&added, (1, 0), (2, 1));
        let (base, head) = ranges(&a);
        assert!(base.is_none());
        assert_eq!(at(head), (2, 1));
        assert_eq!(head.unwrap().digest, digest("a\nb\nc\n"));

        let deleted = change(Some("a\nb\nc\n"), None);
        let a = span(&deleted, (2, 1), (1, 0));
        let (base, head) = ranges(&a);
        assert!(head.is_none());
        assert_eq!(at(base), (2, 1));
        assert_eq!(base.unwrap().digest, digest("a\nb\nc\n"));
    }

    #[test]
    fn a_renamed_file_has_each_side_under_its_own_path_and_digest() {
        let text = "\
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
";
        let parsed = diff::parse(text).unwrap();
        let files = vec![FileDigest {
            old_path: Some("old.txt".into()),
            new_path: Some("new.txt".into()),
            old: Some(digest("a\nb\nc\n")),
            new: Some(digest("a\nB\nc\n")),
        }];
        let scope = AnchorScope::Span {
            file: "new.txt".into(),
            base: LineSpan { start: 3, len: 1 },
            head: LineSpan { start: 3, len: 1 },
        };
        let a = build_anchor(&scope, &parsed, &files, &revisions()).unwrap();
        let (base, head) = ranges(&a);
        let (base, head) = (base.unwrap(), head.unwrap());
        assert_eq!(
            (base.file.as_str(), head.file.as_str()),
            ("old.txt", "new.txt")
        );
        assert_eq!(base.digest, digest("a\nb\nc\n"));
        assert_eq!(head.digest, digest("a\nB\nc\n"));
    }

    #[test]
    fn global_and_file_anchors_record_the_revisions_and_both_file_versions() {
        let c = change(Some("a\nb\n"), Some("a\nB\n"));
        let g = build_anchor(&AnchorScope::Global, &c.diff, &c.files, &revisions()).unwrap();
        assert_eq!(
            g,
            Anchor::Global {
                base: Some("base-rev".into()),
                head: Some("head-rev".into())
            }
        );
        let file = AnchorScope::File {
            file: "f.txt".into(),
        };
        let Anchor::File { base, head } =
            build_anchor(&file, &c.diff, &c.files, &revisions()).unwrap()
        else {
            panic!()
        };
        assert_eq!(base.unwrap().digest, digest("a\nb\n"));
        assert_eq!(head.unwrap().digest, digest("a\nB\n"));
    }

    #[test]
    fn a_span_in_a_file_the_diff_does_not_have_is_refused() {
        let c = change(Some("a\n"), Some("b\n"));
        let scope = AnchorScope::Span {
            file: "other.txt".into(),
            base: LineSpan { start: 1, len: 1 },
            head: LineSpan { start: 1, len: 1 },
        };
        assert!(build_anchor(&scope, &c.diff, &c.files, &revisions()).is_err());
    }
}
