//! End-to-end tests: the real `diffnote` binary, driven through a fake
//! `$EDITOR` (a shell script that inserts a comment after a given diff line).
//! Unix only, since the fake editor is `sh`/`awk`.
#![cfg(unix)]

use diffnote::bundle;
use diffnote::model::Event;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_diffnote");

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

/// A fake editor: for each `AFTER<TAB>TEXT` line in `$DN_SCRIPT` (a file),
/// inserts `> TEXT` after the first buffer line equal to `AFTER`. An `AFTER`
/// of `GLOBAL` puts the comment on top instead.
fn fake_editor(dir: &Path) -> PathBuf {
    let path = dir.join("editor.sh");
    std::fs::write(
        &path,
        r#"#!/bin/sh
buf="$1"
out="$buf.new"
cp "$buf" "$out"
while IFS="$(printf '\t')" read -r after text; do
  [ -z "$after" ] && continue
  if [ "$after" = "GLOBAL" ]; then
    { printf '> %s\n\n' "$text"; cat "$out"; } > "$out.2" && mv "$out.2" "$out"
  else
    awk -v after="$after" -v text="$text" '{print} !done && $0 == after {print "> " text; done=1}' "$out" > "$out.2" && mv "$out.2" "$out"
  fi
done < "$DN_SCRIPT"
mv "$out" "$buf"
"#,
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

struct Env {
    dir: tempfile::TempDir,
    editor: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let editor = fake_editor(dir.path());
        Env { dir, editor }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Runs `diffnote args...` in `cwd`; the fake editor applies `comments`
    /// (`(after-line, text)` pairs).
    fn run(&self, cwd: &Path, comments: &[(&str, &str)], args: &[&str]) -> Output {
        let script = self.path("comments.tsv");
        let body: String = comments
            .iter()
            .map(|(a, t)| format!("{a}\t{t}\n"))
            .collect();
        std::fs::write(&script, body).unwrap();
        Command::new(BIN)
            .current_dir(cwd)
            .env("EDITOR", &self.editor)
            .env("DN_SCRIPT", &script)
            .args(args)
            .output()
            .expect("diffnote runs")
    }

    fn ok(&self, cwd: &Path, comments: &[(&str, &str)], args: &[&str]) -> String {
        let out = self.run(cwd, comments, args);
        assert!(
            out.status.success(),
            "diffnote {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
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

    let out = env.ok(&dir, &[], &["init", "-f", review_arg, "."]);
    assert!(out.contains("2 個のファイルを"), "{out}");
    assert_eq!(count_blobs(&review), 2);

    // Session 1: a.txt changes.
    std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    let out = env.ok(
        &dir,
        &[("+TWO", "why uppercase?"), ("GLOBAL", "overall remark")],
        &["edit", "-f", review_arg, "."],
    );
    assert!(out.contains("コメント 2 件"), "{out}");

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
    let mut tree: Vec<_> = loaded.manifest(latest).into_iter().map(|f| f.path).collect();
    tree.sort();
    assert_eq!(tree, ["a.txt", "b.txt"]);
    let read = loaded.tree_of(latest);
    assert_eq!(read["a.txt"], b"one\nTWO\nthree\n");
    assert_eq!(read["b.txt"], b"x\n");
    // b.txt is unchanged, so it is stored once: 2 (init) + 1 new a.txt.
    assert_eq!(count_blobs(&review), 3);

    // Session 2: only b.txt changes. The comparison base must be session 1's
    // tree (read back from the bundle), not the original.
    std::fs::write(dir.join("b.txt"), "x\ny\n").unwrap();
    let out = env.ok(
        &dir,
        &[("+y", "new line")],
        &["edit", "-f", review_arg, "."],
    );
    assert!(out.contains("コメント 1 件"), "{out}");
    let loaded = bundle::load(&review).unwrap();
    let revisions: Vec<_> = loaded.revisions().collect();
    assert_eq!(revisions.len(), 3);
    let touched: Vec<_> = revisions[2]
        .files
        .iter()
        .filter_map(|f| f.new_path.as_deref())
        .collect();
    assert_eq!(touched, ["b.txt"], "a.txt did not change since session 1");
    assert_eq!(
        comment_bodies(&loaded),
        ["overall remark", "why uppercase?", "new line"]
    );

    // Export: one view per revision that has a diff, with every comment.
    let html_path = env.path("out.html");
    env.ok(&dir, &[], &["export", "-f", review_arg, "-o", html_path.to_str().unwrap()]);
    let html = std::fs::read_to_string(&html_path).unwrap();
    assert_eq!(html.matches(r#"<section class="diffnote-revision"#).count(), 2);
    for body in ["why uppercase?", "overall remark", "new line"] {
        assert_eq!(html.matches(body).count(), 2, "{body} once per view");
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
    env.ok(&dir, &[], &["init", "-f", review_arg, "."]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    env.ok(&dir, &[("+two", "first")], &["edit", "-f", review_arg, "."]);
    let blobs_before = count_blobs(&review);

    // Nothing changed since: the diff of the last revision comes back, so a
    // reply-like comment can still be added, and nothing else is recorded.
    let out = env.ok(&dir, &[("+two", "second")], &["edit", "-f", review_arg, "."]);
    assert!(out.contains("コメント 1 件"), "{out}");
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
    let review_arg = review.to_str().unwrap();

    // Session 1 (c1..c2): a comment on the README, which that diff touches.
    env.ok(
        &repo,
        &[("+A calculator.", "more detail please"), ("+B", "why B?")],
        &["edit", "-f", review_arg, "--snapshot", "changed", "c1..c2"],
    );
    let loaded = bundle::load(&review).unwrap();
    let first = loaded.revisions().next().unwrap();
    assert_eq!(
        loaded.manifest(first).len(),
        2,
        "calc.txt and README.md, both touched"
    );

    // Session 2 (c2..c3) doesn't touch README.md at all, but the thread on
    // it must stay placeable, so its content is kept for this revision too.
    env.ok(
        &repo,
        &[("+d", "new line")],
        &["edit", "-f", review_arg, "c2..c3"],
    );
    let loaded = bundle::load(&review).unwrap();
    let second = loaded.revisions().nth(1).unwrap();
    assert_eq!(second.snapshot_mode, bundle::SnapshotMode::Changed, "inherited");
    let touched: Vec<_> = second
        .files
        .iter()
        .filter_map(|f| f.new_path.as_deref())
        .collect();
    assert_eq!(touched, ["calc.txt"]);
    let mut manifest: Vec<_> = loaded.manifest(second).into_iter().map(|f| f.path).collect();
    manifest.sort();
    assert_eq!(manifest, ["README.md", "calc.txt"]);
    let tree = loaded.tree_of(second);
    assert_eq!(tree["README.md"], b"# calc\n\nA calculator.\n");
    assert_eq!(tree["calc.txt"], b"a\nB\nc\nd\n");
    // calc.txt c1, c2, c3; README c1, c2 (c3 is the same as c2): five blobs.
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
        env.ok(
            &repo,
            &[("+d", "hello")],
            &["edit", "-f", review.to_str().unwrap(), "--snapshot", mode, "c2..c3"],
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
fn an_edit_that_adds_nothing_leaves_no_bundle_behind() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let out = env.ok(
        &repo,
        &[],
        &["edit", "-f", review.to_str().unwrap(), "c1..c2"],
    );
    assert!(out.contains("コメントは追加されませんでした"), "{out}");
    assert!(!review.exists());
}

#[test]
fn a_bad_range_fails_without_touching_the_bundle() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let out = env.run(
        &repo,
        &[("+d", "x")],
        &["edit", "-f", review.to_str().unwrap(), "no-such-rev"],
    );
    assert!(!out.status.success());
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

    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();

    // Session 1: a thread on the README (which c1..c2 changes).
    env.ok(
        &repo,
        &[("+line 3 (edited)", "why edit this?")],
        &["edit", "-f", review_arg, "--snapshot", "changed", "c1..c2"],
    );

    // Session 2 (c2..c3) leaves README.md alone. Its thread is still shown,
    // in a block of context, and a new comment can be written right there.
    let out = env.ok(
        &repo,
        &[(" line 6", "and what about this line?")],
        &["edit", "-f", review_arg, "c2..c3"],
    );
    assert!(out.contains("コメント 1 件"), "{out}");

    let loaded = bundle::load(&review).unwrap();
    assert_eq!(comment_bodies(&loaded), ["why edit this?", "and what about this line?"]);
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
    assert!(loaded.manifest(second).iter().any(|f| f.path == "README.md"));
    assert!(loaded.blob(&h.digest).is_some());

    // The export shows both README threads in the second revision's view,
    // each with its context, and nothing is unplaced.
    let html_path = env.path("out.html");
    env.ok(&repo, &[], &["export", "-f", review_arg, "-o", html_path.to_str().unwrap()]);
    let html = std::fs::read_to_string(&html_path).unwrap();
    assert!(!html.contains(r#"<section class="diffnote-outdated">"#));
    let second_view = &html[html.find(r#"id="rev-1""#).unwrap()..];
    assert!(second_view.contains(r#"id="r1-file-README-md""#));
    for body in ["why edit this?", "and what about this line?"] {
        assert!(second_view.contains(body), "{body}");
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
    env.ok(
        &repo,
        &[("+d", "new line")],
        &["edit", "-f", review.to_str().unwrap(), "c2..c3"],
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
    env.ok(
        &repo,
        &[("+B", "why B?")],
        &["edit", "-f", review.to_str().unwrap(), "c1..c2"],
    );
    let loaded = bundle::load(&review).unwrap();
    let rev = loaded.revisions().next().unwrap();
    let diffnote::model::Source::Git(g) = &rev.source else {
        panic!("a git review");
    };
    assert_eq!(g.base, git(&repo, &["rev-parse", "c1"]));
    assert_eq!(g.head, git(&repo, &["rev-parse", "c2"]));
    assert_eq!(g.spec, "c1..c2");
    assert!(g.base.len() == 40 && g.head.len() == 40, "full ids, not names");
}

#[test]
fn the_mode_of_a_bundles_first_revision_carries_on_and_an_explicit_one_wins() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    // Ask for the whole tree once...
    env.ok(
        &repo,
        &[("+B", "one")],
        &["edit", "-f", review_arg, "--snapshot", "full", "c1..c2"],
    );
    // ...and the next revision, with no flag, does the same.
    env.ok(&repo, &[("+d", "two")], &["edit", "-f", review_arg, "c2..c3"]);
    assert_eq!(
        modes(&review),
        [bundle::SnapshotMode::Full, bundle::SnapshotMode::Full]
    );
    assert_eq!(manifest_paths(&review, 1), ["README.md", "calc.txt"]);

    // An explicit request beats what the bundle started with.
    std::fs::write(repo.join("calc.txt"), "a\nB\nc\nd\ne\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "c4"]);
    env.ok(
        &repo,
        &[("+e", "three")],
        &["edit", "-f", review_arg, "--snapshot", "changed", "c3..HEAD"],
    );
    assert_eq!(modes(&review)[2], bundle::SnapshotMode::Changed);
    assert_eq!(manifest_paths(&review, 2), ["calc.txt"]);
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
    env.ok(&dir, &[], &["init", "-f", review_arg, "."]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();

    // `changed` can't work here, so it is an error, not something quietly
    // ignored -- and nothing is opened or written.
    let before = std::fs::read(&review).unwrap();
    let out = env.run(
        &dir,
        &[("+two", "hello")],
        &["edit", "-f", review_arg, "--snapshot", "changed", "."],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--snapshot changed"), "{err}");
    assert!(err.contains(".diffnoteignore"), "{err}");
    assert_eq!(std::fs::read(&review).unwrap(), before, "the bundle is untouched");
    assert!(!env.path("review.diffnote.draft").exists());

    // `full` is what it does anyway, so asking for it is fine, as is not asking.
    env.ok(
        &dir,
        &[("+two", "hello")],
        &["edit", "-f", review_arg, "--snapshot", "full", "."],
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

fn span_of(loaded: &bundle::Loaded, nth: usize) -> (String, u32, u32, String) {
    let anchors: Vec<_> = loaded
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Comment {
                anchor: Some(diffnote::model::Anchor::Span { head: Some(h), .. }),
                ..
            } => Some(h),
            _ => None,
        })
        .collect();
    let h = anchors[nth];
    (h.file.clone(), h.start, h.len, h.digest.clone())
}

#[test]
fn show_puts_lines_of_an_untouched_file_in_the_buffer_and_comments_on_them_are_recorded() {
    let env = Env::new();
    let repo = repo_with_docs(&env);
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    let docs: String = (1..=20).map(|n| format!("line {n}\n")).collect();

    let out = env.ok(
        &repo,
        &[(" line 10", "what does this mean?")],
        &["edit", "-f", review_arg, "--show", "docs.md:9-11", "c1..c2"],
    );
    assert!(out.contains("コメント 1 件"), "{out}");

    let loaded = bundle::load(&review).unwrap();
    let (file, start, len, digest) = span_of(&loaded, 0);
    assert_eq!((file.as_str(), start, len), ("docs.md", 10, 1));
    assert_eq!(digest, diffnote::digest::digest(&docs));
    // The file's version is kept, though the diff never touches it.
    assert_eq!(manifest_paths(&review, 0), ["calc.txt", "docs.md"]);
    assert!(loaded.blob(&digest).is_some());
}

#[test]
fn show_gives_three_lines_of_context_around_the_range_and_no_more() {
    let env = Env::new();
    let repo = repo_with_docs(&env);
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    // `line 6` is 3 lines before line 9, so it is in the buffer; `line 5` is
    // not. The same at the other end (line 14 yes, 15 no).
    for (shown, hidden) in [(" line 6", " line 5"), (" line 14", " line 15")] {
        let _ = std::fs::remove_file(&review);
        // A comment on a line that must be there succeeds; on one that must
        // not be there, the editor's insertion finds nothing and adds nothing.
        let out = env.ok(
            &repo,
            &[(hidden, "nowhere to put this")],
            &["edit", "-f", review_arg, "--show", "docs.md:9-11", "c1..c2"],
        );
        assert!(out.contains("コメントは追加されませんでした"), "{hidden}: {out}");
        let out = env.ok(
            &repo,
            &[(shown, "this is in the buffer")],
            &["edit", "-f", review_arg, "--show", "docs.md:9-11", "c1..c2"],
        );
        assert!(out.contains("コメント 1 件"), "{shown}: {out}");
    }
}

#[test]
fn show_of_a_whole_file_and_of_lines_in_a_file_the_diff_touches() {
    let env = Env::new();
    let repo = repo_with_docs(&env);
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(
        &repo,
        &[
            (" line 20", "last line of the docs"),
            (" c2", "far above the change in calc.txt"),
        ],
        &["edit", "-f", review_arg, "--show", "docs.md", "--show", "calc.txt:1-4", "c1..c2"],
    );
    let loaded = bundle::load(&review).unwrap();
    // Comments are recorded in buffer order: the diff's own file first.
    let second = span_of(&loaded, 0);
    let first = span_of(&loaded, 1);
    assert_eq!((first.0.as_str(), first.1), ("docs.md", 20));
    // calc.txt has changed: its comment is in the *new* version (c2's).
    let calc_new: String = (1..=29).map(|n| format!("c{n}\n")).collect::<String>() + "C30\n";
    assert_eq!((second.0.as_str(), second.1), ("calc.txt", 2));
    assert_eq!(second.3, diffnote::digest::digest(&calc_new));
}

#[test]
fn show_combines_with_existing_threads_and_asks_nothing_of_them() {
    let env = Env::new();
    let repo = repo_with_docs(&env);
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(
        &repo,
        &[(" line 3", "first, via show")],
        &["edit", "-f", review_arg, "--show", "docs.md:2-4", "c1..c2"],
    );
    // Later: no --show at all, and the thread on docs.md is still in the
    // buffer (the earlier comment makes the file referenced), so a reply works.
    let out = env.run(&repo, &[], &["edit", "-f", review_arg, "c1..c2"]);
    assert!(out.status.success());
    let loaded = bundle::load(&review).unwrap();
    assert_eq!(comment_bodies(&loaded), ["first, via show"]);
}

#[test]
fn show_of_something_that_cannot_be_shown_fails_before_anything_is_written() {
    let env = Env::new();
    let repo = repo_with_docs(&env);
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    for (spec, message) in [
        ("nope.md", "そのファイルはありません"),
        ("docs.md:25", "末尾を過ぎています"),
        ("docs.md:0", "1 から"),
        ("docs.md:9-3", "終わりが始まりより前"),
        ("../elsewhere", "ツリー内のパスではありません"),
        ("logo.bin", "テキストファイルではありません"),
        ("", "ファイル名がありません"),
    ] {
        let out = env.run(
            &repo,
            &[(" line 3", "never written")],
            &["edit", "-f", review_arg, "--show", spec, "c1..c2"],
        );
        assert!(!out.status.success(), "{spec:?} should fail");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--show"), "{spec:?}: {err}");
        assert!(err.contains(message), "{spec:?}: {err}");
        assert!(!review.exists(), "{spec:?} left a bundle behind");
    }
}

#[test]
fn show_of_a_file_the_diff_deletes_says_so() {
    let env = Env::new();
    let repo = repo_with_docs(&env);
    git(&repo, &["rm", "-q", "docs.md"]);
    git(&repo, &["commit", "-q", "-m", "c3"]);
    git(&repo, &["tag", "c3"]);
    let review = env.path("review.diffnote");
    let out = env.run(
        &repo,
        &[],
        &["edit", "-f", review.to_str().unwrap(), "--show", "docs.md", "c2..c3"],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("削除されている"), "{err}");
}

#[test]
fn show_works_for_a_directory_review_too() {
    let env = Env::new();
    let dir = env.path("project");
    std::fs::create_dir(&dir).unwrap();
    let notes: String = (1..=12).map(|n| format!("note {n}\n")).collect();
    std::fs::write(dir.join("notes.md"), &notes).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let review = env.path("review.diffnote");
    let review_arg = review.to_str().unwrap();
    env.ok(&dir, &[], &["init", "-f", review_arg, "."]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    env.ok(
        &dir,
        &[(" note 6", "about note 6")],
        &["edit", "-f", review_arg, "--show", "notes.md:5-7", "."],
    );
    let loaded = bundle::load(&review).unwrap();
    let (file, start, _, digest) = span_of(&loaded, 0);
    assert_eq!((file.as_str(), start), ("notes.md", 6));
    assert_eq!(digest, diffnote::digest::digest(&notes));
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
    env.ok(&repo, &[("+mul", "why mul?")], &["edit", "-f", review_arg, "c1..c2"]);
    // The review is extended to c1..c3 from the same base: a comment on the
    // new header makes this a recorded revision too.
    env.ok(&repo, &[("+header", "why a header?")], &["edit", "-f", review_arg, "c1..c3"]);

    let html_path = env.path("out.html");
    env.ok(&repo, &[], &["export", "-f", review_arg, "-o", html_path.to_str().unwrap()]);
    let html = std::fs::read_to_string(&html_path).unwrap();
    let second_view = &html[html.find(r#"id="rev-1""#).unwrap()..];
    let (before, _) = second_view.split_once("why mul?").expect("the mul thread is in the view");
    let summary = &before[before.rfind("<summary>").unwrap()..];
    // Placed at its line in this view (the header pushed it from 3 to 4), not
    // as a line that was deleted or is not there.
    assert!(summary.contains("(L4)"), "{summary}");
    assert!(!summary.contains("削除された行"), "{summary}");
    assert!(!summary.contains("まだない行") && !summary.contains("この版にない行"), "{summary}");
}

fn titles(review: &Path) -> Vec<String> {
    bundle::load(review)
        .unwrap()
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Title { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect()
}

fn exported(env: &Env, repo: &Path, review: &Path) -> String {
    let html = env.path("t.html");
    env.ok(
        repo,
        &[],
        &["export", "-f", review.to_str().unwrap(), "-o", html.to_str().unwrap()],
    );
    std::fs::read_to_string(html).unwrap()
}

#[test]
fn a_title_given_with_the_first_comment_names_the_export_and_show() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let arg = review.to_str().unwrap();
    let out = env.ok(
        &repo,
        &[("+B", "why?")],
        &["edit", "-f", arg, "--title", "ログイン改修 <v2>", "c1..c2"],
    );
    assert!(out.contains("タイトルを設定しました"), "{out}");
    assert_eq!(titles(&review), ["ログイン改修 <v2>"]);
    let shown = env.ok(&repo, &[], &["show", "-f", arg]);
    assert!(shown.contains("[タイトル] ログイン改修 <v2>"), "{shown}");
    let html = exported(&env, &repo, &review);
    assert!(html.contains("<h1>ログイン改修 &lt;v2&gt;</h1>"), "{html}");
    assert!(html.contains("<title>ログイン改修 &lt;v2&gt;</title>"));
}

#[test]
fn without_a_title_the_export_keeps_the_default_heading() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    env.ok(&repo, &[("+B", "why?")], &["edit", "-f", review.to_str().unwrap(), "c1..c2"]);
    assert!(titles(&review).is_empty());
    assert!(exported(&env, &repo, &review).contains("<h1>diffnote レビュー</h1>"));
}

#[test]
fn a_title_can_be_changed_kept_and_cleared_later() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let arg = review.to_str().unwrap();
    env.ok(&repo, &[("+B", "why?")], &["edit", "-f", arg, "--title", "first", "c1..c2"]);
    // Changed by a session that also comments.
    env.ok(&repo, &[(" a", "and here")], &["edit", "-f", arg, "--title", "second", "c1..c2"]);
    assert_eq!(titles(&review), ["first", "second"]);
    // The same title again records nothing new.
    let out = env.ok(&repo, &[(" c", "more")], &["edit", "-f", arg, "--title", "second", "c1..c2"]);
    assert!(!out.contains("タイトルを設定しました"), "{out}");
    assert_eq!(titles(&review), ["first", "second"]);
    assert!(exported(&env, &repo, &review).contains("<h1>second</h1>"));
    // An empty title takes it away.
    env.ok(&repo, &[], &["edit", "-f", arg, "--title", "", "c1..c2"]);
    assert_eq!(titles(&review), ["first", "second", ""]);
    assert!(exported(&env, &repo, &review).contains("<h1>diffnote レビュー</h1>"));
}

#[test]
fn a_title_alone_is_enough_to_save_a_session() {
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let arg = review.to_str().unwrap();
    // No comment, but a title: the review is created with it.
    let out = env.ok(&repo, &[], &["edit", "-f", arg, "--title", "only a title", "c1..c2"]);
    assert!(out.contains("タイトルを設定しました"), "{out}");
    assert_eq!(titles(&review), ["only a title"]);
    // ...and it can be changed on its own too.
    env.ok(&repo, &[], &["edit", "-f", arg, "--title", "renamed", "c1..c2"]);
    assert_eq!(titles(&review), ["only a title", "renamed"]);
    // Without a title and without comments nothing is saved, as before.
    let before = std::fs::read(&review).unwrap();
    env.ok(&repo, &[], &["edit", "-f", arg, "c1..c2"]);
    assert_eq!(std::fs::read(&review).unwrap(), before);
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
        &[],
        &["init", "-f", review.to_str().unwrap(), "--title", "設計レビュー"],
    );
    assert_eq!(titles(&review), ["設計レビュー"]);
    assert!(exported_dir(&env, &dir, &review).contains("<h1>設計レビュー</h1>"));
}

/// Like `exported`, for a review with no comment yet: give it one first.
fn exported_dir(env: &Env, dir: &Path, review: &Path) -> String {
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    env.ok(dir, &[("+two", "changed")], &["edit", "-f", review.to_str().unwrap()]);
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
    env.ok(&repo, &[("+B", "why?")], &["edit", "-f", review.to_str().unwrap(), "c1..c2"]);
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
    env.ok(&repo, &[("+B", "why?")], &["edit", "-f", review.to_str().unwrap(), "c1..c2"]);
    assert_eq!(authors(&review), ["taro@example.com"]);
}

#[test]
fn author_overrides_git_and_applies_to_every_event_of_the_session() {
    let env = Env::new();
    let repo = git_repo(&env);
    git(&repo, &["config", "user.name", "山田 太郎"]);
    let review = env.path("review.diffnote");
    let arg = review.to_str().unwrap();
    env.ok(&repo, &[("+B", "why?")], &["edit", "-f", arg, "--author", "レビュアーA", "c1..c2"]);
    env.ok(&repo, &[(" a", "and here")], &["edit", "-f", arg, "c1..c2"]);
    // The override is per session, not remembered.
    assert_eq!(authors(&review), ["レビュアーA", "山田 太郎"]);
    let html = exported(&env, &repo, &review);
    assert!(html.contains("レビュアーA") && html.contains("山田 太郎"));
}

#[test]
fn a_blank_author_is_ignored() {
    let env = Env::new();
    let repo = git_repo(&env);
    git(&repo, &["config", "user.name", "山田 太郎"]);
    let review = env.path("review.diffnote");
    env.ok(&repo, &[("+B", "why?")], &["edit", "-f", review.to_str().unwrap(), "--author", "  ", "c1..c2"]);
    assert_eq!(authors(&review), ["山田 太郎"]);
}

#[test]
fn init_takes_an_author_for_its_title() {
    let env = Env::new();
    let dir = env.path("proj");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    let review = env.path("d.diffnote");
    env.ok(
        &dir,
        &[],
        &["init", "-f", review.to_str().unwrap(), "--title", "T", "--author", "作成者"],
    );
    let loaded = bundle::load(&review).unwrap();
    let author = loaded.events.iter().find_map(|e| match e {
        Event::Title { author, .. } => Some(author.as_str()),
        _ => None,
    });
    assert_eq!(author, Some("作成者"));
}

#[test]
fn a_reader_that_has_gone_is_not_an_error() {
    // `diffnote show | head` style: nobody reads stdout any more. It should
    // stop quietly rather than panic on the broken pipe.
    let env = Env::new();
    let repo = git_repo(&env);
    let review = env.path("review.diffnote");
    let arg = review.to_str().unwrap();
    env.ok(&repo, &[("+B", "why?")], &["edit", "-f", arg, "c1..c2"]);
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = Command::new(BIN)
        .current_dir(&repo)
        .args(["show", "-f", arg])
        .stdout(writer)
        .output()
        .expect("diffnote runs");
    assert!(out.status.success(), "{:?}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked") && err.is_empty(), "{err}");
}
