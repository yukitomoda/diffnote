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
    /// 差分が触れた全ファイルの両側と、コメントが参照する全ファイルを保存する
    Changed,
    /// `changed` の内容に加えて、head 全体のツリー(.gitignore は尊重)を保存する
    Full,
}

/// The commits a git-backed review was made against, resolved to full ids
/// at edit time (so a moving ref such as `HEAD~4` is pinned).
/// What an attached file may weigh unless the review says otherwise (5 MB).
pub const DEFAULT_ATTACHMENT_LIMIT: u64 = 5 * 1024 * 1024;

/// The review's settings: state, not history (only what they are now is kept,
/// in the bundle's `settings.json`, and a setting that is left out is its
/// default). What the review says about how it is worked on -- rules that go
/// with it to whoever continues it -- belongs here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The most a file attached to a comment (an image, or any other file) may
    /// weigh, in bytes.
    #[serde(default = "default_attachment_limit")]
    pub attachment_limit: u64,
    /// The review's title, shown where the export names the review. `None`: none
    /// (the page has its own heading).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Whether the page shows lines that differ only in white space as
    /// unchanged. It only changes what is shown: the diff, and where threads
    /// are, stay as they are.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignore_whitespace: bool,
}

fn default_attachment_limit() -> u64 {
    DEFAULT_ATTACHMENT_LIMIT
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            attachment_limit: DEFAULT_ATTACHMENT_LIMIT,
            title: None,
            ignore_whitespace: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSource {
    pub base: String,
    pub head: String,
    /// What the user typed (`HEAD~3..HEAD`), for display only.
    pub spec: String,
}

/// What a revision's content was taken from. Fixed by a bundle's first
/// revision: a git-backed review stays git-backed, a plain-directory review
/// stays a directory review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    Git(GitSource),
    /// A plain directory, compared against the bundle's previous snapshot.
    /// `base` is that snapshot's tree digest (`None` for the first one); the
    /// head tree's digest is the revision's own `digest`.
    Files {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
}

impl Source {
    /// The (base, head) revision ids a revision with this `digest` compares.
    pub fn revisions(&self, digest: &str) -> (Option<String>, Option<String>) {
        match self {
            Source::Git(g) => (Some(g.base.clone()), Some(g.head.clone())),
            Source::Files { base } => (base.clone(), Some(digest.to_string())),
        }
    }
}

/// Where a comment points, always expressed against the two revisions of
/// the diff it was written on: `base` (before) and `head` (after). A `None`
/// side means that revision has no such thing (a file that was added has no
/// base file; a review of a first commit has no base revision).
///
/// An anchor is a *place*, never text: a line range of an immutable file
/// version (named by the digest of its full text, which the bundle always
/// holds). Whether a line was "added" or "removed" is deliberately not
/// stored: it follows from which side has lines at the position (an addition
/// is an empty `base` range plus a non-empty `head` one), and from the diff.
/// Where a place ends up in some other version of the file is worked out from
/// the two versions' texts (see `anchor`), not stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "lowercase")]
pub enum Anchor {
    /// The change as a whole. `base`/`head` identify the two revisions
    /// (commit id for git reviews, tree digest for directory reviews).
    Global {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        head: Option<String>,
    },
    /// A whole file: its base and head versions (the paths differ on a
    /// rename, and one side is absent for an added/deleted file).
    File {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<FileRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        head: Option<FileRef>,
    },
    /// Lines of a file. At least one side has a non-empty range.
    Span {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<LineRange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        head: Option<LineRange>,
    },
}

/// One version of one file: its path and the digest of its full text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    pub file: String,
    pub digest: String,
}

/// A run of lines in one version of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub file: String,
    /// Digest (`sha256:...`) of the full text of `file` in this version.
    pub digest: String,
    /// First line of the range (1-based). For an empty range this is the
    /// line the insertion (or deletion) point sits *before*: the point is
    /// between line `start - 1` and line `start`.
    pub start: u32,
    /// Number of lines; 0 is an insertion or deletion point.
    pub len: u32,
}

impl LineRange {
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Last line of a non-empty range.
    pub fn end(&self) -> u32 {
        self.start + self.len.saturating_sub(1)
    }
}

/// The digests of one file touched by a revision's diff, keyed by the same
/// paths as the diff's `---`/`+++` lines (`None` = no such side, i.e. the
/// file was added or deleted). Lets a viewer tell whether a comment's
/// origin file text is still what's in front of it, without needing the
/// source snapshot itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDigest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeFile {
    pub path: String,
    /// Digest of the file's full text at this revision's head.
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub id: Ulid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Digest (`sha256:...`) identifying this revision, and the key of its
    /// `diffs/` entry in the bundle: the diff text's digest for
    /// a git revision, the whole tree's digest for a directory revision.
    pub digest: String,
    pub source: Source,
    pub snapshot_mode: SnapshotMode,
    /// What the diff changed, per file (both sides' digests).
    pub files: Vec<FileDigest>,
    /// The head tree as far as this revision recorded it, by digest: every
    /// file for `Full`; for `Changed`, the files the diff touches plus the
    /// ones comments referred to. Later `Pin` events add to it. The
    /// contents are in the bundle's blob store.
    #[serde(default)]
    pub tree: Vec<TreeFile>,
}

/// One line of the JSONL document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Event {
    Meta {
        version: u32,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        context_lines: u32,
    },
    /// The review's title (the OLD way of keeping it: no longer written; a
    /// bundle that has them is read as if the last one were in its settings).
    Title {
        title: String,
        author: String,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    /// Whether the page ignores white space in a diff (the OLD way of keeping
    /// it: no longer written; read as the last one being in the settings).
    IgnoreWhitespace {
        value: bool,
        author: String,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    /// A version of the reviewed content that some edit session was made
    /// against. Appended the first time a session that adds anything sees a
    /// diff (by `digest`) the bundle hasn't recorded yet.
    Revision(Revision),
    /// Adds files to the head tree a recorded revision knows about, when a
    /// later session refers to files the revision hadn't recorded.
    Pin {
        revision: Ulid,
        files: Vec<TreeFile>,
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
