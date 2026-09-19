//! The `.diffnote` bundle: a zip file containing a review's event log
//! plus, per unique diff digest it was ever edited against, the diff text
//! itself and (depending on `SnapshotMode`) source file snapshots -- so a
//! review can be shared or continued without separately handing over the
//! diff (or, for `Full`, the whole source tree) it was reviewed against.
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

use crate::diff::UnifiedDiff;
use crate::model::Event;
pub use crate::model::SnapshotMode;
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

pub struct Loaded {
    pub events: Vec<Event>,
    /// Digests (`sha256:...`) already captured in this bundle.
    pub known_digests: BTreeSet<String>,
    /// Existing `diffs/`/`sources/` entries, carried through unchanged
    /// into the rewritten archive.
    carried_entries: Vec<(String, Vec<u8>)>,
}

impl Loaded {
    /// The most recently captured diff (by save order -- new snapshots are
    /// always appended after existing ones, both in memory and when
    /// re-read on the next `load`), if the bundle has captured any yet.
    /// Lets `export`/`show` render against "whatever this review was last
    /// edited against" without the caller having to re-specify the diff.
    pub fn latest_diff(&self) -> Option<(String, String)> {
        self.carried_entries.iter().rev().find_map(|(name, bytes)| {
            let component = name.strip_prefix("diffs/")?.strip_suffix(".diff")?;
            let text = String::from_utf8(bytes.clone()).ok()?;
            Some((format!("sha256:{component}"), text))
        })
    }

    /// The mode this bundle was first created with, if it's been through at
    /// least one save (`Meta` is always the first event once any exist).
    /// `edit` defaults new captures to this rather than the CLI's own
    /// default, so an existing bundle's snapshot behavior never silently
    /// changes out from under it.
    pub fn snapshot_mode(&self) -> Option<SnapshotMode> {
        self.events.iter().find_map(|e| match e {
            Event::Meta { snapshot_mode, .. } => Some(*snapshot_mode),
            _ => None,
        })
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
            known_digests: BTreeSet::new(),
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
    let mut known_digests = BTreeSet::new();
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
            if let Some(rest) = name.strip_prefix("diffs/")
                && let Some(component) = rest.strip_suffix(".diff")
            {
                known_digests.insert(format!("sha256:{component}"));
            }
            carried_entries.push((name, bytes));
        }
    }

    Ok(Loaded {
        events,
        known_digests,
        carried_entries,
    })
}

/// A newly-seen diff digest's snapshot, to be added to the bundle.
pub struct NewSnapshot {
    pub digest: String,
    pub diff_text: String,
    /// (relative path, file content) pairs -- empty for `SnapshotMode::Diff`.
    pub files: Vec<(String, Vec<u8>)>,
}

pub fn capture_snapshot(
    mode: SnapshotMode,
    digest: &str,
    diff_text: &str,
    diff: &UnifiedDiff,
    root: &Path,
    bundle_path: &Path,
) -> Result<NewSnapshot> {
    // Never snapshot the bundle's own file -- with SnapshotMode::Full in
    // particular, the bundle typically lives inside the tree being walked,
    // and embedding a copy of an earlier version of itself on every save
    // would grow it without bound.
    let exclude = std::fs::canonicalize(bundle_path).ok();
    let files = match mode {
        SnapshotMode::Diff => Vec::new(),
        SnapshotMode::Changed => capture_changed_files(diff, root, exclude.as_deref()),
        SnapshotMode::Full => capture_full_tree(root, exclude.as_deref())?,
    };
    Ok(NewSnapshot {
        digest: digest.to_string(),
        diff_text: diff_text.to_string(),
        files,
    })
}

fn is_excluded(path: &Path, exclude: Option<&Path>) -> bool {
    match (exclude, std::fs::canonicalize(path)) {
        (Some(exclude), Ok(canon)) => canon == exclude,
        _ => false,
    }
}

fn capture_changed_files(
    diff: &UnifiedDiff,
    root: &Path,
    exclude: Option<&Path>,
) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    for file in &diff.files {
        let Some(rel) = &file.new_path else {
            // Deleted (or otherwise sourceless) file: nothing to snapshot.
            continue;
        };
        let full = root.join(rel);
        if is_excluded(&full, exclude) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&full) {
            files.push((rel.clone(), bytes));
        }
    }
    files
}

/// Every file a `Full` snapshot of `root` would include: `.gitignore`-aware
/// (via `require_git(false)`, since diffnote must work on a plain, non-git
/// folder too -- see the project's original motivation), `.git` itself
/// skipped, and `exclude` (the bundle's own path) skipped so a `Full`
/// snapshot never embeds an earlier copy of itself.
fn walk_full_tree(root: &Path, exclude: Option<&Path>) -> Result<Vec<ignore::DirEntry>> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .build();
    let mut entries = Vec::new();
    for entry in walker {
        let entry = entry.context("failed to walk the source tree")?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.components().any(|c| c.as_os_str() == ".git") {
            continue;
        }
        if is_excluded(path, exclude) {
            continue;
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn capture_full_tree(root: &Path, exclude: Option<&Path>) -> Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    for entry in walk_full_tree(root, exclude)? {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        files.push((rel.to_string_lossy().replace('\\', "/"), bytes));
    }
    Ok(files)
}

/// Total size in bytes of everything a `Full` snapshot of `root` would
/// capture, without reading any file's content -- used to warn before an
/// unexpectedly large `full` snapshot.
pub fn estimate_full_tree_size(root: &Path, bundle_path: &Path) -> Result<u64> {
    let exclude = std::fs::canonicalize(bundle_path).ok();
    let mut total = 0u64;
    for entry in walk_full_tree(root, exclude.as_deref())? {
        if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    Ok(total)
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
            diff_digest: "sha256:abc123".to_string(),
            branch: None,
            description: None,
            context_lines: 3,
            snapshot_mode: SnapshotMode::Changed,
        }
    }

    #[test]
    fn load_of_a_nonexistent_path_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(&dir.path().join("nope.diffnote")).unwrap();
        assert!(loaded.events.is_empty());
        assert!(loaded.known_digests.is_empty());
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
        let events = vec![sample_event()];
        let snapshot = NewSnapshot {
            digest: "sha256:abc123".to_string(),
            diff_text: "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            files: vec![("f".to_string(), b"new content".to_vec())],
        };
        save(&path, &loaded, &events, Some(&snapshot)).unwrap();

        let reloaded = load(&path).unwrap();
        assert_eq!(reloaded.events.len(), 1);
        assert!(reloaded.known_digests.contains("sha256:abc123"));
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

        // Saving again with no *new* snapshot must still carry the first
        // one through untouched.
        save(&path, &reloaded, &reloaded.events, None).unwrap();
        let reloaded_again = load(&path).unwrap();
        assert!(reloaded_again.known_digests.contains("sha256:abc123"));
        assert_eq!(reloaded_again.events.len(), 1);
    }

    #[test]
    fn capture_changed_files_only_includes_touched_files_present_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("touched.txt"), "touched content").unwrap();
        // "removed.txt" is intentionally absent -- a deleted file has no
        // new_path, and even if it did, there'd be nothing to read.

        let diff_text = "\
--- a/touched.txt
+++ b/touched.txt
@@ -1 +1 @@
-old
+touched content
--- a/removed.txt
+++ /dev/null
@@ -1 +0,0 @@
-gone
";
        let diff = crate::diff::parse(diff_text).unwrap();
        let files = capture_changed_files(&diff, dir.path(), None);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "touched.txt");
        assert_eq!(files[0].1, b"touched content");
    }

    #[test]
    fn capture_full_tree_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(dir.path().join("kept.txt"), "kept").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "ignored").unwrap();

        let files = capture_full_tree(dir.path(), None).unwrap();
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"kept.txt"));
        assert!(names.contains(&".gitignore"));
        assert!(!names.contains(&"ignored.txt"));
    }

    #[test]
    fn estimate_full_tree_size_matches_the_files_a_full_snapshot_would_capture() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(dir.path().join("kept.txt"), "12345").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "should not count").unwrap();

        let files = capture_full_tree(dir.path(), None).unwrap();
        let expected: u64 = files.iter().map(|(_, bytes)| bytes.len() as u64).sum();

        let size =
            estimate_full_tree_size(dir.path(), &dir.path().join("nonexistent.diffnote")).unwrap();
        assert_eq!(size, expected);
    }

    #[test]
    fn loaded_snapshot_mode_reflects_the_saved_events_meta() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.diffnote");

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.snapshot_mode(), None);

        let mut event = sample_event();
        if let Event::Meta { snapshot_mode, .. } = &mut event {
            *snapshot_mode = SnapshotMode::Full;
        }
        save(&path, &loaded, &[event], None).unwrap();

        let reloaded = load(&path).unwrap();
        assert_eq!(reloaded.snapshot_mode(), Some(SnapshotMode::Full));
    }

    #[test]
    fn digest_path_component_strips_the_sha256_prefix() {
        assert_eq!(digest_path_component("sha256:abc"), "abc");
        assert_eq!(digest_path_component("no-prefix"), "no-prefix");
    }

    #[test]
    fn latest_diff_is_the_most_recently_saved_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.diffnote");

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.latest_diff(), None);

        let first = NewSnapshot {
            digest: "sha256:first".to_string(),
            diff_text: "first diff text".to_string(),
            files: Vec::new(),
        };
        save(&path, &loaded, &[sample_event()], Some(&first)).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(
            loaded.latest_diff(),
            Some(("sha256:first".to_string(), "first diff text".to_string()))
        );

        let second = NewSnapshot {
            digest: "sha256:second".to_string(),
            diff_text: "second diff text".to_string(),
            files: Vec::new(),
        };
        save(&path, &loaded, &loaded.events.clone(), Some(&second)).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(
            loaded.latest_diff(),
            Some(("sha256:second".to_string(), "second diff text".to_string()))
        );
        // The earlier digest is still present, just no longer "latest".
        assert!(loaded.known_digests.contains("sha256:first"));
    }
}
