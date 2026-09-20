//! The git provider: turns a commit-range argument list into a diff plus
//! the committed content behind it.
//!
//! Only *committed* content is ever reviewed -- the working tree and index
//! are deliberately ignored, so a review made from a range means the same
//! thing to everyone who opens the resulting bundle. Revision syntax is
//! never parsed here: the arguments are handed to `git rev-parse`, which
//! resolves anything git itself accepts (`HEAD~4^2`, `main@{yesterday}`,
//! `A...B`, ...), and only its resolved output is interpreted.

use crate::model::GitSource;
use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    let output = cmd
        .output()
        .context("git を実行できませんでした(インストールされていて PATH に通っていますか)")?;
    if !output.status.success() {
        let first_line = String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or("(出力なし)")
            .to_string();
        bail!("git が失敗しました: {first_line}");
    }
    Ok(output.stdout)
}

fn run_text(cmd: Command) -> Result<String> {
    String::from_utf8(run(cmd)?).context("git の出力が UTF-8 ではありません")
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

    /// Resolves `args` (what the user typed after `diffnote edit`) into a
    /// concrete base/head commit pair:
    ///
    /// | input        | base                 | head |
    /// |--------------|----------------------|------|
    /// | `X`          | `X^` (empty tree if root commit) | `X` |
    /// | `A B`        | `A`                  | `B`  |
    /// | `A..B`       | `A`                  | `B`  |
    /// | `A...B`      | merge-base(`A`, `B`) | `B`  |
    ///
    /// Anything else (options such as `--cached`, pathspecs, 3+ revisions) is
    /// rejected rather than guessed at.
    pub fn resolve_range(&self, args: &[String]) -> Result<GitSource> {
        if args.is_empty() {
            bail!(
                "レビューするコミットを指定してください(例: `diffnote edit HEAD~3..HEAD`、\
                `diffnote edit main feature`、`diffnote edit <commit>`)"
            );
        }
        let mut cmd = self.git();
        cmd.arg("rev-parse").args(args);
        let out = run_text(cmd)?;

        let mut positives = Vec::new();
        let mut negatives = Vec::new();
        for line in out.lines() {
            match line.strip_prefix('^') {
                Some(oid) if is_object_id(oid) => negatives.push(oid.to_string()),
                None if is_object_id(line) => positives.push(line.to_string()),
                _ => bail!(
                    "引数 '{line}' は使えません。diffnote edit が受け付けるのはコミットの指定\
                    (`A..B`、`A...B`、`A B`、単一のコミット)だけで、オプションやパスは指定できません"
                ),
            }
        }

        let (base, head) = match (positives.as_slice(), negatives.as_slice()) {
            ([x], []) => (self.parent_or_empty_tree(x)?, x.clone()),
            ([a, b], []) => (a.clone(), b.clone()),
            ([b], [a]) => (a.clone(), b.clone()),
            // `A...B` resolves to `B`, `A`, `^merge-base` (in that order).
            ([b, a], [mb]) => {
                let expected = run_text({
                    let mut cmd = self.git();
                    cmd.args(["merge-base", a, b]);
                    cmd
                })?;
                if expected.trim() != mb {
                    bail!(
                        "この組み合わせのコミット指定は使えません。`A..B`、`A...B`、`A B`、または単一のコミットにしてください"
                    );
                }
                (mb.clone(), b.clone())
            }
            _ => {
                bail!("この組み合わせのコミット指定は使えません。`A..B`、`A...B`、`A B`、または単一のコミットにしてください")
            }
        };

        Ok(GitSource {
            base,
            head,
            spec: args.join(" "),
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
            Err(e) => Err(e).context("git を実行できませんでした"),
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
            .context("git を実行できませんでした")?;
        drop(child.stdin.take());
        let mut out = String::new();
        child.stdout.take().unwrap().read_to_string(&mut out)?;
        child.wait()?;
        Ok(out.trim().to_string())
    }

    /// The unified diff from `range.base` to `range.head`.
    pub fn diff(&self, range: &GitSource) -> Result<String> {
        let mut cmd = self.git();
        cmd.args(["diff", "--no-ext-diff", "--no-textconv"])
            .arg(&range.base)
            .arg(&range.head);
        run_text(cmd)
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
    pub fn read_paths(&self, tree: &[TreeEntry], paths: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
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
            .context("git cat-file を実行できませんでした")?;
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
            for oid in oids {
                let mut header = String::new();
                std::io::BufRead::read_line(&mut stdout, &mut header)?;
                // "<oid> blob <size>"; anything else (e.g. "<oid> missing") is an error.
                let mut fields = header.split_whitespace();
                let (Some(_), Some("blob"), Some(size)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    bail!("git cat-file が blob {oid} を読めませんでした: {}", header.trim());
                };
                let size: usize = size.parse().context("git cat-file が不正なサイズを返しました")?;
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

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolves_every_supported_shape() {
        with_repo(|_, repo, [c1, c2, c3]| {
            let r = repo.resolve_range(&args(&["HEAD"])).unwrap();
            assert_eq!(
                (r.base.as_str(), r.head.as_str()),
                (c2.as_str(), c3.as_str())
            );

            let r = repo
                .resolve_range(&args(&["HEAD~2^{commit}", "HEAD"]))
                .unwrap();
            assert_eq!(
                (r.base.as_str(), r.head.as_str()),
                (c1.as_str(), c3.as_str())
            );

            let r = repo.resolve_range(&args(&["HEAD~2..HEAD~1"])).unwrap();
            assert_eq!(
                (r.base.as_str(), r.head.as_str()),
                (c1.as_str(), c2.as_str())
            );

            // Merge base of an ancestor and its descendant is the ancestor.
            let r = repo.resolve_range(&args(&["HEAD~1...HEAD"])).unwrap();
            assert_eq!(
                (r.base.as_str(), r.head.as_str()),
                (c2.as_str(), c3.as_str())
            );
            assert_eq!(r.spec, "HEAD~1...HEAD");
        });
    }

    #[test]
    fn root_commit_is_diffed_against_the_empty_tree() {
        with_repo(|_, repo, [c1, ..]| {
            let r = repo.resolve_range(&args(&[&c1])).unwrap();
            assert_eq!(r.base, repo.empty_tree().unwrap());
            let text = repo.diff(&r).unwrap();
            assert!(text.contains("+version 1"));
            assert!(text.contains("dir/b.txt"));
        });
    }

    #[test]
    fn rejects_options_paths_and_odd_combinations() {
        with_repo(|_, repo, _| {
            assert!(repo.resolve_range(&[]).is_err());
            assert!(repo.resolve_range(&args(&["--cached"])).is_err());
            assert!(repo.resolve_range(&args(&["HEAD", "--", "a.txt"])).is_err());
            assert!(
                repo.resolve_range(&args(&["HEAD~2", "HEAD~1", "HEAD"]))
                    .is_err()
            );
            // `^X` is just another spelling of `X..HEAD`...
            assert!(repo.resolve_range(&args(&["HEAD", "^HEAD~2"])).is_ok());
            // ...but two revisions plus an unrelated exclusion is not `A...B`.
            assert!(
                repo.resolve_range(&args(&["HEAD", "HEAD~1", "^HEAD~2"]))
                    .is_err()
            );
            assert!(repo.resolve_range(&args(&["no-such-rev"])).is_err());
        });
    }

    #[test]
    fn diff_ignores_the_working_tree() {
        with_repo(|p, repo, _| {
            std::fs::write(p.join("a.txt"), "uncommitted\n").unwrap();
            let r = repo.resolve_range(&args(&["HEAD"])).unwrap();
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
            let want = |names: &[&str]| -> Vec<String> { names.iter().map(|s| s.to_string()).collect() };

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
        assert_eq!(got[0], ("bin.dat".to_string(), vec![0u8, 159, 146, 150, 255, 0]));
        assert_eq!(got[1], ("empty.txt".to_string(), Vec::new()));
    }
}
