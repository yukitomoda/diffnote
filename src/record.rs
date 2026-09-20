//! What an `edit` session adds to a bundle besides its events: the new
//! revision (if the diff is one the bundle hasn't recorded), and the file
//! versions that everything the review refers to needs to be placeable from
//! the bundle alone.
//!
//! The rule that matters: **every file a comment refers to must be in the
//! bundle**, whether or not the diff touches it -- a comment on a README
//! that nobody changed still has to show up in every later revision. Files
//! the diff touches come with the revision; the others are read at the head
//! and *pinned* (recorded in the revision's tree, or, for a revision that
//! was already recorded, in a `Pin` event).

use crate::bundle::{Additions, Loaded};
use crate::digest::digest;
use crate::files::Tree;
use crate::model::{Anchor, Event, FileDigest, Revision, SnapshotMode, Source, TreeFile};
use anyhow::Result;
use std::collections::{BTreeSet, HashSet};
use time::OffsetDateTime;
use ulid::Ulid;

/// The reviewed change, as one session saw it.
pub struct Capture<'a> {
    pub diff_text: &'a str,
    /// The revision's digest (see `Revision::digest`).
    pub diff_digest: &'a str,
    pub source: Source,
    /// Per-file digests of what the diff touches.
    pub files: &'a [FileDigest],
    /// Head-side content of the files the diff touches.
    pub new_files: &'a Tree,
    /// Base-side content of the files the diff touches, when the base isn't
    /// already in the bundle.
    pub base_files: &'a Tree,
}

/// Reads the head content of the given paths (missing ones are skipped).
pub type HeadSome<'a> = &'a dyn Fn(&[String]) -> Result<Vec<(String, Vec<u8>)>>;

/// The whole head tree, for a `full` snapshot.
pub type HeadAll<'a> = Box<dyn FnOnce() -> Result<Vec<(String, Vec<u8>)>> + 'a>;

pub fn tree_file(path: &str, bytes: &[u8]) -> TreeFile {
    TreeFile {
        path: path.to_string(),
        digest: digest(bytes),
    }
}

/// Every file some comment's anchor points at.
pub fn referenced_files<'a>(events: impl Iterator<Item = &'a Event>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for event in events {
        let anchor = match event {
            Event::Comment {
                anchor: Some(a), ..
            }
            | Event::Reanchor { anchor: a, .. } => a,
            _ => continue,
        };
        match anchor {
            Anchor::Global { .. } => {}
            Anchor::File { base, head } => {
                out.extend(base.iter().chain(head.iter()).map(|f| f.file.clone()));
            }
            Anchor::Span { base, head } => {
                out.extend(base.iter().chain(head.iter()).map(|s| s.file.clone()));
            }
        }
    }
    out
}

/// Decides what goes into the bundle for this session, pushing the
/// `Revision` (a not-yet-seen diff) or `Pin` (files a recorded revision
/// didn't know) event onto `new_events`, and returns the diff text and file
/// versions to store.
///
/// `snapshot_mode` is only asked when a new revision is recorded.
/// `head_some` reads the given paths at the head (missing ones are skipped);
/// `head_all` reads the whole head tree, and is only called for `Full`.
pub fn record_session(
    loaded: &Loaded,
    new_events: &mut Vec<Event>,
    capture: Capture,
    snapshot_mode: &dyn Fn() -> SnapshotMode,
    head_some: HeadSome,
    head_all: HeadAll,
) -> Result<Additions> {
    let referenced = referenced_files(loaded.events.iter().chain(new_events.iter()));
    let touched: HashSet<&str> = capture
        .files
        .iter()
        .flat_map(|f| [f.old_path.as_deref(), f.new_path.as_deref()])
        .flatten()
        .collect();
    let untouched: Vec<String> = referenced
        .iter()
        .filter(|p| !touched.contains(p.as_str()))
        .cloned()
        .collect();

    if let Some(recorded) = loaded
        .revisions()
        .find(|r| r.digest == capture.diff_digest)
    {
        // Already recorded: only what this session's comments newly refer to.
        let known: HashSet<String> = loaded
            .manifest(recorded)
            .into_iter()
            .map(|f| f.path)
            .collect();
        let missing: Vec<String> = untouched
            .into_iter()
            .filter(|p| !known.contains(p))
            .collect();
        let read = if missing.is_empty() {
            Vec::new()
        } else {
            head_some(&missing)?
        };
        if !read.is_empty() {
            new_events.push(Event::Pin {
                revision: recorded.id,
                files: read.iter().map(|(p, b)| tree_file(p, b)).collect(),
            });
        }
        return Ok(Additions {
            diff: None,
            blobs: read.into_iter().map(|(_, b)| b).collect(),
        });
    }

    let mode = snapshot_mode();
    let (tree, mut blobs): (Vec<TreeFile>, Vec<Vec<u8>>) = match mode {
        SnapshotMode::Full => {
            let all = head_all()?;
            (
                all.iter().map(|(p, b)| tree_file(p, b)).collect(),
                all.into_iter().map(|(_, b)| b).collect(),
            )
        }
        SnapshotMode::Changed => {
            let pinned = if untouched.is_empty() {
                Vec::new()
            } else {
                head_some(&untouched)?
            };
            let mut tree: Vec<TreeFile> = capture
                .new_files
                .iter()
                .map(|(p, b)| tree_file(p, b))
                .collect();
            tree.extend(pinned.iter().map(|(p, b)| tree_file(p, b)));
            let mut blobs: Vec<Vec<u8>> = capture.new_files.values().cloned().collect();
            blobs.extend(pinned.into_iter().map(|(_, b)| b));
            (tree, blobs)
        }
    };
    blobs.extend(capture.base_files.values().cloned());

    // Right after Meta if this session creates it, else first.
    let at = usize::from(matches!(new_events.first(), Some(Event::Meta { .. })));
    new_events.insert(
        at,
        Event::Revision(Revision {
            id: Ulid::new(),
            created_at: OffsetDateTime::now_utc(),
            digest: capture.diff_digest.to_string(),
            source: capture.source,
            snapshot_mode: mode,
            files: capture.files.to_vec(),
            tree,
        }),
    );
    Ok(Additions {
        diff: Some((capture.diff_digest.to_string(), capture.diff_text.to_string())),
        blobs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle;
    use crate::model::{FileRef, LineRange};
    use std::cell::{Cell, RefCell};

    fn tree(files: &[(&str, &str)]) -> Tree {
        files
            .iter()
            .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
            .collect()
    }

    fn touched(path: &str, old: Option<&str>, new: Option<&str>) -> FileDigest {
        FileDigest {
            old_path: old.map(|_| path.to_string()),
            new_path: new.map(|_| path.to_string()),
            old: old.map(digest),
            new: new.map(digest),
        }
    }

    fn side(file: &str) -> LineRange {
        LineRange {
            file: file.to_string(),
            digest: "sha256:whatever".to_string(),
            start: 1,
            len: 1,
        }
    }

    fn comment_with(anchor: Anchor) -> Event {
        Event::Comment {
            id: Ulid::new(),
            parent: None,
            author: "r".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            anchor: Some(anchor),
            body: "c".into(),
        }
    }

    fn comment_on(file: &str) -> Event {
        comment_with(Anchor::Span {
            base: Some(side(file)),
            head: Some(side(file)),
        })
    }

    fn meta() -> Event {
        Event::Meta {
            version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            description: None,
            context_lines: 3,
        }
    }

    fn git_source() -> Source {
        Source::Git(crate::model::GitSource {
            base: "b".into(),
            head: "h".into(),
            spec: "b..h".into(),
        })
    }

    /// A fake head tree that counts how it is read.
    struct Head {
        files: Vec<(String, Vec<u8>)>,
        some_calls: RefCell<Vec<Vec<String>>>,
        all_calls: Cell<u32>,
    }

    impl Head {
        fn new(files: &[(&str, &str)]) -> Self {
            Head {
                files: files
                    .iter()
                    .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
                    .collect(),
                some_calls: RefCell::default(),
                all_calls: Cell::new(0),
            }
        }

        fn some(&self, paths: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
            self.some_calls.borrow_mut().push(paths.to_vec());
            Ok(self
                .files
                .iter()
                .filter(|(p, _)| paths.contains(p))
                .cloned()
                .collect())
        }

        fn all(&self) -> HeadAll<'_> {
            Box::new(move || {
                self.all_calls.set(self.all_calls.get() + 1);
                Ok(self.files.clone())
            })
        }
    }

    /// What `record_session` did, for a change touching `calc.rs` (edited)
    /// and `gone.rs` (deleted), against a head that also has `README.md`
    /// and `docs.md`.
    struct Outcome {
        events: Vec<Event>,
        additions: Additions,
        head: Head,
        mode_asked: Cell<u32>,
    }

    fn run(
        loaded: &bundle::Loaded,
        mut events: Vec<Event>,
        mode: SnapshotMode,
        digest_of_diff: &str,
    ) -> Outcome {
        let head = Head::new(&[
            ("calc.rs", "calc v2"),
            ("README.md", "readme v1"),
            ("docs.md", "docs v1"),
        ]);
        let files = vec![
            touched("calc.rs", Some("calc v1"), Some("calc v2")),
            touched("gone.rs", Some("gone v1"), None),
        ];
        let new_files = tree(&[("calc.rs", "calc v2")]);
        let base_files = tree(&[("calc.rs", "calc v1"), ("gone.rs", "gone v1")]);
        let mode_asked = Cell::new(0);
        let additions = record_session(
            loaded,
            &mut events,
            Capture {
                diff_text: "the diff",
                diff_digest: digest_of_diff,
                source: git_source(),
                files: &files,
                new_files: &new_files,
                base_files: &base_files,
            },
            &|| {
                mode_asked.set(mode_asked.get() + 1);
                mode
            },
            &|paths| head.some(paths),
            head.all(),
        )
        .unwrap();
        Outcome {
            events,
            additions,
            head,
            mode_asked,
        }
    }

    fn empty() -> bundle::Loaded {
        bundle::load(std::path::Path::new("/nonexistent/none.diffnote")).unwrap()
    }

    fn revision_of(events: &[Event]) -> &Revision {
        events
            .iter()
            .find_map(|e| match e {
                Event::Revision(r) => Some(r),
                _ => None,
            })
            .expect("a revision event")
    }

    fn paths(tree: &[TreeFile]) -> Vec<&str> {
        let mut v: Vec<&str> = tree.iter().map(|f| f.path.as_str()).collect();
        v.sort();
        v
    }

    fn blob_texts(additions: &Additions) -> Vec<String> {
        let mut v: Vec<String> = additions
            .blobs
            .iter()
            .map(|b| String::from_utf8(b.clone()).unwrap())
            .collect();
        v.sort();
        v
    }

    /// Saves `events` + `additions` to a real bundle and reloads it.
    fn persist(
        dir: &tempfile::TempDir,
        events: &[Event],
        additions: &Additions,
    ) -> bundle::Loaded {
        let path = dir.path().join("r.diffnote");
        bundle::save(&path, &bundle::load(&path).unwrap(), events, additions).unwrap();
        bundle::load(&path).unwrap()
    }

    #[test]
    fn a_new_revision_records_the_diff_and_both_sides_of_the_files_it_touches() {
        let o = run(&empty(), vec![], SnapshotMode::Changed, "sha256:d1");
        assert_eq!(
            o.additions.diff,
            Some(("sha256:d1".to_string(), "the diff".to_string()))
        );
        assert_eq!(
            blob_texts(&o.additions),
            ["calc v1", "calc v2", "gone v1"],
            "head of touched files and base of touched files, nothing else"
        );
        let r = revision_of(&o.events);
        assert_eq!(r.digest, "sha256:d1");
        assert_eq!(r.snapshot_mode, SnapshotMode::Changed);
        assert_eq!(r.files.len(), 2);
        // Only what exists at the head is in the tree: no `gone.rs`.
        assert_eq!(paths(&r.tree), ["calc.rs"]);
        assert_eq!(r.tree[0].digest, digest("calc v2"));
        assert_eq!(o.mode_asked.get(), 1);
    }

    #[test]
    fn nothing_is_read_from_the_head_when_no_comment_needs_it() {
        let o = run(&empty(), vec![], SnapshotMode::Changed, "sha256:d1");
        assert!(o.head.some_calls.borrow().is_empty());
        assert_eq!(o.head.all_calls.get(), 0);
    }

    #[test]
    fn the_revision_goes_right_after_meta_or_first_without_it() {
        let o = run(&empty(), vec![meta()], SnapshotMode::Changed, "sha256:d1");
        assert!(matches!(o.events[0], Event::Meta { .. }));
        assert!(matches!(o.events[1], Event::Revision(_)));
        let o = run(&empty(), vec![comment_on("calc.rs")], SnapshotMode::Changed, "sha256:d1");
        assert!(matches!(o.events[0], Event::Revision(_)));
        assert!(matches!(o.events[1], Event::Comment { .. }));
    }

    #[test]
    fn files_that_comments_refer_to_are_pinned_even_if_the_diff_never_touches_them() {
        let o = run(
            &empty(),
            vec![meta(), comment_on("README.md"), comment_on("calc.rs")],
            SnapshotMode::Changed,
            "sha256:d1",
        );
        let r = revision_of(&o.events);
        assert_eq!(paths(&r.tree), ["README.md", "calc.rs"]);
        assert_eq!(
            blob_texts(&o.additions),
            ["calc v1", "calc v2", "gone v1", "readme v1"]
        );
        // Only the untouched one is read; `calc.rs` came with the diff.
        assert_eq!(*o.head.some_calls.borrow(), vec![vec!["README.md".to_string()]]);
    }

    #[test]
    fn references_from_earlier_sessions_count_too() {
        // A comment saved by an earlier session on a different revision.
        let dir = tempfile::tempdir().unwrap();
        let earlier = persist(
            &dir,
            &[meta(), comment_on("docs.md")],
            &Additions::default(),
        );
        let o = run(&earlier, vec![], SnapshotMode::Changed, "sha256:d1");
        assert_eq!(paths(&revision_of(&o.events).tree), ["calc.rs", "docs.md"]);
    }

    #[test]
    fn every_kind_of_anchor_is_counted_the_right_way() {
        let file_anchor = comment_with(Anchor::File {
            base: Some(FileRef {
                file: "README.md".into(),
                digest: "d".into(),
            }),
            head: None,
        });
        let global = comment_with(Anchor::Global {
            base: Some("b".into()),
            head: Some("h".into()),
        });
        let reanchor = Event::Reanchor {
            parent: Ulid::new(),
            author: "r".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            anchor: Anchor::Span {
                base: None,
                head: Some(side("docs.md")),
            },
        };
        let reply = Event::Comment {
            id: Ulid::new(),
            parent: Some(Ulid::new()),
            author: "r".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            anchor: None,
            body: "a reply has no anchor".into(),
        };
        let events = [file_anchor, global, reanchor, reply];
        assert_eq!(
            referenced_files(events.iter()).into_iter().collect::<Vec<_>>(),
            ["README.md", "docs.md"]
        );
        // The base side alone is enough.
        let base_only = comment_with(Anchor::Span {
            base: Some(side("old.txt")),
            head: None,
        });
        assert_eq!(
            referenced_files([base_only].iter()).into_iter().collect::<Vec<_>>(),
            ["old.txt"]
        );
    }

    #[test]
    fn a_file_that_no_longer_exists_at_the_head_is_skipped_not_an_error() {
        let o = run(
            &empty(),
            vec![comment_on("deleted-long-ago.md"), comment_on("README.md")],
            SnapshotMode::Changed,
            "sha256:d1",
        );
        assert_eq!(paths(&revision_of(&o.events).tree), ["README.md", "calc.rs"]);
    }

    #[test]
    fn a_renamed_file_counts_as_touched_by_either_path() {
        let files = vec![FileDigest {
            old_path: Some("old.rs".into()),
            new_path: Some("new.rs".into()),
            old: Some(digest("v1")),
            new: Some(digest("v2")),
        }];
        let head = Head::new(&[("new.rs", "v2")]);
        let mut events = vec![comment_on("old.rs"), comment_on("new.rs")];
        let (new_files, base_files) = (tree(&[("new.rs", "v2")]), tree(&[("old.rs", "v1")]));
        record_session(
            &empty(),
            &mut events,
            Capture {
                diff_text: "d",
                diff_digest: "sha256:r",
                source: git_source(),
                files: &files,
                new_files: &new_files,
                base_files: &base_files,
            },
            &|| SnapshotMode::Changed,
            &|paths| head.some(paths),
            head.all(),
        )
        .unwrap();
        assert!(head.some_calls.borrow().is_empty(), "both paths are the diff's own");
    }

    #[test]
    fn full_keeps_the_whole_head_tree_and_the_base_files() {
        let o = run(
            &empty(),
            vec![comment_on("README.md")],
            SnapshotMode::Full,
            "sha256:d1",
        );
        let r = revision_of(&o.events);
        assert_eq!(r.snapshot_mode, SnapshotMode::Full);
        assert_eq!(paths(&r.tree), ["README.md", "calc.rs", "docs.md"]);
        assert_eq!(
            blob_texts(&o.additions),
            ["calc v1", "calc v2", "docs v1", "gone v1", "readme v1"]
        );
        assert_eq!(o.head.all_calls.get(), 1);
        assert!(o.head.some_calls.borrow().is_empty(), "the whole tree already has them");
    }

    #[test]
    fn a_recorded_revision_is_not_recorded_again_and_asks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let first = run(&empty(), vec![meta()], SnapshotMode::Changed, "sha256:d1");
        let loaded = persist(&dir, &first.events, &first.additions);

        // A second session on the very same diff, adding a plain comment.
        let o = run(&loaded, vec![comment_on("calc.rs")], SnapshotMode::Changed, "sha256:d1");
        assert_eq!(o.mode_asked.get(), 0);
        assert!(o.additions.diff.is_none());
        assert!(o.additions.blobs.is_empty());
        assert_eq!(o.events.len(), 1);
        assert!(!o.events.iter().any(|e| matches!(e, Event::Revision(_) | Event::Pin { .. })));
        assert!(o.head.some_calls.borrow().is_empty());
    }

    #[test]
    fn a_recorded_revision_gets_a_pin_for_files_it_did_not_know() {
        let dir = tempfile::tempdir().unwrap();
        let first = run(
            &empty(),
            vec![meta(), comment_on("README.md")],
            SnapshotMode::Changed,
            "sha256:d1",
        );
        let loaded = persist(&dir, &first.events, &first.additions);
        let recorded_id = revision_of(&first.events).id;

        // README is already in the revision's tree; docs.md is new.
        let o = run(
            &loaded,
            vec![comment_on("README.md"), comment_on("docs.md")],
            SnapshotMode::Changed,
            "sha256:d1",
        );
        assert!(o.additions.diff.is_none());
        assert_eq!(blob_texts(&o.additions), ["docs v1"]);
        assert_eq!(*o.head.some_calls.borrow(), vec![vec!["docs.md".to_string()]]);
        let pins: Vec<_> = o
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Pin { revision, files } => Some((*revision, files)),
                _ => None,
            })
            .collect();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].0, recorded_id);
        assert_eq!(paths(pins[0].1), ["docs.md"]);
        assert_eq!(pins[0].1[0].digest, digest("docs v1"));
    }

    #[test]
    fn a_file_pinned_once_is_not_pinned_again_and_touched_files_never_are() {
        let dir = tempfile::tempdir().unwrap();
        let first = run(&empty(), vec![meta()], SnapshotMode::Changed, "sha256:d1");
        let loaded = persist(&dir, &first.events, &first.additions);
        let second = run(&loaded, vec![comment_on("docs.md")], SnapshotMode::Changed, "sha256:d1");
        assert!(second.events.iter().any(|e| matches!(e, Event::Pin { .. })));
        let mut all = first.events.clone();
        all.extend(second.events.iter().cloned());
        let loaded = persist(&dir, &all, &second.additions);

        // The same reference again, plus one to a file the diff touches.
        let third = run(
            &loaded,
            vec![comment_on("docs.md"), comment_on("calc.rs")],
            SnapshotMode::Changed,
            "sha256:d1",
        );
        assert!(!third.events.iter().any(|e| matches!(e, Event::Pin { .. })));
        assert!(third.additions.blobs.is_empty());
        assert!(third.head.some_calls.borrow().is_empty());
    }

    #[test]
    fn what_was_recorded_can_be_read_back_from_the_saved_bundle() {
        let dir = tempfile::tempdir().unwrap();
        // Session 1: a new revision, with a comment on README.
        let first = run(
            &empty(),
            vec![meta(), comment_on("README.md")],
            SnapshotMode::Changed,
            "sha256:d1",
        );
        let loaded = persist(&dir, &first.events, &first.additions);
        // Session 2: the same revision again, now with a comment on docs.md.
        let second = run(&loaded, vec![comment_on("docs.md")], SnapshotMode::Changed, "sha256:d1");
        let mut all = first.events.clone();
        all.extend(second.events.iter().cloned());
        let loaded = persist(&dir, &all, &second.additions);

        let revision = loaded.revisions().next().unwrap().clone();
        // The manifest is the tree plus the pin.
        assert_eq!(
            paths(&loaded.manifest(&revision)),
            ["README.md", "calc.rs", "docs.md"]
        );
        let tree = loaded.tree_of(&revision);
        assert_eq!(tree["README.md"], b"readme v1");
        assert_eq!(tree["calc.rs"], b"calc v2");
        assert_eq!(tree["docs.md"], b"docs v1");
        // Both sides of what the diff changed are findable by digest.
        let blobs = loaded.blobs();
        assert_eq!(blobs.text(&digest("calc v1")), Some("calc v1"));
        assert_eq!(blobs.text(&digest("gone v1")), Some("gone v1"));
        assert_eq!(loaded.revision_diff(&revision).as_deref(), Some("the diff"));
        // ...and calc v1 -> v2 is a recorded step between them.
        assert_eq!(
            blobs.chain(&digest("calc v1"), &digest("calc v2")).unwrap(),
            ["calc v1", "calc v2"]
        );
    }

    #[test]
    fn the_same_content_in_several_revisions_is_stored_once() {
        let dir = tempfile::tempdir().unwrap();
        let first = run(
            &empty(),
            vec![meta(), comment_on("README.md")],
            SnapshotMode::Full,
            "sha256:d1",
        );
        let loaded = persist(&dir, &first.events, &first.additions);
        // A second, different diff over the same unchanged tree.
        let second = run(&loaded, vec![], SnapshotMode::Full, "sha256:d2");
        let mut all = first.events.clone();
        all.extend(second.events.iter().cloned());
        persist(&dir, &all, &second.additions);
        let path = dir.path().join("r.diffnote");
        let archive = std::fs::File::open(&path).unwrap();
        let names: Vec<String> = zip::ZipArchive::new(archive)
            .unwrap()
            .file_names()
            .map(str::to_string)
            .collect();
        let blobs = names.iter().filter(|n| n.starts_with("blobs/")).count();
        // calc v1/v2, gone v1, README v1, docs v1: five, not ten.
        assert_eq!(blobs, 5, "{names:?}");
        assert_eq!(names.iter().filter(|n| n.starts_with("diffs/")).count(), 2);
    }
}
