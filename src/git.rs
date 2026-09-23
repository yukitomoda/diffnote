//! The git provider: turns a commit-range argument list into a diff plus
//! the committed content behind it.
//!
//! Only *committed* content is ever reviewed -- the working tree and index
//! are deliberately ignored, so a review made from a range means the same
//! thing to everyone who opens the resulting bundle. Revision syntax is
//! never parsed here: the arguments are handed to `git rev-parse`, which
//! resolves anything git itself accepts (`HEAD~4^2`, `main@{yesterday}`,
//! `A...B`, ...), and only its resolved output is interpreted.

use crate::messages::{m, mf};
use crate::model::{CommitFile, CommitInfo, GitSource};
use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// One entry of a commit's tree (blobs only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: String,
    pub oid: String,
    pub size: u64,
}

/// A git repository, addressed by any directory inside it. All commands
/// run as `git -C <dir>`, so nothing depends on the process's cwd.
#[derive(Debug, Clone)]
pub struct Repo {
    dir: PathBuf,
}

fn base_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    // Keep output stable regardless of the user's config: no quoting of
    // non-ASCII paths, no colors, no external diff drivers.
    cmd.args(["-c", "core.quotepath=false", "-c", "color.ui=never"]);
    cmd
}

fn run(mut cmd: Command) -> Result<Vec<u8>> {
    let output = cmd.output().context(m("git.exec_failed"))?;
    if !output.status.success() {
        let first_line = String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or(m("git.no_output"))
            .to_string();
        bail!(mf("git.failed", &[("message", &first_line)]));
    }
    Ok(output.stdout)
}

fn run_text(cmd: Command) -> Result<String> {
    String::from_utf8(run(cmd)?).context(m("git.output_not_utf8"))
}

fn is_object_id(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Repo {
    /// The repository containing the current directory (not verified until
    /// a command runs).
    pub fn current() -> Self {
        Self::at(".")
    }

    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn git(&self) -> Command {
        base_command(&self.dir)
    }

    /// The changes one commit made: from its first parent (the empty tree for a
    /// root commit) to it.
    pub fn commit_range(&self, target: &str) -> Result<GitSource> {
        let head = self.commit_id(target)?;
        Ok(GitSource {
            base: self.parent_or_empty_tree(&head)?,
            head,
            spec: target.to_string(),
        })
    }

    /// The changes from the commit `base` names to the one `target` names.
    pub fn between(&self, base: &str, target: &str) -> Result<GitSource> {
        Ok(GitSource {
            base: self.commit_id(base)?,
            head: self.commit_id(target)?,
            spec: format!("{base}..{target}"),
        })
    }

    /// The first parent of `commit`, or the empty tree for a root commit.
    fn parent_or_empty_tree(&self, commit: &str) -> Result<String> {
        let mut cmd = self.git();
        cmd.args(["rev-parse", "--verify", "--quiet"])
            .arg(format!("{commit}^"));
        match cmd.output() {
            Ok(o) if o.status.success() => {
                Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
            }
            Ok(_) => self.empty_tree(),
            Err(e) => Err(e).context(m("git.exec_failed_short")),
        }
    }

    /// The empty tree's id (differs between sha1 and sha256 repositories).
    fn empty_tree(&self) -> Result<String> {
        let mut child = self
            .git()
            .args(["hash-object", "-t", "tree", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context(m("git.exec_failed_short"))?;
        drop(child.stdin.take());
        let mut out = String::new();
        child.stdout.take().unwrap().read_to_string(&mut out)?;
        child.wait()?;
        Ok(out.trim().to_string())
    }

    /// The full id of the commit `rev` names (`HEAD`, a branch, a tag, an id).
    pub fn commit_id(&self, rev: &str) -> Result<String> {
        if rev.starts_with('-') {
            bail!(mf("git.bad_rev_flag", &[("rev", rev)]));
        }
        let mut cmd = self.git();
        cmd.args(["rev-parse", "--verify", "--quiet"])
            .arg(format!("{rev}^{{commit}}"));
        let out = run_text(cmd)
            .map_err(|_| anyhow::anyhow!(mf("git.commit_not_found", &[("rev", rev)])))?;
        let id = out.trim().to_string();
        if !is_object_id(&id) {
            bail!(mf("git.commit_unresolved", &[("rev", rev)]));
        }
        Ok(id)
    }

    /// The unified diff from `range.base` to `range.head`.
    pub fn diff(&self, range: &GitSource) -> Result<String> {
        let mut cmd = self.git();
        cmd.args(["diff", "--no-ext-diff", "--no-textconv"])
            .arg(&range.base)
            .arg(&range.head);
        run_text(cmd)
    }

    /// The commits from `base` to `head`, oldest first, with what each one
    /// says and what it touched. Read once, when a revision is recorded: the
    /// review keeps the answer (see [`crate::model::CommitInfo`]).
    ///
    /// Two passes of `git log`, because one format cannot carry both a
    /// message and a file list without them running into each other. Each
    /// asks for records separated by NUL, so nothing in a message or a path
    /// can be mistaken for a separator.
    pub fn log(&self, base: &str, head: &str) -> Result<Vec<(String, CommitInfo)>> {
        let range = format!("{base}..{head}");
        let mut said = self.git();
        said.args([
            "log",
            "--reverse",
            "-z",
            "--format=%H\x1f%an\x1f%aI\x1f%s\x1f%b",
        ])
        .arg(&range);
        let mut out: Vec<(String, CommitInfo)> = Vec::new();
        for record in run_text(said)?.split('\0').filter(|r| !r.is_empty()) {
            let mut field = record.split('\x1f');
            let (Some(id), Some(author), Some(at), Some(subject)) =
                (field.next(), field.next(), field.next(), field.next())
            else {
                continue;
            };
            let Ok(at) = OffsetDateTime::parse(at, &Rfc3339) else {
                continue;
            };
            out.push((
                id.to_string(),
                CommitInfo {
                    author: author.to_string(),
                    at,
                    subject: subject.to_string(),
                    body: field.next().unwrap_or("").trim_end().to_string(),
                    files: Vec::new(),
                },
            ));
        }

        let mut touched = self.git();
        touched
            .args(["log", "--reverse", "-z", "--format=\x01%H", "--name-status"])
            .arg(&range);
        let text = run_text(touched)?;
        let mut at: Option<usize> = None;
        let mut fields = text.split('\0').map(|f| f.trim_start_matches('\n'));
        while let Some(field) = fields.next() {
            if let Some(id) = field.strip_prefix('\x01') {
                at = out.iter().position(|(known, _)| known == id);
                continue;
            }
            let (Some(i), Some(status), false) = (at, field.chars().next(), field.is_empty())
            else {
                continue;
            };
            // `R100`/`C75` name where the file was as well as where it is.
            let moved = matches!(status, 'R' | 'C');
            let Some(first) = fields.next() else { break };
            let (path, old_path) = if moved {
                match fields.next() {
                    Some(new) => (new.to_string(), Some(first.to_string())),
                    None => break,
                }
            } else {
                (first.to_string(), None)
            };
            out[i].1.files.push(CommitFile {
                path,
                old_path,
                status: match status {
                    'A' => "added",
                    'D' => "deleted",
                    'M' => "modified",
                    'R' => "renamed",
                    'C' => "copied",
                    _ => "changed",
                }
                .to_string(),
            });
        }
        Ok(out)
    }

    /// Whether `dir` is inside a git repository.
    pub fn exists(&self) -> bool {
        let mut cmd = self.git();
        cmd.args(["rev-parse", "--git-dir"]);
        run(cmd).is_ok()
    }

    /// Whether the repository has the commit `sha`.
    pub fn has_commit(&self, sha: &str) -> bool {
        if !is_object_id(sha) {
            return false;
        }
        let mut cmd = self.git();
        cmd.args(["cat-file", "-e", &format!("{sha}^{{commit}}")]);
        run(cmd).is_ok()
    }

    /// Every blob in `rev`'s tree, with paths relative to the repository root.
    pub fn ls_tree(&self, rev: &str) -> Result<Vec<TreeEntry>> {
        let mut cmd = self.git();
        cmd.args(["ls-tree", "-r", "-l", "-z", "--full-tree", rev]);
        let out = run(cmd)?;
        let mut entries = Vec::new();
        for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            // "<mode> SP <type> SP <oid> SP <size> TAB <path>"
            let record = String::from_utf8_lossy(record);
            let Some((meta, path)) = record.split_once('\t') else {
                continue;
            };
            let mut fields = meta.split_whitespace();
            let (_mode, kind, oid, size) =
                (fields.next(), fields.next(), fields.next(), fields.next());
            // Skips submodule entries (type `commit`, size `-`).
            let (Some("blob"), Some(oid), Some(size)) = (kind, oid, size) else {
                continue;
            };
            let Ok(size) = size.parse() else { continue };
            entries.push(TreeEntry {
                path: path.to_string(),
                oid: oid.to_string(),
                size,
            });
        }
        Ok(entries)
    }

    /// The content of the files at `paths` in `tree` (a listing from
    /// [`Repo::ls_tree`]), in tree order. Paths not in the tree are skipped.
    pub fn read_paths(
        &self,
        tree: &[TreeEntry],
        paths: &[String],
    ) -> Result<Vec<(String, Vec<u8>)>> {
        let wanted: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        let entries: Vec<&TreeEntry> = tree
            .iter()
            .filter(|e| wanted.contains(e.path.as_str()))
            .collect();
        let oids: Vec<&str> = entries.iter().map(|e| e.oid.as_str()).collect();
        let blobs = self.read_blobs(&oids)?;
        Ok(entries.iter().map(|e| e.path.clone()).zip(blobs).collect())
    }

    /// Reads blobs by id, in order, through a single `git cat-file --batch`.
    pub fn read_blobs(&self, oids: &[&str]) -> Result<Vec<Vec<u8>>> {
        if oids.is_empty() {
            return Ok(Vec::new());
        }
        let mut child = self
            .git()
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context(m("git.exec_failed_short"))?;
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());

        // Feed requests from a separate thread: for a large tree, writing every
        // request before reading any reply could fill both pipes and deadlock.
        let request: String = oids.iter().map(|oid| format!("{oid}\n")).collect();
        let mut blobs = Vec::with_capacity(oids.len());
        std::thread::scope(|scope| -> Result<()> {
            scope.spawn(move || {
                let _ = stdin.write_all(request.as_bytes());
            });
            for _ in oids {
                let mut header = String::new();
                std::io::BufRead::read_line(&mut stdout, &mut header)?;
                // "<oid> blob <size>"; anything else (e.g. "<oid> missing") is an error.
                let mut fields = header.split_whitespace();
                let (Some(_), Some("blob"), Some(size)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    bail!(mf("git.blob_read_failed", &[("header", header.trim())]));
                };
                let size: usize = size.parse().context(m("git.bad_size"))?;
                let mut content = vec![0u8; size + 1]; // + trailing newline
                stdout.read_exact(&mut content)?;
                content.pop();
                blobs.push(content);
            }
            Ok(())
        })?;
        child.wait()?;
        Ok(blobs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// Runs `f` against a fresh repo containing three commits (`c1`, `c2`,
    /// `c3`) that each rewrite `a.txt`; the first also adds `dir/b.txt`.
    fn with_repo(f: impl FnOnce(&Path, &Repo, [String; 3])) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git_in(p, &["init", "-q", "-b", "main"]);
        std::fs::create_dir(p.join("dir")).unwrap();
        std::fs::write(p.join("dir/b.txt"), "b\n").unwrap();
        let mut commits = Vec::new();
        for n in 1..=3 {
            std::fs::write(p.join("a.txt"), format!("version {n}\n")).unwrap();
            git_in(p, &["add", "-A"]);
            git_in(p, &["commit", "-q", "-m", &format!("c{n}")]);
            commits.push(git_in(p, &["rev-parse", "HEAD"]));
        }
        f(p, &Repo::at(p), commits.try_into().unwrap());
    }

    #[test]
    fn a_commit_is_reviewed_from_its_parent_and_two_commits_from_one_to_the_other() {
        with_repo(|_, repo, [c1, c2, c3]| {
            let r = repo.commit_range("HEAD").unwrap();
            assert_eq!(
                (r.base.as_str(), r.head.as_str()),
                (c2.as_str(), c3.as_str())
            );
            assert_eq!(r.spec, "HEAD");
            let r = repo.between("HEAD~2", "HEAD").unwrap();
            assert_eq!(
                (r.base.as_str(), r.head.as_str()),
                (c1.as_str(), c3.as_str())
            );
            assert_eq!(r.spec, "HEAD~2..HEAD");
        });
    }

    #[test]
    fn the_trail_from_base_to_head_is_read_oldest_first_with_what_each_commit_touched() {
        with_repo(|p, repo, [c1, c2, c3]| {
            // One more, with a message that has a body and a rename in it.
            std::fs::rename(p.join("dir/b.txt"), p.join("dir/c.txt")).unwrap();
            std::fs::write(p.join("new.txt"), "new\n").unwrap();
            git_in(p, &["add", "-A"]);
            git_in(
                p,
                &[
                    "commit",
                    "-q",
                    "-m",
                    "移動した\n\nなぜかというと、\nこうしたかったからです。\n\nCo-Authored-By: t <t@example.com>",
                ],
            );
            let c4 = git_in(p, &["rev-parse", "HEAD"]);

            let trail = repo.log(&c1, &c4).unwrap();
            assert_eq!(
                trail.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
                vec![c2.clone(), c3.clone(), c4.clone()],
                "oldest first, and never the base itself"
            );
            let (_, second) = &trail[0];
            assert_eq!(second.subject, "c2");
            assert_eq!(second.author, "t");
            assert!(second.body.is_empty(), "a one-line message has no body");
            assert_eq!(
                second.files,
                vec![CommitFile {
                    path: "a.txt".into(),
                    old_path: None,
                    status: "modified".into(),
                }]
            );

            let (_, last) = &trail[2];
            assert_eq!(last.subject, "移動した");
            assert!(
                last.body
                    .starts_with("なぜかというと、\nこうしたかったからです。")
            );
            assert!(
                last.body.contains("Co-Authored-By:"),
                "trailers are kept: the page folds them, the review doesn't drop them"
            );
            let mut files = last.files.clone();
            files.sort_by(|a, b| a.path.cmp(&b.path));
            assert_eq!(
                files,
                vec![
                    CommitFile {
                        path: "dir/c.txt".into(),
                        old_path: Some("dir/b.txt".into()),
                        status: "renamed".into(),
                    },
                    CommitFile {
                        path: "new.txt".into(),
                        old_path: None,
                        status: "added".into(),
                    },
                ]
            );
            // Nothing between a commit and itself.
            assert!(repo.log(&c3, &c3).unwrap().is_empty());
        });
    }

    #[test]
    fn root_commit_is_diffed_against_the_empty_tree() {
        with_repo(|_, repo, [c1, ..]| {
            let r = repo.commit_range(&c1).unwrap();
            assert_eq!(r.base, repo.empty_tree().unwrap());
            let text = repo.diff(&r).unwrap();
            assert!(text.contains("+version 1"));
            assert!(text.contains("dir/b.txt"));
        });
    }

    #[test]
    fn what_is_not_a_commit_is_refused() {
        with_repo(|_, repo, _| {
            assert!(repo.commit_range("--cached").is_err());
            assert!(repo.commit_range("no-such-rev").is_err());
            assert!(
                repo.commit_range("HEAD~2..HEAD").is_err(),
                "a range is not a commit"
            );
            assert!(repo.between("HEAD~1", "no-such-rev").is_err());
            assert!(repo.between("-x", "HEAD").is_err());
        });
    }

    #[test]
    fn diff_ignores_the_working_tree() {
        with_repo(|p, repo, _| {
            std::fs::write(p.join("a.txt"), "uncommitted\n").unwrap();
            let r = repo.commit_range("HEAD").unwrap();
            let text = repo.diff(&r).unwrap();
            assert!(text.contains("+version 3"));
            assert!(!text.contains("uncommitted"));
        });
    }

    #[test]
    fn tree_listing_and_batch_reads_return_committed_content() {
        with_repo(|p, repo, _| {
            std::fs::write(p.join("a.txt"), "uncommitted\n").unwrap();
            let entries = repo.ls_tree("HEAD").unwrap();
            let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
            assert_eq!(paths, ["a.txt", "dir/b.txt"]);
            assert_eq!(entries[0].size, "version 3\n".len() as u64);

            let oids: Vec<_> = entries.iter().map(|e| e.oid.as_str()).collect();
            let blobs = repo.read_blobs(&oids).unwrap();
            assert_eq!(blobs, [b"version 3\n".to_vec(), b"b\n".to_vec()]);
        });
    }

    #[test]
    fn read_paths_returns_only_the_requested_files_that_exist() {
        with_repo(|_, repo, commits| {
            let tree = repo.ls_tree("HEAD").unwrap();
            let want =
                |names: &[&str]| -> Vec<String> { names.iter().map(|s| s.to_string()).collect() };

            let got = repo
                .read_paths(&tree, &want(&["dir/b.txt", "not-there.txt"]))
                .unwrap();
            assert_eq!(got, [("dir/b.txt".to_string(), b"b\n".to_vec())]);

            // In tree order, whatever the request's, and each file once.
            let got = repo
                .read_paths(&tree, &want(&["dir/b.txt", "a.txt", "a.txt"]))
                .unwrap();
            let names: Vec<&str> = got.iter().map(|(p, _)| p.as_str()).collect();
            assert_eq!(names, ["a.txt", "dir/b.txt"]);

            // Nothing asked for, or nothing found: no git process, no error.
            assert!(repo.read_paths(&tree, &[]).unwrap().is_empty());
            assert!(repo.read_paths(&tree, &want(&["nope"])).unwrap().is_empty());

            // An older commit's tree gives that commit's content.
            let old = repo.ls_tree(&commits[0]).unwrap();
            let got = repo.read_paths(&old, &want(&["a.txt"])).unwrap();
            assert_eq!(got[0].1, b"version 1\n");
        });
    }

    #[test]
    fn read_paths_handles_binary_and_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git_in(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("bin.dat"), [0u8, 159, 146, 150, 255, 0]).unwrap();
        std::fs::write(p.join("empty.txt"), b"").unwrap();
        git_in(p, &["add", "-A"]);
        git_in(p, &["commit", "-q", "-m", "c"]);
        let repo = Repo::at(p);
        let tree = repo.ls_tree("HEAD").unwrap();
        let got = repo
            .read_paths(&tree, &["bin.dat".to_string(), "empty.txt".to_string()])
            .unwrap();
        assert_eq!(
            got[0],
            ("bin.dat".to_string(), vec![0u8, 159, 146, 150, 255, 0])
        );
        assert_eq!(got[1], ("empty.txt".to_string(), Vec::new()));
    }
}
