//! The `.diffnote` bundle: a zip file containing a review's event log, the
//! diff text of every revision recorded, and a content-addressed store of
//! the file versions the review refers to -- so a review can be shared or
//! continued without separately handing over the repository or directory
//! it was made against. What goes into the store is decided by the caller
//! (the git provider reads committed content), not read from disk here.
//!
//! Layout inside the zip:
//! ```text
//! review.jsonl
//! diffs/<digest>.diff      one per recorded revision (its unified diff)
//! blobs/<sha256 hex>       one per distinct file version, however many
//!                          revisions have it
//! ```
//! `<digest>` is a revision's digest string (`sha256:...`) with the
//! `sha256:` prefix stripped, so it's a plain hex string safe to use as a
//! zip path component on every platform. Which blobs make up a revision's
//! head tree is in the revision's `tree` (plus later `Pin` events), not in
//! the layout.
//!
//! A bundle is always rewritten whole (read every entry into memory, then
//! write a fresh zip) rather than patched in place -- reviews are small
//! enough that this is simple and safe (no risk of a half-written zip from
//! an in-place edit), at the cost of not scaling to huge bundles.

pub use crate::model::SnapshotMode;
use crate::model::{Event, Revision, Source, TreeFile};
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

pub struct Loaded {
    pub events: Vec<Event>,
    /// Existing `diffs/`/`blobs/` entries, carried through unchanged into
    /// the rewritten archive.
    carried_entries: Vec<(String, Vec<u8>)>,
}

impl Loaded {
    /// The most recently recorded revision and its diff text, if the bundle
    /// has recorded any yet. Lets `export`/`show` render against "whatever
    /// this review was last edited against" without the caller having to
    /// re-specify the diff.
    pub fn latest_revision(&self) -> Option<(&Revision, String)> {
        let revision = self.revisions().last()?;
        Some((revision, self.revision_diff(revision)?))
    }

    /// The diff text stored for `revision`, if it has one.
    pub fn revision_diff(&self, revision: &Revision) -> Option<String> {
        let wanted = format!("diffs/{}.diff", digest_path_component(&revision.digest));
        let (_, bytes) = self
            .carried_entries
            .iter()
            .find(|(name, _)| *name == wanted)?;
        String::from_utf8(bytes.clone()).ok()
    }

    /// The source kind fixed by the bundle's first revision.
    pub fn source(&self) -> Option<&Source> {
        self.revisions().next().map(|r| &r.source)
    }

    /// Every file version in the store, findable by content digest, with
    /// each revision's file changes linked as steps between versions.
    pub fn blobs(&self) -> crate::digest::Blobs<'_> {
        let mut blobs = crate::digest::Blobs::default();
        for (name, bytes) in &self.carried_entries {
            if let Some(hex) = name.strip_prefix("blobs/") {
                blobs.add_known(format!("sha256:{hex}"), bytes);
            }
        }
        for revision in self.revisions() {
            link_revision_files(&mut blobs, &revision.files);
        }
        blobs
    }

    /// The stored bytes of the file version with this digest.
    pub fn blob(&self, digest: &str) -> Option<&[u8]> {
        let wanted = format!("blobs/{}", digest_path_component(digest));
        self.carried_entries
            .iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, bytes)| bytes.as_slice())
    }

    /// The head tree `revision` recorded: its own `tree` plus every `Pin`
    /// since (a later entry for a path replaces an earlier one).
    pub fn manifest(&self, revision: &Revision) -> Vec<TreeFile> {
        let mut out: Vec<TreeFile> = revision.tree.clone();
        for event in &self.events {
            if let Event::Pin { revision: id, files } = event
                && *id == revision.id
            {
                for f in files {
                    match out.iter_mut().find(|e| e.path == f.path) {
                        Some(existing) => existing.digest = f.digest.clone(),
                        None => out.push(f.clone()),
                    }
                }
            }
        }
        out
    }

    /// The contents of `manifest(revision)` (relative path -> bytes), for
    /// every file whose blob is in the store.
    pub fn tree_of(&self, revision: &Revision) -> crate::files::Tree {
        self.manifest(revision)
            .into_iter()
            .filter_map(|f| Some((f.path, self.blob(&f.digest)?.to_vec())))
            .collect()
    }

    /// Whether a revision with this digest has been recorded.
    pub fn has_revision(&self, digest: &str) -> bool {
        self.revisions().any(|r| r.digest == digest)
    }

    /// Every recorded revision, oldest first.
    pub fn revisions(&self) -> impl Iterator<Item = &Revision> {
        self.events.iter().filter_map(|e| match e {
            Event::Revision(r) => Some(r),
            _ => None,
        })
    }

    /// The mode this bundle's first revision was captured with. `edit`
    /// defaults new captures to this rather than the CLI's own default, so
    /// an existing bundle's snapshot behavior never silently changes out
    /// from under it.
    pub fn snapshot_mode(&self) -> Option<SnapshotMode> {
        self.revisions().next().map(|r| r.snapshot_mode)
    }
}

/// Links each file's old and new version, as a revision took one to the other.
pub fn link_revision_files(blobs: &mut crate::digest::Blobs, files: &[crate::model::FileDigest]) {
    for f in files {
        if let (Some(old), Some(new)) = (&f.old, &f.new) {
            blobs.link(old, new);
        }
    }
}

fn digest_path_component(digest: &str) -> &str {
    digest.strip_prefix("sha256:").unwrap_or(digest)
}

/// Loads an existing bundle, or (if `path` doesn't exist yet) an empty one
/// ready to be filled in and saved for the first time.
pub fn load(path: &Path) -> Result<Loaded> {
    if !path.exists() {
        return Ok(Loaded {
            events: Vec::new(),
            carried_entries: Vec::new(),
        });
    }

    let file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut archive = ZipArchive::new(file).with_context(|| {
        format!(
            "{} doesn't look like a .diffnote bundle (not a zip)",
            path.display()
        )
    })?;

    let mut events = Vec::new();
    let mut carried_entries = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .with_context(|| format!("failed to read entry {i} of {}", path.display()))?;
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read {name} from {}", path.display()))?;

        if name == "review.jsonl" {
            let text = String::from_utf8(bytes).with_context(|| {
                format!("review.jsonl in {} is not valid UTF-8", path.display())
            })?;
            events =
                crate::review::parse_jsonl(&text, &format!("review.jsonl in {}", path.display()))?;
        } else {
            carried_entries.push((name, bytes));
        }
    }

    Ok(Loaded {
        events,
        carried_entries,
    })
}

/// What a save adds to the bundle beyond the events.
#[derive(Default)]
pub struct Additions {
    /// A newly recorded revision's (digest, diff text).
    pub diff: Option<(String, String)>,
    /// File versions to put in the store (those already there are skipped).
    pub blobs: Vec<Vec<u8>>,
}

pub fn save(path: &Path, loaded: &Loaded, events: &[Event], additions: &Additions) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let temp = tempfile::NamedTempFile::new_in(parent.unwrap_or_else(|| Path::new(".")))
        .context("failed to create a temp file for the bundle")?;

    {
        let mut writer = ZipWriter::new(temp.as_file());
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        writer.start_file("review.jsonl", options)?;
        for event in events {
            let line = serde_json::to_string(event).context("failed to serialize event")?;
            writer.write_all(line.as_bytes())?;
            writer.write_all(b"\n")?;
        }

        let mut written: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (name, bytes) in &loaded.carried_entries {
            writer.start_file(name, options)?;
            writer.write_all(bytes)?;
            written.insert(name.clone());
        }

        if let Some((digest, diff_text)) = &additions.diff {
            let name = format!("diffs/{}.diff", digest_path_component(digest));
            if written.insert(name.clone()) {
                writer.start_file(name, options)?;
                writer.write_all(diff_text.as_bytes())?;
            }
        }
        for bytes in &additions.blobs {
            let name = format!(
                "blobs/{}",
                digest_path_component(&crate::digest::digest(bytes))
            );
            if written.insert(name.clone()) {
                writer.start_file(name, options)?;
                writer.write_all(bytes)?;
            }
        }

        writer
            .finish()
            .context("failed to finalize the bundle zip")?;
    }

    temp.persist(path)
        .with_context(|| format!("failed to save {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::digest;
    use crate::model::FileDigest;
    use time::OffsetDateTime;

    fn sample_event() -> Event {
        Event::Meta {
            version: 1,
            created_at: OffsetDateTime::now_utc(),
            description: None,
            context_lines: 3,
        }
    }

    fn revision_with(
        key: &str,
        snapshot_mode: SnapshotMode,
        files: Vec<FileDigest>,
        tree: Vec<TreeFile>,
    ) -> Revision {
        Revision {
            id: ulid::Ulid::new(),
            created_at: OffsetDateTime::now_utc(),
            digest: key.to_string(),
            source: Source::Files { base: None },
            snapshot_mode,
            files,
            tree,
        }
    }

    fn revision(key: &str, snapshot_mode: SnapshotMode) -> Event {
        Event::Revision(revision_with(key, snapshot_mode, Vec::new(), Vec::new()))
    }

    fn entry_names(loaded: &Loaded) -> Vec<&str> {
        loaded
            .carried_entries
            .iter()
            .map(|(n, _)| n.as_str())
            .collect()
    }

    #[test]
    fn load_of_a_nonexistent_path_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(&dir.path().join("nope.diffnote")).unwrap();
        assert!(loaded.events.is_empty());
        assert_eq!(loaded.revisions().count(), 0);
    }

    #[test]
    fn load_rejects_a_file_that_isnt_a_zip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-bundle.diffnote");
        std::fs::write(&path, "just some text, not a zip").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn round_trip_preserves_events_and_carries_the_store_through_a_second_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.diffnote");

        let loaded = load(&path).unwrap();
        let events = vec![
            sample_event(),
            revision("sha256:abc123", SnapshotMode::Changed),
        ];
        let additions = Additions {
            diff: Some((
                "sha256:abc123".to_string(),
                "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            )),
            blobs: vec![b"old content".to_vec(), b"new content".to_vec()],
        };
        save(&path, &loaded, &events, &additions).unwrap();

        let reloaded = load(&path).unwrap();
        assert_eq!(reloaded.events.len(), 2);
        assert!(reloaded.has_revision("sha256:abc123"));
        let names = entry_names(&reloaded);
        assert!(names.contains(&"diffs/abc123.diff"));
        for content in ["old content", "new content"] {
            let name = format!("blobs/{}", digest(content).trim_start_matches("sha256:"));
            assert!(names.contains(&name.as_str()), "{names:?}");
        }
        assert_eq!(names.len(), 3, "nothing else is stored: {names:?}");
        let blobs = reloaded.blobs();
        assert_eq!(blobs.text(&digest("old content")), Some("old content"));
        assert_eq!(blobs.text(&digest("new content")), Some("new content"));
        assert_eq!(
            reloaded.blob(&digest("new content")),
            Some(b"new content".as_slice())
        );
        assert_eq!(reloaded.blob(&digest("never stored")), None);

        // Saving again with nothing new must still carry the store through.
        save(&path, &reloaded, &reloaded.events, &Additions::default()).unwrap();
        let reloaded_again = load(&path).unwrap();
        assert!(reloaded_again.has_revision("sha256:abc123"));
        assert_eq!(reloaded_again.events.len(), 2);
        assert_eq!(entry_names(&reloaded_again).len(), 3);
    }

    #[test]
    fn a_file_version_is_stored_once_however_often_it_is_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.diffnote");
        let same = b"same in every revision".to_vec();

        let loaded = load(&path).unwrap();
        // Twice in one save...
        let additions = Additions {
            diff: None,
            blobs: vec![same.clone(), same.clone()],
        };
        save(&path, &loaded, &[sample_event()], &additions).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(entry_names(&loaded).len(), 1);
        // ...and again in a later one.
        let additions = Additions {
            diff: None,
            blobs: vec![same, b"and one more".to_vec()],
        };
        save(&path, &loaded, &loaded.events, &additions).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(entry_names(&loaded).len(), 2);
    }

    #[test]
    fn snapshot_mode_is_the_first_revisions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.diffnote");

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.snapshot_mode(), None);

        let events = [
            sample_event(),
            revision("sha256:a", SnapshotMode::Full),
            revision("sha256:b", SnapshotMode::Changed),
        ];
        save(&path, &loaded, &events, &Additions::default()).unwrap();

        let reloaded = load(&path).unwrap();
        assert_eq!(reloaded.snapshot_mode(), Some(SnapshotMode::Full));
    }

    #[test]
    fn digest_path_component_strips_the_sha256_prefix() {
        assert_eq!(digest_path_component("sha256:abc"), "abc");
        assert_eq!(digest_path_component("no-prefix"), "no-prefix");
    }

    #[test]
    fn latest_revision_is_the_most_recently_recorded_one_with_its_diff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.diffnote");

        let loaded = load(&path).unwrap();
        assert!(loaded.latest_revision().is_none());

        let first = Additions {
            diff: Some(("sha256:first".to_string(), "first diff text".to_string())),
            blobs: Vec::new(),
        };
        let events = vec![sample_event(), revision("sha256:first", SnapshotMode::Changed)];
        save(&path, &loaded, &events, &first).unwrap();
        let loaded = load(&path).unwrap();
        let (rev, text) = loaded.latest_revision().unwrap();
        assert_eq!(
            (rev.digest.as_str(), text.as_str()),
            ("sha256:first", "first diff text")
        );

        let second = Additions {
            diff: Some(("sha256:second".to_string(), "second diff text".to_string())),
            blobs: Vec::new(),
        };
        let mut events = loaded.events.clone();
        events.push(revision("sha256:second", SnapshotMode::Changed));
        save(&path, &loaded, &events, &second).unwrap();
        let loaded = load(&path).unwrap();
        let (rev, text) = loaded.latest_revision().unwrap();
        assert_eq!(
            (rev.digest.as_str(), text.as_str()),
            ("sha256:second", "second diff text")
        );
        // The earlier revision is still recorded, just no longer "latest".
        assert!(loaded.has_revision("sha256:first"));
    }

    fn tree_file(path: &str, content: &str) -> TreeFile {
        TreeFile {
            path: path.to_string(),
            digest: digest(content),
        }
    }

    #[test]
    fn a_pin_extends_and_overrides_the_revisions_recorded_tree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.diffnote");
        let rev = revision_with(
            "sha256:r",
            SnapshotMode::Changed,
            Vec::new(),
            vec![tree_file("a.txt", "a v1"), tree_file("b.txt", "b v1")],
        );
        let pin = Event::Pin {
            revision: rev.id,
            files: vec![tree_file("b.txt", "b v2"), tree_file("c.txt", "c v1")],
        };
        let elsewhere = Event::Pin {
            revision: ulid::Ulid::new(),
            files: vec![tree_file("d.txt", "not this revision's")],
        };
        let events = vec![sample_event(), Event::Revision(rev.clone()), pin, elsewhere];
        let additions = Additions {
            diff: None,
            blobs: ["a v1", "b v1", "b v2", "c v1"]
                .iter()
                .map(|s| s.as_bytes().to_vec())
                .collect(),
        };
        save(&path, &load(&path).unwrap(), &events, &additions).unwrap();
        let loaded = load(&path).unwrap();

        let manifest = loaded.manifest(&rev);
        let paths: Vec<&str> = manifest.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "b.txt", "c.txt"]);
        assert_eq!(manifest[1].digest, digest("b v2"));

        let tree = loaded.tree_of(&rev);
        assert_eq!(tree.get("a.txt").map(Vec::as_slice), Some(b"a v1".as_slice()));
        assert_eq!(tree.get("b.txt").map(Vec::as_slice), Some(b"b v2".as_slice()));
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn tree_of_skips_files_whose_blob_is_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.diffnote");
        let rev = revision_with(
            "sha256:r",
            SnapshotMode::Changed,
            Vec::new(),
            vec![tree_file("here.txt", "stored"), tree_file("gone.txt", "missing")],
        );
        let additions = Additions {
            diff: None,
            blobs: vec![b"stored".to_vec()],
        };
        save(
            &path,
            &load(&path).unwrap(),
            &[sample_event(), Event::Revision(rev.clone())],
            &additions,
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        let tree = loaded.tree_of(&rev);
        assert_eq!(tree.keys().collect::<Vec<_>>(), ["here.txt"]);
    }

    #[test]
    fn revisions_link_a_files_versions_so_positions_can_be_followed_through_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.diffnote");
        let step = |old: &str, new: &str| FileDigest {
            old_path: Some("f".into()),
            new_path: Some("f".into()),
            old: Some(digest(old)),
            new: Some(digest(new)),
        };
        let events = vec![
            sample_event(),
            Event::Revision(revision_with(
                "sha256:1",
                SnapshotMode::Changed,
                vec![step("v1\n", "v2\n")],
                Vec::new(),
            )),
            Event::Revision(revision_with(
                "sha256:2",
                SnapshotMode::Changed,
                vec![step("v2\n", "v3\n")],
                Vec::new(),
            )),
        ];
        let additions = Additions {
            diff: None,
            blobs: ["v1\n", "v2\n", "v3\n"]
                .iter()
                .map(|s| s.as_bytes().to_vec())
                .collect(),
        };
        save(&path, &load(&path).unwrap(), &events, &additions).unwrap();
        let loaded = load(&path).unwrap();
        let blobs = loaded.blobs();
        assert_eq!(
            blobs.chain(&digest("v1\n"), &digest("v3\n")).unwrap(),
            vec!["v1\n", "v2\n", "v3\n"]
        );
    }
}
