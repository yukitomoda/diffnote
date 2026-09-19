//! The `.diffnote` bundle: a zip file containing a review's event log
//! plus, per unique diff digest it was ever edited against, the diff text
//! itself and (depending on `SnapshotMode`) source file snapshots -- so a
//! review can be shared or continued without separately handing over the
//! diff (or, for `Full`, the whole source tree) it was reviewed against.
//! What goes into `sources/` is decided by the caller (the git provider
//! reads committed content), not read from disk here.
//!
//! Layout inside the zip:
//! ```text
//! review.jsonl
//! diffs/<digest>.diff
//! sources/<digest>/<relative/path>   (Changed: touched files only; Full: whole tree)
//! ```
//! `<digest>` is the review's digest string (`sha256:...`) with the
//! `sha256:` prefix stripped, so it's a plain hex string safe to use as a
//! zip path component on every platform.
//!
//! A bundle is always rewritten whole (read every entry into memory, then
//! write a fresh zip) rather than patched in place -- reviews are small
//! enough that this is simple and safe (no risk of a half-written zip from
//! an in-place edit), at the cost of not scaling to huge bundles.

pub use crate::model::SnapshotMode;
use crate::model::{Event, Revision, Source};
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

pub struct Loaded {
    pub events: Vec<Event>,
    /// Existing `diffs/`/`sources/` entries, carried through unchanged
    /// into the rewritten archive.
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

    /// The files snapshotted for the revision with this digest
    /// (relative path -> content); empty unless it was a `Full` (or
    /// `Changed`) capture.
    pub fn snapshot_files(&self, digest: &str) -> crate::files::Tree {
        let prefix = format!("sources/{}/", digest_path_component(digest));
        self.carried_entries
            .iter()
            .filter_map(|(name, bytes)| {
                Some((name.strip_prefix(&prefix)?.to_string(), bytes.clone()))
            })
            .collect()
    }

    /// Every snapshotted file version in the bundle (head and base sides),
    /// findable by content digest.
    pub fn blobs(&self) -> crate::digest::Blobs<'_> {
        let mut blobs = crate::digest::Blobs::default();
        for (name, bytes) in &self.carried_entries {
            if name.starts_with("sources/") || name.starts_with("bases/") {
                blobs.add(bytes);
            }
        }
        blobs
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

/// A newly-seen diff digest's snapshot, to be added to the bundle.
pub struct NewSnapshot {
    pub digest: String,
    pub diff_text: String,
    /// (relative path, file content) pairs at the reviewed (head) side --
    /// empty for `SnapshotMode::Diff`.
    pub files: Vec<(String, Vec<u8>)>,
    /// The base-side versions of the files the diff touches, stored under
    /// `bases/` (a directory review's base is the previous revision's own
    /// snapshot, so it passes none).
    pub base_files: Vec<(String, Vec<u8>)>,
}

/// Rewrites `path` from scratch: `loaded`'s carried-over entries, `events`
/// serialized as `review.jsonl` (the full, merged event list -- existing
/// plus new), and `new_snapshot` if a not-previously-seen digest was
/// captured this session.
pub fn save(
    path: &Path,
    loaded: &Loaded,
    events: &[Event],
    new_snapshot: Option<&NewSnapshot>,
) -> Result<()> {
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

        for (name, bytes) in &loaded.carried_entries {
            writer.start_file(name, options)?;
            writer.write_all(bytes)?;
        }

        if let Some(snapshot) = new_snapshot {
            let component = digest_path_component(&snapshot.digest);
            writer.start_file(format!("diffs/{component}.diff"), options)?;
            writer.write_all(snapshot.diff_text.as_bytes())?;
            for (rel_path, bytes) in &snapshot.files {
                writer.start_file(format!("sources/{component}/{rel_path}"), options)?;
                writer.write_all(bytes)?;
            }
            for (rel_path, bytes) in &snapshot.base_files {
                writer.start_file(format!("bases/{component}/{rel_path}"), options)?;
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
    use time::OffsetDateTime;

    fn sample_event() -> Event {
        Event::Meta {
            version: 1,
            created_at: OffsetDateTime::now_utc(),
            description: None,
            context_lines: 3,
        }
    }

    fn revision(digest: &str, snapshot_mode: SnapshotMode) -> Event {
        Event::Revision(Revision {
            id: ulid::Ulid::new(),
            created_at: OffsetDateTime::now_utc(),
            digest: digest.to_string(),
            source: Source::Files { base: None },
            snapshot_mode,
            files: Vec::new(),
        })
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
    fn round_trip_preserves_events_and_carries_the_snapshot_through_a_second_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.diffnote");

        let loaded = load(&path).unwrap();
        let events = vec![
            sample_event(),
            revision("sha256:abc123", SnapshotMode::Changed),
        ];
        let snapshot = NewSnapshot {
            digest: "sha256:abc123".to_string(),
            diff_text: "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            files: vec![("f".to_string(), b"new content".to_vec())],
            base_files: vec![("f".to_string(), b"old content".to_vec())],
        };
        save(&path, &loaded, &events, Some(&snapshot)).unwrap();

        let reloaded = load(&path).unwrap();
        assert_eq!(reloaded.events.len(), 2);
        assert!(reloaded.has_revision("sha256:abc123"));
        assert!(
            reloaded
                .carried_entries
                .iter()
                .any(|(n, _)| n == "diffs/abc123.diff")
        );
        assert!(
            reloaded
                .carried_entries
                .iter()
                .any(|(n, _)| n == "sources/abc123/f")
        );
        let blobs = reloaded.blobs();
        assert_eq!(
            blobs.text(&crate::digest::digest("old content")),
            Some("old content")
        );
        assert_eq!(
            blobs.text(&crate::digest::digest("new content")),
            Some("new content")
        );

        // Saving again with no *new* snapshot must still carry the first
        // one through untouched.
        save(&path, &reloaded, &reloaded.events, None).unwrap();
        let reloaded_again = load(&path).unwrap();
        assert!(reloaded_again.has_revision("sha256:abc123"));
        assert_eq!(reloaded_again.events.len(), 2);
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
            revision("sha256:b", SnapshotMode::Diff),
        ];
        save(&path, &loaded, &events, None).unwrap();

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

        let first = NewSnapshot {
            digest: "sha256:first".to_string(),
            diff_text: "first diff text".to_string(),
            files: Vec::new(),
            base_files: Vec::new(),
        };
        let events = vec![sample_event(), revision("sha256:first", SnapshotMode::Diff)];
        save(&path, &loaded, &events, Some(&first)).unwrap();
        let loaded = load(&path).unwrap();
        let (rev, text) = loaded.latest_revision().unwrap();
        assert_eq!(
            (rev.digest.as_str(), text.as_str()),
            ("sha256:first", "first diff text")
        );

        let second = NewSnapshot {
            digest: "sha256:second".to_string(),
            diff_text: "second diff text".to_string(),
            files: Vec::new(),
            base_files: Vec::new(),
        };
        let mut events = loaded.events.clone();
        events.push(revision("sha256:second", SnapshotMode::Diff));
        save(&path, &loaded, &events, Some(&second)).unwrap();
        let loaded = load(&path).unwrap();
        let (rev, text) = loaded.latest_revision().unwrap();
        assert_eq!(
            (rev.digest.as_str(), text.as_str()),
            ("sha256:second", "second diff text")
        );
        // The earlier revision is still recorded, just no longer "latest".
        assert!(loaded.has_revision("sha256:first"));
    }
}
