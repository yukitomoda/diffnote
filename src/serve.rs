//! `diffnote serve`: the review in a browser, from a small server that only
//! this computer can reach.
//!
//! The page is the export's own (see `html`) with buttons on the threads. A
//! button sends a small JSON request; the server appends to the bundle and
//! answers with the HTML of what changed (the thread's card and its entry in
//! the thread list), which the page swaps in place. Nothing is reloaded.
//!
//! The server keeps no state: every request loads the bundle, and a change
//! saves it, so a `diffnote edit` at the same time is not overwritten
//! unseen (each request starts from what is on disk).
//!
//! Who may talk to it: it listens on `127.0.0.1` only; the address the
//! browser uses must be its own (a page from another site, or a rebinding of
//! its name, is refused); and every request needs the token printed at start,
//! which the first visit turns into a cookie.

use crate::annotation::{AnchorScope, LineSpan};
use crate::model::Event;
use crate::{anchor, author, bundle, create, html, review};
use anyhow::{Context, Result};
use std::path::PathBuf;
use time::OffsetDateTime;
use ulid::Ulid;

/// Bigger bodies (a comment is text) are refused.
const MAX_BODY: usize = 1024 * 1024;

const COOKIE: &str = "diffnote_token";

pub struct Options {
    /// The review bundle.
    pub review: PathBuf,
    /// The port to listen on; `0` picks a free one.
    pub port: u16,
    /// `--author`, if given (see `author::resolve`).
    pub author: Option<String>,
}

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

/// The server's identity and rules; [`Server::handle`] is the whole of its
/// behavior and does no networking.
pub struct Server {
    review: PathBuf,
    author: String,
    token: String,
    port: u16,
}

impl Server {
    pub fn new(options: &Options, port: u16) -> Server {
        // 80 random bits each; the time part of a ULID isn't secret, so two
        // are used and it is only their random parts that count.
        let token = format!("{}{}", Ulid::new(), Ulid::new());
        Server {
            review: options.review.clone(),
            author: author::resolve(options.author.as_deref()),
            token,
            port,
        }
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
            return Reply::error(403, "このサーバーのアドレス以外からの要求は受け付けません");
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
                format!("{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/", self.token),
            ));
            return reply;
        }
        if !self.has_token(request) {
            return Reply::html(
                403,
                "<!DOCTYPE html><meta charset=\"utf-8\"><title>diffnote</title>\
                 <p>アクセスできません。diffnote を起動した画面に表示された URL から開いてください。",
            );
        }

        match (request.method, path) {
            ("GET", "/") => self.page(),
            ("GET", p) if p.starts_with("/api/views/") => self.view(&p["/api/views/".len()..]),
            ("GET", p) if p.starts_with("/api/files/") => {
                self.files(&p["/api/files/".len()..], query)
            }
            ("POST", _) => {
                // A page from another site can't set this header without
                // asking the server first (which it doesn't allow).
                if request.header("x-diffnote") != Some("1") || !self.origin_is_ours(request) {
                    return Reply::error(403, "この操作は許可されていません");
                }
                self.post(path, request)
            }
            _ => Reply::error(404, "見つかりません"),
        }
    }

    fn page(&self) -> Reply {
        match bundle::load(&self.review).and_then(|l| html::render_bundle_interactive(&l)) {
            Ok(page) => Reply::html(200, page),
            Err(e) => Reply::html(
                500,
                format!(
                    "<!DOCTYPE html><meta charset=\"utf-8\"><title>diffnote</title>\
                     <p>レビューを表示できません: {}",
                    e.to_string()
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;")
                ),
            ),
        }
    }

    /// The inside of one revision's section, drawn afresh: for a view the
    /// page has let go stale, or one a patch can't be made for.
    fn view(&self, index: &str) -> Reply {
        let loaded = match bundle::load(&self.review) {
            Ok(l) => l,
            Err(e) => return Reply::error(500, &format!("処理に失敗しました: {e}")),
        };
        match index
            .parse::<usize>()
            .ok()
            .and_then(|i| html::render_view_inner(&loaded, i))
        {
            Some(inner) => Reply::json(200, &serde_json::json!({ "ok": true, "html": inner })),
            None => Reply::error(404, "そのリビジョンはありません"),
        }
    }

    /// The stored files of a revision, to look at: `{rev}/tree` lists them,
    /// `{rev}/open` draws one, `{rev}/more` draws its next lines.
    fn files(&self, what: &str, query: &str) -> Reply {
        let loaded = match bundle::load(&self.review) {
            Ok(l) => l,
            Err(e) => return Reply::error(500, &format!("処理に失敗しました: {e}")),
        };
        let param = |key: &str| query_param(query, key).unwrap_or_default();
        let Some((revision, action)) = what
            .split_once('/')
            .and_then(|(r, a)| Some((r.parse::<usize>().ok()?, a)))
        else {
            return Reply::error(404, "見つかりません");
        };
        let refused = |message: String| Reply::error(400, &message);
        match action {
            "tree" => match html::tree_listing(&loaded, revision, &param("dir"), &param("q")) {
                Some(list) => Reply::json(200, &serde_json::json!({ "ok": true, "html": list })),
                None => Reply::error(404, "そのリビジョンはありません"),
            },
            "open" => match html::open_file(&loaded, revision, &param("path")) {
                Ok(opened) => Reply::json(
                    200,
                    &serde_json::json!({
                        "ok": true,
                        "html": opened.html,
                        "list_item": opened.list_item,
                    }),
                ),
                Err(message) => refused(message),
            },
            "more" => {
                let from = param("from").parse::<usize>().unwrap_or(1);
                match html::file_chunk(&loaded, revision, &param("path"), from) {
                    Ok((rows, next)) => Reply::json(
                        200,
                        &serde_json::json!({ "ok": true, "html": rows, "next": next }),
                    ),
                    Err(message) => refused(message),
                }
            }
            _ => Reply::error(404, "見つかりません"),
        }
    }

    /// A new thread on lines of a revision's diff. The page says which lines
    /// as counters on each side (see `data-diffnote-*-next` in `html`).
    fn create_thread(&self, body: &[u8]) -> Result<Reply, Failure> {
        let bad = |m: &str| Failure(400, m.to_string());
        let value: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| bad("送られた内容を読めません"))?;
        let text = value
            .get("body")
            .and_then(|b| b.as_str())
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .ok_or_else(|| bad("コメントの本文が空です"))?;
        let revision = value
            .get("revision")
            .and_then(|r| r.as_u64())
            .ok_or_else(|| bad("リビジョンが指定されていません"))? as usize;
        // What the thread is about: lines (the default), a whole file, or the
        // whole review.
        let kind = value
            .get("scope")
            .and_then(|s| s.as_str())
            .unwrap_or("lines");
        let file = value.get("file").and_then(|f| f.as_str());
        let span = |side: &str| -> Result<LineSpan, Failure> {
            let part = value
                .get(side)
                .ok_or_else(|| bad("行の範囲が指定されていません"))?;
            let number = |key: &str| {
                part.get(key)
                    .and_then(|n| n.as_u64())
                    .filter(|n| *n <= 100_000_000)
                    .map(|n| n as u32)
                    .ok_or_else(|| bad("行の範囲が不正です"))
            };
            let (start, len) = (number("start")?, number("len")?);
            if start == 0 {
                return Err(bad("行番号は 1 から始まります"));
            }
            Ok(LineSpan { start, len })
        };
        let scope = match kind {
            "lines" => {
                let file = file.ok_or_else(|| bad("ファイルが指定されていません"))?;
                let (base, head) = (span("base")?, span("head")?);
                if base.len == 0 && head.len == 0 {
                    return Err(bad("行が選ばれていません"));
                }
                AnchorScope::Span {
                    file: file.to_string(),
                    base,
                    head,
                }
            }
            "file" => AnchorScope::File {
                file: file
                    .ok_or_else(|| bad("ファイルが指定されていません"))?
                    .to_string(),
            },
            "global" => AnchorScope::Global,
            _ => return Err(bad("コメントの種類が不正です")),
        };

        let loaded = bundle::load(&self.review).map_err(internal)?;
        let (rev, diff, files) = html::anchor_view(&loaded, revision, file)
            .ok_or_else(|| Failure(404, "そのリビジョンはありません".into()))?;
        // Lines and files are those of the page's diff (of the revision, and
        // the files it doesn't touch that threads have brought in).
        if let AnchorScope::Span { file, .. } | AnchorScope::File { file } = &scope
            && anchor::find_file(&diff, file).is_none()
        {
            return Err(bad("そのファイルはこのリビジョンの差分にありません"));
        }
        let anchor =
            create::build_anchor(&scope, &diff, &files, &rev.source.revisions(&rev.digest))
                .map_err(internal)?;
        let id = Ulid::new();
        self.append(Event::Comment {
            id,
            parent: None,
            author: self.author.clone(),
            created_at: OffsetDateTime::now_utc(),
            anchor: Some(anchor),
            body: text.to_string(),
        })?;

        let loaded = bundle::load(&self.review).map_err(internal)?;
        let threads = review::build_threads(&loaded.events);
        let mut answer = serde_json::json!({
            "ok": true,
            "thread": id.to_string(),
            "revision": revision,
            "views": html::view_count(&loaded),
            "open": threads.iter().filter(|t| !t.resolved).count(),
            "all": threads.len(),
        });
        // Where the thread sits is drawn as a patch; anything else (it is
        // not on lines of this view) means the view is drawn afresh.
        match html::thread_patch(&loaded, id, revision) {
            Some(patch) => {
                let row = |r: &html::RowRef| serde_json::json!({ "file": r.file, "old": r.old, "new": r.new });
                let mut out = match &patch.place {
                    html::PatchPlace::Lines {
                        card_row,
                        after,
                        rows,
                    } => serde_json::json!({
                        "kind": "lines",
                        "card_row": card_row,
                        "after": row(after),
                        "rows": rows.iter().map(|m| serde_json::json!({
                            "row": row(&m.row),
                            "threads": m.threads,
                            "bars": m.bars,
                        })).collect::<Vec<_>>(),
                    }),
                    html::PatchPlace::Card { card, file } => serde_json::json!({
                        "kind": "card",
                        "card": card,
                        "file": file,
                    }),
                };
                out["list_item"] = serde_json::json!(patch.list_item);
                out["list_before"] = serde_json::json!(patch.list_before.map(|u| u.to_string()));
                answer["patch"] = out;
            }
            None => answer["reload"] = serde_json::json!(true),
        }
        Ok(Reply::json(200, &answer))
    }

    fn post(&self, path: &str, request: &Request) -> Reply {
        if request.body.len() > MAX_BODY {
            return Reply::error(413, "送られた内容が大きすぎます");
        }
        if path == "/api/shutdown" {
            let mut reply = Reply::json(200, &serde_json::json!({ "ok": true }));
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
            _ => return Reply::error(404, "見つかりません"),
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
            Ulid::from_string(id).map_err(|_| Failure(400, "スレッドの ID が不正です".into()))?;
        let loaded = bundle::load(&self.review).map_err(internal)?;
        let thread = review::build_threads(&loaded.events)
            .into_iter()
            .find(|t| t.root_id == id)
            .ok_or_else(|| Failure(404, "そのスレッドはありません".into()))?;
        action(&thread)?;
        let loaded = bundle::load(&self.review).map_err(internal)?;
        let fragments = html::thread_fragments(&loaded, id)
            .ok_or_else(|| Failure(500, "表示を作れませんでした".into()))?;
        Ok(Reply::json(
            200,
            &serde_json::json!({
                "ok": true,
                "thread": id.to_string(),
                "open": fragments.open,
                "all": fragments.all,
                "views": fragments.views.iter().map(|v| serde_json::json!({
                    "revision": v.revision,
                    "card": v.card,
                    "list_item": v.list_item,
                })).collect::<Vec<_>>(),
            }),
        ))
    }

    fn append(&self, event: Event) -> Result<(), Failure> {
        let loaded = bundle::load(&self.review).map_err(internal)?;
        let mut events = loaded.events.clone();
        events.push(event);
        let nothing_else = bundle::Additions {
            diff: None,
            blobs: Vec::new(),
        };
        bundle::save(&self.review, &loaded, &events, &nothing_else).map_err(internal)
    }

    fn reply(&self, thread: &review::Thread, body: &[u8]) -> Result<(), Failure> {
        let value: serde_json::Value = serde_json::from_slice(body)
            .map_err(|_| Failure(400, "送られた内容を読めません".into()))?;
        let text = value
            .get("body")
            .and_then(|b| b.as_str())
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .ok_or_else(|| Failure(400, "返信の本文が空です".into()))?;
        self.append(Event::Comment {
            id: Ulid::new(),
            parent: Some(thread.root_id),
            author: self.author.clone(),
            created_at: OffsetDateTime::now_utc(),
            anchor: None,
            body: text.to_string(),
        })
    }

    fn set_resolved(&self, thread: &review::Thread, resolved: bool) -> Result<(), Failure> {
        if thread.resolved == resolved {
            // Already so (another tab, or the command line, got there first).
            return Ok(());
        }
        let (parent, author, created_at) = (
            thread.root_id,
            self.author.clone(),
            OffsetDateTime::now_utc(),
        );
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
        })
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

    fn has_token(&self, request: &Request) -> bool {
        request
            .header("cookie")
            .into_iter()
            .flat_map(|c| c.split(';'))
            .filter_map(|p| p.trim().split_once('='))
            .any(|(name, value)| name == COOKIE && value == self.token)
    }
}

struct Failure(u16, String);

fn internal(e: anyhow::Error) -> Failure {
    Failure(500, format!("処理に失敗しました: {e}"))
}

/// The value of `key` in a query string, with `%XX` and `+` decoded.
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
pub fn run(options: &Options, on_ready: impl FnOnce(&str)) -> Result<()> {
    let http = tiny_http::Server::http(("127.0.0.1", options.port))
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("ポート {} で待ち受けを始められませんでした", options.port))?;
    let port = http
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .context("待ち受けているポートを調べられませんでした")?;
    let server = Server::new(options, port);
    on_ready(&server.url());
    for mut request in http.incoming_requests() {
        let mut body = Vec::new();
        {
            use std::io::Read;
            // One byte more than allowed: enough to tell it is too big.
            let _ = request
                .as_reader()
                .take(MAX_BODY as u64 + 1)
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
        // Not to be kept: the page changes with every request.
        response = response.with_header(header("Cache-Control", "no-store"));
        let _ = request.respond(response);
        if shutdown {
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
                source: Source::Files { base: None },
                snapshot_mode: SnapshotMode::Full,
                files,
                tree: Vec::new(),
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
        };
        let loaded = bundle::load(&path).unwrap();
        bundle::save(&path, &loaded, &events, &additions).unwrap();
        let server = Server::new(
            &Options {
                review: path.clone(),
                port: 0,
                author: Some("tester".into()),
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
            format!("{COOKIE}={}", self.server.token())
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
        assert_eq!(bare("GET", "/", Some("diffnote_token=wrong")).status, 403);
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
    fn the_page_is_the_export_with_buttons_and_a_way_to_stop() {
        let f = fixture();
        let reply = f.request("GET", "/", &[], "");
        assert_eq!(reply.status, 200);
        let page = text(&reply);
        assert!(page.contains(r#"<body data-diffnote-api="1">"#));
        assert!(page.contains("why B?"));
        assert!(page.contains(r#"class="diffnote-reply""#));
        assert!(page.contains(r#"data-diffnote-action="resolve""#));
        assert!(page.contains("data-diffnote-shutdown"));
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
    fn a_reply_is_appended_and_the_answer_carries_the_new_card() {
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
        assert_eq!(
            (answer["open"].as_u64(), answer["all"].as_u64()),
            (Some(1), Some(1))
        );
        let views = answer["views"].as_array().unwrap();
        assert_eq!(views.len(), 1);
        let card = views[0]["card"].as_str().unwrap();
        // The card as the page draws it: its id carries the view, it has the
        // reply, and the reply box for the next one.
        assert!(
            card.contains(&format!(r#"id="r0-thread-{}""#, f.thread)),
            "{card}"
        );
        assert!(
            card.contains("because it was wrong") && card.contains("why B?"),
            "{card}"
        );
        assert!(card.contains("diffnote-reply"), "{card}");
        let item = views[0]["list_item"].as_str().unwrap();
        assert!(
            item.starts_with("<li") && item.contains(&format!("#r0-thread-{}", f.thread)),
            "{item}"
        );
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
    fn resolving_and_reopening_change_the_card_the_counts_and_the_buttons() {
        let f = fixture();
        let resolve = format!("/api/threads/{}/resolve", f.thread);
        let answer = json(&f.post(&resolve, "{}"));
        assert_eq!(
            (answer["open"].as_u64(), answer["all"].as_u64()),
            (Some(0), Some(1))
        );
        let card = answer["views"][0]["card"].as_str().unwrap();
        assert!(card.contains("diffnote-thread--resolved"), "{card}");
        assert!(card.contains(r#"data-diffnote-action="reopen""#), "{card}");
        assert!(
            matches!(f.events().last(), Some(Event::Resolve { author, .. }) if author == "tester")
        );
        assert!(
            answer["views"][0]["list_item"]
                .as_str()
                .unwrap()
                .contains("is-resolved")
        );

        let answer = json(&f.post(&format!("/api/threads/{}/reopen", f.thread), "{}"));
        assert_eq!(answer["open"].as_u64(), Some(1));
        let card = answer["views"][0]["card"].as_str().unwrap();
        assert!(!card.contains("diffnote-thread--resolved"), "{card}");
        assert!(card.contains(r#"data-diffnote-action="resolve""#), "{card}");
        assert!(matches!(f.events().last(), Some(Event::Reopen { .. })));
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
        let card = answer["views"][0]["card"].as_str().unwrap();
        assert!(
            card.contains("from the command line") && card.contains("second"),
            "{card}"
        );
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
    fn the_answer_has_a_patch_for_the_view_it_was_written_in() {
        let f = fixture();
        let reply = new_thread(
            &f,
            r#"{"revision":0,"file":"f.txt","base":{"start":3,"len":1},"head":{"start":3,"len":1},"body":"about c"}"#,
        );
        let answer = json(&reply);
        assert_eq!(answer["ok"], true);
        assert_eq!(
            (answer["open"].as_u64(), answer["all"].as_u64()),
            (Some(2), Some(2))
        );
        assert_eq!(answer["views"].as_u64(), Some(1));
        assert_eq!(answer["revision"].as_u64(), Some(0));
        let id = answer["thread"].as_str().unwrap();
        let patch = &answer["patch"];
        // The card, as a table row for the diff, with this view's ids.
        let row = patch["card_row"].as_str().unwrap();
        assert!(
            row.starts_with(r#"<tr class="diffnote-thread-row">"#),
            "{row}"
        );
        assert!(
            row.contains(&format!(r#"id="r0-thread-{id}""#)) && row.contains("about c"),
            "{row}"
        );
        // After the row of the line's last number (the new side for a context row).
        assert_eq!(patch["after"]["file"], "f.txt");
        assert_eq!(patch["after"]["new"].as_u64(), Some(3));
        // The row now belongs to the thread, with a bar of its color.
        let rows = patch["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["row"]["old"].as_u64(), Some(3));
        assert_eq!(rows[0]["row"]["new"].as_u64(), Some(3));
        assert_eq!(rows[0]["threads"], id);
        assert!(
            rows[0]["bars"]
                .as_str()
                .unwrap()
                .starts_with("inset 3px 0 0 0 #"),
            "{rows:?}"
        );
        // In the list, after the earlier thread (line 2), so at the end.
        assert!(patch["list_item"].as_str().unwrap().contains(id));
        assert!(patch["list_before"].is_null());
        assert!(answer.get("reload").is_none());
    }

    #[test]
    fn a_thread_above_another_goes_before_it_in_the_list() {
        let f = fixture();
        // Line 1 is above the fixture's thread on line 2.
        let answer = json(&new_thread(
            &f,
            r#"{"revision":0,"file":"f.txt","base":{"start":1,"len":1},"head":{"start":1,"len":1},"body":"about a"}"#,
        ));
        assert_eq!(
            answer["patch"]["list_before"].as_str(),
            Some(f.thread.to_string().as_str())
        );
    }

    #[test]
    fn a_row_already_commented_lists_both_threads_in_its_mark() {
        let f = fixture();
        // The fixture's thread is on line 2 (new); comment on the same line.
        let answer = json(&new_thread(
            &f,
            r#"{"revision":0,"file":"f.txt","base":{"start":2,"len":0},"head":{"start":2,"len":1},"body":"same line"}"#,
        ));
        let rows = answer["patch"]["rows"].as_array().unwrap();
        let mark = rows
            .iter()
            .find(|r| r["row"]["new"].as_u64() == Some(2))
            .unwrap();
        let ids: Vec<&str> = mark["threads"].as_str().unwrap().split(' ').collect();
        assert_eq!(ids.len(), 2, "{mark:?}");
        assert!(ids.contains(&f.thread.to_string().as_str()));
        assert_eq!(mark["bars"].as_str().unwrap().matches("inset").count(), 2);
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

    #[test]
    fn a_view_can_be_drawn_afresh_with_the_new_thread_in_it() {
        let f = fixture();
        new_thread(
            &f,
            r#"{"revision":0,"file":"f.txt","base":{"start":3,"len":1},"head":{"start":3,"len":1},"body":"fresh one"}"#,
        );
        let reply = f.request("GET", "/api/views/0", &[], "");
        assert_eq!(reply.status, 200);
        let html = json(&reply)["html"].as_str().unwrap().to_string();
        assert!(
            html.contains("fresh one") && html.contains("why B?"),
            "{html}"
        );
        assert!(html.contains(r#"data-diffnote-file="f.txt""#));
        assert_eq!(f.request("GET", "/api/views/9", &[], "").status, 404);
        assert_eq!(f.request("GET", "/api/views/x", &[], "").status, 404);
        // Reading needs the token like everything else.
        let bare = f.server.handle(&Request {
            method: "GET",
            target: "/api/views/0",
            headers: vec![("host".into(), "127.0.0.1:4242".into())],
            body: b"",
        });
        assert_eq!(bare.status, 403);
    }

    #[test]
    fn the_served_page_names_every_row_and_table_for_selection() {
        let f = fixture();
        let page = text(&f.request("GET", "/", &[], ""));
        assert!(page.contains(r#"<table class="diffnote-diff" data-diffnote-file="f.txt">"#));
        // The changed line: removed row (old 2), added row (new 2); counters
        // before each row.
        assert!(page.contains(r#"data-diffnote-old="2" data-diffnote-new="" data-diffnote-old-next="2" data-diffnote-new-next="2""#), "{page}");
        assert!(page.contains(r#"data-diffnote-old="" data-diffnote-new="2" data-diffnote-old-next="3" data-diffnote-new-next="2""#), "{page}");
        // The static export has none of it.
        let export = html::render_bundle(&bundle::load(&f.path).unwrap()).unwrap();
        assert!(!export.contains(r#" data-diffnote-old-next=""#));
        assert!(!export.contains(r#"<table class="diffnote-diff" data-diffnote-file="#));
        assert!(export.contains(r#"<table class="diffnote-diff">"#));
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
        let patch = &answer["patch"];
        assert_eq!(patch["kind"], "card");
        assert_eq!(patch["file"], "f.txt");
        let card = patch["card"].as_str().unwrap();
        let id = answer["thread"].as_str().unwrap();
        assert!(
            card.contains(&format!(r#"id="r0-thread-{id}""#))
                && card.contains("about the whole file"),
            "{card}"
        );
        // The location is just the path; the list has it after the line thread?
        // No: file threads come before the lines of their file.
        assert!(card.contains(r#"data-diffnote-copy="f.txt""#), "{card}");
        assert_eq!(
            patch["list_before"].as_str(),
            Some(f.thread.to_string().as_str())
        );
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
        let answer = json(&reply);
        assert_eq!(answer["patch"]["kind"], "card");
        assert!(answer["patch"]["file"].is_null());
        // Review-wide threads lead the list.
        assert_eq!(
            answer["patch"]["list_before"].as_str(),
            Some(f.thread.to_string().as_str())
        );
        let card = answer["patch"]["card"].as_str().unwrap();
        assert!(
            !card.contains("data-diffnote-copy"),
            "no location to copy: {card}"
        );
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
            json(&reply)["views"][0]["card"]
                .as_str()
                .unwrap()
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

    #[test]
    fn the_served_page_has_a_place_and_a_button_for_each_kind_of_thread() {
        let f = fixture();
        let page = text(&f.request("GET", "/", &[], ""));
        // The review-wide section is there even with no such thread yet.
        assert!(
            page.contains(r#"<section class="diffnote-global-comments" data-diffnote-global>"#),
            "{page}"
        );
        assert!(page.contains(r#"data-diffnote-add="global""#));
        // The file: named, with a button in its header and a place for cards.
        assert!(
            page.contains(
                r#"<section class="diffnote-file" id="r0-file-f-txt" data-diffnote-file="f.txt">"#
            ),
            "{page}"
        );
        assert!(page.contains(r#"data-diffnote-add="file""#));
        assert!(page.contains("data-diffnote-cards"));
        // The static export has none of it.
        let export = html::render_bundle(&bundle::load(&f.path).unwrap()).unwrap();
        assert!(!export.contains(r#"<button type="button" class="diffnote-mini""#));
        assert!(!export.contains(r#"data-diffnote-global>"#));
        assert!(!export.contains(r#"<div data-diffnote-cards>"#));
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
        };
        bundle::save(&f.path, &loaded, &events, &more).unwrap();
        (f, g.to_string())
    }

    #[test]
    fn a_file_the_diff_does_not_touch_can_be_commented_on_when_the_page_shows_it() {
        let (f, g) = fixture_with_an_untouched_file();
        // The page shows it (as context, for the thread it has), with a button.
        let page = text(&f.request("GET", "/", &[], ""));
        assert!(page.contains(r#"data-diffnote-file="g.txt""#), "{page}");
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
        assert_eq!(json(&reply)["patch"]["kind"], "lines");
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
        };
        bundle::save(&f.path, &loaded, &events, &more).unwrap();
        f
    }

    fn get(f: &Fixture, target: &str) -> Reply {
        f.request("GET", target, &[], "")
    }

    fn listing(f: &Fixture, query: &str) -> String {
        let reply = get(f, &format!("/api/files/0/tree{query}"));
        assert_eq!(reply.status, 200, "{}", text(&reply));
        json(&reply)["html"].as_str().unwrap().to_string()
    }

    #[test]
    fn the_tree_lists_stored_files_the_view_does_not_have_folding_directories() {
        let f = fixture_with_a_tree();
        let root = listing(&f, "");
        // Directories fold (with how many files are in them), files are buttons.
        assert!(
            root.contains(r#"data-diffnote-dir="src""#)
                && root.contains(r#"data-diffnote-dir="docs""#),
            "{root}"
        );
        assert!(
            root.contains(r#"<span class="diffnote-tree__count">2</span>"#),
            "src has two: {root}"
        );
        assert!(root.contains(r#"data-diffnote-open="big.txt""#), "{root}");
        // The diff's own file is already on the page: not offered again.
        assert!(!root.contains("f.txt"), "{root}");
        // Nothing of a directory's contents until it is opened.
        assert!(!root.contains("a.rs"), "{root}");
        let src = listing(&f, "?dir=src");
        assert!(
            src.contains(r#"data-diffnote-dir="src/lib""#)
                && src.contains(r#"data-diffnote-open="src/a.rs""#),
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
        assert!(
            found.contains(r#"data-diffnote-open="docs/readme.md""#),
            "{found}"
        );
        assert!(!found.contains("big.txt"), "{found}");
        assert!(listing(&f, "?q=nothing-like-this").contains("見つかりません"));
        // Japanese paths come through the query as %XX.
        let jp = listing(&f, "?q=%E8%A8%AD%E8%A8%88");
        assert!(jp.contains("docs/設計 メモ.md"), "{jp}");
    }

    #[test]
    fn a_review_that_stores_nothing_else_says_so() {
        let f = fixture();
        assert!(listing(&f, "").contains("保存されているファイルはありません"));
    }

    #[test]
    fn opening_a_file_draws_it_as_unchanged_lines_and_records_nothing() {
        let f = fixture_with_a_tree();
        let before = f.events().len();
        let reply = get(&f, "/api/files/0/open?path=src/a.rs");
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let answer = json(&reply);
        let html = answer["html"].as_str().unwrap();
        // A file section like the others, open, named, marked as only looked at.
        assert!(
            html.contains(r#"data-diffnote-opened id="r0-file-src-a-rs""#),
            "{html}"
        );
        assert!(
            html.contains(r#"data-diffnote-file="src/a.rs""#) && html.contains("<details open>"),
            "{html}"
        );
        assert!(
            html.contains("data-diffnote-close") && html.contains(r#"data-diffnote-add="file""#),
            "{html}"
        );
        assert!(
            html.contains(r#"<table class="diffnote-diff" data-diffnote-file="src/a.rs">"#),
            "{html}"
        );
        // Rows are lines to choose: both numbers, counters.
        assert!(html.contains(r#"data-diffnote-old="2" data-diffnote-new="2" data-diffnote-old-next="2" data-diffnote-new-next="2""#), "{html}");
        // (Colored, so the words are in spans.)
        assert!(html.contains(">b</span>"), "{html}");
        assert!(
            !html.contains("data-diffnote-more"),
            "a short file is whole"
        );
        assert!(
            answer["list_item"]
                .as_str()
                .unwrap()
                .contains(r##"href="#r0-file-src-a-rs""##)
        );
        // Looking at it changes nothing on disk.
        assert_eq!(f.events().len(), before);
    }

    #[test]
    fn a_long_file_is_opened_in_chunks() {
        let f = fixture_with_a_tree();
        let first = json(&get(&f, "/api/files/0/open?path=big.txt"));
        let html = first["html"].as_str().unwrap();
        assert!(
            html.contains("line 500<") || html.contains("line 500"),
            "{html}"
        );
        assert!(!html.contains("line 501"), "only the first 500 lines");
        assert!(
            html.contains(r#"data-diffnote-more data-path="big.txt" data-from="501""#),
            "{html}"
        );
        assert!(html.contains("全 1200 行"), "{html}");
        let second = json(&get(&f, "/api/files/0/more?path=big.txt&from=501"));
        assert_eq!(second["next"].as_u64(), Some(1001));
        let rows = second["html"].as_str().unwrap();
        assert!(
            rows.contains("line 501") && rows.contains("line 1000") && !rows.contains("line 1001"),
            "chunk"
        );
        assert!(
            rows.contains(r#"<tr class="diffnote-hunk-header">"#),
            "each chunk starts its own hunk"
        );
        assert!(rows.contains(r#"data-diffnote-new="501" data-diffnote-old-next="501" data-diffnote-new-next="501""#), "{rows}");
        let last = json(&get(&f, "/api/files/0/more?path=big.txt&from=1001"));
        assert!(last["next"].is_null());
        assert!(last["html"].as_str().unwrap().contains("line 1200"));
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
        assert_eq!(answer["patch"]["kind"], "lines");
        assert_eq!(answer["patch"]["after"]["file"], "src/a.rs");
        // ...and a file thread on it.
        let reply = new_thread(
            &f,
            r#"{"scope":"file","revision":0,"file":"src/a.rs","body":"whole file"}"#,
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        assert_eq!(json(&reply)["patch"]["kind"], "card");
        // Once it has threads the page has it itself, so it is not offered as "other".
        assert!(!listing(&f, "?dir=src").contains(r#"data-diffnote-open="src/a.rs""#));
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
    fn the_page_has_a_folded_quiet_place_for_other_files_and_the_export_does_not() {
        let f = fixture_with_a_tree();
        let page = text(&f.request("GET", "/", &[], ""));
        assert!(page.contains(r#"<details class="diffnote-side diffnote-side--quiet" data-diffnote-tree><summary>その他のファイル</summary>"#), "{page}");
        assert!(!page.contains("data-diffnote-tree open"), "folded at first");
        assert!(
            !page.contains(r#"data-diffnote-open="#) || page.contains("data-diffnote-open]"),
            "the list is read only when opened"
        );
        let export = html::render_bundle(&bundle::load(&f.path).unwrap()).unwrap();
        assert!(!export.contains("その他のファイル"));
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
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            run(&options, |url| sender.send(url.to_string()).unwrap()).unwrap();
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
        assert!(head.contains("set-cookie: diffnote_token="), "{head}");
        assert!(head.contains("location: /"), "{head}");

        // With the cookie: the page, then a change, whose answer is JSON.
        let cookie = format!("diffnote_token={token}");
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
