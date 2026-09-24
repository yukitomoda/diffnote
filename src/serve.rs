//! `diffnote review` and `diffnote open`: the review in a browser, from a
//! small server that only this computer can reach.
//!
//! The page is the export's own (see `html`) with buttons on the threads. A
//! button sends a small JSON request; the server appends to the bundle and
//! answers with the HTML of what changed (the thread's card and its entry in
//! the thread list), which the page swaps in place. Nothing is reloaded.
//!
//! The server keeps no state: every request loads the bundle, and a change
//! saves it, so another `diffnote review` at the same time is not overwritten
//! unseen (each request starts from what is on disk).
//!
//! Who may talk to it: it listens on `127.0.0.1` only; the address the
//! browser uses must be its own (a page from another site, or a rebinding of
//! its name, is refused); and every request needs the token printed at start,
//! which the first visit turns into a cookie.

use crate::create::{AnchorScope, LineSpan};
use crate::git::Repo;
use crate::messages::{m, mf};
use crate::model::Event;
use crate::{anchor, author, bundle, create, html, review};
use anyhow::{Context, Result};
use std::path::PathBuf;
use time::OffsetDateTime;
use ulid::Ulid;

/// Bigger bodies (a comment is text) are refused.
const MAX_BODY: usize = 1024 * 1024;
/// The widths (px) the left pane can be kept at.
const SIDEBAR_WIDTHS: std::ops::RangeInclusive<u32> = 120..=1600;

/// The cookie that carries the token. Its name has the port in it: cookies are
/// kept per host, not per port, so two servers on this machine (each with its
/// own token) would otherwise overwrite each other's.
const COOKIE: &str = "diffnote_token";

pub struct Options {
    /// The review bundle.
    pub review: PathBuf,
    /// The port to listen on; `0` picks a free one.
    pub port: u16,
    /// `--author`, if given (see `author::resolve`).
    pub author: Option<String>,
    /// The git repository a review made from git was taken from (default:
    /// the directory the server is started in), to open files the bundle
    /// doesn't store.
    pub repo: Option<PathBuf>,
    /// Takes in what was added to the target since the server started (the
    /// page's button): what it says was done, or `None` if there was nothing.
    /// Told `false`, it only looks: `Some` if there is something to take in,
    /// and nothing is written.
    pub refresh: Option<Refresher>,
    /// The bundle as it was before the server did anything to it (the difference
    /// it added, the title): `Some(None)` if there was none. Where it is
    /// `None`, the bundle as the server finds it is taken.
    pub before: Option<Option<Vec<u8>>>,
}

/// See [`Options::refresh`].
pub type Refresher = std::sync::Arc<dyn Fn(bool) -> anyhow::Result<Option<String>> + Send + Sync>;

/// A request, reduced to what the server looks at.
pub struct Request<'a> {
    pub method: &'a str,
    /// The path with its query, as sent.
    pub target: &'a str,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: &'a [u8],
}

impl Request<'_> {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
    /// Stop serving after this reply.
    pub shutdown: bool,
}

impl Reply {
    fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Reply {
            status,
            content_type,
            body: body.into(),
            headers: Vec::new(),
            shutdown: false,
        }
    }

    fn html(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self::new(status, "text/html; charset=utf-8", body)
    }

    fn json(status: u16, value: &serde_json::Value) -> Self {
        Self::new(status, "application/json; charset=utf-8", value.to_string())
    }

    fn error(status: u16, message: &str) -> Self {
        Self::json(
            status,
            &serde_json::json!({ "ok": false, "error": message }),
        )
    }
}

/// Where the lines `head` of the new side are on the old side of a file's diff:
/// the old lines they sit on (unchanged lines), or the point they are added at
/// (`len` 0), as the page counts them when it is shown the diff itself.
fn base_span_for(file: Option<&crate::diff::FileDiff>, head: LineSpan) -> LineSpan {
    // The old line a new line is at, and whether it is an unchanged one.
    let old_next = |n: u32| -> (u32, bool) {
        let mut delta: i64 = 0;
        for hunk in file.map_or(&[][..], |f| &f.hunks[..]) {
            // (A hunk with no lines on a side names the line before it.)
            let new_first = hunk.new_start + u32::from(hunk.new_lines == 0);
            let old_first = hunk.old_start + u32::from(hunk.old_lines == 0);
            if n < new_first {
                break;
            }
            let mut old_now = old_first;
            for line in &hunk.lines {
                if line.new_line == Some(n) {
                    return (line.old_line.unwrap_or(old_now), line.old_line.is_some());
                }
                if let Some(o) = line.old_line {
                    old_now = o + 1;
                }
            }
            delta = i64::from(old_first + hunk.old_lines) - i64::from(new_first + hunk.new_lines);
        }
        ((i64::from(n) + delta).max(1) as u32, true)
    };
    let (start, _) = old_next(head.start);
    let last = head.start + head.len.saturating_sub(1);
    let (end, unchanged) = old_next(last);
    LineSpan {
        start,
        len: (end + u32::from(unchanged)).saturating_sub(start),
    }
}

/// Whether a text can be a reaction: short, not a word or a number, no blanks
/// (a letter of the alphabet is not an emoji).
fn is_emoji(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 32
        && !text.is_ascii()
        && text
            .chars()
            .all(|c| !c.is_control() && !c.is_whitespace() && !c.is_ascii_alphanumeric())
}

/// A size as a person says it: `5 MB`, `830 KB`.
fn size_words(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)).replace(".0 MB", " MB")
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

/// The bytes of a text in the `%XX` form (all but letters, digits and `-_.~`).
fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The server's identity and rules; [`Server::handle`] is the whole of its
/// behavior and does no networking.
pub struct Server {
    review: PathBuf,
    /// The review as it was before the server did anything (`None`: there was
    /// none), to go back to if the session is to be thrown away (「保存せずに終了」).
    original: Option<Vec<u8>>,
    discarded: std::sync::atomic::AtomicBool,
    refresh: Option<Refresher>,
    /// The name comments are written under: `--author` or the default, and
    /// what the page sets for the rest of the session.
    author: std::sync::Mutex<String>,
    /// `--author`, kept to recompute `author` if the user settings screen
    /// clears the configured name (see `author::resolve`: `--author` still
    /// wins over it).
    explicit_author: Option<String>,
    token: String,
    port: u16,
    git: GitFiles,
    /// The comments added since this server started: the ones the page may
    /// still edit or delete. Once the server stops, they are settled.
    session: std::sync::Mutex<std::collections::HashSet<Ulid>>,
    /// The comments this session added or edited (the page tints them).
    changed: std::sync::Mutex<std::collections::HashSet<Ulid>>,
    /// What this session attached, as `image:<id>`/`file:<id>`: deleting one
    /// of these again is not worth telling anyone about at the end.
    attached: std::sync::Mutex<std::collections::HashSet<String>>,
    /// What was done since the server started, to say so when it stops.
    stats: std::sync::Mutex<Stats>,
}

/// A rough account of what a session changed.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
struct Stats {
    threads: u32,
    replies: u32,
    resolved: u32,
    reopened: u32,
    edited: u32,
    deleted: u32,
    titled: u32,
    settings: u32,
    reactions: u32,
    images: u32,
    files: u32,
    /// Attachments taken out that were not added in this session (the ones
    /// that were cancel out against `images`/`files` instead).
    removed: u32,
}

impl Stats {
    /// In words, roughly: "スレッド 2 件・返信 1 件を追加、解決 1 件".
    fn describe(&self) -> String {
        let count = |key: &str, n: u32| mf(key, &[("n", &n.to_string())]);
        let mut added = Vec::new();
        if self.threads > 0 {
            added.push(count("serve.stats.threads", self.threads));
        }
        if self.replies > 0 {
            added.push(count("serve.stats.replies", self.replies));
        }
        if self.images > 0 {
            added.push(count("serve.stats.images", self.images));
        }
        if self.files > 0 {
            added.push(count("serve.stats.files", self.files));
        }
        if self.reactions > 0 {
            added.push(count("serve.stats.reactions", self.reactions));
        }
        let mut parts = Vec::new();
        if !added.is_empty() {
            parts.push(mf(
                "serve.stats.added",
                &[("items", &added.join(m("serve.stats.separator")))],
            ));
        }
        for (n, what) in [
            (self.resolved, m("serve.stats.resolved_label")),
            (self.reopened, m("serve.stats.reopened_label")),
            (self.edited, m("serve.stats.edited_label")),
            (self.deleted, m("serve.stats.deleted_label")),
            (self.titled, m("serve.stats.titled_label")),
            (self.settings, m("serve.stats.settings_label")),
            (self.removed, m("serve.stats.removed_label")),
        ] {
            if n > 0 {
                parts.push(mf(
                    "serve.stats.count",
                    &[("what", what), ("n", &n.to_string())],
                ));
            }
        }
        if parts.is_empty() {
            m("serve.stats.none").to_string()
        } else {
            parts.join(m("serve.stats.list_separator"))
        }
    }
}

/// The commits' files, read from the repository (a tree is read once).
struct GitFiles {
    repo: Repo,
    trees: std::sync::Mutex<
        std::collections::HashMap<String, Option<std::sync::Arc<Vec<crate::git::TreeEntry>>>>,
    >,
}

impl html::CommitFiles for GitFiles {
    fn tree(&self, commit: &str) -> Option<std::sync::Arc<Vec<crate::git::TreeEntry>>> {
        let mut trees = self.trees.lock().ok()?;
        trees
            .entry(commit.to_string())
            .or_insert_with(|| {
                if self.repo.has_commit(commit) {
                    self.repo.ls_tree(commit).ok().map(std::sync::Arc::new)
                } else {
                    None
                }
            })
            .clone()
    }

    fn read(&self, entry: &crate::git::TreeEntry) -> Result<Vec<u8>, String> {
        self.repo
            .read_blobs(&[entry.oid.as_str()])
            .map_err(|e| mf("serve.git_read_failed", &[("error", &e.to_string())]))?
            .pop()
            .ok_or_else(|| m("serve.git_read_failed_unknown").to_string())
    }
}

impl Server {
    pub fn new(options: &Options, port: u16) -> Server {
        // 80 random bits each; the time part of a ULID isn't secret, so two
        // are used and it is only their random parts that count.
        let token = format!("{}{}", Ulid::new(), Ulid::new());
        Server {
            review: options.review.clone(),
            original: options
                .before
                .clone()
                .unwrap_or_else(|| std::fs::read(&options.review).ok()),
            discarded: Default::default(),
            refresh: options.refresh.clone(),
            author: std::sync::Mutex::new(author::resolve(options.author.as_deref())),
            explicit_author: options.author.clone(),
            token,
            port,
            git: GitFiles {
                repo: options.repo.clone().map_or_else(Repo::current, Repo::at),
                trees: Default::default(),
            },
            session: Default::default(),
            changed: Default::default(),
            attached: Default::default(),
            stats: Default::default(),
        }
    }

    fn author(&self) -> String {
        self.author
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn count(&self, change: impl FnOnce(&mut Stats)) {
        change(&mut self.stats.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// What this session did, roughly, and where the review is kept: the words
    /// for the terminal, and the parts for the page to lay out.
    fn farewell_parts(&self) -> serde_json::Value {
        let stats = *self.stats.lock().unwrap_or_else(|e| e.into_inner());
        let totals = bundle::load(&self.review).ok().map(|l| {
            let threads = review::build_threads(&l.events);
            (
                threads.len(),
                threads.iter().map(|t| 1 + t.replies.len()).sum::<usize>(),
            )
        });
        serde_json::json!({
            "discarded": self.was_discarded(),
            "removed": self.was_discarded() && self.original.is_none(),
            "changes": stats.describe(),
            "path": self.review.display().to_string(),
            "threads": totals.map(|t| t.0),
            "comments": totals.map(|t| t.1),
        })
    }

    fn was_discarded(&self) -> bool {
        self.discarded.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Puts the review back as it was before the server did anything: as it was, or
    /// not there at all if the server made it.
    fn discard(&self) -> Result<(), Failure> {
        let Some(original) = &self.original else {
            std::fs::remove_file(&self.review).map_err(|e| internal(e.into()))?;
            self.discarded
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return Ok(());
        };
        let dir = self
            .review
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|e| internal(e.into()))?;
        std::io::Write::write_all(&mut temp, original).map_err(|e| internal(e.into()))?;
        temp.persist(&self.review)
            .map_err(|e| internal(e.error.into()))?;
        self.discarded
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    /// What is said when the server stops.
    pub fn farewell(&self) -> String {
        let parts = self.farewell_parts();
        if self.was_discarded() {
            let path = parts["path"].as_str().unwrap_or("");
            return if self.original.is_none() {
                mf("serve.farewell.discarded_created", &[("path", path)])
            } else {
                mf("serve.farewell.discarded_kept", &[("path", path)])
            };
        }
        let kept = match (parts["threads"].as_u64(), parts["comments"].as_u64()) {
            (Some(t), Some(c)) => mf(
                "serve.farewell.saved_counts",
                &[
                    ("path", parts["path"].as_str().unwrap_or("")),
                    ("threads", &t.to_string()),
                    ("comments", &c.to_string()),
                ],
            ),
            _ => parts["path"].as_str().unwrap_or("").to_string(),
        };
        mf(
            "serve.farewell.summary",
            &[
                ("changes", parts["changes"].as_str().unwrap_or("")),
                ("kept", &kept),
            ],
        )
    }

    /// The comments the page may edit or delete: all of them (a review is
    /// shared by people who trust one another; the page asks first if a comment
    /// is somebody else's).
    fn editable(&self, loaded: &bundle::Loaded) -> Vec<String> {
        loaded
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Comment { id, .. } => Some(id.to_string()),
                _ => None,
            })
            .collect()
    }

    /// Which of the comments there are this session added or edited.
    fn changed(&self, loaded: &bundle::Loaded) -> Vec<String> {
        let changed = self.changed.lock().unwrap_or_else(|e| e.into_inner());
        self.editable(loaded)
            .into_iter()
            .filter(|id| id.parse::<Ulid>().is_ok_and(|u| changed.contains(&u)))
            .collect()
    }

    fn remember(&self, id: Ulid) {
        self.changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id);
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id);
    }

    /// The whole model, as the page has it.
    fn model_of(&self, loaded: &bundle::Loaded) -> Result<html::ViewModel, Failure> {
        let mut model = html::view_model_for(loaded, true).map_err(internal)?;
        model.editable = self.editable(loaded);
        model.changed = self.changed(loaded);
        model.author = Some(self.author());
        model.refreshable = self.refresh.is_some();
        model.settings = Some(loaded.settings.clone());
        let size = std::fs::metadata(&self.review)
            .map(|m| m.len())
            .unwrap_or(0);
        model.bundle = Some(html::bundle_info(loaded, size, model.revisions.len()));
        model.user_settings = Some(crate::user_config::load());
        Ok(model)
    }

    fn git(&self) -> Option<&dyn html::CommitFiles> {
        Some(&self.git)
    }

    /// Things the person starting the server should know: a review made from
    /// git whose repository can't be found from here (only the files the
    /// bundle stores can then be opened).
    pub fn notices(&self) -> Vec<String> {
        let Ok(loaded) = bundle::load(&self.review) else {
            return Vec::new();
        };
        let heads: Vec<String> = loaded
            .revisions()
            .filter_map(|r| match &r.source {
                crate::model::Source::Git(g) => Some(g.head.clone()),
                crate::model::Source::Files { .. } => None,
            })
            .collect();
        if heads.is_empty() {
            return Vec::new();
        }
        if !self.git.repo.exists() {
            return vec![m("serve.notice.no_repo").to_string()];
        }
        let missing = heads
            .iter()
            .filter(|h| !self.git.repo.has_commit(h))
            .count();
        if missing > 0 {
            return vec![mf(
                "serve.notice.missing_commits",
                &[("missing", &missing.to_string())],
            )];
        }
        Vec::new()
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// The address to open, with the token.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/?t={}", self.port, self.token)
    }

    pub fn handle(&self, request: &Request) -> Reply {
        if !self.host_is_ours(request) {
            return Reply::error(403, m("serve.forbidden_host"));
        }
        let (path, query) = request
            .target
            .split_once('?')
            .unwrap_or((request.target, ""));

        // The first visit brings the token in the address: keep it in a
        // cookie, and drop it from the address.
        if request.method == "GET"
            && path == "/"
            && query_value(query, "t") == Some(self.token.as_str())
        {
            let mut reply = Reply::new(302, "text/plain", "");
            reply.headers.push(("Location".into(), "/".into()));
            reply.headers.push((
                "Set-Cookie".into(),
                format!(
                    "{}={}; HttpOnly; SameSite=Strict; Path=/",
                    self.cookie_name(),
                    self.token
                ),
            ));
            return reply;
        }
        if !self.has_token(request) {
            return Reply::html(
                403,
                format!(
                    "<!DOCTYPE html><meta charset=\"utf-8\"><title>diffnote</title>\
                     <p>{}",
                    m("serve.forbidden_no_token")
                ),
            );
        }

        match (request.method, path) {
            ("GET", "/") => self.page(),
            ("GET", "/export") => self.export(),
            ("GET", "/download") => self.download(),
            ("GET", "/api/model") => self.model(),
            ("GET", "/api/version") => self.version(),
            ("GET", "/api/compare") => self.compare(query),
            ("GET", p) if p.starts_with("/api/images/") => self.image(&p["/api/images/".len()..]),
            ("GET", p) if p.starts_with("/api/attachments/") => {
                self.attachment(&p["/api/attachments/".len()..], query)
            }
            ("GET", p) if p.starts_with("/api/files/") => {
                self.files(&p["/api/files/".len()..], query)
            }
            ("POST", _) => {
                // A page from another site can't set this header without
                // asking the server first (which it doesn't allow).
                if request.header("x-diffnote") != Some("1") || !self.origin_is_ours(request) {
                    return Reply::error(403, m("serve.forbidden_operation"));
                }
                self.post(path, request)
            }
            _ => Reply::error(404, m("serve.not_found")),
        }
    }

    fn page(&self) -> Reply {
        match bundle::load(&self.review).and_then(|l| {
            let editable = self.editable(&l);
            let changed = self.changed(&l);
            html::render_served_page(
                &l,
                editable,
                changed,
                self.author(),
                self.refresh.is_some(),
                std::fs::metadata(&self.review)
                    .map(|m| m.len())
                    .unwrap_or(0),
            )
        }) {
            Ok(page) => Reply::html(200, page),
            Err(e) => Reply::html(
                500,
                format!(
                    "<!DOCTYPE html><meta charset=\"utf-8\"><title>diffnote</title>\
                     <p>{}",
                    mf(
                        "serve.page_error",
                        &[(
                            "error",
                            &e.to_string()
                                .replace('&', "&amp;")
                                .replace('<', "&lt;")
                                .replace('>', "&gt;")
                        )]
                    )
                ),
            ),
        }
    }

    /// The review as the file `diffnote export` writes, to be saved by the
    /// browser (named after the bundle: `review.diffnote` -> `review.html`).
    fn export(&self) -> Reply {
        let page = bundle::load(&self.review).and_then(|l| html::render_export(&l));
        let page = match page {
            Ok(page) => page,
            Err(e) => {
                return Reply::error(
                    500,
                    &mf("serve.export_failed", &[("error", &e.to_string())]),
                );
            }
        };
        let stem = self
            .review
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "diffnote".into());
        let name = format!("{stem}.html");
        // A plain name for old browsers, and the real one (UTF-8, %-encoded).
        let ascii: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "._-".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let encoded: String = name
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"._-".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let mut reply = Reply::html(200, page);
        reply.headers.push((
            "Content-Disposition".into(),
            format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"),
        ));
        reply
    }

    /// The bundle exactly as it is on disk right now, to be saved by the
    /// browser under the name it already has (e.g. `review.diffnote`).
    fn download(&self) -> Reply {
        let bytes = match std::fs::read(&self.review) {
            Ok(bytes) => bytes,
            Err(e) => {
                return Reply::error(
                    500,
                    &mf("serve.download_failed", &[("error", &e.to_string())]),
                );
            }
        };
        let name = self
            .review
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "review.diffnote".into());
        let ascii: String = name
            .chars()
            .map(|c| if c.is_ascii() { c } else { '_' })
            .collect();
        let mut reply = Reply::new(200, "application/zip", bytes);
        reply.headers.push((
            "Content-Disposition".into(),
            format!(
                "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
                percent_encode(&name)
            ),
        ));
        reply
            .headers
            .push(("X-Content-Type-Options".into(), "nosniff".into()));
        reply
    }

    /// The whole model, for a page that finds the review has changed under it.
    fn model(&self) -> Reply {
        let loaded = match bundle::load(&self.review) {
            Ok(l) => l,
            Err(e) => {
                return Reply::error(
                    500,
                    &mf("serve.internal_failed", &[("error", &e.to_string())]),
                );
            }
        };
        match self.model_of(&loaded) {
            Ok(model) => Reply::json(200, &serde_json::json!({ "ok": true, "model": model })),
            Err(Failure(status, message)) => Reply::error(status, &message),
        }
    }

    /// An image of the review, to show in an `<img>`. Sent so that it can do
    /// nothing else if it is opened by itself (an SVG runs nothing).
    fn image(&self, id: &str) -> Reply {
        if !crate::image::is_id(id) {
            return Reply::error(404, m("serve.not_found"));
        }
        let Some(bytes) = bundle::read_image(&self.review, id) else {
            return Reply::error(404, m("serve.image_missing"));
        };
        let Ok(mime) = crate::image::kind(&bytes) else {
            return Reply::error(404, m("serve.image_unsupported"));
        };
        let mut reply = Reply::new(200, mime, bytes);
        reply.headers = vec![
            (
                "Content-Security-Policy".into(),
                "sandbox; default-src 'none'; style-src 'unsafe-inline'".into(),
            ),
            ("X-Content-Type-Options".into(), "nosniff".into()),
            ("Cross-Origin-Resource-Policy".into(), "same-origin".into()),
            (
                "Cache-Control".into(),
                "private, max-age=31536000, immutable".into(),
            ),
        ];
        reply
    }

    /// Puts an image (the bytes sent) in the review, and says what it is called
    /// there, how big it is, and how big the bundle now is.
    fn add_image(&self, target: &str, bytes: &[u8]) -> Result<Reply, Failure> {
        let mime = crate::image::kind(bytes).map_err(|m| Failure(400, m))?;
        self.within_limit(bytes.len())?;
        let id = crate::image::id_of(bytes);
        let mut loaded = bundle::load(&self.review).map_err(internal)?;
        let new = loaded.image(&id).is_none();
        let named = remember_name(&mut loaded, &id, target);
        if new || named {
            let none = bundle::Additions::default();
            let added = if new {
                vec![bytes.to_vec()]
            } else {
                Vec::new()
            };
            let images = bundle::Images {
                add: &added,
                ..Default::default()
            };
            bundle::save_with(&self.review, &loaded, &loaded.events, &none, &images)
                .map_err(internal)?;
        }
        if new {
            self.remember_attached("image", &id);
            self.count(|s| s.images += 1);
        }
        let bundle_size = std::fs::metadata(&self.review)
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(Reply::json(
            200,
            &serde_json::json!({
                "ok": true,
                "id": id,
                "type": mime,
                "size": bytes.len(),
                "bundle_size": bundle_size,
            }),
        ))
    }

    /// Whether a file of this many bytes may be attached, by the limit the
    /// review has.
    fn within_limit(&self, size: usize) -> Result<(), Failure> {
        let loaded = bundle::load(&self.review).map_err(internal)?;
        let limit = loaded.settings.attachment_limit;
        if size as u64 > limit {
            return Err(Failure(
                413,
                mf(
                    "serve.attachment_too_big",
                    &[
                        ("limit", &size_words(limit)),
                        ("size", &size_words(size as u64)),
                    ],
                ),
            ));
        }
        Ok(())
    }

    /// Puts a file that is not an image in the review (`name` is only what the
    /// page calls it: it is asked for again when the file is fetched).
    fn add_attachment(&self, target: &str, bytes: &[u8]) -> Result<Reply, Failure> {
        if bytes.is_empty() {
            return Err(Failure(400, m("serve.attachment_empty").into()));
        }
        self.within_limit(bytes.len())?;
        let id = crate::image::id_of(bytes);
        let mut loaded = bundle::load(&self.review).map_err(internal)?;
        let new = loaded.attachment(&id).is_none();
        let named = remember_name(&mut loaded, &id, target);
        if new || named {
            let none = bundle::Additions::default();
            let added = if new {
                vec![bytes.to_vec()]
            } else {
                Vec::new()
            };
            let files = bundle::Images {
                add_files: &added,
                ..Default::default()
            };
            bundle::save_with(&self.review, &loaded, &loaded.events, &none, &files)
                .map_err(internal)?;
        }
        if new {
            self.remember_attached("file", &id);
            self.count(|s| s.files += 1);
        }
        let name = crate::image::file_name(
            &query_param(target.split_once('?').map_or("", |(_, q)| q), "name").unwrap_or_default(),
        );
        let bundle_size = std::fs::metadata(&self.review)
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(Reply::json(
            200,
            &serde_json::json!({
                "ok": true,
                "id": id,
                "name": name,
                "size": bytes.len(),
                "bundle_size": bundle_size,
            }),
        ))
    }

    fn remember_attached(&self, kind: &str, id: &str) {
        self.attached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(format!("{kind}:{id}"));
    }

    /// Takes an image or another attached file out of the review, whether or
    /// not a comment still refers to it (the page asks first, and says which
    /// comments do). The text of those comments is left as it was, so the
    /// image or link in them is simply not there any more.
    fn delete_attached(&self, kind: &str, id: &str) -> Result<Reply, Failure> {
        let image = kind == "image";
        if !crate::image::is_id(id) {
            return Err(Failure(404, m("serve.not_found").into()));
        }
        let loaded = bundle::load(&self.review).map_err(internal)?;
        let held = if image {
            loaded.image(id).is_some()
        } else {
            loaded.attachment(id).is_some()
        };
        if !held {
            return Err(Failure(404, m("serve.attachment_missing").into()));
        }
        let before = html::stamp(&loaded);
        let keep: std::collections::HashSet<String> = if image {
            loaded.image_ids()
        } else {
            loaded.attachment_ids()
        }
        .into_iter()
        .filter(|held| held != id)
        .collect();
        let none = bundle::Additions::default();
        let images = if image {
            bundle::Images {
                keep: Some(&keep),
                ..Default::default()
            }
        } else {
            bundle::Images {
                keep_files: Some(&keep),
                ..Default::default()
            }
        };
        bundle::save_with(&self.review, &loaded, &loaded.events, &none, &images)
            .map_err(internal)?;
        // One this session attached is simply undone; anything older is a
        // change of its own, worth saying at the end.
        let ours = self
            .attached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&format!("{kind}:{id}"));
        self.count(|s| match (ours, image) {
            (true, true) => s.images = s.images.saturating_sub(1),
            (true, false) => s.files = s.files.saturating_sub(1),
            (false, _) => s.removed += 1,
        });
        self.model_answer(&before, serde_json::json!({}))
    }

    /// A file attached to a comment, to be saved: never shown by itself.
    fn attachment(&self, id: &str, query: &str) -> Reply {
        if !crate::image::is_id(id) {
            return Reply::error(404, m("serve.not_found"));
        }
        let Some(bytes) = bundle::read_attachment(&self.review, id) else {
            return Reply::error(404, m("serve.attachment_missing"));
        };
        let name = crate::image::file_name(&query_param(query, "name").unwrap_or_default());
        let mut reply = Reply::new(200, "application/octet-stream", bytes);
        reply.headers = vec![
            (
                "Content-Disposition".into(),
                format!(
                    "attachment; filename=\"{}\"; filename*=UTF-8''{}",
                    name.chars()
                        .map(|c| if c.is_ascii() { c } else { '_' })
                        .collect::<String>(),
                    percent_encode(&name)
                ),
            ),
            (
                "Content-Security-Policy".into(),
                "sandbox; default-src 'none'".into(),
            ),
            ("X-Content-Type-Options".into(), "nosniff".into()),
            (
                "Cache-Control".into(),
                "private, max-age=31536000, immutable".into(),
            ),
        ];
        reply
    }

    /// At the end of the session: the images that no comment shows any more
    /// (one was pasted and the comment given up, or taken out) are let go.
    fn drop_unused_images(&self) {
        let Ok(loaded) = bundle::load(&self.review) else {
            return;
        };
        let mut used = std::collections::HashSet::new();
        let mut used_files = std::collections::HashSet::new();
        for event in &loaded.events {
            if let Event::Comment { body, .. } = event {
                used.extend(crate::image::ids_in(body));
                used_files.extend(crate::image::file_ids_in(body));
            }
        }
        if loaded.image_ids().iter().all(|id| used.contains(id))
            && loaded
                .attachment_ids()
                .iter()
                .all(|id| used_files.contains(id))
        {
            return;
        }
        let none = bundle::Additions::default();
        let images = bundle::Images {
            keep: Some(&used),
            keep_files: Some(&used_files),
            ..Default::default()
        };
        let _ = bundle::save_with(&self.review, &loaded, &loaded.events, &none, &images);
    }

    /// A revision as it looks against an earlier one (`rev` and `from`, as the
    /// tabs number them, from 0) instead of against the base: for looking only.
    fn compare(&self, query: &str) -> Reply {
        let number = |key: &str| query_param(query, key).and_then(|v| v.parse::<usize>().ok());
        let (Some(to), Some(from)) = (number("rev"), number("from")) else {
            return Reply::error(400, m("serve.compare_missing_revisions"));
        };
        let loaded = match bundle::load(&self.review) {
            Ok(l) => l,
            Err(e) => {
                return Reply::error(
                    500,
                    &mf("serve.internal_failed", &[("error", &e.to_string())]),
                );
            }
        };
        match html::compare_data(&loaded, to, from) {
            Ok(revision) => Reply::json(
                200,
                &serde_json::json!({ "ok": true, "revision": revision, "stamp": html::stamp(&loaded) }),
            ),
            Err(e) => Reply::error(400, &e.to_string()),
        }
    }

    /// The stamp of the review's log: a page compares it with its own to see
    /// whether the review has changed.
    fn version(&self) -> Reply {
        match bundle::load(&self.review) {
            Ok(l) => Reply::json(
                200,
                &serde_json::json!({
                    "ok": true,
                    "stamp": html::stamp(&l),
                    // Something new to take in (a failure to look says no).
                    "pending": self.refresh.as_ref().is_some_and(|r| r(false).ok().flatten().is_some()),
                }),
            ),
            Err(e) => Reply::error(
                500,
                &mf("serve.internal_failed", &[("error", &e.to_string())]),
            ),
        }
    }

    /// The stored files of a revision, to look at: `{rev}/tree` lists them,
    /// `{rev}/open` reads one, `{rev}/more` its next lines.
    fn files(&self, what: &str, query: &str) -> Reply {
        let loaded = match bundle::load(&self.review) {
            Ok(l) => l,
            Err(e) => {
                return Reply::error(
                    500,
                    &mf("serve.internal_failed", &[("error", &e.to_string())]),
                );
            }
        };
        let param = |key: &str| query_param(query, key).unwrap_or_default();
        let Some((revision, action)) = what
            .split_once('/')
            .and_then(|(r, a)| Some((r.parse::<usize>().ok()?, a)))
        else {
            return Reply::error(404, m("serve.not_found"));
        };
        let refused = |message: String| Reply::error(400, &message);
        match action {
            "tree" => {
                match html::tree_json(&loaded, revision, &param("dir"), &param("q"), self.git()) {
                    Some(mut list) => {
                        list["ok"] = true.into();
                        Reply::json(200, &list)
                    }
                    None => Reply::error(404, m("serve.revision_missing")),
                }
            }
            // Lines of a file the diff leaves out: `count` from line `from`.
            "lines" => {
                let number =
                    |key: &str, default: usize| param(key).parse::<usize>().unwrap_or(default);
                match html::lines_json(
                    &loaded,
                    revision,
                    &param("path"),
                    number("from", 1),
                    number("count", 20),
                    self.git(),
                ) {
                    Ok(lines) => {
                        Reply::json(200, &serde_json::json!({ "ok": true, "lines": lines }))
                    }
                    Err(message) => refused(message),
                }
            }
            "open" => match html::opened_data(&loaded, revision, &param("path"), self.git()) {
                Ok(file) => Reply::json(200, &serde_json::json!({ "ok": true, "file": file })),
                Err(message) => refused(message),
            },
            "more" => {
                let from = param("from").parse::<usize>().unwrap_or(1);
                match html::chunk_data(&loaded, revision, &param("path"), from, self.git()) {
                    Ok((hunk, next)) => Reply::json(
                        200,
                        &serde_json::json!({ "ok": true, "hunk": hunk, "next": next }),
                    ),
                    Err(message) => refused(message),
                }
            }
            _ => Reply::error(404, m("serve.not_found")),
        }
    }

    /// A new thread on lines of a revision's diff, on a file, or on the whole
    /// review. The page says which lines as counters on each side (see
    /// `lib.counters` in `ui/src`).
    fn create_thread(&self, body: &[u8]) -> Result<Reply, Failure> {
        let bad = |key: &str| Failure(400, m(key).to_string());
        let value: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| bad("serve.body_unreadable"))?;
        let text = value
            .get("body")
            .and_then(|b| b.as_str())
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .ok_or_else(|| bad("serve.comment_body_empty"))?;
        let revision = value
            .get("revision")
            .and_then(|r| r.as_u64())
            .ok_or_else(|| bad("serve.revision_missing_param"))? as usize;
        // What the thread is about: lines (the default), a whole file, or the
        // whole review.
        let kind = value
            .get("scope")
            .and_then(|s| s.as_str())
            .unwrap_or("lines");
        let file = value.get("file").and_then(|f| f.as_str());
        let span = |side: &str| -> Result<LineSpan, Failure> {
            let part = value.get(side).ok_or_else(|| bad("serve.range_missing"))?;
            let number = |key: &str| {
                part.get(key)
                    .and_then(|n| n.as_u64())
                    .filter(|n| *n <= 100_000_000)
                    .map(|n| n as u32)
                    .ok_or_else(|| bad("serve.range_invalid"))
            };
            let (start, len) = (number("start")?, number("len")?);
            // A side with no lines may sit at 0: a file that is new has no old
            // side, and a deleted one has no new side.
            if start == 0 && len > 0 {
                return Err(bad("serve.line_number_starts_at_1"));
            }
            Ok(LineSpan { start, len })
        };
        let mut derive_base = false;
        let mut scope = match kind {
            "lines" => {
                let file = file.ok_or_else(|| bad("serve.file_missing_param"))?;
                let head = span("head")?;
                // The page says the lines of the new side only (as it does when it
                // shows the revision against another one): where they are on the
                // old side is what the revision's own diff says.
                let base = if value.get("base").is_some() {
                    span("base")?
                } else {
                    derive_base = true;
                    LineSpan { start: 0, len: 0 }
                };
                if (base.len == 0 && head.len == 0) && !derive_base || derive_base && head.len == 0
                {
                    return Err(bad("serve.no_lines_selected"));
                }
                AnchorScope::Span {
                    file: file.to_string(),
                    base,
                    head,
                }
            }
            "file" => AnchorScope::File {
                file: file
                    .ok_or_else(|| bad("serve.file_missing_param"))?
                    .to_string(),
            },
            "global" => AnchorScope::Global,
            _ => return Err(bad("serve.bad_comment_kind")),
        };

        let loaded = bundle::load(&self.review).map_err(internal)?;
        let view = html::anchor_view(&loaded, revision, file, self.git())
            .ok_or_else(|| Failure(404, m("serve.revision_missing").into()))?;
        let (rev, diff, files) = (&view.revision, &view.diff, &view.files);
        // Lines and files are those of the page's diff (of the revision, and
        // the files it doesn't touch that threads have brought in).
        if let AnchorScope::Span { file, .. } | AnchorScope::File { file } = &scope
            && anchor::find_file(diff, file).is_none()
        {
            return Err(bad("serve.file_not_in_diff"));
        }
        if derive_base && let AnchorScope::Span { file, base, head } = &mut scope {
            *base = base_span_for(anchor::find_file(diff, file), *head);
        }
        let anchor = create::build_anchor(&scope, diff, files, &rev.source.revisions(&rev.digest))
            .map_err(internal)?;
        let id = Ulid::new();
        let mut events = Vec::new();
        let mut blobs = Vec::new();
        // A file of the commit that the bundle doesn't have yet: kept with the
        // thread (its entry for this revision, and its content).
        if let Some((entry, bytes)) = view.store {
            events.push(Event::Pin {
                revision: rev.id,
                files: vec![entry],
            });
            blobs.push(bytes);
        }
        events.push(Event::Comment {
            id,
            parent: None,
            author: self.author(),
            created_at: OffsetDateTime::now_utc(),
            anchor: Some(anchor),
            body: text.to_string(),
        });
        let before = html::stamp(&loaded);
        self.append_all(events, blobs)?;
        self.remember(id);
        self.count(|s| s.threads += 1);
        self.model_answer(&before, serde_json::json!({ "thread": id.to_string() }))
    }

    /// The answer to a change that gives the page the whole model: it has, as
    /// `before`, the stamp the review had before the change.
    fn model_answer(&self, before: &str, mut extra: serde_json::Value) -> Result<Reply, Failure> {
        let loaded = bundle::load(&self.review).map_err(internal)?;
        let model = self.model_of(&loaded)?;
        extra["ok"] = true.into();
        extra["before"] = before.into();
        extra["model"] = serde_json::to_value(&model).map_err(|e| internal(e.into()))?;
        Ok(Reply::json(200, &extra))
    }

    fn post(&self, path: &str, request: &Request) -> Reply {
        if request.body.len() > MAX_BODY && path != "/api/images" && path != "/api/attachments" {
            return Reply::error(413, m("serve.body_too_big"));
        }
        if path == "/api/shutdown" {
            let discard = serde_json::from_slice::<serde_json::Value>(request.body)
                .ok()
                .and_then(|v| v.get("discard").and_then(|d| d.as_bool()))
                .unwrap_or(false);
            if discard && let Err(Failure(status, message)) = self.discard() {
                return Reply::error(status, &message);
            }
            if !discard {
                self.drop_unused_images();
            }
            let mut reply = Reply::json(
                200,
                &serde_json::json!({ "ok": true, "farewell": self.farewell(), "summary": self.farewell_parts() }),
            );
            reply.shutdown = true;
            return reply;
        }
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        let result = match segments.as_slice() {
            ["api", "threads"] => self.create_thread(request.body),
            ["api", "threads", id, "replies"] => {
                self.with_thread(id, |thread| self.reply(thread, request.body))
            }
            ["api", "threads", id, "resolve"] => {
                self.with_thread(id, |thread| self.set_resolved(thread, true))
            }
            ["api", "threads", id, "reopen"] => {
                self.with_thread(id, |thread| self.set_resolved(thread, false))
            }
            ["api", "settings"] => self.set_settings(request.body),
            ["api", "user-settings"] => self.set_user_settings(request.body),
            ["api", "view"] => self.set_view(request.body),
            ["api", "images"] => self.add_image(request.target, request.body),
            ["api", "attachments"] => self.add_attachment(request.target, request.body),
            ["api", "images", id, "delete"] => self.delete_attached("image", id),
            ["api", "attachments", id, "delete"] => self.delete_attached("file", id),
            ["api", "refresh"] => self.refresh(),
            ["api", "comments", id, "edit"] => self.edit_comment(id, request.body),
            ["api", "comments", id, "delete"] => self.delete_comment(id),
            ["api", "comments", id, "react"] => self.react(id, request.body),
            _ => return Reply::error(404, m("serve.not_found")),
        };
        match result {
            Ok(reply) => reply,
            Err(Failure(status, message)) => Reply::error(status, &message),
        }
    }

    /// Runs `action` on the thread with this id, then answers with what the
    /// page needs to show it as it now is.
    fn with_thread(
        &self,
        id: &str,
        action: impl FnOnce(&review::Thread) -> Result<(), Failure>,
    ) -> Result<Reply, Failure> {
        let id =
            Ulid::from_string(id).map_err(|_| Failure(400, m("serve.thread_id_invalid").into()))?;
        let loaded = bundle::load(&self.review).map_err(internal)?;
        let thread = review::build_threads(&loaded.events)
            .into_iter()
            .find(|t| t.root_id == id)
            .ok_or_else(|| Failure(404, m("serve.thread_missing").into()))?;
        let before = html::stamp(&loaded);
        action(&thread)?;
        let loaded = bundle::load(&self.review).map_err(internal)?;
        Ok(Reply::json(
            200,
            &serde_json::json!({
                "ok": true,
                "thread": id.to_string(),
                "thread_data": html::thread_json(&loaded, id),
                "before": before,
                "stamp": html::stamp(&loaded),
                "editable": self.editable(&loaded),
                "changed": self.changed(&loaded),
            }),
        ))
    }

    /// Changes this machine's user settings (today, just `author`; see
    /// `diffnote config`): saved to the OS config file, so every bundle's
    /// `init`/`review`/`open` uses it from now on, not only this one. Also
    /// updates the name comments are written under for the rest of this
    /// session (an empty name clears the configured one: the session's name
    /// falls back through `--author`, git, and the login name, as it would
    /// at startup with nothing configured).
    fn set_user_settings(&self, body: &[u8]) -> Result<Reply, Failure> {
        let value: serde_json::Value = serde_json::from_slice(body)
            .map_err(|_| Failure(400, m("serve.body_unreadable").into()))?;
        let name = value
            .get("author")
            .and_then(|a| a.as_str())
            .ok_or_else(|| Failure(400, m("serve.author_missing").into()))?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if name.chars().count() > 100 {
            return Err(Failure(400, m("serve.author_too_long").into()));
        }
        let mut config = crate::user_config::load();
        config.author = (!name.is_empty()).then(|| name.clone());
        crate::user_config::save(&config).map_err(internal)?;
        let effective = if name.is_empty() {
            author::resolve(self.explicit_author.as_deref())
        } else {
            name
        };
        *self.author.lock().unwrap_or_else(|e| e.into_inner()) = effective;
        let loaded = bundle::load(&self.review).map_err(internal)?;
        let before = html::stamp(&loaded);
        self.model_answer(&before, serde_json::json!({}))
    }

    /// Keeps how the page is shown (layout, wrapping, ...) in this machine's
    /// user settings, for every review and every run: what the body names
    /// is changed, the rest is left as it was. Nothing on the page changes.
    fn set_view(&self, body: &[u8]) -> Result<Reply, Failure> {
        let asked: crate::user_config::ViewPrefs = serde_json::from_slice(body)
            .map_err(|_| Failure(400, m("serve.body_unreadable").into()))?;
        // (The page keeps the width in its own bounds; anything far outside
        // them didn't come from it.)
        if asked
            .sidebar_width
            .is_some_and(|w| !(SIDEBAR_WIDTHS).contains(&w))
        {
            return Err(Failure(400, m("serve.body_unreadable").into()));
        }
        let mut config = crate::user_config::load();
        config.view.merge(asked);
        crate::user_config::save(&config).map_err(internal)?;
        Ok(Reply::json(200, &serde_json::json!({ "ok": true })))
    }

    /// Takes in what was added to the target since the server started, as a
    /// new revision (the page is told, and keeps showing what it showed).
    fn refresh(&self) -> Result<Reply, Failure> {
        let Some(refresh) = &self.refresh else {
            return Err(Failure(400, m("serve.refresh_unavailable").into()));
        };
        let before = html::stamp(&bundle::load(&self.review).map_err(internal)?);
        let said = refresh(true).map_err(|e| {
            Failure(
                500,
                mf("serve.refresh_failed", &[("error", &e.to_string())]),
            )
        })?;
        let added = said.is_some();
        let message = said.unwrap_or_else(|| m("serve.refresh_nothing").into());
        self.model_answer(
            &before,
            serde_json::json!({ "message": message, "added": added }),
        )
    }

    /// Changes the review's settings: what is given is set (a title that is empty
    /// takes the title away), the rest is left as it is. They are kept in the
    /// review.
    fn set_settings(&self, body: &[u8]) -> Result<Reply, Failure> {
        let value: serde_json::Value = serde_json::from_slice(body)
            .map_err(|_| Failure(400, m("serve.body_unreadable").into()))?;
        let bad = |key: &str| Failure(400, m(key).to_string());
        let title = match value.get("title") {
            None | Some(serde_json::Value::Null) => None,
            Some(t) => {
                let title = t
                    .as_str()
                    .ok_or_else(|| bad("serve.title_not_string"))?
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if title.chars().count() > 200 {
                    return Err(bad("serve.title_too_long"));
                }
                Some(title)
            }
        };
        let ignore = match value.get("ignore_whitespace") {
            None => None,
            Some(v) => Some(
                v.as_bool()
                    .ok_or_else(|| bad("serve.ignore_whitespace_invalid"))?,
            ),
        };
        let limit = match value.get("attachment_limit") {
            None => None,
            Some(v) => {
                let bytes = v
                    .as_u64()
                    .ok_or_else(|| bad("serve.attachment_limit_invalid"))?;
                if !(1024..=crate::image::CEILING as u64).contains(&bytes) {
                    return Err(Failure(
                        400,
                        mf(
                            "serve.attachment_limit_range",
                            &[
                                ("min", &size_words(1024)),
                                ("max", &size_words(crate::image::CEILING as u64)),
                            ],
                        ),
                    ));
                }
                Some(bytes)
            }
        };
        let mut loaded = bundle::load(&self.review).map_err(internal)?;
        let before = html::stamp(&loaded);
        let titled = title.is_some_and(|t| review::set_title(&mut loaded.settings, &t));
        let mut others = false;
        if let Some(on) = ignore {
            others |= review::set_ignore_whitespace(&mut loaded.settings, on);
        }
        if let Some(bytes) = limit
            && loaded.settings.attachment_limit != bytes
        {
            loaded.settings.attachment_limit = bytes;
            others = true;
        }
        if titled || others {
            let events = loaded.events.clone();
            self.save(&loaded, &events)?;
            self.count(|s| {
                s.titled += u32::from(titled);
                s.settings += u32::from(others);
            });
        }
        self.model_answer(&before, serde_json::json!({}))
    }

    /// The id a comment is asked for by.
    fn comment_id(id: &str) -> Result<Ulid, Failure> {
        Ulid::from_string(id).map_err(|_| Failure(400, m("serve.comment_id_invalid").into()))
    }

    /// The signed-in name reacting to a comment with an emoji: taken back if it
    /// was there. Kept in the review.
    fn react(&self, id: &str, body: &[u8]) -> Result<Reply, Failure> {
        let id = Self::comment_id(id)?;
        let value: serde_json::Value = serde_json::from_slice(body)
            .map_err(|_| Failure(400, m("serve.body_unreadable").into()))?;
        let emoji = value
            .get("emoji")
            .and_then(|e| e.as_str())
            .filter(|e| is_emoji(e))
            .ok_or_else(|| Failure(400, m("serve.emoji_invalid").into()))?;
        let mut loaded = bundle::load(&self.review).map_err(internal)?;
        let before = html::stamp(&loaded);
        let deleted = loaded.events.iter().find_map(|e| match e {
            Event::Comment { id: c, body, .. } if *c == id => Some(body.is_empty()),
            _ => None,
        });
        match deleted {
            None => return Err(Failure(404, m("serve.comment_missing").into())),
            Some(true) => {
                return Err(Failure(409, m("serve.reaction_on_deleted").into()));
            }
            Some(false) => {}
        }
        let key = id.to_string();
        let new_kind = loaded
            .reactions
            .get(&key)
            .is_none_or(|list| !list.iter().any(|r| r.emoji == emoji));
        if new_kind && loaded.reactions.get(&key).is_some_and(|l| l.len() >= 30) {
            return Err(Failure(409, m("serve.reaction_kind_limit").into()));
        }
        let now = review::toggle_reaction(&mut loaded.reactions, &key, emoji, &self.author());
        let events = loaded.events.clone();
        self.save(&loaded, &events)?;
        self.count(|s| {
            if now {
                s.reactions += 1;
            } else {
                s.reactions = s.reactions.saturating_sub(1);
            }
        });
        self.model_answer(&before, serde_json::json!({ "reacted": now }))
    }

    /// Rewrites the text of a comment (whoever wrote it, and whenever).
    fn edit_comment(&self, id: &str, body: &[u8]) -> Result<Reply, Failure> {
        let id = Self::comment_id(id)?;
        let value: serde_json::Value = serde_json::from_slice(body)
            .map_err(|_| Failure(400, m("serve.body_unreadable").into()))?;
        let text = value
            .get("body")
            .and_then(|b| b.as_str())
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .ok_or_else(|| Failure(400, m("serve.comment_body_empty").into()))?;
        let loaded = bundle::load(&self.review).map_err(internal)?;
        let before = html::stamp(&loaded);
        let mut events = loaded.events.clone();
        let target = events
            .iter_mut()
            .find_map(|e| match e {
                Event::Comment { id: c, body, .. } if *c == id => Some(body),
                _ => None,
            })
            .ok_or_else(|| Failure(404, m("serve.comment_missing").into()))?;
        if target.is_empty() {
            return Err(Failure(409, m("serve.edit_deleted_refused").into()));
        }
        *target = text.to_string();
        self.save(&loaded, &events)?;
        self.changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id);
        self.count(|s| s.edited += 1);
        self.model_answer(&before, serde_json::json!({}))
    }

    /// Takes a comment out of the review, that one only. A reply is taken out of
    /// the log. The first comment of a thread has the thread's place in it, and
    /// the replies stand on it: while there are replies it stays, with no text
    /// (the page says it was deleted); a thread with nothing else is taken out
    /// whole (with resolving, and where it was moved to), as is one whose
    /// last reply goes when its first comment was deleted before.
    fn delete_comment(&self, id: &str) -> Result<Reply, Failure> {
        let id = Self::comment_id(id)?;
        let mut loaded = bundle::load(&self.review).map_err(internal)?;
        let before = html::stamp(&loaded);
        let found = loaded.events.iter().find_map(|e| match e {
            Event::Comment { id: c, parent, .. } if *c == id => Some(*parent),
            _ => None,
        });
        let Some(parent) = found else {
            return Err(Failure(404, m("serve.comment_missing").into()));
        };
        let replies_of = |root: Ulid, except: Option<Ulid>| {
            loaded
                .events
                .iter()
                .filter(|e| matches!(e, Event::Comment { id: c, parent: Some(p), .. } if *p == root && Some(*c) != except))
                .count()
        };
        // The thread that goes whole, if one does (its root's id).
        let whole_thread = match parent {
            None if replies_of(id, None) == 0 => Some(id),
            Some(root) => {
                let root_deleted = loaded.events.iter().any(
                    |e| matches!(e, Event::Comment { id: c, body, .. } if *c == root && body.is_empty()),
                );
                (root_deleted && replies_of(root, Some(id)) == 0).then_some(root)
            }
            None => None,
        };
        let gone = |e: &Event| match e {
            Event::Comment {
                id: c, parent: p, ..
            } => match whole_thread {
                Some(root) => *c == root || *p == Some(root),
                None => parent.is_some() && *c == id,
            },
            Event::Resolve { parent: p, .. }
            | Event::Reopen { parent: p, .. }
            | Event::Reanchor { parent: p, .. } => whole_thread == Some(*p),
            _ => false,
        };
        let removed: Vec<Ulid> = loaded
            .events
            .iter()
            .filter(|e| gone(e))
            .filter_map(|e| match e {
                Event::Comment { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        let events: Vec<Event> = if parent.is_none() && whole_thread.is_none() {
            // The first comment of a thread that has replies: it stays, with no text.
            let mut events = loaded.events.clone();
            for e in &mut events {
                if let Event::Comment { id: c, body, .. } = e
                    && *c == id
                {
                    body.clear();
                }
            }
            events
        } else {
            loaded.events.iter().filter(|e| !gone(e)).cloned().collect()
        };
        // What was reacted with goes with the comment, and with the text that a
        // first comment was given, when it is deleted and kept as a mark.
        for gone in removed.iter().chain(std::iter::once(&id)) {
            if parent.is_none() && whole_thread.is_none() || removed.contains(gone) {
                loaded.reactions.remove(&gone.to_string());
            }
        }
        self.save(&loaded, &events)?;
        // What this session added and is now taken out no longer counts as added
        // (what was there before is only counted as taken out).
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let thread_here = whole_thread.is_some_and(|root| session.contains(&root));
        let replies_here = removed
            .iter()
            .filter(|c| Some(**c) != whole_thread && session.contains(*c))
            .count() as u32;
        for c in &removed {
            session.remove(c);
        }
        drop(session);
        self.count(|s| {
            if thread_here {
                s.threads = s.threads.saturating_sub(1);
            }
            s.replies = s.replies.saturating_sub(replies_here);
            s.deleted += 1;
        });
        self.model_answer(&before, serde_json::json!({}))
    }

    fn save(&self, loaded: &bundle::Loaded, events: &[Event]) -> Result<(), Failure> {
        bundle::save(&self.review, loaded, events, &bundle::Additions::default()).map_err(internal)
    }

    fn append(&self, event: Event) -> Result<(), Failure> {
        self.append_all(vec![event], Vec::new())
    }

    fn append_all(&self, added: Vec<Event>, blobs: Vec<Vec<u8>>) -> Result<(), Failure> {
        let loaded = bundle::load(&self.review).map_err(internal)?;
        let mut events = loaded.events.clone();
        events.extend(added);
        let more = bundle::Additions {
            diff: None,
            blobs,
            commits: Vec::new(),
        };
        bundle::save(&self.review, &loaded, &events, &more).map_err(internal)
    }

    fn reply(&self, thread: &review::Thread, body: &[u8]) -> Result<(), Failure> {
        let value: serde_json::Value = serde_json::from_slice(body)
            .map_err(|_| Failure(400, m("serve.body_unreadable").into()))?;
        let text = value
            .get("body")
            .and_then(|b| b.as_str())
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .ok_or_else(|| Failure(400, m("serve.reply_body_empty").into()))?;
        let id = Ulid::new();
        self.append(Event::Comment {
            id,
            parent: Some(thread.root_id),
            author: self.author(),
            created_at: OffsetDateTime::now_utc(),
            anchor: None,
            body: text.to_string(),
        })?;
        self.remember(id);
        self.count(|s| s.replies += 1);
        Ok(())
    }

    fn set_resolved(&self, thread: &review::Thread, resolved: bool) -> Result<(), Failure> {
        if thread.resolved == resolved {
            // Already so (another tab, or the command line, got there first).
            return Ok(());
        }
        let (parent, author, created_at) =
            (thread.root_id, self.author(), OffsetDateTime::now_utc());
        self.append(if resolved {
            Event::Resolve {
                parent,
                author,
                created_at,
            }
        } else {
            Event::Reopen {
                parent,
                author,
                created_at,
            }
        })?;
        self.count(|s| {
            if resolved {
                s.resolved += 1;
            } else {
                s.reopened += 1;
            }
        });
        Ok(())
    }

    fn host_is_ours(&self, request: &Request) -> bool {
        let ours = [
            format!("127.0.0.1:{}", self.port),
            format!("localhost:{}", self.port),
        ];
        request
            .header("host")
            .is_some_and(|h| ours.iter().any(|o| o == h))
    }

    fn origin_is_ours(&self, request: &Request) -> bool {
        let ours = [
            format!("http://127.0.0.1:{}", self.port),
            format!("http://localhost:{}", self.port),
        ];
        request
            .header("origin")
            .is_none_or(|o| ours.iter().any(|x| x == o))
    }

    pub fn cookie_name(&self) -> String {
        format!("{COOKIE}_{}", self.port)
    }

    fn has_token(&self, request: &Request) -> bool {
        request
            .header("cookie")
            .into_iter()
            .flat_map(|c| c.split(';'))
            .filter_map(|p| p.trim().split_once('='))
            .any(|(name, value)| name == self.cookie_name() && value == self.token)
    }
}

struct Failure(u16, String);

fn internal(e: anyhow::Error) -> Failure {
    Failure(
        500,
        mf("serve.internal_failed", &[("error", &e.to_string())]),
    )
}

/// The value of `key` in a query string, with `%XX` and `+` decoded.
/// Keeps what the file being attached was called where it came from (the
/// page sends it as `?name=`), unless the same bytes already arrived under a
/// name: an attachment is its bytes, so the first name they came with is the
/// one it goes by. Says whether that changed anything.
fn remember_name(loaded: &mut bundle::Loaded, id: &str, target: &str) -> bool {
    if loaded.attached.get(id).is_some_and(|a| a.name.is_some()) {
        return false;
    }
    let Some(name) = attached_name(target) else {
        return false;
    };
    loaded.attached.entry(id.to_string()).or_default().name = Some(name);
    true
}

/// The name the page says the file had, as a file name; `None` if it said
/// none, or said one that stands for nothing. A screenshot pasted out of the
/// clipboard arrives as `image.png` whatever it is a picture of, which is
/// worse than saying nothing: the comment's own words are better than that.
fn attached_name(target: &str) -> Option<String> {
    let query = target.split_once('?').map_or("", |(_, q)| q);
    let asked = query_param(query, "name")?;
    if asked.trim().is_empty() {
        return None;
    }
    let name = crate::image::file_name(&asked);
    (name != "image.png").then_some(name)
}

fn query_param(query: &str, key: &str) -> Option<String> {
    let raw = query
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)?;
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if bytes.get(i + 1..i + 3).is_some() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// Starts the server and serves until told to stop (the page's "終了" button,
/// or Ctrl+C in the terminal). `on_ready` gets the address to open.
pub fn run(options: &Options, on_ready: impl FnOnce(&str, &[String])) -> Result<()> {
    if let Some(dir) = &options.repo
        && !Repo::at(dir).exists()
    {
        anyhow::bail!(mf(
            "main.repo_of.not_a_repo",
            &[("dir", &dir.display().to_string())]
        ));
    }
    let http = tiny_http::Server::http(("127.0.0.1", options.port))
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| {
            mf(
                "serve.run.listen_failed",
                &[("port", &options.port.to_string())],
            )
        })?;
    let port = http
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .context(m("serve.run.port_unknown"))?;
    let server = Server::new(options, port);
    on_ready(&server.url(), &server.notices());
    for mut request in http.incoming_requests() {
        let mut body = Vec::new();
        {
            use std::io::Read;
            // One byte more than allowed: enough to tell it is too big.
            let path = request.url().split('?').next().unwrap_or("");
            let limit = if path == "/api/images" || path == "/api/attachments" {
                crate::image::CEILING
            } else {
                MAX_BODY
            };
            let _ = request
                .as_reader()
                .take(limit as u64 + 1)
                .read_to_end(&mut body);
        }
        let headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .map(|h| {
                (
                    h.field.as_str().as_str().to_ascii_lowercase(),
                    h.value.as_str().to_string(),
                )
            })
            .collect();
        let target = request.url().to_string();
        let method = request.method().as_str().to_string();
        let reply = server.handle(&Request {
            method: &method,
            target: &target,
            headers,
            body: &body,
        });
        let shutdown = reply.shutdown;
        let mut response = tiny_http::Response::from_data(reply.body)
            .with_status_code(reply.status)
            .with_header(header("Content-Type", reply.content_type));
        for (name, value) in &reply.headers {
            response = response.with_header(header(name, value));
        }
        // Not to be kept: the page changes with every request (an image doesn't).
        if !reply.headers.iter().any(|(n, _)| n == "Cache-Control") {
            response = response.with_header(header("Cache-Control", "no-store"));
        }
        let _ = request.respond(response);
        if shutdown {
            println!("{}", server.farewell());
            break;
        }
    }
    Ok(())
}

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("a header with plain ASCII name and value")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::Additions;
    use crate::digest::digest;
    use crate::files::diff_trees;
    use crate::model::{Anchor, LineRange, Revision, SnapshotMode, Source};

    const BASE: &str = "a\nb\nc\nd\n";
    const HEAD: &str = "a\nB\nc\nd\n";

    struct Fixture {
        _dir: tempfile::TempDir,
        path: PathBuf,
        server: Server,
        thread: Ulid,
    }

    /// A review of one revision with one thread (on line 2), and a server
    /// for it that writes as `tester`.
    fn fixture() -> Fixture {
        fixture_with(Source::Files { base: None }, None)
    }

    /// The fixture for a review with this `source`, its server next to `repo`.
    fn fixture_with(source: Source, repo: Option<PathBuf>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.diffnote");
        let tree = |t: &str| -> crate::files::Tree {
            [("f.txt".to_string(), t.as_bytes().to_vec())].into()
        };
        let (diff_text, files) = diff_trees(&tree(BASE), &tree(HEAD));
        let key = digest("revision");
        let thread = Ulid::new();
        let range = |text: &str| LineRange {
            file: "f.txt".to_string(),
            digest: digest(text),
            start: 2,
            len: 1,
        };
        let events = vec![
            Event::Meta {
                version: 1,
                created_at: OffsetDateTime::UNIX_EPOCH,
                description: None,
                context_lines: 3,
            },
            Event::Revision(Revision {
                id: Ulid::new(),
                created_at: OffsetDateTime::UNIX_EPOCH,
                digest: key.clone(),
                source,
                snapshot_mode: SnapshotMode::Full,
                files,
                tree: Vec::new(),
                commits: Vec::new(),
            }),
            Event::Comment {
                id: thread,
                parent: None,
                author: "r@example.com".into(),
                created_at: OffsetDateTime::UNIX_EPOCH,
                anchor: Some(Anchor::Span {
                    base: Some(range(BASE)),
                    head: Some(range(HEAD)),
                }),
                body: "why B?".into(),
            },
        ];
        let additions = Additions {
            diff: Some((key, diff_text)),
            blobs: vec![HEAD.as_bytes().to_vec(), BASE.as_bytes().to_vec()],
            commits: Vec::new(),
        };
        let loaded = bundle::load(&path).unwrap();
        bundle::save(&path, &loaded, &events, &additions).unwrap();
        let server = Server::new(
            &Options {
                review: path.clone(),
                port: 0,
                author: Some("tester".into()),
                repo,
                refresh: None,
                before: None,
            },
            4242,
        );
        Fixture {
            _dir: dir,
            path,
            server,
            thread,
        }
    }

    impl Fixture {
        fn cookie(&self) -> String {
            format!("{}={}", self.server.cookie_name(), self.server.token())
        }

        /// A request as the page makes it: our host, the cookie, the header.
        fn post(&self, path: &str, body: &str) -> Reply {
            self.request("POST", path, &[], body)
        }

        fn request(&self, method: &str, path: &str, extra: &[(&str, &str)], body: &str) -> Reply {
            let mut headers = vec![
                ("host".to_string(), "127.0.0.1:4242".to_string()),
                ("cookie".to_string(), self.cookie()),
                ("x-diffnote".to_string(), "1".to_string()),
            ];
            for (name, value) in extra {
                headers.retain(|(n, _)| n != name);
                headers.push((name.to_string(), value.to_string()));
            }
            self.server.handle(&Request {
                method,
                target: path,
                headers,
                body: body.as_bytes(),
            })
        }

        /// A request with bytes for a body, as an image is sent.
        fn send_bytes(&self, path: &str, bytes: &[u8]) -> Reply {
            self.server.handle(&Request {
                method: "POST",
                target: path,
                headers: vec![
                    ("host".to_string(), "127.0.0.1:4242".to_string()),
                    ("cookie".to_string(), self.cookie()),
                    ("x-diffnote".to_string(), "1".to_string()),
                ],
                body: bytes,
            })
        }

        fn events(&self) -> Vec<Event> {
            bundle::load(&self.path).unwrap().events
        }

        fn comments(&self) -> Vec<(Option<Ulid>, String, String)> {
            self.events()
                .into_iter()
                .filter_map(|e| match e {
                    Event::Comment {
                        parent,
                        author,
                        body,
                        ..
                    } => Some((parent, author, body)),
                    _ => None,
                })
                .collect()
        }
    }

    fn text(reply: &Reply) -> String {
        String::from_utf8_lossy(&reply.body).to_string()
    }

    fn json(reply: &Reply) -> serde_json::Value {
        serde_json::from_slice(&reply.body).unwrap_or_else(|_| panic!("JSON: {}", text(reply)))
    }

    // ---- who may talk to it ----------------------------------------------

    #[test]
    fn the_first_visit_turns_the_token_into_a_cookie_and_drops_it_from_the_address() {
        let f = fixture();
        let url = format!("/?t={}", f.server.token());
        let reply = f.server.handle(&Request {
            method: "GET",
            target: &url,
            headers: vec![("host".into(), "127.0.0.1:4242".into())],
            body: b"",
        });
        assert_eq!(reply.status, 302);
        let get = |name: &str| {
            reply
                .headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("Location"), "/");
        let cookie = get("Set-Cookie");
        assert!(cookie.starts_with(&f.cookie()), "{cookie}");
        assert!(
            cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"),
            "{cookie}"
        );
    }

    #[test]
    fn nothing_is_served_without_the_token() {
        let f = fixture();
        let bare = |method: &str, target: &str, cookie: Option<&str>| {
            let mut headers = vec![
                ("host".to_string(), "127.0.0.1:4242".to_string()),
                ("x-diffnote".to_string(), "1".to_string()),
            ];
            if let Some(c) = cookie {
                headers.push(("cookie".to_string(), c.to_string()));
            }
            f.server.handle(&Request {
                method,
                target,
                headers,
                body: b"{}",
            })
        };
        assert_eq!(bare("GET", "/", None).status, 403);
        assert_eq!(bare("GET", "/?t=wrong", None).status, 403);
        assert_eq!(
            bare("GET", "/", Some("diffnote_token_4242=wrong")).status,
            403
        );
        let resolve = format!("/api/threads/{}/resolve", f.thread);
        assert_eq!(bare("POST", &resolve, None).status, 403);
        assert_eq!(f.events().len(), 3, "nothing was written");
    }

    #[test]
    fn a_request_addressed_to_another_host_is_refused() {
        // DNS rebinding: a name the attacker controls, pointing here.
        let f = fixture();
        for host in ["evil.example:4242", "127.0.0.1:9999", "127.0.0.1", ""] {
            let reply = f.request("GET", "/", &[("host", host)], "");
            assert_eq!(reply.status, 403, "{host:?}");
        }
        let reply = f.request("GET", "/", &[("host", "localhost:4242")], "");
        assert_eq!(reply.status, 200, "localhost is fine");
    }

    #[test]
    fn a_change_needs_the_custom_header_and_our_own_origin() {
        let f = fixture();
        let resolve = format!("/api/threads/{}/resolve", f.thread);
        // The header a foreign page can't add without a preflight.
        let mut headers = vec![
            ("host".to_string(), "127.0.0.1:4242".to_string()),
            ("cookie".to_string(), f.cookie()),
        ];
        let no_header = f.server.handle(&Request {
            method: "POST",
            target: &resolve,
            headers: headers.clone(),
            body: b"{}",
        });
        assert_eq!(no_header.status, 403);
        headers.push(("x-diffnote".into(), "1".into()));
        headers.push(("origin".into(), "https://evil.example".into()));
        let foreign = f.server.handle(&Request {
            method: "POST",
            target: &resolve,
            headers,
            body: b"{}",
        });
        assert_eq!(foreign.status, 403);
        assert_eq!(f.events().len(), 3, "nothing was written");
        let ours = f.request(
            "POST",
            &resolve,
            &[("origin", "http://127.0.0.1:4242")],
            "{}",
        );
        assert_eq!(ours.status, 200);
    }

    // ---- the page --------------------------------------------------------

    #[test]
    fn the_page_is_the_client_app_with_its_data_and_a_way_to_talk_to_the_server() {
        let f = fixture();
        let reply = f.request("GET", "/", &[], "");
        assert_eq!(reply.status, 200);
        let page = text(&reply);
        assert!(page.contains(r#"id="diffnote-data""#));
        assert!(page.contains("why B?"), "the data has the comments");
        // The served page's bundle is the one that talks to the server.
        assert!(page.contains("fetch("));
        assert!(page.contains(r#""interactive":true"#));
    }

    #[test]
    fn the_review_can_be_downloaded_as_the_export_page() {
        let f = fixture();
        let reply = f.request("GET", "/export", &[], "");
        assert_eq!(reply.status, 200);
        let disposition = reply
            .headers
            .iter()
            .find(|(n, _)| n == "Content-Disposition")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert!(
            disposition.starts_with("attachment; filename=\""),
            "{disposition}"
        );
        assert!(disposition.contains(".html"), "{disposition}");
        let page = text(&reply);
        // What the export writes: the data, and nothing that talks to a server.
        assert!(page.contains(r#"id="diffnote-data""#) && page.contains("why B?"));
        assert!(!page.contains("D.api = ") && page.contains(r#""interactive":false"#));
        // Not without the token, like the rest.
        let bare = f.server.handle(&Request {
            method: "GET",
            target: "/export",
            headers: vec![("host".into(), "127.0.0.1:4242".into())],
            body: b"",
        });
        assert_eq!(bare.status, 403);
    }

    #[test]
    fn the_bundle_can_be_downloaded_exactly_as_it_is_on_disk() {
        let f = fixture();
        let on_disk = std::fs::read(&f.path).unwrap();
        let reply = f.request("GET", "/download", &[], "");
        assert_eq!(reply.status, 200);
        assert_eq!(reply.content_type, "application/zip");
        assert_eq!(reply.body, on_disk);
        let disposition = reply
            .headers
            .iter()
            .find(|(n, _)| n == "Content-Disposition")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert!(
            disposition.starts_with("attachment; filename=\"r.diffnote\""),
            "{disposition}"
        );
        // A change made through the session is in the download too.
        f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body":"かきくけこ"}"#,
        );
        let reply = f.request("GET", "/download", &[], "");
        assert_ne!(reply.body, on_disk, "reflects what changed since");
        assert_eq!(reply.body, std::fs::read(&f.path).unwrap());
        assert!(
            f.events()
                .iter()
                .any(|e| matches!(e, Event::Comment { body, .. } if body == "かきくけこ"))
        );
        // Not without the token, like the rest.
        let bare = f.server.handle(&Request {
            method: "GET",
            target: "/download",
            headers: vec![("host".into(), "127.0.0.1:4242".into())],
            body: b"",
        });
        assert_eq!(bare.status, 403);
    }

    // ---- editing and deleting what this session added ---------------------------

    /// The id of the comment a reply or a new thread made.
    fn id_of(answer: &serde_json::Value) -> String {
        answer["thread"].as_str().unwrap().to_string()
    }

    fn last_comment_id(f: &Fixture) -> String {
        f.events()
            .into_iter()
            .rev()
            .find_map(|e| match e {
                Event::Comment { id, .. } => Some(id.to_string()),
                _ => None,
            })
            .unwrap()
    }

    fn model_comments(f: &Fixture) -> Vec<(String, String)> {
        let model = json(&f.request("GET", "/api/model", &[], ""));
        model["model"]["threads"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|t| t["comments"].as_array().unwrap().clone())
            .map(|c| {
                (
                    c["id"].as_str().unwrap().to_string(),
                    c["body"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn any_comment_can_be_edited_including_one_from_before_the_server_started() {
        let f = fixture();
        // The fixture's own comment is from before the server started.
        let old = last_comment_id(&f);
        let model = json(&f.request("GET", "/api/model", &[], ""));
        assert_eq!(model["model"]["editable"], serde_json::json!([old.clone()]));
        let edited = json(&f.post(
            &format!("/api/comments/{old}/edit"),
            r#"{"body":"reworded"}"#,
        ));
        assert_eq!(edited["ok"], true, "{edited}");
        assert!(model_comments(&f).contains(&(old.clone(), "reworded".to_string())));
        // A reply of this session can.
        let reply = json(&f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body":"mine"}"#,
        ));
        let mine = last_comment_id(&f);
        assert_eq!(reply["editable"], serde_json::json!([old, mine.clone()]));
        assert_eq!(
            reply["thread_data"]["comments"][1]["body"], "mine",
            "the text as written comes with it, to edit"
        );
        let edited = json(&f.post(
            &format!("/api/comments/{mine}/edit"),
            r#"{"body":"  changed  "}"#,
        ));
        assert_eq!(edited["ok"], true, "{edited}");
        assert!(model_comments(&f).contains(&(mine.clone(), "changed".to_string())));
        assert_eq!(f.events().len(), 4, "rewritten in place, nothing added");
        assert_eq!(
            f.post(&format!("/api/comments/{mine}/edit"), r#"{"body":" "}"#)
                .status,
            400
        );
        assert_eq!(
            f.post("/api/comments/not-an-id/edit", r#"{"body":"x"}"#)
                .status,
            400
        );
    }

    #[test]
    fn a_reply_of_this_session_can_be_deleted_and_the_thread_stays() {
        let f = fixture();
        f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body":"oops"}"#,
        );
        let mine = last_comment_id(&f);
        let before = f.events().len();
        let answer = json(&f.post(&format!("/api/comments/{mine}/delete"), "{}"));
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(f.events().len(), before - 1);
        assert!(!model_comments(&f).iter().any(|(id, _)| *id == mine));
        assert!(!answer["model"]["editable"].to_string().contains(&mine));
        // It is gone: nothing more to change.
        assert_eq!(
            f.post(&format!("/api/comments/{mine}/delete"), "{}").status,
            404
        );
    }

    #[test]
    fn a_thread_with_nothing_else_is_deleted_whole_with_its_resolving() {
        let f = fixture();
        let base = f.events().len();
        let id = id_of(&json(&new_thread(
            &f,
            r#"{"scope":"global","revision":0,"body":"draft thought"}"#,
        )));
        f.post(&format!("/api/threads/{id}/resolve"), "{}");
        assert_eq!(f.events().len(), base + 2);
        let answer = json(&f.post(&format!("/api/comments/{id}/delete"), "{}"));
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(f.events().len(), base, "back to what it was");
        // The review still loads and the fixture's own thread is untouched.
        assert_eq!(model_comments(&f).len(), 1);
    }

    #[test]
    fn the_first_comment_of_a_thread_with_replies_is_deleted_alone_and_the_replies_stay() {
        let f = fixture();
        let base = f.events().len();
        let id = id_of(&json(&new_thread(
            &f,
            r#"{"scope":"global","revision":0,"body":"first"}"#,
        )));
        f.post(&format!("/api/threads/{id}/replies"), r#"{"body":"one"}"#);
        let one = last_comment_id(&f);
        // Somebody else replies from the command line meanwhile.
        let loaded = bundle::load(&f.path).unwrap();
        let mut events = loaded.events.clone();
        events.push(Event::Comment {
            id: Ulid::new(),
            parent: Some(Ulid::from_string(&id).unwrap()),
            author: "someone else".into(),
            created_at: OffsetDateTime::now_utc(),
            anchor: None,
            body: "from the command line".into(),
        });
        bundle::save(&f.path, &loaded, &events, &Additions::default()).unwrap();
        f.post(&format!("/api/threads/{id}/resolve"), "{}");
        let before = f.events().len();
        let answer = json(&f.post(&format!("/api/comments/{id}/delete"), "{}"));
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(f.events().len(), before, "nothing is taken out of the log");
        // The first comment is there with no text, marked as deleted; the rest stands.
        let thread = answer["model"]["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id.as_str())
            .unwrap()
            .clone();
        let comments = thread["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 3);
        assert_eq!(comments[0]["deleted"], true);
        assert!(comments[0].get("body").is_none());
        assert_eq!(comments[1]["body"], "one");
        assert_eq!(comments[2]["body"], "from the command line");
        assert_eq!(thread["resolved"], true, "resolving stays");
        // It can't be edited back.
        assert_eq!(
            f.post(&format!("/api/comments/{id}/edit"), r#"{"body":"x"}"#)
                .status,
            409
        );
        // Taking out the replies one by one: the thread goes with the last.
        f.post(&format!("/api/comments/{one}/delete"), "{}");
        assert!(f.events().len() < before);
        let elsewhere = last_comment_id(&f);
        assert_ne!(elsewhere, id);
        let answer = json(&f.post(&format!("/api/comments/{elsewhere}/delete"), "{}"));
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(
            f.events().len(),
            base,
            "the whole thread is gone, resolving too"
        );
        assert_eq!(
            model_comments(&f).len(),
            1,
            "the fixture's own thread is untouched"
        );
    }

    #[test]
    fn a_new_server_can_still_change_what_the_last_one_added() {
        let f = fixture();
        f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body":"mine"}"#,
        );
        let mine = last_comment_id(&f);
        let later = Server::new(
            &Options {
                review: f.path.clone(),
                port: 0,
                author: None,
                repo: None,
                refresh: None,
                before: None,
            },
            4242,
        );
        let deleted = later.handle(&Request {
            method: "POST",
            target: &format!("/api/comments/{mine}/delete"),
            headers: vec![
                ("host".into(), "127.0.0.1:4242".into()),
                (
                    "cookie".into(),
                    format!("{}={}", later.cookie_name(), later.token()),
                ),
                ("x-diffnote".into(), "1".into()),
                ("origin".into(), "http://127.0.0.1:4242".into()),
            ],
            body: b"{}",
        });
        assert_eq!(deleted.status, 200);
    }

    #[test]
    fn what_a_session_did_is_told_roughly_when_it_stops() {
        let f = fixture();
        // Nothing yet.
        let quiet = json(&f.post("/api/shutdown", "{}"))["farewell"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(quiet.contains("変更はありませんでした"), "{quiet}");
        assert!(quiet.contains("保存しました"), "{quiet}");

        let f = fixture();
        new_thread(&f, r#"{"scope":"global","revision":0,"body":"overall"}"#);
        f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body":"one"}"#,
        );
        f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body":"two"}"#,
        );
        f.post(&format!("/api/threads/{}/resolve", f.thread), "{}");
        let mine = last_comment_id_of_reply(&f);
        f.post(&format!("/api/comments/{mine}/edit"), r#"{"body":"two!"}"#);
        f.post(&format!("/api/comments/{mine}/delete"), "{}");
        let told = json(&f.post("/api/shutdown", "{}"))["farewell"]
            .as_str()
            .unwrap()
            .to_string();
        // One thread, one reply left (the other was taken out again), resolved,
        // edited and deleted once each; and where it is kept, with the totals.
        assert!(
            told.contains("スレッド 1 件・返信 1 件を追加、解決 1 件、編集 1 件、削除 1 件"),
            "{told}"
        );
        assert!(told.contains("スレッド 2 件・コメント 3 件"), "{told}");
    }

    fn last_comment_id_of_reply(f: &Fixture) -> String {
        f.events()
            .into_iter()
            .rev()
            .find_map(|e| match e {
                Event::Comment {
                    id,
                    parent: Some(_),
                    ..
                } => Some(id.to_string()),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn how_the_page_is_shown_is_kept_in_the_user_settings_a_choice_at_a_time() {
        crate::user_config::with_test_config_dir(|_| {
            let f = fixture();
            let view = || {
                json(&f.request("GET", "/api/model", &[], ""))["model"]["user_settings"]["view"]
                    .clone()
            };
            assert_eq!(view(), serde_json::Value::Null, "nothing chosen yet");
            assert_eq!(
                json(&f.post("/api/view", r#"{"layout":"split"}"#))["ok"],
                true
            );
            assert_eq!(json(&f.post("/api/view", r#"{"wrap":false}"#))["ok"], true);
            assert_eq!(
                view(),
                serde_json::json!({ "layout": "split", "wrap": false }),
                "each choice is added to the others"
            );
            json(&f.post("/api/view", r#"{"layout":"unified","sync_scroll":true}"#));
            assert_eq!(f.post("/api/view", r#"{"sidebar_width":5}"#).status, 400);
            assert_eq!(
                json(&f.post("/api/view", r#"{"sidebar_width":360}"#))["ok"],
                true
            );
            assert_eq!(view()["sidebar_width"], 360);
            assert_eq!(
                view(),
                serde_json::json!({ "layout": "unified", "wrap": false, "sync_scroll": true, "sidebar_width": 360 })
            );
            // What the page can't have is refused, and nothing is changed.
            assert_eq!(f.post("/api/view", r#"{"layout":"sideways"}"#).status, 400);
            assert_eq!(f.post("/api/view", r#"{"wrap":"no"}"#).status, 400);
            assert_eq!(view()["layout"], "unified");
            // The author, kept in the same file, is left alone, and the other way round.
            json(&f.post("/api/user-settings", r#"{"author":"山田"}"#));
            assert_eq!(view()["wrap"], false);
            let config = crate::user_config::load();
            assert_eq!(config.author.as_deref(), Some("山田"));
            assert_eq!(config.view.wrap, Some(false));
        });
    }

    #[test]
    fn the_user_settings_screen_sets_the_author_persistently_and_for_the_rest_of_the_session() {
        crate::user_config::with_test_config_dir(|_| {
            let f = fixture();
            let model = json(&f.request("GET", "/api/model", &[], ""));
            assert_eq!(model["model"]["author"], "tester");
            assert_eq!(
                model["model"]["user_settings"]["author"],
                serde_json::Value::Null
            );
            let set = json(&f.post("/api/user-settings", r#"{"author":"  山田   太郎 "}"#));
            assert_eq!(set["ok"], true, "{set}");
            assert_eq!(set["model"]["author"], "山田 太郎");
            assert_eq!(set["model"]["user_settings"]["author"], "山田 太郎");
            // Kept on this machine, not only in memory: another server sees it too.
            assert_eq!(
                crate::user_config::load().author.as_deref(),
                Some("山田 太郎")
            );
            let reply = json(&f.post(
                &format!("/api/threads/{}/replies", f.thread),
                r#"{"body":"hi"}"#,
            ));
            assert_eq!(reply["thread_data"]["comments"][1]["author"], "山田 太郎");
            let model = json(&f.request("GET", "/api/model", &[], ""));
            assert_eq!(model["model"]["author"], "山田 太郎");
            // Too long: refused, and nothing changes.
            let long = format!(r#"{{"author":"{}"}}"#, "あ".repeat(101));
            assert_eq!(f.post("/api/user-settings", &long).status, 400);
            assert_eq!(
                f.post("/api/user-settings", r#"{}"#).status,
                400,
                "no author key at all"
            );
            let model = json(&f.request("GET", "/api/model", &[], ""));
            assert_eq!(model["model"]["author"], "山田 太郎");
            // Cleared: the session falls back to --author ("tester" here), and
            // nothing is configured for the next bundle either.
            let unset = json(&f.post("/api/user-settings", r#"{"author":"   "}"#));
            assert_eq!(unset["ok"], true, "{unset}");
            assert_eq!(unset["model"]["author"], "tester");
            assert_eq!(
                unset["model"]["user_settings"]["author"],
                serde_json::Value::Null
            );
            assert_eq!(crate::user_config::load().author, None);
        });
    }

    #[test]
    fn the_settings_are_changed_together_or_one_by_one_and_kept_in_the_review() {
        let f = fixture();
        let before = f.events().len();
        let settings = |f: &Fixture| bundle::load(&f.path).unwrap().settings;
        // The page is given them, and what the bundle holds, to show.
        let model = json(&f.request("GET", "/api/model", &[], ""))["model"].clone();
        assert_eq!(model["settings"]["attachment_limit"], 5 * 1024 * 1024);
        assert!(model["bundle"]["size"].as_u64().unwrap() > 0);
        assert_eq!(model["bundle"]["revisions"], 1);
        assert_eq!(model["bundle"]["images"]["count"], 0);
        // Some at a time: what is not given stays.
        let set = json(&f.post(
            "/api/settings",
            r#"{"title":" ログイン  改修 ","ignore_whitespace":true}"#,
        ));
        assert_eq!(set["ok"], true, "{set}");
        assert_eq!(set["model"]["title"], "ログイン 改修");
        assert_eq!(set["model"]["ignore_whitespace"], true);
        assert_eq!(set["model"]["settings"]["title"], "ログイン 改修");
        assert_eq!(settings(&f).title.as_deref(), Some("ログイン 改修"));
        assert_eq!(settings(&f).attachment_limit, 5 * 1024 * 1024);
        f.post("/api/settings", r#"{"attachment_limit":2000000}"#);
        let now = settings(&f);
        assert_eq!(now.attachment_limit, 2_000_000);
        assert_eq!(
            now.title.as_deref(),
            Some("ログイン 改修"),
            "left as it was"
        );
        assert!(now.ignore_whitespace);
        assert_eq!(f.events().len(), before, "state, not events");
        // An empty title takes it away; the same again changes (and counts) nothing.
        let cleared = json(&f.post("/api/settings", r#"{"title":""}"#));
        assert!(cleared["model"]["title"].is_null());
        assert_eq!(settings(&f).title, None);
        let stamp = json(&f.request("GET", "/api/version", &[], ""))["stamp"].clone();
        f.post(
            "/api/settings",
            r#"{"title":"","attachment_limit":2000000}"#,
        );
        assert_eq!(
            json(&f.request("GET", "/api/version", &[], ""))["stamp"],
            stamp
        );
        // Refused, with a reason, and nothing is changed.
        for bad in [
            r#"{"title":5}"#.to_string(),
            format!(r#"{{"title":"{}"}}"#, "あ".repeat(201)),
            r#"{"ignore_whitespace":"yes"}"#.to_string(),
            r#"{"attachment_limit":10}"#.to_string(),
            r#"{"attachment_limit":999999999999}"#.to_string(),
            r#"{"attachment_limit":"5"}"#.to_string(),
            "not json".to_string(),
        ] {
            let refused = f.post("/api/settings", &bad);
            assert_eq!(refused.status, 400, "{bad}");
            assert_eq!(settings(&f).attachment_limit, 2_000_000, "{bad}");
        }
        // It counts in what is said when the server stops.
        let told = json(&f.post("/api/shutdown", "{}"))["farewell"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            told.contains("タイトル変更 2 件") && told.contains("設定変更 2 件"),
            "{told}"
        );
    }

    #[test]
    fn the_pull_asks_the_refresher_and_answers_with_what_it_did_and_the_model() {
        let f = fixture();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let server = Server::new(
            &Options {
                review: f.path.clone(),
                port: 0,
                author: None,
                repo: None,
                refresh: Some(std::sync::Arc::new(move |_| {
                    match counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                        0 | 2 => Ok(Some("差分を記録しました".to_string())),
                        1 | 3 => Ok(None),
                        _ => anyhow::bail!("壊れました"),
                    }
                })),
                before: None,
            },
            4242,
        );
        let post = || {
            server.handle(&Request {
                method: "POST",
                target: "/api/refresh",
                headers: vec![
                    ("host".into(), "127.0.0.1:4242".into()),
                    (
                        "cookie".into(),
                        format!("{}={}", server.cookie_name(), server.token()),
                    ),
                    ("x-diffnote".into(), "1".into()),
                ],
                body: b"{}",
            })
        };
        // Looking (the version check) doesn't take anything in: the first call
        // says there is something, the second that there is not.
        let version = || {
            json(&server.handle(&Request {
                method: "GET",
                target: "/api/version",
                headers: vec![
                    ("host".into(), "127.0.0.1:4242".into()),
                    (
                        "cookie".into(),
                        format!("{}={}", server.cookie_name(), server.token()),
                    ),
                ],
                body: b"",
            }))
        };
        assert_eq!(version()["pending"], true);
        assert_eq!(version()["pending"], false);
        let done = json(&post());
        assert_eq!(done["message"], "差分を記録しました");
        assert_eq!(done["added"], true);
        assert_eq!(done["model"]["refreshable"], true);
        let none = json(&post());
        assert_eq!(none["message"], "新しい変更はありません");
        assert_eq!(none["added"], false);
        let broken = post();
        assert_eq!(broken.status, 500);
        assert!(
            json(&broken)["error"]
                .as_str()
                .unwrap()
                .contains("壊れました")
        );
        // A server that was not given one has no button, and refuses.
        assert_eq!(f.post("/api/refresh", "{}").status, 400);
        let model = json(&f.request("GET", "/api/model", &[], ""));
        assert!(model["model"].get("refreshable").is_none());
    }

    #[test]
    fn shutting_down_with_discard_puts_the_review_back_as_it_was_at_the_start() {
        let f = fixture();
        let before = std::fs::read(&f.path).unwrap();
        // The server that is started now is the one that keeps what it found.
        let server = Server::new(
            &Options {
                review: f.path.clone(),
                port: 0,
                author: Some("tester".into()),
                repo: None,
                refresh: None,
                before: None,
            },
            4242,
        );
        let post = |target: &str, body: &[u8]| {
            server.handle(&Request {
                method: "POST",
                target,
                headers: vec![
                    ("host".into(), "127.0.0.1:4242".into()),
                    (
                        "cookie".into(),
                        format!("{}={}", server.cookie_name(), server.token()),
                    ),
                    ("x-diffnote".into(), "1".into()),
                ],
                body,
            })
        };
        let reply = r#"{"body":"捨てる"}"#;
        assert_eq!(
            post(
                &format!("/api/threads/{}/replies", f.thread),
                reply.as_bytes()
            )
            .status,
            200
        );
        assert_ne!(std::fs::read(&f.path).unwrap(), before, "it was written");
        let told = json(&post("/api/shutdown", br#"{"discard":true}"#));
        assert_eq!(told["summary"]["discarded"], true);
        assert_eq!(std::fs::read(&f.path).unwrap(), before, "and is back");
        assert!(server.farewell().contains("保存せずに終了"));
    }

    #[test]
    fn the_old_side_of_lines_of_the_new_side_is_read_from_the_diff() {
        let diff =
            crate::diff::parse("--- a/f\n+++ b/f\n@@ -2,3 +2,4 @@\n a2\n-b3\n+B3\n+B3b\n a4\n")
                .unwrap();
        let file = diff.files.first();
        let base = |start, len| base_span_for(file, LineSpan { start, len });
        let span = |start, len| LineSpan { start, len };
        // Before the hunk, on an unchanged line, on added lines (a point), after it.
        assert_eq!(base(1, 1), span(1, 1));
        assert_eq!(base(2, 1), span(2, 1));
        assert_eq!(base(3, 2), span(4, 0));
        assert_eq!(base(5, 1), span(4, 1));
        assert_eq!(base(2, 4), span(2, 3), "from a2 to a4: old 2 to 4");
        assert_eq!(base(9, 1), span(8, 1));
        // A file the diff doesn't have is the same on both sides.
        assert_eq!(base_span_for(None, span(7, 2)), span(7, 2));
    }

    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R',
    ];

    #[test]
    fn a_safe_svg_is_taken_and_sent_as_an_svg_that_can_do_nothing() {
        let f = fixture();
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8"/></svg>"#;
        let told = json(&f.send_bytes("/api/images", svg));
        assert_eq!(told["type"], "image/svg+xml");
        let got = f.request(
            "GET",
            &format!("/api/images/{}", told["id"].as_str().unwrap()),
            &[],
            "",
        );
        assert_eq!(got.status, 200);
        assert_eq!(got.content_type, "image/svg+xml");
        assert_eq!(got.body, svg);
        assert!(
            got.headers
                .iter()
                .any(|(n, v)| n == "Content-Security-Policy" && v.starts_with("sandbox")),
            "even a safe one runs nothing if opened by itself"
        );
    }

    #[test]
    fn an_image_is_put_in_the_review_shown_to_the_page_and_let_go_of_if_no_comment_has_it() {
        let f = fixture();
        // Refused: not an image, an SVG that runs.
        assert_eq!(f.send_bytes("/api/images", b"just text").status, 400);
        assert_eq!(f.send_bytes("/api/images", b"").status, 400);
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" onload="x()"></svg>"#;
        assert_eq!(f.send_bytes("/api/images", svg).status, 400);
        // Taken; the same again is not a second.
        let told = json(&f.send_bytes("/api/images", PNG));
        assert_eq!(told["ok"], true);
        assert_eq!(told["type"], "image/png");
        assert_eq!(told["size"], PNG.len());
        assert!(told["bundle_size"].as_u64().unwrap() > 0);
        let id = told["id"].as_str().unwrap().to_string();
        assert_eq!(json(&f.send_bytes("/api/images", PNG))["id"], id.as_str());
        assert_eq!(bundle::load(&f.path).unwrap().image_ids(), vec![id.clone()]);
        // Sent to an <img>: with the type, and a policy that lets it do nothing.
        let got = f.request("GET", &format!("/api/images/{id}"), &[], "");
        assert_eq!(got.status, 200);
        assert_eq!(got.content_type, "image/png");
        assert_eq!(got.body, PNG);
        let header = |name: &str| {
            got.headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        assert!(header("Content-Security-Policy").is_some_and(|v| v.starts_with("sandbox")));
        assert_eq!(header("X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(f.request("GET", "/api/images/nope", &[], "").status, 404);
        assert_eq!(
            f.request("GET", &format!("/api/images/{}", "0".repeat(64)), &[], "")
                .status,
            404
        );
        // A comment that shows it makes it a node of the model.
        let reply = format!(r#"{{"body":"see ![shot](diffnote-image:{id})"}}"#);
        f.post(&format!("/api/threads/{}/replies", f.thread), &reply);
        let model = json(&f.request("GET", "/api/model", &[], ""));
        assert!(model.to_string().contains(r#""t":"image""#), "{model}");
        // Another that nothing shows goes when the session ends.
        let other = [PNG, &[1, 2, 3]].concat();
        f.send_bytes("/api/images", &other);
        assert_eq!(bundle::load(&f.path).unwrap().image_ids().len(), 2);
        let told = json(&f.post("/api/shutdown", "{}"))["farewell"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(told.contains("画像 2 件"), "{told}");
        assert_eq!(bundle::load(&f.path).unwrap().image_ids(), vec![id]);
    }

    #[test]
    fn the_model_lists_what_is_attached_biggest_first_with_an_images_type() {
        let f = fixture();
        let big = [PNG, &[7; 40]].concat();
        f.send_bytes("/api/images", PNG);
        f.send_bytes("/api/images", &big);
        f.send_bytes("/api/attachments?name=log.zip", b"PK\x03\x04 log");
        let listed =
            json(&f.request("GET", "/api/model", &[], ""))["model"]["bundle"]["attachments"]
                .clone();
        let of = |i: usize| listed[i].clone();
        assert_eq!(listed.as_array().unwrap().len(), 3);
        assert_eq!(of(0)["kind"], "image");
        assert_eq!(of(0)["size"], big.len());
        assert_eq!(of(0)["media_type"], "image/png");
        assert_eq!(of(1)["kind"], "image");
        assert_eq!(of(1)["size"], PNG.len());
        // The other files' type is never read, so none is said.
        assert_eq!(of(2)["kind"], "file");
        assert_eq!(of(2)["media_type"], serde_json::Value::Null);
        assert!(crate::image::is_id(of(2)["id"].as_str().unwrap()));
    }

    #[test]
    fn an_attachment_keeps_the_name_of_the_file_it_was_attached_from() {
        let f = fixture();
        let sent = |target: &str, bytes: &[u8]| {
            json(&f.send_bytes(target, bytes))["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let names = || {
            json(&f.request("GET", "/api/model", &[], ""))["model"]["bundle"]["attachments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| {
                    (
                        a["id"].as_str().unwrap().to_string(),
                        a["name"].as_str().map(str::to_string),
                    )
                })
                .collect::<std::collections::HashMap<_, _>>()
        };
        let image = sent("/api/images?name=%E5%9B%B3.png", PNG);
        let file = sent("/api/attachments?name=log.zip", b"PK\x03\x04 log");
        assert_eq!(names()[&image].as_deref(), Some("図.png"));
        assert_eq!(names()[&file].as_deref(), Some("log.zip"));

        // A screenshot pasted out of the clipboard comes with no name, or with
        // one Chrome made up that says nothing: neither is worth keeping.
        let pasted = sent("/api/images", &[PNG, &[1]].concat());
        let made_up = sent("/api/images?name=image.png", &[PNG, &[2]].concat());
        assert_eq!(names()[&pasted], None);
        assert_eq!(names()[&made_up], None);

        // The bytes are the attachment, so the first name they come with is
        // the one it goes by -- but one that came without a name takes the
        // first it is given.
        f.send_bytes("/api/images?name=other.png", PNG);
        assert_eq!(names()[&image].as_deref(), Some("図.png"));
        f.send_bytes("/api/images?name=shot.png", &[PNG, &[1]].concat());
        assert_eq!(names()[&pasted].as_deref(), Some("shot.png"));

        // Taken out, and the same bytes attached again with no name: what was
        // known about it went with it.
        assert_eq!(
            json(&f.post(&format!("/api/images/{image}/delete"), "{}"))["ok"],
            true
        );
        assert_eq!(sent("/api/images", PNG), image);
        assert_eq!(names()[&image], None);
    }

    #[test]
    fn an_attachment_can_be_deleted_whether_or_not_a_comment_still_refers_to_it() {
        let f = fixture();
        let image = json(&f.send_bytes("/api/images", PNG))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let file = json(&f.send_bytes("/api/attachments?name=log.zip", b"PK\x03\x04 log"))["id"]
            .as_str()
            .unwrap()
            .to_string();
        // A comment shows the image: it still goes, and the text stays as it was.
        let body = format!(r#"{{"body":"see ![shot](diffnote-image:{image})"}}"#);
        f.post(&format!("/api/threads/{}/replies", f.thread), &body);
        let gone = json(&f.post(&format!("/api/images/{image}/delete"), "{}"));
        assert_eq!(gone["ok"], true);
        assert!(bundle::load(&f.path).unwrap().image_ids().is_empty());
        assert!(
            f.comments()
                .iter()
                .any(|c| c.2.contains(&format!("diffnote-image:{image}"))),
            "the comment is left as it was written"
        );
        assert_eq!(
            gone["model"]["bundle"]["attachments"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "the list in the answer is already without it"
        );
        // Unknown ids are refused, and the other store is untouched by the first.
        assert_eq!(
            f.post(&format!("/api/images/{image}/delete"), "{}").status,
            404
        );
        assert_eq!(f.post("/api/attachments/nope/delete", "{}").status, 404);
        assert_eq!(
            f.post(&format!("/api/attachments/{file}/delete"), "{}")
                .status,
            200
        );
        assert!(bundle::load(&f.path).unwrap().attachment_ids().is_empty());
        // Both were attached in this session, so nothing is said to have been added.
        let told = json(&f.post("/api/shutdown", "{}"))["farewell"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!told.contains("画像"), "{told}");
        assert!(!told.contains("添付の削除"), "{told}");
    }

    #[test]
    fn deleting_an_attachment_from_an_earlier_session_is_told_at_the_end() {
        let f = fixture();
        // Put in behind the server's back: as if an earlier session attached it.
        let loaded = bundle::load(&f.path).unwrap();
        let events = loaded.events.clone();
        bundle::save_with(
            &f.path,
            &loaded,
            &events,
            &bundle::Additions::default(),
            &bundle::Images {
                add: &[PNG.to_vec()],
                ..Default::default()
            },
        )
        .unwrap();
        let id = crate::image::id_of(PNG);
        assert_eq!(
            f.post(&format!("/api/images/{id}/delete"), "{}").status,
            200
        );
        let told = json(&f.post("/api/shutdown", "{}"))["farewell"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(told.contains("添付の削除 1 件"), "{told}");
    }

    /// Says in the review how big an attached file may be.
    fn set_attachment_limit(f: &Fixture, bytes: u64) {
        let mut loaded = bundle::load(&f.path).unwrap();
        loaded.settings.attachment_limit = bytes;
        let events = loaded.events.clone();
        bundle::save(&f.path, &loaded, &events, &bundle::Additions::default()).unwrap();
    }

    #[test]
    fn any_file_can_be_attached_and_is_only_ever_sent_to_be_saved() {
        let f = fixture();
        let data = b"PK\x03\x04 some log or archive \xff\x00";
        let told = json(&f.send_bytes("/api/attachments?name=%E3%83%AD%E3%82%B0.zip", data));
        assert_eq!(told["ok"], true);
        assert_eq!(told["name"], "ログ.zip");
        assert_eq!(told["size"], data.len());
        let id = told["id"].as_str().unwrap().to_string();
        assert_eq!(f.send_bytes("/api/attachments", b"").status, 400);
        // Kept apart from the images.
        assert_eq!(
            bundle::load(&f.path).unwrap().attachment_ids(),
            vec![id.clone()]
        );
        assert!(bundle::load(&f.path).unwrap().image_ids().is_empty());
        // Sent as a download that can do nothing else.
        let got = f.request(
            "GET",
            &format!("/api/attachments/{id}?name=%E3%83%AD%E3%82%B0.zip"),
            &[],
            "",
        );
        assert_eq!(got.status, 200);
        assert_eq!(got.content_type, "application/octet-stream");
        assert_eq!(got.body, data);
        let header = |name: &str| {
            got.headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        };
        let disposition = header("Content-Disposition").unwrap();
        assert!(disposition.starts_with("attachment;"), "{disposition}");
        assert!(
            disposition.contains("filename*=UTF-8''%E3%83%AD%E3%82%B0.zip"),
            "{disposition}"
        );
        assert_eq!(header("X-Content-Type-Options").as_deref(), Some("nosniff"));
        assert!(header("Content-Security-Policy").is_some_and(|v| v.starts_with("sandbox")));
        // A name can't be a path or spoil the header.
        let got = f.request(
            "GET",
            &format!("/api/attachments/{id}?name=..%2F..%2Fa%22b.txt"),
            &[],
            "",
        );
        assert!(header_of(&got, "Content-Disposition").contains("filename=\"ab.txt\""));
        assert_eq!(
            f.request("GET", "/api/attachments/nope", &[], "").status,
            404
        );
        // A comment that names it makes a node; the one that nothing names goes at the end.
        let reply = format!(r#"{{"body":"[ログ](diffnote-file:{id})"}}"#);
        f.post(&format!("/api/threads/{}/replies", f.thread), &reply);
        f.send_bytes("/api/attachments?name=other", b"other data");
        assert_eq!(bundle::load(&f.path).unwrap().attachment_ids().len(), 2);
        let told = json(&f.post("/api/shutdown", "{}"))["farewell"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(told.contains("ファイル 2 件"), "{told}");
        assert_eq!(bundle::load(&f.path).unwrap().attachment_ids(), vec![id]);
    }

    fn header_of(reply: &Reply, name: &str) -> String {
        reply
            .headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    #[test]
    fn what_may_be_attached_is_limited_by_the_review_5_mb_unless_it_says_otherwise() {
        let f = fixture();
        let model = json(&f.request("GET", "/api/model", &[], ""));
        assert_eq!(model["model"]["attachment_limit"], 5 * 1024 * 1024);
        // Under the limit; then the review says 100 bytes.
        assert_eq!(
            f.send_bytes("/api/attachments?name=a", &[7u8; 90]).status,
            200
        );
        set_attachment_limit(&f, 100);
        let model = json(&f.request("GET", "/api/model", &[], ""));
        assert_eq!(model["model"]["attachment_limit"], 100);
        let over = f.send_bytes("/api/attachments?name=b", &[8u8; 101]);
        assert_eq!(over.status, 413);
        let said = json(&over)["error"].as_str().unwrap().to_string();
        assert!(said.contains("100 B") && said.contains("101 B"), "{said}");
        // The same rule for an image.
        let mut png = PNG.to_vec();
        png.extend([0u8; 200]);
        assert_eq!(f.send_bytes("/api/images", &png).status, 413);
        assert_eq!(f.send_bytes("/api/images", PNG).status, 200);
        // Bigger again.
        set_attachment_limit(&f, 1000);
        assert_eq!(f.send_bytes("/api/images", &png).status, 200);
    }

    #[test]
    fn a_page_that_only_shows_the_review_has_the_images_in_it() {
        let f = fixture();
        let id = json(&f.send_bytes("/api/images", PNG))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let reply = format!(r#"{{"body":"![shot](diffnote-image:{id})"}}"#);
        f.post(&format!("/api/threads/{}/replies", f.thread), &reply);
        let loaded = bundle::load(&f.path).unwrap();
        let page = html::render_export_with(&loaded, html::ExpandLimit::Lines(0)).unwrap();
        assert!(
            page.contains("data:image/png;base64,iVBORw0KGgo"),
            "embedded"
        );
        // (The served page asks the server instead.)
        let served =
            html::render_served_page(&loaded, Vec::new(), Vec::new(), "a".into(), false, 0)
                .unwrap();
        assert!(!served.contains("data:image/png"));
    }

    #[test]
    fn a_reaction_is_added_and_taken_back_kept_in_the_review_and_goes_with_its_comment() {
        crate::user_config::with_test_config_dir(|_| {
            let f = fixture();
            let old = last_comment_id(&f);
            let react = |f: &Fixture, id: &str, emoji: &str| {
                json(&f.post(
                    &format!("/api/comments/{id}/react"),
                    &format!(r#"{{"emoji":"{emoji}"}}"#),
                ))
            };
            let reactions = |f: &Fixture| bundle::load(&f.path).unwrap().reactions;
            let before = f.events().len();
            let answer = react(&f, &old, "👍");
            assert_eq!(answer["ok"], true, "{answer}");
            assert_eq!(answer["reacted"], true);
            // The model has them with the comment: who, in the order they came.
            let comment = |a: &serde_json::Value| a["model"]["threads"][0]["comments"][0].clone();
            assert_eq!(comment(&answer)["reactions"][0]["emoji"], "👍");
            assert_eq!(
                comment(&answer)["reactions"][0]["authors"],
                serde_json::json!(["tester"])
            );
            assert_eq!(f.events().len(), before, "state, not an event");
            // Another name adds to it; the same again takes it back.
            f.post("/api/user-settings", r#"{"author":"別の人"}"#);
            let both = react(&f, &old, "👍");
            assert_eq!(
                comment(&both)["reactions"][0]["authors"],
                serde_json::json!(["tester", "別の人"])
            );
            react(&f, &old, "🎉");
            let back = react(&f, &old, "👍");
            assert_eq!(back["reacted"], false);
            assert_eq!(comment(&back)["reactions"].as_array().unwrap().len(), 2);
            assert_eq!(reactions(&f)[&old][0].authors, ["tester"]);
            // The stamp says the review changed.
            let stamp = json(&f.request("GET", "/api/version", &[], ""))["stamp"].clone();
            react(&f, &old, "🚀");
            assert_ne!(
                json(&f.request("GET", "/api/version", &[], ""))["stamp"],
                stamp
            );
            // Refused: not an emoji, no such comment, a deleted one.
            for bad in ["a", "ok", "", " ", "1", "👍 "] {
                assert_eq!(
                    f.post(
                        &format!("/api/comments/{old}/react"),
                        &format!(r#"{{"emoji":"{bad}"}}"#)
                    )
                    .status,
                    400,
                    "{bad:?}"
                );
            }
            assert_eq!(
                f.post(&format!("/api/comments/{old}/react"), "{}").status,
                400
            );
            let nobody = Ulid::new();
            assert_eq!(
                f.post(
                    &format!("/api/comments/{nobody}/react"),
                    r#"{"emoji":"👍"}"#
                )
                .status,
                404
            );
            // A reply of its own; deleting it takes its reactions with it.
            f.post(
                &format!("/api/threads/{}/replies", f.thread),
                r#"{"body":"mine"}"#,
            );
            let mine = last_comment_id(&f);
            react(&f, &mine, "❤️");
            assert!(reactions(&f).contains_key(&mine));
            f.post(&format!("/api/comments/{mine}/delete"), "{}");
            assert!(!reactions(&f).contains_key(&mine), "gone with the comment");
            // The first comment deleted and kept as a mark: nothing more to react to, and no reactions kept.
            f.post(
                &format!("/api/threads/{}/replies", f.thread),
                r#"{"body":"another"}"#,
            );
            let other = last_comment_id(&f);
            f.post(&format!("/api/comments/{old}/delete"), "{}");
            assert!(!reactions(&f).contains_key(&old));
            assert_eq!(
                f.post(&format!("/api/comments/{old}/react"), r#"{"emoji":"👍"}"#)
                    .status,
                409
            );
            assert_ne!(other, old);
            // What was added counts in what is said when the server stops.
            react(&f, &other, "🎉");
            let told = json(&f.post("/api/shutdown", "{}"))["farewell"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(told.contains("リアクション"), "{told}");
        });
    }

    #[test]
    fn a_page_that_only_shows_the_review_has_the_reactions_in_it() {
        let f = fixture();
        let old = last_comment_id(&f);
        f.post(&format!("/api/comments/{old}/react"), r#"{"emoji":"🎉"}"#);
        let loaded = bundle::load(&f.path).unwrap();
        let page = html::render_export_with(&loaded, html::ExpandLimit::Lines(0)).unwrap();
        assert!(
            page.contains(r#""reactions":[{"emoji":"🎉","authors":["tester"]}]"#),
            "in the page"
        );
    }

    #[test]
    fn two_servers_on_one_machine_do_not_take_each_others_cookie() {
        let one = fixture();
        let other = Server::new(
            &Options {
                review: one.path.clone(),
                port: 0,
                author: None,
                repo: None,
                refresh: None,
                before: None,
            },
            4243,
        );
        assert_ne!(one.server.cookie_name(), other.cookie_name());
        let ask = |server: &Server, port: u16, cookie: String| {
            server
                .handle(&Request {
                    method: "GET",
                    target: "/api/version",
                    headers: vec![
                        ("host".into(), format!("127.0.0.1:{port}")),
                        ("cookie".into(), cookie),
                    ],
                    body: b"",
                })
                .status
        };
        let mine = format!("{}={}", one.server.cookie_name(), one.server.token());
        let theirs = format!("{}={}", other.cookie_name(), other.token());
        // Each takes its own; neither takes the other's.
        assert_eq!(ask(&one.server, 4242, mine.clone()), 200);
        assert_eq!(ask(&other, 4243, theirs.clone()), 200);
        assert_eq!(ask(&one.server, 4242, theirs.clone()), 403);
        assert_eq!(ask(&other, 4243, mine.clone()), 403);
        // A browser that has visited both sends both, and both work.
        let both = format!("{mine}; {theirs}");
        assert_eq!(ask(&one.server, 4242, both.clone()), 200);
        assert_eq!(ask(&other, 4243, both), 200);
    }

    #[test]
    fn a_review_that_cannot_be_shown_says_so() {
        let f = fixture();
        std::fs::write(&f.path, b"not a zip").unwrap();
        let reply = f.request("GET", "/", &[], "");
        assert_eq!(reply.status, 500);
        assert!(text(&reply).contains("表示できません"));
    }

    #[test]
    fn unknown_paths_are_not_found() {
        let f = fixture();
        assert_eq!(f.request("GET", "/nope", &[], "").status, 404);
        assert_eq!(f.post("/api/nope", "{}").status, 404);
    }

    // ---- replying and resolving --------------------------------------------

    #[test]
    fn a_reply_is_appended_and_the_answer_carries_the_thread() {
        let f = fixture();
        let reply = f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body": "  because it was wrong  "}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let comments = f.comments();
        assert_eq!(comments.len(), 2);
        assert_eq!(
            comments[1],
            (
                Some(f.thread),
                "tester".to_string(),
                "because it was wrong".to_string()
            )
        );
        let answer = json(&reply);
        assert_eq!(answer["ok"], true);
        assert_ne!(answer["before"], answer["stamp"], "the log changed");
        // The thread as it now is: the reply is in it.
        let bodies: Vec<String> = answer["thread_data"]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["doc"].to_string())
            .collect();
        assert_eq!(bodies.len(), 2);
        assert!(bodies[1].contains("because it was wrong"), "{bodies:?}");
    }

    #[test]
    fn a_bad_reply_is_refused_and_writes_nothing() {
        let f = fixture();
        let path = format!("/api/threads/{}/replies", f.thread);
        for body in ["", "not json", "{}", r#"{"body": "   "}"#, r#"{"body": 5}"#] {
            assert_eq!(f.post(&path, body).status, 400, "{body:?}");
        }
        assert_eq!(
            f.post(
                &format!("/api/threads/{}/replies", Ulid::new()),
                r#"{"body":"x"}"#
            )
            .status,
            404
        );
        assert_eq!(
            f.post("/api/threads/not-an-id/replies", r#"{"body":"x"}"#)
                .status,
            400
        );
        assert_eq!(f.events().len(), 3, "nothing was written");
    }

    #[test]
    fn a_reply_to_a_reply_id_is_not_a_thread() {
        // Threads are flat: only a root has replies.
        let f = fixture();
        f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body":"one"}"#,
        );
        let reply_id = f
            .events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Comment {
                    id,
                    parent: Some(_),
                    ..
                } => Some(id),
                _ => None,
            })
            .next()
            .unwrap();
        let path = format!("/api/threads/{reply_id}/replies");
        assert_eq!(f.post(&path, r#"{"body":"two"}"#).status, 404);
    }

    #[test]
    fn the_client_page_and_its_model_and_version_come_from_the_server() {
        let f = fixture();
        let get = |t: &str| f.request("GET", t, &[], "");
        let page = get("/");
        assert_eq!(page.status, 200);
        assert!(text(&page).contains("fetch("));
        let model = json(&get("/api/model"));
        assert_eq!(model["model"]["interactive"], true);
        let stamp = model["model"]["stamp"].as_str().unwrap().to_string();
        assert_eq!(json(&get("/api/version"))["stamp"], stamp.as_str());
        f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body":"more"}"#,
        );
        assert_ne!(json(&get("/api/version"))["stamp"], stamp.as_str());
    }

    #[test]
    fn a_change_says_the_stamp_before_and_after_and_the_thread_as_it_now_is() {
        let f = fixture();
        let before = json(&f.request("GET", "/api/version", &[], ""))["stamp"].clone();
        let resolve = format!("/api/threads/{}/resolve", f.thread);
        let answer = json(&f.post(&resolve, "{}"));
        assert_eq!(answer["before"], before);
        assert_ne!(answer["stamp"], before);
        assert_eq!(answer["thread_data"]["resolved"], true);
        // Already resolved: nothing changes, and the page can tell.
        let again = json(&f.post(&resolve, "{}"));
        assert_eq!(again["before"], answer["stamp"]);
        assert_eq!(again["stamp"], answer["stamp"]);
    }

    #[test]
    fn saying_the_same_state_again_writes_nothing_more() {
        // Another tab, or the command line, may have got there first.
        let f = fixture();
        let resolve = format!("/api/threads/{}/resolve", f.thread);
        f.post(&resolve, "{}");
        let before = f.events().len();
        assert_eq!(f.post(&resolve, "{}").status, 200);
        assert_eq!(f.events().len(), before);
        let reopen = format!("/api/threads/{}/reopen", f.thread);
        f.post(&reopen, "{}");
        let before = f.events().len();
        assert_eq!(f.post(&reopen, "{}").status, 200);
        assert_eq!(f.events().len(), before);
    }

    #[test]
    fn a_change_made_elsewhere_in_between_is_not_lost() {
        // The server holds nothing: each request starts from the file.
        let f = fixture();
        f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body":"first"}"#,
        );
        let loaded = bundle::load(&f.path).unwrap();
        let mut events = loaded.events.clone();
        events.push(Event::Comment {
            id: Ulid::new(),
            parent: Some(f.thread),
            author: "someone else".into(),
            created_at: OffsetDateTime::now_utc(),
            anchor: None,
            body: "from the command line".into(),
        });
        bundle::save(&f.path, &loaded, &events, &Additions::default()).unwrap();
        let answer = json(&f.post(
            &format!("/api/threads/{}/replies", f.thread),
            r#"{"body":"second"}"#,
        ));
        let bodies: Vec<String> = f.comments().into_iter().map(|c| c.2).collect();
        assert_eq!(
            bodies,
            ["why B?", "first", "from the command line", "second"]
        );
        // ...and the answer shows all of it.
        let shown = answer["thread_data"]["comments"].to_string();
        assert!(
            shown.contains("from the command line") && shown.contains("second"),
            "{shown}"
        );
        // (and says the log was not what the page had: the command line's
        // reply came in between)
        assert_ne!(answer["before"], answer["stamp"]);
    }

    #[test]
    fn a_too_big_body_is_refused() {
        let f = fixture();
        let big = format!(r#"{{"body": "{}"}}"#, "x".repeat(MAX_BODY));
        assert_eq!(
            f.post(&format!("/api/threads/{}/replies", f.thread), &big)
                .status,
            413
        );
        assert_eq!(f.events().len(), 3);
    }

    #[test]
    fn shutdown_stops_the_server() {
        let f = fixture();
        let reply = f.post("/api/shutdown", "{}");
        assert_eq!(reply.status, 200);
        assert!(reply.shutdown);
        assert!(
            !f.post(&format!("/api/threads/{}/resolve", f.thread), "{}")
                .shutdown
        );
    }

    // ---- new threads on lines ------------------------------------------------

    fn new_thread(f: &Fixture, body: &str) -> Reply {
        f.post("/api/threads", body)
    }

    /// The anchor of the last thread in the file.
    fn last_anchor(f: &Fixture) -> Anchor {
        f.events()
            .into_iter()
            .rev()
            .find_map(|e| match e {
                Event::Comment {
                    parent: None,
                    anchor: Some(a),
                    ..
                } => Some(a),
                _ => None,
            })
            .unwrap()
    }

    fn span(a: &Anchor) -> ((u32, u32), (u32, u32)) {
        let Anchor::Span { base, head } = a else {
            panic!("{a:?}");
        };
        let side = |r: &Option<LineRange>| r.as_ref().map_or((0, 0), |r| (r.start, r.len));
        (side(base), side(head))
    }

    #[test]
    fn a_thread_on_lines_is_anchored_to_those_lines_of_that_revision() {
        let f = fixture();
        // `c` (context line 3 on both sides) and `d` (4): a two-line range.
        let reply = new_thread(
            &f,
            r#"{"revision":0,"file":"f.txt","base":{"start":3,"len":2},"head":{"start":3,"len":2},"body":" a note "}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let anchor = last_anchor(&f);
        assert_eq!(span(&anchor), ((3, 2), (3, 2)));
        // The place is in the revision's own file versions (by digest).
        let Anchor::Span { base, head } = &anchor else {
            unreachable!()
        };
        assert_eq!(base.as_ref().unwrap().digest, digest(BASE));
        assert_eq!(head.as_ref().unwrap().digest, digest(HEAD));
        assert_eq!(base.as_ref().unwrap().file, "f.txt");
        let comments = f.comments();
        assert_eq!(
            comments.last().unwrap(),
            &(None, "tester".to_string(), "a note".to_string())
        );
    }

    #[test]
    fn an_added_or_removed_line_has_an_empty_span_on_the_other_side() {
        let f = fixture();
        // Line 2 changed: `b` (old) became `B` (new). Choosing only the added
        // row: the base is the point before old line 2.
        new_thread(
            &f,
            r#"{"revision":0,"file":"f.txt","base":{"start":2,"len":0},"head":{"start":2,"len":1},"body":"added"}"#,
        );
        assert_eq!(span(&last_anchor(&f)), ((2, 0), (2, 1)));
        new_thread(
            &f,
            r#"{"revision":0,"file":"f.txt","base":{"start":2,"len":1},"head":{"start":3,"len":0},"body":"removed"}"#,
        );
        assert_eq!(span(&last_anchor(&f)), ((2, 1), (3, 0)));
    }

    #[test]
    fn a_bad_new_thread_is_refused_and_writes_nothing() {
        let f = fixture();
        let ok = |r: &str, file: &str, b: (u32, u32), h: (u32, u32), body: &str| {
            format!(
                r#"{{"revision":{r},"file":"{file}","base":{{"start":{},"len":{}}},"head":{{"start":{},"len":{}}},"body":"{body}"}}"#,
                b.0, b.1, h.0, h.1
            )
        };
        let cases = [
            (400, "".to_string()),
            (400, "not json".to_string()),
            (400, ok("0", "f.txt", (3, 1), (3, 1), "  ")),
            (400, ok("0", "f.txt", (3, 0), (3, 0), "x")), // nothing chosen
            (400, ok("0", "f.txt", (0, 1), (3, 1), "x")), // lines start at 1
            (400, ok("0", "nope.txt", (3, 1), (3, 1), "x")), // not in the diff
            (404, ok("5", "f.txt", (3, 1), (3, 1), "x")), // no such revision
            (400, r#"{"revision":0,"file":"f.txt","body":"x"}"#.to_string()),
            (400, r#"{"revision":-1,"file":"f.txt","base":{"start":1,"len":1},"head":{"start":1,"len":1},"body":"x"}"#.to_string()),
        ];
        for (status, body) in cases {
            assert_eq!(new_thread(&f, &body).status, status, "{body}");
        }
        assert_eq!(f.events().len(), 3, "nothing was written");
    }

    // ---- threads on a file or on the whole review -----------------------------

    #[test]
    fn a_file_thread_is_anchored_to_the_files_versions_and_comes_back_as_a_card() {
        let f = fixture();
        let reply = new_thread(
            &f,
            r#"{"scope":"file","revision":0,"file":"f.txt","body":"about the whole file"}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let Anchor::File { base, head } = last_anchor(&f) else {
            panic!("a file anchor");
        };
        assert_eq!(base.as_ref().unwrap().digest, digest(BASE));
        assert_eq!(head.as_ref().unwrap().digest, digest(HEAD));
        assert_eq!(head.unwrap().file, "f.txt");
        let answer = json(&reply);
        let id = answer["thread"].as_str().unwrap();
        let place = &answer["model"]["revisions"][0]["placements"][id];
        assert_eq!(
            (place["kind"].as_str(), place["file"].as_str()),
            (Some("file"), Some("f.txt"))
        );
    }

    #[test]
    fn the_client_page_gets_the_whole_model_with_the_new_thread_in_it() {
        let f = fixture();
        let reply = new_thread(
            &f,
            r#"{"scope":"global","revision":0,"body":"overall","model":true}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let answer = json(&reply);
        let model = &answer["model"];
        assert!(model["stamp"].as_str().is_some());
        assert_ne!(answer["before"], model["stamp"]);
        let id = answer["thread"].as_str().unwrap();
        assert!(
            model["threads"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["id"] == id)
        );
        assert_eq!(model["revisions"][0]["placements"][id]["kind"], "global");
    }

    #[test]
    fn a_review_wide_thread_has_no_file_and_points_at_the_revisions() {
        let f = fixture();
        let reply = new_thread(&f, r#"{"scope":"global","revision":0,"body":"overall"}"#);
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let Anchor::Global { base, head } = last_anchor(&f) else {
            panic!("a global anchor");
        };
        // A directory review: no base, the head is the revision's own digest.
        assert_eq!(base, None);
        assert_eq!(head, Some(digest("revision")));
    }

    #[test]
    fn a_file_or_review_wide_thread_can_be_replied_to_like_any_other() {
        let f = fixture();
        let id = json(&new_thread(
            &f,
            r#"{"scope":"global","revision":0,"body":"overall"}"#,
        ))["thread"]
            .as_str()
            .unwrap()
            .to_string();
        let reply = f.post(
            &format!("/api/threads/{id}/replies"),
            r#"{"body":"agreed"}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        assert!(
            json(&reply)["thread_data"]["comments"]
                .to_string()
                .contains("agreed")
        );
    }

    #[test]
    fn a_bad_scope_or_a_file_outside_the_diff_is_refused() {
        let f = fixture();
        for body in [
            r#"{"scope":"file","revision":0,"body":"x"}"#,
            r#"{"scope":"file","revision":0,"file":"nope.txt","body":"x"}"#,
            r#"{"scope":"file","revision":0,"file":"f.txt","body":"  "}"#,
            r#"{"scope":"weird","revision":0,"body":"x"}"#,
            r#"{"scope":"global","body":"x"}"#,
        ] {
            assert_eq!(new_thread(&f, body).status, 400, "{body}");
        }
        assert_eq!(
            new_thread(&f, r#"{"scope":"global","revision":7,"body":"x"}"#).status,
            404
        );
        assert_eq!(f.events().len(), 3, "nothing was written");
    }

    /// The fixture plus a file the diff doesn't touch (`g.txt`, kept in the
    /// bundle) with a thread on it, so the page shows that file as context.
    fn fixture_with_an_untouched_file() -> (Fixture, String) {
        use crate::model::TreeFile;
        let f = fixture();
        let g = "one\ntwo\nthree\n";
        let loaded = bundle::load(&f.path).unwrap();
        let revision = loaded.revisions().next().unwrap().id;
        let mut events = loaded.events.clone();
        events.push(Event::Pin {
            revision,
            files: vec![TreeFile {
                path: "g.txt".into(),
                digest: digest(g),
            }],
        });
        events.push(Event::Comment {
            id: Ulid::new(),
            parent: None,
            author: "r@example.com".into(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            anchor: Some(Anchor::Span {
                base: None,
                head: Some(LineRange {
                    file: "g.txt".into(),
                    digest: digest(g),
                    start: 2,
                    len: 1,
                }),
            }),
            body: "on g".into(),
        });
        let more = Additions {
            diff: None,
            blobs: vec![g.as_bytes().to_vec()],
            commits: Vec::new(),
        };
        bundle::save(&f.path, &loaded, &events, &more).unwrap();
        (f, g.to_string())
    }

    #[test]
    fn a_file_the_diff_does_not_touch_can_be_commented_on_when_the_page_shows_it() {
        let (f, g) = fixture_with_an_untouched_file();
        // The page shows it (as context, for the thread it has), with a button.
        let model = json(&f.request("GET", "/api/model", &[], ""));
        let files = model["model"]["revisions"][0]["files"].to_string();
        assert!(files.contains(r#""path":"g.txt""#), "{files}");
        // A file thread: the file's one version on both sides.
        let reply = new_thread(
            &f,
            r#"{"scope":"file","revision":0,"file":"g.txt","body":"about g"}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let Anchor::File { base, head } = last_anchor(&f) else {
            panic!("a file anchor");
        };
        for side in [base, head] {
            let side = side.unwrap();
            assert_eq!(
                (side.file.as_str(), side.digest.as_str()),
                ("g.txt", digest(&g).as_str())
            );
        }
        // A line thread on a row of the context.
        let reply = new_thread(
            &f,
            r#"{"revision":0,"file":"g.txt","base":{"start":1,"len":1},"head":{"start":1,"len":1},"body":"line 1 of g"}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let Anchor::Span { head, .. } = last_anchor(&f) else {
            panic!("a span");
        };
        let head = head.unwrap();
        assert_eq!(
            (head.start, head.len, head.digest.as_str()),
            (1, 1, digest(&g).as_str())
        );
        // A file that is neither in the diff nor shown is still refused.
        assert_eq!(
            new_thread(
                &f,
                r#"{"scope":"file","revision":0,"file":"h.txt","body":"x"}"#
            )
            .status,
            400
        );
    }

    // ---- other stored files: looked at, and commented on ----------------------

    fn big_text(lines: usize) -> String {
        (1..=lines).map(|n| format!("line {n}\n")).collect()
    }

    /// The fixture plus a stored project tree the diff doesn't touch.
    fn fixture_with_a_tree() -> Fixture {
        use crate::model::TreeFile;
        let f = fixture();
        let files: Vec<(&str, Vec<u8>)> = vec![
            ("src/a.rs", b"fn a() {}\nfn b() {}\n".to_vec()),
            ("src/lib/b.rs", b"pub fn c() {}\n".to_vec()),
            ("docs/readme.md", b"# read me\n".to_vec()),
            (
                "docs/\u{8a2d}\u{8a08} \u{30e1}\u{30e2}.md",
                "memo\n".as_bytes().to_vec(),
            ),
            ("big.txt", big_text(1200).into_bytes()),
            ("bin.dat", vec![0xff, 0xfe, 0x00, 0x9f]),
        ];
        let loaded = bundle::load(&f.path).unwrap();
        let revision = loaded.revisions().next().unwrap().id;
        let mut events = loaded.events.clone();
        events.push(Event::Pin {
            revision,
            files: files
                .iter()
                .map(|(p, b)| TreeFile {
                    path: p.to_string(),
                    digest: digest(b),
                })
                .collect(),
        });
        let more = Additions {
            diff: None,
            blobs: files.into_iter().map(|(_, b)| b).collect(),
            commits: Vec::new(),
        };
        bundle::save(&f.path, &loaded, &events, &more).unwrap();
        f
    }

    fn get(f: &Fixture, target: &str) -> Reply {
        f.request("GET", target, &[], "")
    }

    /// The list of other files, flattened to text to look into: one line per
    /// entry (`dir:PATH:COUNT`, `file:PATH`), then the message, the note and
    /// how many more.
    fn listing(f: &Fixture, query: &str) -> String {
        let reply = get(f, &format!("/api/files/0/tree{query}"));
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let list = json(&reply);
        let mut out: Vec<String> = list["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| match e["kind"].as_str() {
                Some("dir") => format!("dir:{}:{}", e["path"].as_str().unwrap(), e["count"]),
                _ => format!("file:{}", e["path"].as_str().unwrap()),
            })
            .collect();
        out.extend(list["message"].as_str().map(str::to_string));
        out.extend(list["note"].as_str().map(str::to_string));
        out.join("\n")
    }

    #[test]
    fn the_tree_lists_stored_files_the_view_does_not_have_folding_directories() {
        let f = fixture_with_a_tree();
        let root = listing(&f, "");
        // Directories fold (with how many files are in them), files are buttons.
        assert!(
            root.contains("dir:src:") && root.contains("dir:docs:"),
            "{root}"
        );
        assert!(root.contains("dir:src:2"), "src has two: {root}");
        assert!(root.contains("file:big.txt"), "{root}");
        // The diff's own file is already on the page: not offered again.
        assert!(!root.contains("f.txt"), "{root}");
        // Nothing of a directory's contents until it is opened.
        assert!(!root.contains("a.rs"), "{root}");
        let src = listing(&f, "?dir=src");
        assert!(
            src.contains("dir:src/lib:") && src.contains("file:src/a.rs"),
            "{src}"
        );
        assert!(!src.contains("b.rs"), "{src}");
        // Directories come before files.
        assert!(
            src.find("src/lib").unwrap() < src.find("src/a.rs").unwrap(),
            "{src}"
        );
    }

    #[test]
    fn a_search_lists_matching_paths_flat_ignoring_case() {
        let f = fixture_with_a_tree();
        let found = listing(&f, "?q=README");
        assert!(found.contains("file:docs/readme.md"), "{found}");
        assert!(!found.contains("big.txt"), "{found}");
        assert!(listing(&f, "?q=nothing-like-this").contains("見つかりません"));
        // Japanese paths come through the query as %XX.
        let jp = listing(&f, "?q=%E8%A8%AD%E8%A8%88");
        assert!(jp.contains("docs/設計 メモ.md"), "{jp}");
    }

    #[test]
    fn a_review_that_stores_nothing_else_says_so() {
        let f = fixture();
        assert!(listing(&f, "").contains("ほかに開けるファイルはありません"));
    }

    #[test]
    fn opening_a_file_gives_its_first_lines_as_unchanged_rows_and_records_nothing() {
        let f = fixture_with_a_tree();
        let before = f.events().len();
        let reply = get(&f, "/api/files/0/open?path=src/a.rs");
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let file = &json(&reply)["file"];
        assert_eq!(
            (file["path"].as_str(), file["total"].as_u64()),
            (Some("src/a.rs"), Some(2))
        );
        assert!(file["next"].is_null(), "a short file is whole");
        let rows = file["hunks"][0]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        // Every row is unchanged, with both numbers, and the text colored.
        assert_eq!(
            (
                rows[1]["k"].as_str(),
                rows[1]["o"].as_u64(),
                rows[1]["n"].as_u64()
            ),
            (Some("c"), Some(2), Some(2))
        );
        // The text as pieces with kinds: `fn` a keyword, `b` a function.
        assert_eq!(
            rows[1]["t"],
            serde_json::json!([["keyword", "fn"], " ", ["function", "b"], "() {}"])
        );
        assert!(
            file["hunks"][0]["header"]
                .as_str()
                .unwrap()
                .starts_with("@@ -1,2 +1,2")
        );
        assert_eq!(f.events().len(), before, "looking records nothing");
    }

    #[test]
    fn a_long_file_is_opened_in_chunks() {
        let f = fixture_with_a_tree();
        let first = json(&get(&f, "/api/files/0/open?path=big.txt"));
        let file = &first["file"];
        assert_eq!(
            (file["total"].as_u64(), file["next"].as_u64()),
            (Some(1200), Some(501))
        );
        let rows = file["hunks"][0]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 500, "only the first 500 lines");
        let second = json(&get(&f, "/api/files/0/more?path=big.txt&from=501"));
        assert_eq!(second["next"].as_u64(), Some(1001));
        let rows = second["hunk"]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 500);
        assert_eq!(
            rows[0]["n"].as_u64(),
            Some(501),
            "each chunk starts its own hunk"
        );
        assert!(
            second["hunk"]["header"]
                .as_str()
                .unwrap()
                .starts_with("@@ -501,500 +501,500 ")
        );
        let last = json(&get(&f, "/api/files/0/more?path=big.txt&from=1001"));
        assert!(last["next"].is_null());
        assert_eq!(last["hunk"]["rows"].as_array().unwrap().len(), 200);
    }

    #[test]
    fn lines_a_diff_leaves_out_are_given_a_piece_at_a_time_from_a_line() {
        let f = fixture_with_a_tree();
        let reply = get(&f, "/api/files/0/lines?path=big.txt&from=101&count=3");
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let lines = &json(&reply)["lines"];
        assert_eq!(lines.as_array().unwrap().len(), 3);
        assert_eq!(lines[0], serde_json::json!(["line 101"]));
        // Past the end there is nothing; the count is capped; a file that isn't
        // there or isn't text is refused with a reason.
        let end = json(&get(
            &f,
            "/api/files/0/lines?path=big.txt&from=1199&count=50",
        ));
        assert_eq!(end["lines"].as_array().unwrap().len(), 2);
        let all = json(&get(
            &f,
            "/api/files/0/lines?path=big.txt&from=1&count=999999",
        ));
        assert_eq!(all["lines"].as_array().unwrap().len(), 1200);
        assert_eq!(
            get(&f, "/api/files/0/lines?path=bin.dat&from=1&count=3").status,
            400
        );
        assert_eq!(
            get(&f, "/api/files/0/lines?path=nope.txt&from=1&count=3").status,
            400
        );
    }

    #[test]
    fn a_file_that_cannot_be_shown_is_refused_with_a_reason() {
        let f = fixture_with_a_tree();
        let binary = get(&f, "/api/files/0/open?path=bin.dat");
        assert_eq!(binary.status, 400);
        assert!(text(&binary).contains("テキストファイルではない"));
        assert_eq!(get(&f, "/api/files/0/open?path=nope.txt").status, 400);
        assert_eq!(get(&f, "/api/files/0/open").status, 400);
        // A path outside what the review stores is not read from anywhere.
        assert_eq!(
            get(&f, "/api/files/0/open?path=../../etc/passwd").status,
            400
        );
        assert_eq!(
            get(&f, "/api/files/0/more?path=nope.txt&from=1").status,
            400
        );
        assert_eq!(get(&f, "/api/files/9/tree").status, 404);
        assert_eq!(get(&f, "/api/files/x/tree").status, 404);
        assert_eq!(get(&f, "/api/files/0/unknown").status, 404);
    }

    #[test]
    fn the_other_files_need_the_token_too() {
        let f = fixture_with_a_tree();
        let bare = f.server.handle(&Request {
            method: "GET",
            target: "/api/files/0/open?path=src/a.rs",
            headers: vec![("host".into(), "127.0.0.1:4242".into())],
            body: b"",
        });
        assert_eq!(bare.status, 403);
    }

    #[test]
    fn a_comment_on_an_opened_file_is_kept_with_the_files_stored_version() {
        let f = fixture_with_a_tree();
        let stored = b"fn a() {}\nfn b() {}\n";
        // On line 2 (a row of the opened file).
        let reply = new_thread(
            &f,
            r#"{"revision":0,"file":"src/a.rs","base":{"start":2,"len":1},"head":{"start":2,"len":1},"body":"about b"}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let Anchor::Span { base, head } = last_anchor(&f) else {
            panic!("a span");
        };
        for side in [base, head] {
            let side = side.unwrap();
            assert_eq!(
                (side.file.as_str(), side.start, side.len),
                ("src/a.rs", 2, 1)
            );
            assert_eq!(side.digest, digest(stored));
        }
        let answer = json(&reply);
        let id = answer["thread"].as_str().unwrap();
        let place = &answer["model"]["revisions"][0]["placements"][id];
        assert_eq!(place["kind"], "line");
        assert_eq!(place["file"], "src/a.rs");
        // ...and a file thread on it.
        let reply = new_thread(
            &f,
            r#"{"scope":"file","revision":0,"file":"src/a.rs","body":"whole file"}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        assert_eq!(
            json(&reply)["model"]["revisions"][0]["placements"]
                [json(&reply)["thread"].as_str().unwrap()]["kind"],
            "file"
        );
        // Once it has threads the page has it itself, so it is not offered as "other".
        assert!(!listing(&f, "?dir=src").contains("file:src/a.rs"));
        // A file the review doesn't store is still refused.
        assert_eq!(
            new_thread(
                &f,
                r#"{"scope":"file","revision":0,"file":"nope.rs","body":"x"}"#
            )
            .status,
            400
        );
    }

    #[test]
    fn percent_decoding_reads_plain_and_encoded_text() {
        assert_eq!(
            query_param("path=a%20b&x=1", "path").as_deref(),
            Some("a b")
        );
        assert_eq!(query_param("q=a+b", "q").as_deref(), Some("a b"));
        assert_eq!(query_param("q=%E8%A8%AD", "q").as_deref(), Some("設"));
        assert_eq!(query_param("q=100%", "q").as_deref(), Some("100%"));
        assert_eq!(query_param("q=%zz", "q").as_deref(), Some("%zz"));
        assert_eq!(query_param("a=1", "q"), None);
    }

    // ---- files of a git review that the bundle doesn't store ------------------

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.email=t@example.com", "-c", "user.name=T"])
            .args(args)
            .output()
            .expect("git is installed");
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    struct GitReview {
        f: Fixture,
        repo: tempfile::TempDir,
        head: String,
    }

    /// A repository with two commits (`f.txt` changes in the second; other
    /// files are untouched) and a review of that change that stores only
    /// `f.txt`, opened with its server next to the repository.
    fn git_review() -> GitReview {
        let repo = tempfile::tempdir().unwrap();
        let p = repo.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(p.join("src/lib")).unwrap();
        std::fs::create_dir_all(p.join("docs")).unwrap();
        std::fs::write(p.join("f.txt"), BASE).unwrap();
        std::fs::write(p.join("src/a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(p.join("src/lib/b.rs"), "pub fn c() {}\n").unwrap();
        std::fs::write(p.join("docs/readme.md"), "# read me\n").unwrap();
        std::fs::write(p.join("bin.dat"), [0xffu8, 0xfe, 0x00, 0x9f]).unwrap();
        std::fs::write(p.join("huge.txt"), "x\n".repeat(1_500_000)).unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "c1"]);
        let base = git(p, &["rev-parse", "HEAD"]);
        std::fs::write(p.join("f.txt"), HEAD).unwrap();
        git(p, &["commit", "-q", "-am", "c2"]);
        let head = git(p, &["rev-parse", "HEAD"]);
        let source = Source::Git(crate::model::GitSource {
            base,
            head: head.clone(),
            spec: "c1..c2".into(),
        });
        let f = fixture_with(source, Some(p.to_path_buf()));
        GitReview { f, repo, head }
    }

    #[test]
    fn a_git_review_lists_the_rest_of_its_head_commit_next_to_the_repository() {
        let g = git_review();
        let root = listing(&g.f, "");
        assert!(
            root.contains("dir:src:") && root.contains("dir:docs:"),
            "{root}"
        );
        assert!(root.contains("file:bin.dat"), "{root}");
        assert!(
            !root.contains("f.txt"),
            "the diff's own file is on the page: {root}"
        );
        assert!(listing(&g.f, "?dir=src").contains("file:src/a.rs"));
        assert!(listing(&g.f, "?q=READ").contains("docs/readme.md"));
        assert!(!root.contains("リポジトリが見つからない"), "{root}");
    }

    #[test]
    fn a_stored_file_and_the_same_file_in_the_commit_are_listed_once() {
        let g = git_review();
        // Store docs/readme.md in the bundle too (as a comment on it would).
        let content = b"# read me\n";
        let loaded = bundle::load(&g.f.path).unwrap();
        let revision = loaded.revisions().next().unwrap().id;
        let mut events = loaded.events.clone();
        events.push(Event::Pin {
            revision,
            files: vec![crate::model::TreeFile {
                path: "docs/readme.md".into(),
                digest: digest(content),
            }],
        });
        let more = Additions {
            diff: None,
            blobs: vec![content.to_vec()],
            commits: Vec::new(),
        };
        bundle::save(&g.f.path, &loaded, &events, &more).unwrap();
        let found = listing(&g.f, "?q=readme");
        assert!(found.contains("docs/readme.md"), "{found}");
        assert_eq!(found.matches("file:docs/readme.md").count(), 1, "{found}");
    }

    #[test]
    fn opening_a_file_of_the_commit_reads_the_committed_content_and_records_nothing() {
        let g = git_review();
        let before = g.f.events().len();
        // An uncommitted edit is not what the review is about.
        std::fs::write(g.repo.path().join("src/a.rs"), "fn edited() {}\n").unwrap();
        let reply = get(&g.f, "/api/files/0/open?path=src/a.rs");
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let opened = json(&reply)["file"].to_string();
        assert!(
            opened.contains(r#"["function","a"]"#) && opened.contains(r#"["function","b"]"#),
            "the committed lines: {opened}"
        );
        assert!(!opened.contains("edited"), "{opened}");
        assert_eq!(g.f.events().len(), before, "looking records nothing");
        // The next lines of a file come from the commit too.
        let more = get(&g.f, "/api/files/0/more?path=src/a.rs&from=2");
        assert_eq!(more.status, 200, "{}", text(&more));
    }

    #[test]
    fn a_file_that_cannot_be_opened_from_the_commit_says_why() {
        let g = git_review();
        let refused = |path: &str| {
            let reply = get(&g.f, &format!("/api/files/0/open?path={path}"));
            assert_eq!(reply.status, 400, "{path}: {}", text(&reply));
            text(&reply)
        };
        assert!(refused("bin.dat").contains("テキストファイルではない"));
        let huge = refused("huge.txt");
        assert!(huge.contains("大きすぎる") && huge.contains("MB"), "{huge}");
        assert!(refused("nope.txt").contains("このコミットにありません"));
        // Only what the commit has: nothing else is read.
        assert!(refused("../etc/passwd").contains("このコミットにありません"));
        assert!(
            refused("src").contains("このコミットにありません"),
            "a directory is not a file"
        );
    }

    #[test]
    fn a_comment_on_a_file_of_the_commit_stores_that_file_with_it() {
        let g = git_review();
        let content = std::fs::read(g.repo.path().join("src/a.rs")).unwrap();
        let before = g.f.events().len();
        let reply = new_thread(
            &g.f,
            r#"{"revision":0,"file":"src/a.rs","base":{"start":2,"len":1},"head":{"start":2,"len":1},"body":"about b"}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        // A Pin (the file for this revision), then the thread.
        let events = g.f.events();
        assert_eq!(events.len(), before + 2);
        let revision = events
            .iter()
            .find_map(|e| match e {
                Event::Revision(r) => Some(r.id),
                _ => None,
            })
            .unwrap();
        assert!(
            matches!(&events[before], Event::Pin { revision: r, files } if *r == revision && files.len() == 1 && files[0].path == "src/a.rs")
        );
        assert!(matches!(
            &events[before + 1],
            Event::Comment { parent: None, .. }
        ));
        // The content is in the bundle, by its digest, and the thread points at it.
        let loaded = bundle::load(&g.f.path).unwrap();
        assert_eq!(loaded.blob(&digest(&content)), Some(content.as_slice()));
        let Anchor::Span { head, .. } = last_anchor(&g.f) else {
            panic!("a span");
        };
        assert_eq!(head.unwrap().digest, digest(&content));
        // From now on the page has the file itself (it has a thread).
        assert!(!listing(&g.f, "?dir=src").contains("file:src/a.rs"));
        // And it stays readable from the bundle alone, with no repository.
        let alone = Server::new(
            &Options {
                review: g.f.path.clone(),
                port: 0,
                author: None,
                repo: Some(g.f.path.parent().unwrap().to_path_buf()),
                refresh: None,
                before: None,
            },
            4242,
        );
        let page = alone.handle(&Request {
            method: "GET",
            target: "/api/model",
            headers: vec![
                ("host".into(), "127.0.0.1:4242".into()),
                (
                    "cookie".into(),
                    format!("{}={}", alone.cookie_name(), alone.token()),
                ),
            ],
            body: b"",
        });
        let page = text(&page);
        assert!(
            page.contains("about b") && page.contains(r#""path":"src/a.rs""#),
            "{page}"
        );
    }

    #[test]
    fn a_file_thread_on_a_file_of_the_commit_stores_it_too() {
        let g = git_review();
        let reply = new_thread(
            &g.f,
            r#"{"scope":"file","revision":0,"file":"docs/readme.md","body":"about it"}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let loaded = bundle::load(&g.f.path).unwrap();
        assert!(loaded.blob(&digest(b"# read me\n")).is_some());
        let Anchor::File { head, .. } = last_anchor(&g.f) else {
            panic!("a file anchor");
        };
        assert_eq!(head.unwrap().digest, digest(b"# read me\n"));
        // A file that isn't in the commit is still refused, and stores nothing.
        let before = g.f.events().len();
        assert_eq!(
            new_thread(
                &g.f,
                r#"{"scope":"file","revision":0,"file":"nope.txt","body":"x"}"#
            )
            .status,
            400
        );
        assert_eq!(g.f.events().len(), before);
        // So is a binary file (a comment needs lines to point at).
        assert_eq!(
            new_thread(
                &g.f,
                r#"{"scope":"file","revision":0,"file":"bin.dat","body":"x"}"#
            )
            .status,
            400
        );
    }

    #[test]
    fn without_the_repository_only_stored_files_open_and_the_page_says_so() {
        // The server was started somewhere that is not a repository.
        let elsewhere = tempfile::tempdir().unwrap();
        let g = git_review();
        let f = fixture_with(
            Source::Git(crate::model::GitSource {
                base: "0".repeat(40),
                head: g.head.clone(),
                spec: "c1..c2".into(),
            }),
            Some(elsewhere.path().to_path_buf()),
        );
        let notices = f.server.notices();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("git リポジトリの中で起動していない"),
            "{notices:?}"
        );
        let root = listing(&f, "");
        assert!(root.contains("リポジトリが見つからない"), "{root}");
        assert!(
            !root.contains("src"),
            "nothing of the commit is listed: {root}"
        );
        let refused = get(&f, "/api/files/0/open?path=src/a.rs");
        assert_eq!(refused.status, 400);
        assert!(
            text(&refused).contains("リポジトリが見つからない"),
            "{}",
            text(&refused)
        );
    }

    #[test]
    fn a_repository_without_the_reviews_commit_is_noticed() {
        let g = git_review();
        // Another repository, which doesn't have the review's commits.
        let other = tempfile::tempdir().unwrap();
        git(other.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(other.path().join("x.txt"), "x\n").unwrap();
        git(other.path(), &["add", "-A"]);
        git(other.path(), &["commit", "-q", "-m", "x"]);
        let f = fixture_with(
            Source::Git(crate::model::GitSource {
                base: g.head.clone(),
                head: g.head.clone(),
                spec: "c1..c2".into(),
            }),
            Some(other.path().to_path_buf()),
        );
        let notices = f.server.notices();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("コミットが") && notices[0].contains("ありません"),
            "{notices:?}"
        );
        assert!(listing(&f, "").contains("リポジトリが見つからない"));
        // A review made from git and its repository: nothing to say.
        assert!(g.f.server.notices().is_empty());
    }

    #[test]
    fn a_review_of_a_directory_has_no_repository_business() {
        let f = fixture();
        assert!(f.server.notices().is_empty());
        assert!(!listing(&f, "").contains("リポジトリ"));
    }

    #[test]
    fn a_given_repository_that_is_not_one_stops_the_server_before_it_starts() {
        let f = fixture();
        let elsewhere = tempfile::tempdir().unwrap();
        let options = Options {
            review: f.path.clone(),
            port: 0,
            author: None,
            repo: Some(elsewhere.path().to_path_buf()),
            refresh: None,
            before: None,
        };
        let err = run(&options, |_, _| panic!("must not start")).unwrap_err();
        assert!(
            err.to_string().contains("git リポジトリではありません"),
            "{err}"
        );
    }

    // ---- over a real socket ----------------------------------------------

    /// One HTTP/1.1 request to 127.0.0.1:`port`; the status, the headers
    /// (lower-cased, one per line) and the body of the answer.
    fn http(
        port: u16,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, String) {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut request =
            format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let raw = String::from_utf8_lossy(&raw).to_string();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, head.to_ascii_lowercase(), body.to_string())
    }

    #[test]
    fn it_serves_over_a_real_socket_and_stops_when_asked() {
        let f = fixture();
        let options = Options {
            review: f.path.clone(),
            port: 0,
            author: Some("tester".into()),
            repo: None,
            refresh: None,
            before: None,
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            run(&options, |url, _| sender.send(url.to_string()).unwrap()).unwrap();
        });
        let url = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let (address, token) = url
            .trim_start_matches("http://")
            .split_once("/?t=")
            .unwrap();
        let port: u16 = address.rsplit(':').next().unwrap().parse().unwrap();
        assert!(address.starts_with("127.0.0.1:"), "{address}");

        // No token: refused. The address with it: a cookie and a redirect.
        assert_eq!(http(port, "GET", "/", &[], "").0, 403);
        let (status, head, _) = http(port, "GET", &format!("/?t={token}"), &[], "");
        assert_eq!(status, 302);
        let name = format!("diffnote_token_{port}");
        assert!(head.contains(&format!("set-cookie: {name}=")), "{head}");
        assert!(head.contains("location: /"), "{head}");

        // With the cookie: the page, then a change, whose answer is JSON.
        let cookie = format!("{name}={token}");
        let (status, head, page) = http(port, "GET", "/", &[("Cookie", &cookie)], "");
        assert_eq!(status, 200);
        assert!(
            head.contains("cache-control: no-store") && head.contains("text/html"),
            "{head}"
        );
        assert!(page.contains("why B?"));
        let path = format!("/api/threads/{}/replies", f.thread);
        let (status, head, body) = http(
            port,
            "POST",
            &path,
            &[
                ("Cookie", &cookie),
                ("X-Diffnote", "1"),
                ("Content-Type", "application/json"),
            ],
            r#"{"body": "日本語の返信"}"#,
        );
        assert_eq!(status, 200, "{body}");
        assert!(head.contains("application/json"), "{head}");
        assert!(body.contains("日本語の返信"));
        assert!(f.comments().iter().any(|c| c.2 == "日本語の返信"));

        // Stopping: the loop ends and `run` returns.
        let (status, _, _) = http(
            port,
            "POST",
            "/api/shutdown",
            &[("Cookie", &cookie), ("X-Diffnote", "1")],
            "{}",
        );
        assert_eq!(status, 200);
        thread.join().unwrap();
    }
}
