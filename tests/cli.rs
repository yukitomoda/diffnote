//! End-to-end tests: the real `diffnote` binary. A review is made the way a
//! person makes one -- `serve` records what is reviewed, and its own HTTP
//! API writes the comments (see `Served`) -- so the tests need nothing but
//! Rust and git, on any platform.

use diffnote::bundle;
use diffnote::model::Event;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The binary under test. `DIFFNOTE_BIN` points at another one, which lets a
/// test executable built elsewhere (for another platform) run against a
/// binary that lives at a different path there.
fn bin() -> PathBuf {
    std::env::var_os("DIFFNOTE_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_diffnote")))
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.email=t@example.com", "-c", "user.name=T"])
        .args(args)
        .output()
        .expect("git is installed");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Env {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Runs `diffnote args...` in `cwd`.
    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(bin())
            .current_dir(cwd)
            // Isolated from whatever `diffnote config` this machine actually
            // has (an empty directory unless a test's `env.run` set it up).
            .env("DIFFNOTE_CONFIG_DIR", self.path("user-config"))
            .args(args)
            .output()
            .expect("diffnote runs")
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.run(cwd, args);
        assert!(
            out.status.success(),
            "diffnote {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }
}

/// `diffnote serve` on a review, driven over its own HTTP API: the way the
/// page writes to a review, and so the way a test writes a comment.
struct Served {
    child: std::process::Child,
    port: u16,
    cookie: String,
    /// What it said before it was ready (what it recorded, notices).
    said: String,
}

/// Where a comment goes, as the page says it.
enum Note<'a> {
    /// The whole review: what is said.
    Global(&'a str),
    /// A whole file: its path, what is said.
    File(&'a str, &'a str),
    /// One line of a file, named by what it reads (not by counting, so a
    /// line that moves does not move the test): path, the line, what is said.
    Line(&'a str, &'a str, &'a str),
    /// A line by its number, for one the diff does not show (a file it
    /// leaves alone has no rows to read a line from): path, line, what is said.
    At(&'a str, u64, &'a str),
}

/// One request, as the page makes it: our host, the cookie, the header the
/// server insists on. Gives back the status, any cookie set, and the body.
fn http(
    port: u16,
    method: &str,
    path: &str,
    cookie: &str,
    body: Option<&str>,
) -> (u16, Option<String>, String) {
    use std::io::{Read, Write};
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("the server is there");
    let body = body.unwrap_or("");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nX-Diffnote: 1\r\n"
    );
    if !cookie.is_empty() {
        request.push_str(&format!("Cookie: {cookie}\r\n"));
    }
    if method == "POST" {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let rest = raw.get(split + 4..).unwrap_or(&[]);
    let header = |name: &str| {
        head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    };
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let bytes = if header("Transfer-Encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        let (mut out, mut at) = (Vec::new(), 0);
        while let Some(end) = rest[at..].windows(2).position(|w| w == b"\r\n") {
            let size =
                usize::from_str_radix(String::from_utf8_lossy(&rest[at..at + end]).trim(), 16)
                    .unwrap_or(0);
            if size == 0 {
                break;
            }
            at += end + 2;
            out.extend_from_slice(&rest[at..at + size]);
            at += size + 2;
        }
        out
    } else {
        rest.to_vec()
    };
    (
        status,
        header("Set-Cookie"),
        String::from_utf8_lossy(&bytes).to_string(),
    )
}

impl Served {
    fn api(&self, path: &str, body: Option<serde_json::Value>) -> serde_json::Value {
        let text = body.map(|b| b.to_string());
        let method = if text.is_some() { "POST" } else { "GET" };
        let (status, _, answer) = http(self.port, method, path, &self.cookie, text.as_deref());
        assert_eq!(status, 200, "{path}: {answer}");
        serde_json::from_str(&answer).unwrap_or(serde_json::Value::Null)
    }

    /// Which line of `path`, on the new side of the revision being shown,
    /// reads exactly `text`: read from what the page is drawn from, so it is
    /// the same whether the review is of git or of a directory.
    fn line(&self, path: &str, text: &str) -> u64 {
        let model = self.api("/api/model", None);
        let revisions = model["model"]["revisions"].as_array().unwrap();
        let file = revisions.last().unwrap()["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path)
            .unwrap_or_else(|| panic!("the revision has no {path}"));
        for hunk in file["hunks"].as_array().unwrap() {
            for row in hunk["rows"].as_array().unwrap() {
                let said: String = row["t"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        p.as_str()
                            .unwrap_or_else(|| p[1].as_str().unwrap())
                            .to_string()
                    })
                    .collect();
                if said == text && row["n"].is_u64() {
                    return row["n"].as_u64().unwrap();
                }
            }
        }
        panic!("{path} has no line {text:?} on its new side");
    }

    /// The revision being shown: the last one recorded, counting from zero.
    fn revision(&self) -> usize {
        self.api("/api/model", None)["model"]["revisions"]
            .as_array()
            .unwrap()
            .len()
            - 1
    }

    /// Writes a comment, and gives back the thread it made.
    fn note(&self, note: &Note) -> String {
        let revision = self.revision();
        let ask = match *note {
            Note::Global(body) => {
                serde_json::json!({ "body": body, "revision": revision, "scope": "global" })
            }
            Note::File(path, body) => {
                serde_json::json!({ "body": body, "revision": revision, "scope": "file", "file": path })
            }
            Note::Line(path, text, body) => serde_json::json!({
                "body": body, "revision": revision, "scope": "lines", "file": path,
                "head": { "start": self.line(path, text), "len": 1 },
            }),
            Note::At(path, line, body) => serde_json::json!({
                "body": body, "revision": revision, "scope": "lines", "file": path,
                "head": { "start": line, "len": 1 },
            }),
        };
        self.api("/api/threads", Some(ask))["thread"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn reply(&self, thread: &str, body: &str) {
        self.api(
            &format!("/api/threads/{thread}/replies"),
            Some(serde_json::json!({ "body": body })),
        );
    }

    fn resolve(&self, thread: &str) {
        self.api(
            &format!("/api/threads/{thread}/resolve"),
            Some(serde_json::json!({})),
        );
    }

    /// The page's 「最新を取り込む」: whether something was added.
    fn refresh(&self) -> bool {
        self.api("/api/refresh", Some(serde_json::json!({})))["added"]
            .as_bool()
            .unwrap()
    }

    /// Ends the server the way the page's button does.
    fn stop(self) {
        self.shut_down("{}");
    }

    /// 「保存せずに終了」: everything this server did is taken back.
    fn discard(self) {
        self.shut_down(r#"{"discard":true}"#);
    }

    fn shut_down(mut self, body: &str) {
        let _ = http(self.port, "POST", "/api/shutdown", &self.cookie, Some(body));
        for _ in 0..50 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Env {
    /// Starts `diffnote serve -f review args...` in `cwd` and waits for it to
    /// say where it is. A server that will not start fails the test with
    /// what it said.
    fn serve(&self, cwd: &Path, review: &Path, args: &[&str]) -> Served {
        use std::io::{BufRead, BufReader, Read};
        let mut child = Command::new(bin())
            .current_dir(cwd)
            .env("DIFFNOTE_CONFIG_DIR", self.path("user-config"))
            .arg("serve")
            .arg("-f")
            .arg(review)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("diffnote serve starts");
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut said = String::new();
        let url = loop {
            match lines.next() {
                Some(Ok(line)) => {
                    if let Some(at) = line.find("http://127.0.0.1:") {
                        break line[at..].trim().to_string();
                    }
                    said.push_str(&line);
                    said.push('\n');
                }
                _ => {
                    if let Some(mut err) = child.stderr.take() {
                        let _ = err.read_to_string(&mut said);
                    }
                    let _ = child.wait();
                    panic!("diffnote serve {args:?} did not start: {said}");
                }
            }
        };
        // Whatever it says from here on is read, so it never waits on a pipe.
        std::thread::spawn(move || lines.for_each(drop));
        let rest = url.trim_start_matches("http://127.0.0.1:");
        let port: u16 = rest[..rest.find('/').unwrap()].parse().unwrap();
        let start = &rest[rest.find('/').unwrap()..];
        let (_, cookie, _) = http(port, "GET", start, "", None);
        let cookie = cookie.expect("the first visit is given a cookie");
        Served {
            child,
            port,
            cookie: cookie.split(';').next().unwrap().to_string(),
            said,
        }
    }

    /// `diffnote serve` that is expected to refuse to start: gives back what
    /// it said. One that starts after all is stopped and fails the test, so
    /// a refusal that goes missing cannot hang the suite.
    fn serve_refused(&self, cwd: &Path, review: &Path, args: &[&str]) -> String {
        use std::io::Read;
        let mut child = Command::new(bin())
            .current_dir(cwd)
            .env("DIFFNOTE_CONFIG_DIR", self.path("user-config"))
            .arg("serve")
            .arg("-f")
            .arg(review)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("diffnote serve runs");
        for _ in 0..100 {
            if let Ok(Some(status)) = child.try_wait() {
                assert!(!status.success(), "serve {args:?} was expected to refuse");
                let mut said = String::new();
                child
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut said)
                    .unwrap();
                return said;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("serve {args:?} started, and was expected to refuse");
    }

    /// Adds what `args` names to `review` (making it, with `--base`, if it
    /// is not there yet) and writes `notes` into it, through `serve` -- the
    /// way a person makes a review. Gives back the threads, in order.
    fn review(&self, cwd: &Path, review: &Path, args: &[&str], notes: &[Note]) -> Vec<String> {
        let served = self.serve(cwd, review, args);
        let threads = notes.iter().map(|n| served.note(n)).collect();
        served.stop();
        threads
    }
}

fn bundle_names(path: &Path) -> Vec<String> {
    let file = std::fs::File::open(path).unwrap();
    zip::ZipArchive::new(file)
        .unwrap()
        .file_names()
        .map(str::to_string)
        .collect()
}

fn comment_bodies(loaded: &bundle::Loaded) -> Vec<String> {
    loaded
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Comment { body, .. } => Some(body.clone()),
            _ => None,
        })
        .collect()
}

fn count_blobs(path: &Path) -> usize {
    bundle_names(path)
        .iter()
        .filter(|n| n.starts_with("blobs/"))
        .count()
}

#[test]
fn a_directory_review_over_several_sessions() {
    let env = Env::new();
    let dir = env.path("project");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(dir.join("b.txt"), "x\n").unwrap();
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();

    let out = env.ok(&dir, &["init", "-f", review_arg, "."]);
    assert!(out.contains("2 個のファイルを"), "{out}");
    assert_eq!(count_blobs(&review), 2);

    // Session 1: a.txt changes.
    std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    env.review(
        &dir,
        &review,
        &["."],
        &[
            Note::Global("overall remark"),
            Note::Line("a.txt", "TWO", "why uppercase?"),
        ],
    );

    let loaded = bundle::load(&review).unwrap();
    let revisions: Vec<_> = loaded.revisions().collect();
    // The init snapshot, then this session's revision.
    assert_eq!(revisions.len(), 2);
    let latest = revisions[1];
    let touched: Vec<_> = latest
        .files
        .iter()
        .filter_map(|f| f.new_path.as_deref())
        .collect();
    assert_eq!(touched, ["a.txt"]);
    // A directory review keeps the whole tree, every time.
    let mut tree: Vec<_> = loaded
        .manifest(latest)
        .into_iter()
        .map(|f| f.path)
        .collect();
    tree.sort();
    assert_eq!(tree, ["a.txt", "b.txt"]);
    let read = loaded.tree_of(latest);
    assert_eq!(read["a.txt"], b"one\nTWO\nthree\n");
    assert_eq!(read["b.txt"], b"x\n");
    // b.txt is unchanged, so it is stored once: 2 (init) + 1 new a.txt.
    assert_eq!(count_blobs(&review), 3);

    // Session 2: b.txt changes too. It is compared with the base (the tree
    // `init` took), so a.txt is in it as well.
    std::fs::write(dir.join("b.txt"), "x\ny\n").unwrap();
    env.review(
        &dir,
        &review,
        &["."],
        &[Note::Line("b.txt", "y", "new line")],
    );
    let loaded = bundle::load(&review).unwrap();
    let revisions: Vec<_> = loaded.revisions().collect();
    assert_eq!(revisions.len(), 3);
    let mut touched: Vec<_> = revisions[2]
        .files
        .iter()
        .filter_map(|f| f.new_path.as_deref())
        .collect();
    touched.sort();
    assert_eq!(touched, ["a.txt", "b.txt"], "both differ from the base");
    assert_eq!(
        comment_bodies(&loaded),
        ["overall remark", "why uppercase?", "new line"]
    );

    // Export: one view per revision that has a diff, with every comment.
    let html_path = env.path("out.html");
    env.ok(
        &dir,
        &[
            "export",
            "-f",
            review_arg,
            "-o",
            html_path.to_str().unwrap(),
        ],
    );
    let html = std::fs::read_to_string(&html_path).unwrap();
    let model = model_of(&html);
    assert_eq!(model["revisions"].as_array().unwrap().len(), 2);
    for body in ["why uppercase?", "overall remark", "new line"] {
        // One thread, which every revision's view has (placed in each).
        let id = thread_saying(&model, body)["id"].as_str().unwrap();
        for rev in model["revisions"].as_array().unwrap() {
            assert!(rev["placements"].get(id).is_some(), "{body} in every view");
        }
    }
}

#[test]
fn an_unchanged_directory_reopens_the_last_diff_and_records_nothing_new() {
    let env = Env::new();
    let dir = env.path("project");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(&dir, &["init", "-f", review_arg, "."]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    env.review(
        &dir,
        &review,
        &["."],
        &[Note::Line("a.txt", "two", "first")],
    );
    let blobs_before = count_blobs(&review);

    // Nothing changed since: the diff of the last revision comes back, so a
    // comment can still be added, and nothing else is recorded.
    env.review(
        &dir,
        &review,
        &["."],
        &[Note::Line("a.txt", "two", "second")],
    );
    let loaded = bundle::load(&review).unwrap();
    assert_eq!(loaded.revisions().count(), 2);
    assert_eq!(count_blobs(&review), blobs_before);
    assert_eq!(comment_bodies(&loaded), ["first", "second"]);
}

fn git_repo(env: &Env) -> PathBuf {
    let repo = env.path("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("calc.txt"), "a\nb\nc\n").unwrap();
    std::fs::write(repo.join("README.md"), "# calc\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "c1"]);
    git(&repo, &["tag", "c1"]);
    std::fs::write(repo.join("calc.txt"), "a\nB\nc\n").unwrap();
    std::fs::write(repo.join("README.md"), "# calc\n\nA calculator.\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c2"]);
    git(&repo, &["tag", "c2"]);
    std::fs::write(repo.join("calc.txt"), "a\nB\nc\nd\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c3"]);
    git(&repo, &["tag", "c3"]);
    repo
}

#[test]
fn a_git_review_keeps_files_that_comments_refer_to_even_when_a_later_diff_leaves_them_alone() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");

    // Session 1 (base c1, up to c2): a comment on the README, which that diff touches.
    env.review(
        &repo,
        &review,
        &["--snapshot", "changed", "--base", "c1", "c2"],
        &[
            Note::Line("README.md", "A calculator.", "more detail please"),
            Note::Line("calc.txt", "B", "why B?"),
        ],
    );
    let loaded = bundle::load(&review).unwrap();
    let first = loaded.revisions().next().unwrap();
    assert_eq!(
        loaded.manifest(first).len(),
        2,
        "calc.txt and README.md, both touched"
    );

    // Session 2 (c1..c3, the same base) has README.md and calc.txt in it too:
    // the thread on README stays placeable and its content is kept.
    env.review(
        &repo,
        &review,
        &["c3"],
        &[Note::Line("calc.txt", "d", "new line")],
    );
    let loaded = bundle::load(&review).unwrap();
    let second = loaded.revisions().nth(1).unwrap();
    assert_eq!(
        second.snapshot_mode,
        bundle::SnapshotMode::Changed,
        "inherited"
    );
    let mut touched: Vec<_> = second
        .files
        .iter()
        .filter_map(|f| f.new_path.as_deref())
        .collect();
    touched.sort();
    assert_eq!(touched, ["README.md", "calc.txt"]);
    let mut manifest: Vec<_> = loaded
        .manifest(second)
        .into_iter()
        .map(|f| f.path)
        .collect();
    manifest.sort();
    assert_eq!(manifest, ["README.md", "calc.txt"]);
    let tree = loaded.tree_of(second);
    assert_eq!(tree["README.md"], b"# calc\n\nA calculator.\n");
    assert_eq!(tree["calc.txt"], b"a\nB\nc\nd\n");
    // calc.txt c1, c2, c3; README c1, c2 (c3 is the same as c2): five blobs
    // (the base side is kept once, though both revisions have it).
    assert_eq!(count_blobs(&review), 5, "{:?}", bundle_names(&review));
}

#[test]
fn full_snapshots_keep_the_whole_tree_and_changed_ones_do_not() {
    for (mode, expect_untouched) in [("full", true), ("changed", false)] {
        let env = Env::new();
        let repo = git_repo(&env);
        std::fs::write(repo.join("other.txt"), "unrelated\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "c4"]);
        let review = env.path("review.diffnote");
        env.review(
            &repo,
            &review,
            &["--snapshot", mode, "--base", "c2", "c3"],
            &[Note::Line("calc.txt", "d", "hello")],
        );
        let loaded = bundle::load(&review).unwrap();
        let rev = loaded.revisions().next().unwrap();
        let paths: Vec<_> = loaded.manifest(rev).into_iter().map(|f| f.path).collect();
        // The range is c2..c3, so `other.txt` (added in c4) is not in it.
        assert_eq!(
            paths.iter().any(|p| p == "README.md"),
            expect_untouched,
            "{mode}: {paths:?}"
        );
        assert!(paths.iter().any(|p| p == "calc.txt"));
        assert!(!paths.iter().any(|p| p == "other.txt"), "not in c3");
    }
}

#[test]
fn a_bad_range_fails_without_touching_the_bundle() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    env.serve_refused(&repo, &review, &["no-such-rev"]);
    assert!(!review.exists());
}

#[test]
fn threads_on_a_file_a_later_diff_leaves_alone_are_shown_and_can_be_added_to() {
    use diffnote::model::Anchor;
    let env = Env::new();
    let repo = env.path("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    let readme_v1 = "# calc\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\n";
    std::fs::write(repo.join("README.md"), readme_v1).unwrap();
    std::fs::write(repo.join("calc.txt"), "a\nb\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "c1"]);
    git(&repo, &["tag", "c1"]);
    let readme_v2 = "# calc\nline 2\nline 3 (edited)\nline 4\nline 5\nline 6\nline 7\n";
    std::fs::write(repo.join("README.md"), readme_v2).unwrap();
    std::fs::write(repo.join("calc.txt"), "a\nB\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c2"]);
    git(&repo, &["tag", "c2"]);
    std::fs::write(repo.join("calc.txt"), "a\nB\nc\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c3"]);
    git(&repo, &["tag", "c3"]);
    std::fs::write(repo.join("calc.txt"), "a\nB\nc\nd\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c4"]);
    git(&repo, &["tag", "c4"]);

    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();

    // Session 1 (base c2, up to c3): the diff leaves README.md alone, and a
    // thread is written on it (the page opens files the diff does not show).
    env.review(
        &repo,
        &review,
        &["--snapshot", "changed", "--base", "c2", "c3"],
        &[Note::At("README.md", 3, "why edit this?")],
    );

    // Session 2 (the same base, up to c4) leaves README.md alone too. Its
    // thread is still shown, and a new comment can be written there.
    env.review(
        &repo,
        &review,
        &["c4"],
        &[Note::At("README.md", 6, "and what about this line?")],
    );

    let loaded = bundle::load(&review).unwrap();
    assert_eq!(
        comment_bodies(&loaded),
        ["why edit this?", "and what about this line?"]
    );
    let anchors: Vec<&Anchor> = loaded
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Comment {
                anchor: Some(a), ..
            } => Some(a),
            _ => None,
        })
        .collect();
    let Anchor::Span { head: Some(h), .. } = anchors[1] else {
        panic!("{:?}", anchors[1]);
    };
    // A place in the README's version at c2/c3, recorded by digest.
    assert_eq!((h.file.as_str(), h.start, h.len), ("README.md", 6, 1));
    assert_eq!(h.digest, diffnote::digest::digest(readme_v2));
    // The bundle keeps that version for the second revision even though the
    // diff doesn't touch it.
    let second = loaded.revisions().nth(1).unwrap();
    assert!(
        loaded
            .manifest(second)
            .iter()
            .any(|f| f.path == "README.md")
    );
    assert!(loaded.blob(&h.digest).is_some());

    // The export shows both README threads in the second revision's view,
    // each with its context, and nothing is unplaced.
    let html_path = env.path("out.html");
    env.ok(
        &repo,
        &[
            "export",
            "-f",
            review_arg,
            "-o",
            html_path.to_str().unwrap(),
        ],
    );
    let html = std::fs::read_to_string(&html_path).unwrap();
    let model = model_of(&html);
    let second = &model["revisions"][1];
    // README.md is a file of the second view (as context: the diff leaves it
    // alone), and both threads on it are placed on lines there.
    let readme = second["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "README.md")
        .expect("README.md is in the second view");
    assert_eq!(readme["status"], "context");
    for body in ["why edit this?", "and what about this line?"] {
        let id = thread_saying(&model, body)["id"].as_str().unwrap();
        let placement = &second["placements"][id];
        assert_eq!(placement["kind"], "line", "{body}: {placement}");
        assert_eq!(placement["file"], "README.md");
    }
}

/// The mode each recorded revision of `review` was kept with.
fn modes(review: &Path) -> Vec<bundle::SnapshotMode> {
    bundle::load(review)
        .unwrap()
        .revisions()
        .map(|r| r.snapshot_mode)
        .collect()
}

fn manifest_paths(review: &Path, nth: usize) -> Vec<String> {
    let loaded = bundle::load(review).unwrap();
    let rev = loaded.revisions().nth(nth).unwrap();
    let mut paths: Vec<String> = loaded.manifest(rev).into_iter().map(|f| f.path).collect();
    paths.sort();
    paths
}

#[test]
fn a_git_review_keeps_only_what_it_needs_by_default() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    // c2..c3 changes calc.txt only; README.md is neither touched nor commented on.
    env.review(
        &repo,
        &review,
        &["--base", "c2", "c3"],
        &[Note::Line("calc.txt", "d", "new line")],
    );
    assert_eq!(modes(&review), [bundle::SnapshotMode::Changed]);
    assert_eq!(manifest_paths(&review, 0), ["calc.txt"]);
    // Only calc.txt's two versions are stored: git has everything else.
    assert_eq!(count_blobs(&review), 2, "{:?}", bundle_names(&review));
}

#[test]
fn a_git_review_remembers_the_commits_it_was_made_against() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    env.review(
        &repo,
        &review,
        &["--base", "c1", "c2"],
        &[Note::Line("calc.txt", "B", "why B?")],
    );
    let loaded = bundle::load(&review).unwrap();
    let rev = loaded.revisions().next().unwrap();
    let diffnote::model::Source::Git(g) = &rev.source else {
        panic!("a git review");
    };
    assert_eq!(g.base, git(&repo, &["rev-parse", "c1"]));
    assert_eq!(g.head, git(&repo, &["rev-parse", "c2"]));
    assert_eq!(g.spec, "c1..c2");
    assert!(
        g.base.len() == 40 && g.head.len() == 40,
        "full ids, not names"
    );
}

#[test]
fn a_directory_review_keeps_the_full_tree_and_refuses_to_be_told_otherwise() {
    let env = Env::new();
    let dir = env.path("project");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    std::fs::write(dir.join("b.txt"), "x\n").unwrap();
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(&dir, &["init", "-f", review_arg, "."]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();

    // `changed` can't work here, so it is an error, not something quietly
    // ignored -- and nothing is opened or written.
    let before = std::fs::read(&review).unwrap();
    let err = env.serve_refused(&dir, &review, &["--snapshot", "changed", "."]);
    assert!(err.contains("full"), "it says what the review is: {err}");
    assert_eq!(
        std::fs::read(&review).unwrap(),
        before,
        "the bundle is untouched"
    );

    // `full` is what it is already, so saying so is fine, as is not saying.
    env.review(
        &dir,
        &review,
        &["--snapshot", "full", "."],
        &[Note::Line("a.txt", "two", "hello")],
    );
    assert_eq!(
        modes(&review),
        [bundle::SnapshotMode::Full, bundle::SnapshotMode::Full]
    );
    assert_eq!(manifest_paths(&review, 1), ["a.txt", "b.txt"]);
}

/// A repo whose `docs.md` (20 lines: `line 1`..`line 20`) is never touched,
/// with a binary `logo.bin`, and `calc.txt` (30 lines) changing at line 30.
fn repo_with_docs(env: &Env) -> PathBuf {
    let repo = env.path("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    let docs: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    let calc: String = (1..=30).map(|n| format!("c{n}\n")).collect();
    std::fs::write(repo.join("docs.md"), &docs).unwrap();
    std::fs::write(repo.join("calc.txt"), &calc).unwrap();
    std::fs::write(repo.join("logo.bin"), [0u8, 159, 146, 150, 255, 0]).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "c1"]);
    git(&repo, &["tag", "c1"]);
    std::fs::write(repo.join("calc.txt"), calc.replace("c30\n", "C30\n")).unwrap();
    git(&repo, &["commit", "-q", "-am", "c2"]);
    git(&repo, &["tag", "c2"]);
    repo
}

#[test]
fn a_thread_survives_a_second_review_from_the_same_base_with_a_longer_range() {
    let env = Env::new();
    let repo = env.path("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("calc.txt"), "add\nsub\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "c1"]);
    git(&repo, &["tag", "c1"]);
    std::fs::write(repo.join("calc.txt"), "add\nsub\nmul\ndiv\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c2"]);
    git(&repo, &["tag", "c2"]);
    std::fs::write(repo.join("calc.txt"), "header\nadd\nsub\nmul\ndiv\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c3"]);
    git(&repo, &["tag", "c3"]);

    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    // c1..c2 adds mul and div; comment on mul.
    env.review(
        &repo,
        &review,
        &["--base", "c1", "c2"],
        &[Note::Line("calc.txt", "mul", "why mul?")],
    );
    // The review is extended to c1..c3 from the same base.
    env.review(
        &repo,
        &review,
        &["c3"],
        &[Note::Line("calc.txt", "header", "why a header?")],
    );

    let html_path = env.path("out.html");
    env.ok(
        &repo,
        &[
            "export",
            "-f",
            review_arg,
            "-o",
            html_path.to_str().unwrap(),
        ],
    );
    let html = std::fs::read_to_string(&html_path).unwrap();
    let model = model_of(&html);
    // Placed at its line in this view (the header pushed it from 3 to 4), not
    // as a line that was deleted or is not there.
    let id = thread_saying(&model, "why mul?")["id"].as_str().unwrap();
    let placement = &model["revisions"][1]["placements"][id];
    assert_eq!(placement["kind"], "line", "{placement}");
    assert_eq!(placement["file"], "calc.txt");
    assert_eq!(
        (placement["start"].as_u64(), placement["end"].as_u64()),
        (Some(4), Some(4))
    );
}

/// The data an exported page is drawn from (its `#diffnote-data` script).
fn model_of(html: &str) -> serde_json::Value {
    let start = html
        .find(r#"id="diffnote-data">"#)
        .expect("the page has its data")
        + r#"id="diffnote-data">"#.len();
    let end = start + html[start..].find("</script>").unwrap();
    serde_json::from_str(&html[start..end]).expect("the data is JSON")
}

/// The thread whose first comment says `text`.
fn thread_saying<'a>(model: &'a serde_json::Value, text: &str) -> &'a serde_json::Value {
    model["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["comments"][0]["doc"].to_string().contains(text))
        .unwrap_or_else(|| panic!("no thread says {text:?}"))
}

/// The review's title: a setting (state, not a history of them).
fn titles(review: &Path) -> Vec<String> {
    bundle::load(review)
        .unwrap()
        .settings
        .title
        .into_iter()
        .collect()
}

fn exported(env: &Env, repo: &Path, review: &Path) -> String {
    let html = env.path("t.html");
    env.ok(
        repo,
        &[
            "export",
            "-f",
            review.to_str().unwrap(),
            "-o",
            html.to_str().unwrap(),
        ],
    );
    std::fs::read_to_string(html).unwrap()
}

#[test]
fn a_title_given_at_init_names_the_export() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let arg = review.to_str().unwrap();
    env.ok(
        &repo,
        &["init", "-f", arg, "--title", "ログイン改修 <v2>", "c1"],
    );
    env.review(
        &repo,
        &review,
        &["c2"],
        &[Note::Line("calc.txt", "B", "why?")],
    );
    assert_eq!(titles(&review), ["ログイン改修 <v2>"]);
    let html = exported(&env, &repo, &review);
    assert_eq!(model_of(&html)["title"], "ログイン改修 <v2>");
    assert!(html.contains("<title>ログイン改修 &lt;v2&gt;</title>"));
}

#[test]
fn without_a_title_the_export_keeps_the_default_heading() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    env.review(
        &repo,
        &review,
        &["--base", "c1", "c2"],
        &[Note::Line("calc.txt", "B", "why?")],
    );
    assert!(titles(&review).is_empty());
    assert!(model_of(&exported(&env, &repo, &review))["title"].is_null());
}

#[test]
fn a_directory_review_takes_a_title_at_init() {
    let env = Env::new();
    let dir = env.path("proj");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let review = env.path("d.diffnote");
    env.ok(
        &dir,
        &[
            "init",
            "-f",
            review.to_str().unwrap(),
            "--title",
            "設計レビュー",
        ],
    );
    assert_eq!(titles(&review), ["設計レビュー"]);
    assert_eq!(
        model_of(&exported_dir(&env, &dir, &review))["title"],
        "設計レビュー"
    );
}

/// Like `exported`, for a review with no comment yet: give it one first.
fn exported_dir(env: &Env, dir: &Path, review: &Path) -> String {
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    env.review(
        dir,
        review,
        &["."],
        &[Note::Line("a.txt", "two", "changed")],
    );
    exported(env, dir, review)
}

/// The author of every comment event, in order.
fn authors(review: &Path) -> Vec<String> {
    bundle::load(review)
        .unwrap()
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Comment { author, .. } => Some(author.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_author_is_git_user_name_first() {
    let env = Env::new();
    let repo = git_repo(&env);
    git(&repo, &["config", "user.name", "山田 太郎"]);
    git(&repo, &["config", "user.email", "taro@example.com"]);
    let review = env.path("review.diffnote");
    env.review(
        &repo,
        &review,
        &["--base", "c1", "c2"],
        &[Note::Line("calc.txt", "B", "why?")],
    );
    assert_eq!(authors(&review), ["山田 太郎"]);
}

#[test]
fn without_a_git_name_the_email_is_the_author() {
    let env = Env::new();
    let repo = git_repo(&env);
    // A blank name in the repo's own config hides any global one.
    git(&repo, &["config", "user.name", ""]);
    git(&repo, &["config", "user.email", "taro@example.com"]);
    let review = env.path("review.diffnote");
    env.review(
        &repo,
        &review,
        &["--base", "c1", "c2"],
        &[Note::Line("calc.txt", "B", "why?")],
    );
    assert_eq!(authors(&review), ["taro@example.com"]);
}

#[test]
fn a_configured_author_is_remembered_across_bundles_and_beats_git() {
    let env = Env::new();
    let repo = git_repo(&env);
    git(&repo, &["config", "user.name", "山田 太郎"]);
    let set = env.ok(&repo, &["config", "set", "author", "鈴木 花子"]);
    assert!(set.contains("設定しました"), "{set}");
    let got = env.ok(&repo, &["config", "get", "author"]);
    assert_eq!(got.trim(), "鈴木 花子");
    // It wins over git's own user.name, in any bundle.
    let review_a = env.path("a.diffnote");
    env.review(
        &repo,
        &review_a,
        &["--base", "c1", "c2"],
        &[Note::Line("calc.txt", "B", "why?")],
    );
    assert_eq!(authors(&review_a), ["鈴木 花子"]);
    // A second, unrelated bundle: remembered there too.
    let review_b = env.path("b.diffnote");
    env.review(
        &repo,
        &review_b,
        &["--base", "c1", "c2"],
        &[Note::Line("calc.txt", "B", "why?")],
    );
    assert_eq!(authors(&review_b), ["鈴木 花子"]);
    // Unset: git's name takes over again.
    let unset = env.ok(&repo, &["config", "unset", "author"]);
    assert!(unset.contains("取り消しました"), "{unset}");
    let review_c = env.path("c.diffnote");
    env.review(
        &repo,
        &review_c,
        &["--base", "c1", "c2"],
        &[Note::Line("calc.txt", "B", "why?")],
    );
    assert_eq!(authors(&review_c), ["山田 太郎"]);
}

#[test]
fn setting_an_empty_value_is_refused_and_unset_needs_no_value() {
    let env = Env::new();
    let repo = git_repo(&env);
    let out = env.run(&repo, &["config", "set", "author", ""]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("空の値"));
    // Never set: unsetting it anyway is not an error, and get says so.
    let unset = env.ok(&repo, &["config", "unset", "author"]);
    assert!(unset.contains("取り消しました"), "{unset}");
    let got = env.ok(&repo, &["config", "get", "author"]);
    assert!(got.contains("設定されていません"), "{got}");
}

#[test]
fn a_reader_that_has_gone_is_not_an_error() {
    // `diffnote config get | head` style: nobody reads stdout any more. It
    // should stop quietly rather than panic on the broken pipe.
    let env = Env::new();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = Command::new(bin())
        .env("DIFFNOTE_CONFIG_DIR", env.path("user-config"))
        .args(["config", "get"])
        .stdout(writer)
        .output()
        .expect("diffnote runs");
    assert!(out.status.success(), "{:?}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked") && err.is_empty(), "{err}");
}

/// A directory review of a 40-line file with two changes far apart, so that
/// the diff leaves out lines before, between and after its two hunks.
fn review_with_left_out_lines(env: &Env) -> (PathBuf, PathBuf) {
    let dir = env.path("project");
    std::fs::create_dir(&dir).unwrap();
    let text = |a: &str, b: &str| -> String {
        (1..=40)
            .map(|n| match n {
                10 => format!("{a}\n"),
                30 => format!("{b}\n"),
                _ => format!("line {n}\n"),
            })
            .collect()
    };
    std::fs::write(dir.join("a.txt"), text("ten", "thirty")).unwrap();
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(&dir, &["init", "-f", review_arg, "."]);
    std::fs::write(dir.join("a.txt"), text("TEN", "THIRTY")).unwrap();
    env.review(
        &dir,
        &review,
        &["."],
        &[Note::Line("a.txt", "TEN", "a comment")],
    );
    (dir, review)
}

/// The places the diff leaves out of `a.txt` in the last revision, as
/// `(count, first new line, lines carried)`.
fn gaps_of_a_txt(html: &str) -> Vec<Option<(u64, u64, Option<usize>)>> {
    let model = model_of(html);
    let rev = model["revisions"].as_array().unwrap().last().unwrap();
    let file = rev["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "a.txt")
        .unwrap();
    file["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            g.as_object().map(|g| {
                (
                    g["n"].as_u64().unwrap(),
                    g["w"].as_u64().unwrap(),
                    g.get("t").map(|t| t.as_array().unwrap().len()),
                )
            })
        })
        .collect()
}

#[test]
fn the_export_carries_the_lines_a_diff_leaves_out_up_to_a_limit_smaller_places_first() {
    let env = Env::new();
    let (dir, review) = review_with_left_out_lines(&env);
    let export = |extra: &[&str]| -> String {
        let out = env.path("out.html");
        let mut args = vec![
            "export",
            "-f",
            review.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        env.ok(&dir, &args);
        std::fs::read_to_string(out).unwrap()
    };
    // Lines 1-6 (before the first hunk), 14-26 (between), 34-40 (after).
    let all = gaps_of_a_txt(&export(&["--expand-limit", "all"]));
    assert_eq!(
        all,
        vec![
            Some((6, 1, Some(6))),
            Some((13, 14, Some(13))),
            Some((7, 34, Some(7)))
        ]
    );
    // The default carries them too (they are few).
    assert_eq!(gaps_of_a_txt(&export(&[])), all);
    // None: the places are there, the lines are not.
    let none = gaps_of_a_txt(&export(&["--expand-limit", "0"]));
    assert_eq!(
        none,
        vec![
            Some((6, 1, None)),
            Some((13, 14, None)),
            Some((7, 34, None))
        ]
    );
    // A limit of 13: the two smaller places (6 + 7), not the larger (13).
    let some = gaps_of_a_txt(&export(&["--expand-limit", "13"]));
    assert_eq!(
        some,
        vec![
            Some((6, 1, Some(6))),
            Some((13, 14, None)),
            Some((7, 34, Some(7)))
        ]
    );
    // Not a number.
    let out = env.path("bad.html");
    let bad = env.run(
        &dir,
        &[
            "export",
            "-f",
            review.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--expand-limit",
            "many",
        ],
    );
    assert!(!bad.status.success());
}

fn git_sources(review: &Path) -> Vec<diffnote::model::GitSource> {
    bundle::load(review)
        .unwrap()
        .revisions()
        .filter_map(|r| match &r.source {
            diffnote::model::Source::Git(g) => Some(g.clone()),
            _ => None,
        })
        .collect()
}

fn commit_id(repo: &Path, rev: &str) -> String {
    git(repo, &["rev-parse", rev]).trim().to_string()
}

#[test]
fn init_in_a_git_repository_takes_a_commit_as_the_base_head_by_default() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    let said = env.ok(&repo, &["init", "-f", review_arg]);
    assert!(said.contains("HEAD") && said.contains("基準"), "{said}");
    let sources = git_sources(&review);
    assert_eq!(sources.len(), 1);
    let head = commit_id(&repo, "HEAD");
    assert_eq!(
        (sources[0].base.as_str(), sources[0].head.as_str()),
        (head.as_str(), head.as_str())
    );
    assert_eq!(sources[0].spec, "HEAD");
    // The commit's id is all it stores: git has the rest.
    assert_eq!(count_blobs(&review), 0);
    // It is an existing bundle now.
    let again = env.run(&repo, &["init", "-f", review_arg]);
    assert!(!again.status.success());
}

#[test]
fn init_takes_a_branch_a_tag_or_an_id_and_refuses_what_is_not_a_commit() {
    let env = Env::new();
    let repo = git_repo(&env);
    let base = commit_id(&repo, "c1");
    for (name, rev) in [("tag", "c1"), ("branch", "main"), ("id", base.as_str())] {
        let review = env.path(&format!("{name}.diffnote"));
        env.ok(&repo, &["init", "-f", review.to_str().unwrap(), rev]);
        let want = commit_id(&repo, rev);
        assert_eq!(git_sources(&review)[0].head, want, "{name}");
        assert_eq!(git_sources(&review)[0].spec, rev);
    }
    let review = env.path("bad.diffnote");
    let bad = env.run(
        &repo,
        &["init", "-f", review.to_str().unwrap(), "no-such-branch"],
    );
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("no-such-branch"));
    assert!(!review.exists(), "nothing is written for a bad ref");
}

#[test]
fn serve_after_a_git_init_reviews_what_changed_since_and_then_reopens_that() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(&repo, &["init", "-f", review_arg, "c2"]);
    // HEAD is c3: what has changed since c2 is reviewed, without being told.
    env.review(
        &repo,
        &review,
        &[],
        &[Note::Line("calc.txt", "d", "d を追加した理由は?")],
    );
    let sources = git_sources(&review);
    assert_eq!(sources.len(), 2, "the base, then what was reviewed since");
    assert_eq!(sources[1].base, commit_id(&repo, "c2"));
    assert_eq!(sources[1].head, commit_id(&repo, "c3"));
    let loaded = bundle::load(&review).unwrap();
    assert_eq!(comment_bodies(&loaded), ["d を追加した理由は?"]);
    // HEAD hasn't moved: the same revision again (to reply to what is there),
    // not a new one.
    env.review(&repo, &review, &[], &[]);
    assert_eq!(git_sources(&review).len(), 2);
    // A new commit: reviewed from where the last review stopped.
    std::fs::write(repo.join("calc.txt"), "a\nB\nc\nd\ne\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c4"]);
    env.review(&repo, &review, &[], &[Note::Line("calc.txt", "e", "e も")]);
    let sources = git_sources(&review);
    assert_eq!(sources.len(), 3);
    assert_eq!(sources[2].base, commit_id(&repo, "c2"), "the base stays");
    assert_eq!(sources[2].head, commit_id(&repo, "HEAD"));
}

#[test]
fn reopen_refuses_a_comparison_target_and_a_bundle_with_no_revision_yet() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    // No bundle at all yet: nothing to reopen.
    let said = env.serve_refused(&repo, &review, &["--reopen"]);
    assert!(said.contains("--reopen"), "{said}");
    assert!(!review.exists());
    env.ok(&repo, &["init", "-f", review_arg, "c2"]);
    // A comparison target makes no sense together with --reopen.
    let said = env.serve_refused(&repo, &review, &["--reopen", "c3"]);
    assert!(said.contains("--reopen"), "{said}");
}

#[test]
fn reopen_works_for_a_directory_review_too() {
    let env = Env::new();
    let dir = env.path("project");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(&dir, &["init", "-f", review_arg, "."]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    let threads = env.review(
        &dir,
        &review,
        &["."],
        &[Note::Line("a.txt", "two", "first")],
    );
    let revisions_before = bundle::load(&review).unwrap().revisions().count();
    // The directory changes again, but --reopen ignores it entirely.
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let served = env.serve(&dir, &review, &["--reopen"]);
    served.reply(&threads[0], "second");
    served.stop();
    let loaded = bundle::load(&review).unwrap();
    assert_eq!(loaded.revisions().count(), revisions_before);
    assert_eq!(comment_bodies(&loaded), ["first", "second"]);
}

#[test]
fn init_makes_a_file_review_outside_git_and_when_told_to_inside_it() {
    let env = Env::new();
    // Not in a repository: the directory, `.` by default.
    let dir = env.path("plain");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let review = env.path("plain.diffnote");
    env.ok(&dir, &["init", "-f", review.to_str().unwrap()]);
    assert_eq!(bundle::load(&review).unwrap().revisions().count(), 1);
    assert!(git_sources(&review).is_empty());
    // In a repository, `--files` takes the directory's files instead of a commit.
    let repo = git_repo(&env);
    let review = env.path("files.diffnote");
    let said = env.ok(&repo, &["init", "-f", review.to_str().unwrap(), "--files"]);
    assert!(said.contains("個のファイル"), "{said}");
    assert!(git_sources(&review).is_empty());
    assert!(count_blobs(&review) > 0);
}

#[test]
fn the_target_is_compared_with_the_base_and_ranges_are_not_accepted() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    // No bundle and no base: the commit's own changes, from its parent.
    env.review(
        &repo,
        &review,
        &["HEAD"],
        &[Note::Line("calc.txt", "d", "the last commit")],
    );
    let sources = git_sources(&review);
    assert_eq!(sources[0].base, commit_id(&repo, "c2"));
    assert_eq!(sources[0].head, commit_id(&repo, "c3"));
    // A range: not accepted, and nothing is written.
    let before = std::fs::read(&review).unwrap();
    assert!(
        env.serve_refused(&repo, &review, &["c1..c3"])
            .contains("範囲")
    );
    env.serve_refused(&repo, &review, &["c1", "c3"]);
    assert_eq!(
        std::fs::read(&review).unwrap(),
        before,
        "neither is written"
    );
    // The base is fixed by the bundle: the same one may be said again, another may not.
    assert!(
        env.serve_refused(&repo, &review, &["--base", "c1", "c3"])
            .contains("ベース")
    );
    env.review(&repo, &review, &["--base", "c2", "c3"], &[]);
    // Nothing named and no bundle: it says what to do.
    let none = env.path("none.diffnote");
    assert!(env.serve_refused(&repo, &none, &[]).contains("init"));
    assert!(!none.exists());
}

#[test]
fn a_base_makes_the_bundle_as_init_would() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    env.review(
        &repo,
        &review,
        &["--base", "c1", "c3"],
        &[Note::Line("calc.txt", "d", "since c1")],
    );
    let sources = git_sources(&review);
    assert_eq!(
        sources.len(),
        1,
        "the base is the revision's own base, not a separate one"
    );
    assert_eq!(sources[0].base, commit_id(&repo, "c1"));
    assert_eq!(sources[0].head, commit_id(&repo, "c3"));
    assert_eq!(sources[0].spec, "c1..c3");
    // The bundle's later revisions keep that base, and are named with it.
    std::fs::write(repo.join("calc.txt"), "a\nB\nc\nd\ne\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c4"]);
    env.review(
        &repo,
        &review,
        &[],
        &[Note::Line("calc.txt", "e", "and now")],
    );
    let sources = git_sources(&review);
    assert_eq!(sources.len(), 2);
    assert_eq!(
        (sources[1].base.as_str(), sources[1].spec.as_str()),
        (sources[0].base.as_str(), "c1..HEAD")
    );
}

/// Two directories, before and after.
fn two_directories(env: &Env) -> (PathBuf, PathBuf) {
    let (old, new) = (env.path("old"), env.path("new"));
    for d in [&old, &new] {
        std::fs::create_dir(d).unwrap();
    }
    std::fs::write(old.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(old.join("gone.txt"), "bye\n").unwrap();
    std::fs::write(new.join("a.txt"), "one\nTWO\n").unwrap();
    std::fs::write(new.join("added.txt"), "hello\n").unwrap();
    (old, new)
}

#[test]
fn a_base_directory_compares_two_directories_in_one_step() {
    let env = Env::new();
    let (old, new) = two_directories(&env);
    let review = env.path("review.diffnote");
    // No bundle: the base directory starts it, as `init OLD` would.
    env.review(
        &new,
        &review,
        &["--base", old.to_str().unwrap(), "."],
        &[Note::Line("a.txt", "TWO", "why?")],
    );
    let loaded = bundle::load(&review).unwrap();
    let revisions: Vec<_> = loaded.revisions().collect();
    assert_eq!(
        revisions.len(),
        2,
        "the base, then the directory compared with it"
    );
    assert_eq!(loaded.tree_of(revisions[0])["a.txt"], b"one\ntwo\n");
    assert!(loaded.tree_of(revisions[0]).contains_key("gone.txt"));
    let mut touched: Vec<_> = revisions[1]
        .files
        .iter()
        .flat_map(|f| [f.old_path.clone(), f.new_path.clone()])
        .flatten()
        .collect();
    touched.sort();
    touched.dedup();
    assert_eq!(touched, ["a.txt", "added.txt", "gone.txt"]);
    assert_eq!(comment_bodies(&loaded), ["why?"]);
    // The same base said again is fine; a different one is not.
    env.review(&new, &review, &["--base", old.to_str().unwrap(), "."], &[]);
    let before = std::fs::read(&review).unwrap();
    let said = env.serve_refused(&new, &review, &["--base", new.to_str().unwrap(), "."]);
    assert!(said.contains("ベース"), "{said}");
    assert_eq!(std::fs::read(&review).unwrap(), before);
}

#[test]
fn nothing_to_review_leaves_no_bundle_and_says_what_to_do() {
    let env = Env::new();
    let (old, _) = two_directories(&env);
    let review = env.path("review.diffnote");
    // Two equal directories: nothing to review, and the base alone is not kept.
    let same = env.path("same");
    std::fs::create_dir(&same).unwrap();
    std::fs::write(same.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(same.join("gone.txt"), "bye\n").unwrap();
    let said = env.serve_refused(&same, &review, &["--base", old.to_str().unwrap(), "."]);
    assert!(said.contains("差分がありません"), "{said}");
    assert!(!review.exists(), "the base alone is not kept");
    // A directory review with no base at all says what to do.
    let said = env.serve_refused(&same, &review, &["--files", "."]);
    assert!(said.contains("--base"), "{said}");
}

#[test]
fn a_directory_bundle_compares_every_session_with_the_first_snapshot() {
    let env = Env::new();
    let dir = env.path("project");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(&dir, &["init", "-f", review_arg]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    env.review(
        &dir,
        &review,
        &["."],
        &[Note::Line("a.txt", "two", "first")],
    );
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    // The second session's diff has both new lines: it starts from the base.
    env.review(
        &dir,
        &review,
        &["."],
        &[Note::Line("a.txt", "three", "second")],
    );
    let loaded = bundle::load(&review).unwrap();
    let latest = loaded.revisions().last().unwrap();
    let text = loaded.revision_diff(latest).unwrap();
    assert!(text.contains("+two") && text.contains("+three"), "{text}");
    // Back to what an earlier session saw: that diff is reopened, not a new one.
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    env.review(&dir, &review, &["."], &[]);
    assert_eq!(bundle::load(&review).unwrap().revisions().count(), 3);
}

#[test]
fn init_and_serve_work_on_the_repository_named_from_anywhere() {
    let env = Env::new();
    let repo = git_repo(&env);
    // Run from a directory that is not in a repository.
    let elsewhere = env.path("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let repo_arg = repo.to_str().unwrap();
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(
        &elsewhere,
        &["init", "-f", review_arg, "--repo", repo_arg, "c1"],
    );
    assert_eq!(git_sources(&review)[0].head, commit_id(&repo, "c1"));
    env.review(
        &elsewhere,
        &review,
        &["--repo", repo_arg, "c2"],
        &[Note::Line("calc.txt", "B", "from far away")],
    );
    let sources = git_sources(&review);
    assert_eq!(sources.len(), 2);
    assert_eq!(
        (sources[1].base.clone(), sources[1].head.clone()),
        (commit_id(&repo, "c1"), commit_id(&repo, "c2"))
    );
    assert_eq!(
        comment_bodies(&bundle::load(&review).unwrap()),
        ["from far away"]
    );
    // Without it, this directory is not a repository: nothing to compare with.
    env.serve_refused(&elsewhere, &review, &["c3"]);
    // A directory that is not a repository is refused, with its name.
    let plain = env.path("plain");
    std::fs::create_dir(&plain).unwrap();
    let said = env.serve_refused(
        &elsewhere,
        &review,
        &["--repo", plain.to_str().unwrap(), "c3"],
    );
    assert!(said.contains("git リポジトリではありません"), "{said}");
}

#[test]
fn a_revision_records_the_commits_it_was_reached_by_and_keeps_each_one_once() {
    let env = Env::new();
    let repo = repo_with_docs(&env);
    let review = env.path("r.diffnote");
    // Two more commits on top of c2, the second with a body.
    std::fs::write(repo.join("docs.md"), "changed\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c3"]);
    git(&repo, &["tag", "c3"]);
    std::fs::write(repo.join("docs.md"), "changed again\n").unwrap();
    git(
        &repo,
        &[
            "commit",
            "-q",
            "-am",
            "直した\n\nこうしたほうが読みやすいため。",
        ],
    );
    git(&repo, &["tag", "c4"]);

    // #1 is c1..c3, #2 is c1..c4: the trails overlap.
    env.review(
        &repo,
        &review,
        &["--base", "c1", "c3"],
        &[Note::Line("calc.txt", "C30", "ひとつめ")],
    );
    env.review(
        &repo,
        &review,
        &["HEAD"],
        &[Note::Line("docs.md", "changed again", "ふたつめ")],
    );

    let loaded = bundle::load(&review).unwrap();
    let trails: Vec<Vec<String>> = loaded.revisions().map(|r| r.commits.clone()).collect();
    assert_eq!(trails.len(), 2);
    assert_eq!(trails[0].len(), 2, "c2 and c3");
    assert_eq!(trails[1].len(), 3, "and c4 after them");
    assert_eq!(
        trails[1][..2],
        trails[0][..],
        "a trail starts at the review's base, so the second contains the first"
    );
    assert_eq!(
        loaded.commits.len(),
        3,
        "each commit is kept once, not once per trail that names it"
    );
    assert!(bundle_names(&review).contains(&"commits.json".to_string()));

    let last = &loaded.commits[trails[1].last().unwrap()];
    assert_eq!(last.subject, "直した");
    assert_eq!(last.body, "こうしたほうが読みやすいため。");
    assert_eq!(last.author, "T", "the name git records");
    assert_eq!(
        last.files
            .iter()
            .map(|f| (f.path.as_str(), f.status.as_str()))
            .collect::<Vec<_>>(),
        [("docs.md", "modified")]
    );
}

#[test]
fn a_review_of_a_directory_records_no_commits() {
    let env = Env::new();
    let dir = env.path("work");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let review = env.path("r.diffnote");
    let arg = review.to_str().unwrap();
    env.ok(&dir, &["init", "-f", arg, "--files", "."]);
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    env.review(
        &dir,
        &review,
        &["."],
        &[Note::Line("a.txt", "two", "変えました")],
    );
    let loaded = bundle::load(&review).unwrap();
    assert!(loaded.revisions().all(|r| r.commits.is_empty()));
    assert!(loaded.commits.is_empty());
    assert!(!bundle_names(&review).contains(&"commits.json".to_string()));
}

#[test]
fn the_exported_model_says_what_happened_to_the_review_in_order() {
    let env = Env::new();
    let repo = repo_with_docs(&env);
    let review = env.path("r.diffnote");
    let arg = review.to_str().unwrap();
    std::fs::write(repo.join("docs.md"), "changed\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c3"]);
    let served = env.serve(&repo, &review, &["--base", "c1", "HEAD"]);
    served.note(&Note::Global("全体として"));
    let thread = served.note(&Note::Line("docs.md", "changed", "ここは？"));
    served.resolve(&thread);
    served.stop();
    let html = env.path("out.html");
    env.ok(&repo, &["export", "-f", arg, "-o", html.to_str().unwrap()]);
    let model = model_of(&std::fs::read_to_string(&html).unwrap());
    let timeline = model["timeline"].as_array().unwrap();
    let kinds: Vec<&str> = timeline
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["started", "revision", "comment", "comment", "resolved"]
    );

    // The revision brought the commits of its trail, with what they said.
    let revision = &timeline[1];
    assert_eq!(revision["rev"], 0);
    let commits = revision["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 2, "c2 and c3, not the base itself");
    assert_eq!(commits[1]["subject"], "c3");
    assert_eq!(
        commits[1]["author"], "T",
        "the name git recorded, not the reviewer's"
    );
    assert_eq!(commits[1]["short"].as_str().unwrap().len(), 7);
    assert_eq!(commits[1]["files"][0]["path"], "docs.md");
    // A comment says which one it is, not what it says: the text is in the
    // threads, where the rest of the page reads it from.
    assert!(timeline[2]["comment"].is_string());
    assert!(timeline[2]["text"].is_null());
    assert!(
        !timeline[2]["author"].as_str().unwrap().is_empty(),
        "who wrote it"
    );
    assert_eq!(timeline[4]["thread"], timeline[3]["thread"]);
    // Every time is there for the page to put in the reader's own zone.
    for entry in timeline {
        assert!(entry["at"].as_str().unwrap().contains('T'), "{entry}");
    }
}

#[test]
fn a_review_of_a_directory_has_a_timeline_with_no_commits_in_it() {
    let env = Env::new();
    let dir = env.path("work");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let review = env.path("r.diffnote");
    let arg = review.to_str().unwrap();
    env.ok(&dir, &["init", "-f", arg, "--files", "."]);
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    env.review(
        &dir,
        &review,
        &["."],
        &[Note::Line("a.txt", "two", "変えました")],
    );
    let html = env.path("out.html");
    env.ok(&dir, &["export", "-f", arg, "-o", html.to_str().unwrap()]);
    let model = model_of(&std::fs::read_to_string(&html).unwrap());
    let timeline = model["timeline"].as_array().unwrap();
    assert!(timeline.iter().any(|e| e["kind"] == "revision"));
    assert!(
        timeline.iter().all(|e| e["commits"].is_null()),
        "nothing to say about commits there are none of"
    );
}

#[test]
fn the_snapshot_range_is_chosen_when_the_review_is_made_and_not_after() {
    let env = Env::new();
    let repo = repo_with_docs(&env);
    let review = env.path("r.diffnote");
    let arg = review.to_str().unwrap();
    env.ok(&repo, &["init", "-f", arg, "--snapshot", "full", "c1"]);
    assert_eq!(
        bundle::load(&review).unwrap().snapshot_mode(),
        Some(bundle::SnapshotMode::Full),
        "the review is made with it"
    );
    // What is recorded later keeps to it, without being asked again.
    env.review(
        &repo,
        &review,
        &["c2"],
        &[Note::Line("calc.txt", "C30", "ひとつ")],
    );
    let loaded = bundle::load(&review).unwrap();
    assert!(
        loaded
            .revisions()
            .all(|r| r.snapshot_mode == bundle::SnapshotMode::Full)
    );
    // (Revision 0 is `init`'s own, which records nothing; 1 is the first
    // with a diff.)
    assert!(
        manifest_paths(&review, 1).contains(&"logo.bin".to_string()),
        "a full snapshot keeps files the diff never touched: {:?}",
        manifest_paths(&review, 1)
    );

    // Asking an existing review for another range is refused, not ignored.
    let out = env.run(&repo, &["serve", "-f", arg, "--snapshot", "changed", "c2"]);
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("レビューを作るとき"), "{said}");
    assert!(
        said.contains("full"),
        "it says what the review already is: {said}"
    );
}

#[test]
fn a_directory_review_cannot_be_asked_for_a_changed_snapshot() {
    let env = Env::new();
    let dir = env.path("work");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let arg = env.path("r.diffnote");
    let out = env.run(
        &dir,
        &[
            "init",
            "-f",
            arg.to_str().unwrap(),
            "--files",
            "--snapshot",
            "changed",
            ".",
        ],
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(".diffnoteignore"),
        "it says what to do instead"
    );
    assert!(!arg.exists(), "and nothing was made");
}

#[test]
fn a_comment_on_a_whole_file_is_kept_with_the_file_and_placed_on_it() {
    // A thread about a file as a whole, followed into a later revision like
    // any other.
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    env.review(
        &repo,
        &review,
        &["--base", "c1", "c2"],
        &[Note::File("README.md", "全体の構成について")],
    );
    env.review(&repo, &review, &["c3"], &[]);
    let model = model_of(&exported(&env, &repo, &review));
    let id = thread_saying(&model, "全体の構成について")["id"]
        .as_str()
        .unwrap();
    for rev in model["revisions"].as_array().unwrap() {
        let placement = &rev["placements"][id];
        assert_eq!(placement["kind"], "file", "{placement}");
        assert_eq!(placement["file"], "README.md");
    }
}

#[test]
fn serve_takes_in_later_commits_when_asked_but_a_named_commit_does_not_move() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(&repo, &["init", "-f", review_arg, "c2"]);
    let served = env.serve(&repo, &review, &[]);
    assert!(
        served.said.contains("差分を記録しました"),
        "{}",
        served.said
    );
    assert_eq!(git_sources(&review).len(), 2);
    // A commit made while it runs: taken in once, when asked.
    std::fs::write(repo.join("calc.txt"), "a\nB\nc\nd\ne\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c4"]);
    assert_eq!(git_sources(&review).len(), 2, "not before it is asked for");
    assert!(served.refresh());
    let sources = git_sources(&review);
    assert_eq!(sources.len(), 3);
    assert_eq!(sources[2].head, commit_id(&repo, "HEAD"));
    assert!(!served.refresh(), "and only once");
    served.stop();

    // A commit named on the command line stays what is reviewed.
    let served = env.serve(&repo, &review, &["c3"]);
    std::fs::write(repo.join("calc.txt"), "x\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c5"]);
    assert!(!served.refresh());
    served.stop();
    assert_eq!(git_sources(&review).len(), 3);
}

#[test]
fn quitting_without_saving_takes_back_what_serve_added_and_a_bundle_it_made() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    env.ok(&repo, &["init", "-f", review.to_str().unwrap(), "c1"]);
    let before = std::fs::read(&review).unwrap();
    let served = env.serve(&repo, &review, &[]);
    served.note(&Note::Global("消える"));
    assert_ne!(std::fs::read(&review).unwrap(), before);
    served.discard();
    assert_eq!(
        std::fs::read(&review).unwrap(),
        before,
        "as if serve had not run"
    );

    let made = env.path("made.diffnote");
    let served = env.serve(&repo, &made, &["--base", "c1", "c2"]);
    assert!(made.exists());
    served.discard();
    assert!(!made.exists(), "a bundle serve made goes with it");
}

#[test]
fn serve_says_when_there_is_nothing_yet_and_takes_a_directory_only_when_named() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    env.ok(&repo, &["init", "-f", review.to_str().unwrap(), "HEAD"]);
    let served = env.serve(&repo, &review, &[]);
    assert!(
        served.said.contains("差分がまだありません"),
        "{}",
        served.said
    );
    served.stop();
    assert_eq!(git_sources(&review).len(), 1, "nothing was added");

    let dir = env.path("project");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let review = env.path("dir.diffnote");
    env.ok(&dir, &["init", "-f", review.to_str().unwrap()]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    // `.` may be anywhere: with no directory named, none is taken in.
    env.serve(&dir, &review, &[]).stop();
    assert_eq!(bundle::load(&review).unwrap().revisions().count(), 1);
    let served = env.serve(&dir, &review, &["."]);
    assert!(
        served.said.contains("ディレクトリの変更を記録しました"),
        "{}",
        served.said
    );
    served.stop();
    assert_eq!(bundle::load(&review).unwrap().revisions().count(), 2);
}

#[test]
fn reopen_adds_to_the_last_revision_and_never_looks_at_a_later_commit() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    env.ok(&repo, &["init", "-f", review.to_str().unwrap(), "c2"]);
    let threads = env.review(&repo, &review, &[], &[Note::Line("calc.txt", "d", "d は?")]);
    std::fs::write(repo.join("calc.txt"), "x\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c4"]);
    let served = env.serve(&repo, &review, &["--reopen"]);
    assert!(
        !served.said.contains("差分を記録しました"),
        "{}",
        served.said
    );
    served.reply(&threads[0], "了解です");
    let model = served.api("/api/model", None);
    assert!(
        model["model"].get("refreshable").is_none(),
        "nothing to take in, so no button for it"
    );
    served.stop();
    let loaded = bundle::load(&review).unwrap();
    assert_eq!(loaded.revisions().count(), 2);
    assert_eq!(comment_bodies(&loaded), ["d は?", "了解です"]);
}
