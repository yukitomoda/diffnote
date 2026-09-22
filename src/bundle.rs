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
//! reactions.json            the reactions to comments as they are now (left out if
//!                          there are none)
//! settings.json             the review's settings as they are now (left out if all
//!                          are their defaults)
//! diffs/<digest>.diff      one per recorded revision (its unified diff)
//! blobs/<sha256 hex>       one per distinct file version, however many
//!                          revisions have it
//! images/<sha256 hex>      one per distinct image attached to a comment
//! attachments/<sha256 hex> one per distinct other file attached to a comment
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

use crate::messages::{m, mf};
pub use crate::model::SnapshotMode;
use crate::model::{Event, Reactions, Revision, Settings, Source, TreeFile};
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

pub struct Loaded {
    pub events: Vec<Event>,
    /// The review's settings (see [`Settings`]); a change is made here and
    /// saved with the bundle.
    pub settings: Settings,
    /// The reactions to comments (state, see [`Reactions`]); a change is made
    /// here and saved with the bundle.
    pub reactions: Reactions,
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
            if let Event::Pin {
                revision: id,
                files,
            } = event
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
            settings: Settings::default(),
            reactions: Reactions::new(),
            carried_entries: Vec::new(),
        });
    }

    let file = std::fs::File::open(path).with_context(|| {
        mf(
            "bundle.open_failed",
            &[("path", &path.display().to_string())],
        )
    })?;
    let mut archive = ZipArchive::new(file).with_context(|| {
        mf(
            "bundle.not_a_bundle",
            &[("path", &path.display().to_string())],
        )
    })?;

    let mut events = Vec::new();
    let mut settings = Settings::default();
    let mut from_file: Option<serde_json::Map<String, serde_json::Value>> = None;
    let mut reactions = Reactions::new();
    let mut carried_entries = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).with_context(|| {
            mf(
                "bundle.entry_read_failed",
                &[
                    ("path", &path.display().to_string()),
                    ("index", &i.to_string()),
                ],
            )
        })?;
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).with_context(|| {
            mf(
                "bundle.member_read_failed",
                &[("path", &path.display().to_string()), ("name", &name)],
            )
        })?;

        if name == "review.jsonl" {
            let text = String::from_utf8(bytes).with_context(|| {
                mf(
                    "bundle.review_not_utf8",
                    &[("path", &path.display().to_string())],
                )
            })?;
            let label = mf(
                "bundle.review_label",
                &[("path", &path.display().to_string())],
            );
            events = crate::review::parse_jsonl(&text, &label)?;
        } else if name == "reactions.json" {
            reactions = serde_json::from_slice(&bytes).with_context(|| {
                mf(
                    "bundle.reactions_read_failed",
                    &[("path", &path.display().to_string())],
                )
            })?;
        } else if name == "settings.json" {
            from_file = Some(serde_json::from_slice(&bytes).with_context(|| {
                mf(
                    "bundle.settings_read_failed",
                    &[("path", &path.display().to_string())],
                )
            })?);
        } else {
            carried_entries.push((name, bytes));
        }
    }

    fold_old_settings(&mut events, &mut settings, from_file);
    Ok(Loaded {
        events,
        settings,
        reactions,
        carried_entries,
    })
}

/// A bundle made before the settings were state has the title, and whether to
/// ignore white space, as events (the last of each says how it is). They are
/// folded into the settings, and are not kept as events any more: a setting
/// that `settings.json` has itself is the newer word and stays.
fn fold_old_settings(
    events: &mut Vec<Event>,
    settings: &mut Settings,
    from_file: Option<serde_json::Map<String, serde_json::Value>>,
) {
    let mut old = Settings::default();
    for event in events.iter() {
        match event {
            Event::Title { title, .. } => {
                let title = title.trim();
                old.title = (!title.is_empty()).then(|| title.to_string());
            }
            Event::IgnoreWhitespace { value, .. } => old.ignore_whitespace = *value,
            _ => {}
        }
    }
    events.retain(|e| !matches!(e, Event::Title { .. } | Event::IgnoreWhitespace { .. }));
    // What the events said, then what the file says over it.
    let mut merged = serde_json::to_value(&old).unwrap_or_default();
    if let (Some(map), Some(file)) = (merged.as_object_mut(), from_file) {
        for (key, value) in file {
            map.insert(key, value);
        }
    }
    if let Ok(now) = serde_json::from_value::<Settings>(merged) {
        *settings = now;
    }
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
    save_with(path, loaded, events, additions, &Images::default())
}

/// What a save does about the images (`images/`) beyond carrying them on.
#[derive(Default)]
pub struct Images<'a> {
    /// Images to put in (those already there are skipped).
    pub add: &'a [Vec<u8>],
    /// If given, the ids of the images to keep: the others are left out.
    pub keep: Option<&'a std::collections::HashSet<String>>,
    /// The same for the other files attached to comments.
    pub add_files: &'a [Vec<u8>],
    pub keep_files: Option<&'a std::collections::HashSet<String>>,
}

/// The ids (hex digests) of the images a bundle holds.
impl Loaded {
    /// The bytes of an image of the bundle.
    pub fn image(&self, id: &str) -> Option<&[u8]> {
        let wanted = format!("images/{id}");
        self.carried_entries
            .iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, bytes)| bytes.as_slice())
    }

    /// The bytes of a file attached to a comment (not an image).
    pub fn attachment(&self, id: &str) -> Option<&[u8]> {
        let wanted = format!("attachments/{id}");
        self.carried_entries
            .iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, bytes)| bytes.as_slice())
    }

    pub fn attachment_ids(&self) -> Vec<String> {
        self.carried_entries
            .iter()
            .filter_map(|(name, _)| name.strip_prefix("attachments/").map(str::to_string))
            .collect()
    }

    /// How many images the bundle holds, and how many bytes they are.
    pub fn image_stats(&self) -> (usize, u64) {
        Self::stats(&self.carried_entries, "images/")
    }

    /// The same for the other attached files.
    pub fn attachment_stats(&self) -> (usize, u64) {
        Self::stats(&self.carried_entries, "attachments/")
    }

    fn stats(entries: &[(String, Vec<u8>)], prefix: &str) -> (usize, u64) {
        entries
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .fold((0, 0), |(n, bytes), (_, b)| (n + 1, bytes + b.len() as u64))
    }

    pub fn image_ids(&self) -> Vec<String> {
        self.carried_entries
            .iter()
            .filter_map(|(name, _)| name.strip_prefix("images/").map(str::to_string))
            .collect()
    }
}

/// One image of a bundle, read without reading the rest of it.
pub fn read_image(path: &Path, id: &str) -> Option<Vec<u8>> {
    read_entry(path, &format!("images/{id}"))
}

/// One attached file of a bundle, read without reading the rest of it.
pub fn read_attachment(path: &Path, id: &str) -> Option<Vec<u8>> {
    read_entry(path, &format!("attachments/{id}"))
}

fn read_entry(path: &Path, name: &str) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;
    let mut entry = archive.by_name(name).ok()?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

pub fn save_with(
    path: &Path,
    loaded: &Loaded,
    events: &[Event],
    additions: &Additions,
    images: &Images,
) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent).with_context(|| {
            mf(
                "bundle.dir_create_failed",
                &[("path", &parent.display().to_string())],
            )
        })?;
    }
    let temp = tempfile::NamedTempFile::new_in(parent.unwrap_or_else(|| Path::new(".")))
        .context(m("bundle.temp_file_failed"))?;

    {
        let mut writer = ZipWriter::new(temp.as_file());
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        writer.start_file("review.jsonl", options)?;
        for event in events {
            let line = serde_json::to_string(event).context(m("bundle.event_json_failed"))?;
            writer.write_all(line.as_bytes())?;
            writer.write_all(b"\n")?;
        }

        // Settings are only written if some is not its default.
        if loaded.settings != Settings::default() {
            writer.start_file("settings.json", options)?;
            writer.write_all(
                serde_json::to_string_pretty(&loaded.settings)
                    .context(m("bundle.settings_json_failed"))?
                    .as_bytes(),
            )?;
        }

        if !loaded.reactions.is_empty() {
            writer.start_file("reactions.json", options)?;
            writer.write_all(
                serde_json::to_string_pretty(&loaded.reactions)
                    .context(m("bundle.reactions_json_failed"))?
                    .as_bytes(),
            )?;
        }

        let mut written: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (name, bytes) in &loaded.carried_entries {
            if let (Some(keep), Some(id)) = (images.keep, name.strip_prefix("images/"))
                && !keep.contains(id)
            {
                continue;
            }
            if let (Some(keep), Some(id)) = (images.keep_files, name.strip_prefix("attachments/"))
                && !keep.contains(id)
            {
                continue;
            }
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

        for bytes in images.add {
            let name = format!(
                "images/{}",
                digest_path_component(&crate::digest::digest(bytes))
            );
            if written.insert(name.clone()) {
                writer.start_file(name, options)?;
                writer.write_all(bytes)?;
            }
        }

        for bytes in images.add_files {
            let name = format!(
                "attachments/{}",
                digest_path_component(&crate::digest::digest(bytes))
            );
            if written.insert(name.clone()) {
                writer.start_file(name, options)?;
                writer.write_all(bytes)?;
            }
        }

        writer.finish().context(m("bundle.zip_finish_failed"))?;
    }

    temp.persist(path).with_context(|| {
        mf(
            "bundle.save_failed",
            &[("path", &path.display().to_string())],
        )
    })?;
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
        let events = vec![
            sample_event(),
            revision("sha256:first", SnapshotMode::Changed),
        ];
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
        assert_eq!(
            tree.get("a.txt").map(Vec::as_slice),
            Some(b"a v1".as_slice())
        );
        assert_eq!(
            tree.get("b.txt").map(Vec::as_slice),
            Some(b"b v2".as_slice())
        );
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
            vec![
                tree_file("here.txt", "stored"),
                tree_file("gone.txt", "missing"),
            ],
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

    #[test]
    fn settings_are_state_kept_in_settings_json_only_when_one_is_not_its_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.diffnote");
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.settings, Settings::default());
        assert_eq!(loaded.settings.attachment_limit, 5 * 1024 * 1024);
        let events = vec![sample_event()];
        save(&path, &loaded, &events, &Additions::default()).unwrap();
        let names = |p: &Path| -> Vec<String> {
            let mut a = ZipArchive::new(std::fs::File::open(p).unwrap()).unwrap();
            (0..a.len())
                .map(|i| a.by_index(i).unwrap().name().to_string())
                .collect()
        };
        assert!(
            !names(&path).contains(&"settings.json".to_string()),
            "nothing to say"
        );
        // Changed: written, and read back; the state, not a history.
        let mut loaded = load(&path).unwrap();
        loaded.settings.attachment_limit = 1234;
        save(&path, &loaded, &events, &Additions::default()).unwrap();
        assert!(names(&path).contains(&"settings.json".to_string()));
        let mut loaded = load(&path).unwrap();
        assert_eq!(loaded.settings.attachment_limit, 1234);
        assert!(
            !entry_names(&loaded).contains(&"settings.json"),
            "not carried as an entry"
        );
        // It stays through a save that doesn't touch it, and is one entry, not two.
        save(&path, &loaded, &events, &Additions::default()).unwrap();
        assert_eq!(
            names(&path)
                .iter()
                .filter(|n| *n == "settings.json")
                .count(),
            1
        );
        assert_eq!(load(&path).unwrap().settings.attachment_limit, 1234);
        // Back to the default: the entry goes.
        loaded.settings = Settings::default();
        save(&path, &loaded, &events, &Additions::default()).unwrap();
        assert!(!names(&path).contains(&"settings.json".to_string()));
        // A settings.json that says less than all of them: the rest are defaults.
        let mut loaded = load(&path).unwrap();
        loaded.settings.attachment_limit = 9;
        save(&path, &loaded, &events, &Additions::default()).unwrap();
        assert_eq!(
            serde_json::from_str::<Settings>("{}").unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn a_bundle_with_the_old_title_and_white_space_events_reads_them_as_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.diffnote");
        let title = |t: &str| Event::Title {
            title: t.to_string(),
            author: "a".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        };
        let whitespace = |on: bool| Event::IgnoreWhitespace {
            value: on,
            author: "a".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        };
        // As it was written then: the last of each is how it is.
        let old = vec![
            sample_event(),
            title("first"),
            whitespace(true),
            title("second"),
            whitespace(false),
            whitespace(true),
        ];
        save(&path, &load(&path).unwrap(), &old, &Additions::default()).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.settings.title.as_deref(), Some("second"));
        assert!(loaded.settings.ignore_whitespace);
        assert_eq!(loaded.events.len(), 1, "no longer events");
        // Kept as settings from the next save on, and not as events.
        save(&path, &loaded, &loaded.events, &Additions::default()).unwrap();
        let again = load(&path).unwrap();
        assert_eq!(again.settings.title.as_deref(), Some("second"));
        assert!(again.settings.ignore_whitespace);
        let text = {
            let mut a = ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
            let mut s = String::new();
            a.by_name("review.jsonl")
                .unwrap()
                .read_to_string(&mut s)
                .unwrap();
            s
        };
        assert!(
            !text.contains("\"kind\":\"title\"") && !text.contains("ignorewhitespace"),
            "{text}"
        );
        // A title that `settings.json` has itself is the newer word, and what it does
        // not say is what the events said.
        let mut both = again;
        both.settings.title = Some("newer".into());
        let with_old = vec![sample_event(), whitespace(true)];
        save(&path, &both, &with_old, &Additions::default()).unwrap();
        let read = load(&path).unwrap();
        assert_eq!(read.settings.title.as_deref(), Some("newer"));
        assert!(read.settings.ignore_whitespace);
    }

    #[test]
    fn reactions_are_state_kept_in_reactions_json_only_while_there_are_some() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.diffnote");
        let events = vec![sample_event()];
        let mut loaded = load(&path).unwrap();
        assert!(loaded.reactions.is_empty());
        let names = |p: &Path| -> Vec<String> {
            let mut a = ZipArchive::new(std::fs::File::open(p).unwrap()).unwrap();
            (0..a.len())
                .map(|i| a.by_index(i).unwrap().name().to_string())
                .collect()
        };
        save(&path, &loaded, &events, &Additions::default()).unwrap();
        assert!(!names(&path).contains(&"reactions.json".to_string()));
        loaded = load(&path).unwrap();
        crate::review::toggle_reaction(&mut loaded.reactions, "c1", "👍", "a");
        save(&path, &loaded, &events, &Additions::default()).unwrap();
        assert!(names(&path).contains(&"reactions.json".to_string()));
        let mut again = load(&path).unwrap();
        assert_eq!(again.reactions["c1"][0].emoji, "👍");
        assert_eq!(again.reactions["c1"][0].authors, ["a"]);
        assert!(
            !entry_names(&again).contains(&"reactions.json"),
            "not carried as an entry"
        );
        // Kept through a save that doesn't touch it (once), and gone when the last is taken back.
        save(&path, &again, &events, &Additions::default()).unwrap();
        assert_eq!(
            names(&path)
                .iter()
                .filter(|n| *n == "reactions.json")
                .count(),
            1
        );
        crate::review::toggle_reaction(&mut again.reactions, "c1", "👍", "a");
        save(&path, &again, &events, &Additions::default()).unwrap();
        assert!(!names(&path).contains(&"reactions.json".to_string()));
    }
}
