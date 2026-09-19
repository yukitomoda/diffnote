//! The persisted, shareable, re-editable review format (JSON Lines, append-only).
//!
//! One JSONL document = one diff review target. The first record is
//! always `Meta`; every following record is an immutable, appended event.
//! Existing records are never rewritten — corrections happen by appending a
//! new `Reanchor`/`Resolve`/`Reopen` event, not by editing an old one. This
//! keeps independently-edited copies mergeable under a plain `git merge`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ulid::Ulid;

/// Which side of the diff an anchor's line numbers refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Old,
    New,
}

/// What to snapshot into a `.diffnote` bundle the first time a new diff
/// digest is captured (see `bundle`). Persisted on `Meta` as the mode the
/// bundle was first created with, so later `edit` sessions default to the
/// same mode instead of silently switching when the CLI's own default
/// changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotMode {
    /// Only the diff text -- no source files at all.
    Diff,
    /// The diff text, plus a full copy of every file the diff touches.
    Changed,
    /// The diff text, plus a full copy of the entire source tree
    /// (respecting .gitignore), for reviews of folders with no other
    /// history to fall back on.
    Full,
}

/// Old bundles (from before `Meta` carried `snapshot_mode`) implicitly
/// behaved like `Changed`, the only mode that existed at the time.
fn default_snapshot_mode() -> SnapshotMode {
    SnapshotMode::Changed
}

/// A few lines of frozen source text kept alongside an anchor so a comment
/// stays meaningful even if the file it points at can no longer be found
/// (see re-anchoring in the `anchor` module).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    pub target: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

/// Optional, non-authoritative enrichment. Never required for correctness:
/// the anchor must stand on its own, without a git repository behind it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceHint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_target_commit: Option<String>,
}

/// Where a comment points. `Global`/`File`/`Hunk` are positional only (their
/// validity isn't re-checked against source drift in v1 — see `anchor::resolve`,
/// which treats them as always current). Only `Span` (a single line or a
/// range on one side of one file) carries the frozen context and digest
/// needed to relocate or freeze-display it later; `Line`-level annotation
/// comments are just a `Span` with `line_start == line_end`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "lowercase")]
pub enum Anchor {
    Global,
    File {
        file: String,
    },
    Hunk {
        file: String,
        hunk_index: usize,
    },
    Span {
        file: String,
        side: Side,
        line_start: u32,
        line_end: u32,
        context: Context,
        /// Digest of the diff this anchor was captured against. If it
        /// matches the diff currently being viewed, re-anchoring can be
        /// skipped entirely.
        origin_diff_digest: String,
        #[serde(default, skip_serializing_if = "is_default_source_hint")]
        source_hint: SourceHint,
        /// A range comment's `>[`..`>]` markers can wrap removed lines
        /// *and* context/added lines at once; `side`/`line_start`/`line_end`
        /// above are always the new-side range (preferred since that's what
        /// re-anchoring searches against), and this is the old-side
        /// sub-range it also covered, kept purely so the initial (and any
        /// still-`current`, non-relocated) rendering highlights the removed
        /// lines too instead of silently dropping them. `None` for a
        /// single-side range/line comment. Always cleared (`None`) on
        /// relocation -- there's nothing to search for on the old side in a
        /// future diff, since removed content is gone by definition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_range: Option<(u32, u32)>,
    },
}

fn is_default_source_hint(hint: &SourceHint) -> bool {
    hint.git_target_commit.is_none()
}

/// One line of the JSONL document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Event {
    Meta {
        version: u32,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        diff_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        context_lines: u32,
        #[serde(default = "default_snapshot_mode")]
        snapshot_mode: SnapshotMode,
    },
    /// A thread root (`parent: None`, carries an `anchor`) or a reply
    /// (`parent: Some(root_id)`, no `anchor` — threading is flat, like a
    /// GitHub PR review thread, not an arbitrary tree).
    Comment {
        id: Ulid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<Ulid>,
        author: String,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anchor: Option<Anchor>,
        body: String,
    },
    Resolve {
        parent: Ulid,
        author: String,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    Reopen {
        parent: Ulid,
        author: String,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    /// Promotes `anchor` to the new authoritative anchor/search-key for
    /// `parent`'s thread, whether from an explicit `>!reanchor` directive or
    /// a silently-accepted automatic relocation.
    Reanchor {
        parent: Ulid,
        author: String,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        anchor: Anchor,
    },
}
