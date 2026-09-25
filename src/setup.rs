//! The first screen: how a review that doesn't exist yet is to be made.
//!
//! `diffnote review` with nothing said and no review at hand opens the page
//! on this screen instead of a review. It asks what the review is of (one
//! repository, several under this directory, or a directory's files as they
//! are), which commit each repository is reviewed from, a title, and what
//! the review keeps -- then makes the review, and the page goes on to it.
//!
//! What is here is what the screen is drawn from and what its answers are
//! checked against; making the review itself is the command's job
//! (`Setup::create`), since it is the same as `review --base ...` does.

use crate::files;
use crate::git::Repo;
use crate::messages::{m, mf};
use crate::model::{GitSource, RepoSource, SnapshotMode};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A commit, as the screen names it.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct CommitRef {
    pub id: String,
    pub short: String,
    pub subject: String,
}

/// What a commit is to the repository: `HEAD` itself, the tip of its
/// default branch, or where `HEAD`'s work left that branch.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// One commit a repository can be reviewed from, with what choosing it
/// would review (up to `HEAD`).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// What to ask for it by (`HEAD`, a branch, or the id itself).
    pub rev: String,
    #[serde(flatten)]
    pub commit: CommitRef,
    /// The names it goes by (one commit may be all three).
    pub names: Vec<Name>,
    pub commits: usize,
    pub files: usize,
}

/// A repository as the screen shows it: where it is, where it stands, and
/// the commits it can be reviewed from.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct RepoInfo {
    /// Relative to the project directory, `/`-separated; empty for the one
    /// repository of a `git` review.
    pub path: String,
    pub branch: Option<String>,
    pub head: CommitRef,
    pub default_branch: Option<String>,
    /// Best first: where the work left the default branch, its tip, `HEAD`.
    pub candidates: Vec<Candidate>,
}

/// Whether a kind of review can be made here, and why not.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct KindInfo {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Kinds {
    pub git: KindInfo,
    pub workspace: KindInfo,
    pub raw: KindInfo,
}

/// What the screen starts from.
#[derive(Serialize, Clone, Debug)]
pub struct Description {
    /// Where the review will be.
    pub review: String,
    /// The kind this directory looks like: in a repository `git`, with
    /// repositories under it `workspace`, else `raw`.
    pub kind: &'static str,
    pub kinds: Kinds,
    /// The one repository (a `git` review), if this is in one.
    pub git: Option<RepoInfo>,
    /// The repositories found under the directory (a `workspace` review).
    pub repos: Vec<RepoInfo>,
    /// How deep they were looked for; deeper ones can be named by hand.
    pub depth: usize,
    /// What the command line already said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotMode>,
}

/// What choosing a commit as a base would review.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    #[serde(flatten)]
    pub commit: CommitRef,
    pub commits: usize,
    pub files: usize,
}

/// A directory's files as they are.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct RawInfo {
    pub files: usize,
    pub bytes: u64,
}

/// What the screen chose.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Choice {
    pub kind: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub snapshot: Option<SnapshotMode>,
    /// `git`: the commit to review from.
    #[serde(default)]
    pub base: Option<String>,
    /// `workspace`: the repositories, each with its own.
    #[serde(default)]
    pub repos: Vec<RepoChoice>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RepoChoice {
    pub path: String,
    pub base: String,
}

/// What makes the review once it is chosen: what it said, as `review` says
/// it at the terminal.
pub type Creator = Arc<dyn Fn(&Choice) -> Result<String> + Send + Sync>;

pub struct Setup {
    pub review: PathBuf,
    /// The directory `review` was run in: where repositories are looked for,
    /// and what a `raw` review is of.
    pub project: PathBuf,
    /// The one repository (`--repo`, else the directory itself).
    pub repo: Repo,
    pub title: Option<String>,
    pub snapshot: Option<SnapshotMode>,
    pub create: Creator,
}

impl Setup {
    pub fn describe(&self) -> Result<Description> {
        let git = describe_repo(&self.repo, String::new());
        let found = files::find_repos(&self.project)?;
        let mut repos = Vec::new();
        for path in found {
            // A repository with nothing in it yet is listed by the others;
            // it can't be reviewed until it has a commit.
            if let Ok(info) = describe_repo(&Repo::at(self.project.join(&path)), path) {
                repos.push(info);
            }
        }
        let kinds = Kinds {
            git: match &git {
                Ok(_) => KindInfo {
                    ok: true,
                    why: None,
                },
                Err(why) => KindInfo {
                    ok: false,
                    why: Some(why.clone()),
                },
            },
            workspace: if repos.is_empty() {
                KindInfo {
                    ok: false,
                    why: Some(m("setup.no_repos").to_string()),
                }
            } else {
                KindInfo {
                    ok: true,
                    why: None,
                }
            },
            raw: KindInfo {
                ok: true,
                why: None,
            },
        };
        let kind = if kinds.git.ok {
            "git"
        } else if kinds.workspace.ok {
            "workspace"
        } else {
            "raw"
        };
        Ok(Description {
            review: self.review.display().to_string(),
            kind,
            kinds,
            git: git.ok(),
            repos,
            depth: files::REPO_DEPTH,
            title: self.title.clone(),
            snapshot: self.snapshot,
        })
    }

    /// What reviewing `path` (empty: the one repository) from `rev` would
    /// take in.
    pub fn preview(&self, path: &str, rev: &str) -> Result<Preview> {
        preview_of(&self.repo_of(path)?, rev)
    }

    /// A repository named by hand (one deeper than they are looked for).
    pub fn repo_at(&self, path: &str) -> Result<RepoInfo> {
        repo_info(&self.project, path)
    }

    /// The directory's files as they are, which a `raw` review keeps.
    pub fn raw(&self) -> Result<RawInfo> {
        let tree = files::read_tree(&self.project, std::slice::from_ref(&self.review))?;
        Ok(RawInfo {
            files: tree.len(),
            bytes: tree.values().map(|b| b.len() as u64).sum(),
        })
    }

    /// Whether the choice can be made here. Nothing is made.
    pub fn check(&self, choice: &Choice) -> Result<()> {
        match choice.kind.as_str() {
            "git" => {
                if !self.repo.exists() {
                    bail!(m("setup.not_in_a_repo"));
                }
                let Some(base) = choice.base.as_deref().filter(|b| !b.trim().is_empty()) else {
                    bail!(m("setup.base_missing"));
                };
                self.repo.commit_id(base.trim())?;
            }
            "workspace" => {
                if choice.repos.is_empty() {
                    bail!(m("setup.repos_missing"));
                }
                let mut seen = std::collections::BTreeSet::new();
                for repo in &choice.repos {
                    let path = clean_path(&repo.path)?;
                    if !seen.insert(path.clone()) {
                        bail!(mf("setup.repo_twice", &[("path", &path)]));
                    }
                    if !self.project.join(&path).join(".git").exists() {
                        bail!(mf("setup.repo_not_found", &[("path", &path)]));
                    }
                    if repo.base.trim().is_empty() {
                        bail!(mf("setup.repo_base_missing", &[("path", &path)]));
                    }
                    Repo::at(self.project.join(&path))
                        .commit_id(repo.base.trim())
                        .map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
                }
            }
            "raw" => {}
            other => bail!(mf("setup.kind_invalid", &[("kind", other)])),
        }
        if let Some(title) = &choice.title
            && title.chars().count() > 200
        {
            bail!(m("serve.title_too_long"));
        }
        Ok(())
    }

    fn repo_of(&self, path: &str) -> Result<Repo> {
        if path.is_empty() {
            if !self.repo.exists() {
                bail!(m("setup.not_in_a_repo"));
            }
            return Ok(self.repo.clone());
        }
        let path = clean_path(path)?;
        if !self.project.join(&path).join(".git").exists() {
            bail!(mf("setup.repo_not_found", &[("path", &path)]));
        }
        Ok(Repo::at(self.project.join(path)))
    }
}

/// A repository under `project`, named by its path, as the page shows it:
/// its branch, its `HEAD`, and the commits it can be reviewed from.
pub fn repo_info(project: &Path, path: &str) -> Result<RepoInfo> {
    let path = clean_path(path)?;
    if !project.join(&path).join(".git").exists() {
        bail!(mf("setup.repo_not_found", &[("path", &path)]));
    }
    describe_repo(&Repo::at(project.join(&path)), path.clone())
        .map_err(|why| anyhow::anyhow!("{path}: {why}"))
}

/// What reviewing `repo` from `rev` (up to its `HEAD`) would take in.
pub fn preview_of(repo: &Repo, rev: &str) -> Result<Preview> {
    let id = repo.commit_id(rev)?;
    let head = repo.commit_id("HEAD")?;
    Ok(Preview {
        commit: commit_ref(repo, &id)?,
        commits: repo.count_commits(&id, &head)?,
        files: repo.count_changed_files(&id, &head)?,
    })
}

/// One repository to add to a review of several, from `rev`: as it is
/// recorded (its `HEAD` now is what the next revision compares).
pub fn repo_from(project: &Path, path: &str, rev: &str) -> Result<RepoSource> {
    let path = clean_path(path)?;
    if !project.join(&path).join(".git").exists() {
        bail!(mf("setup.repo_not_found", &[("path", &path)]));
    }
    let git = Repo::at(project.join(&path));
    let rev = rev.trim();
    if rev.is_empty() {
        bail!(mf("setup.repo_base_missing", &[("path", &path)]));
    }
    let base = git.commit_id(rev)?;
    let head = git.commit_id("HEAD")?;
    Ok(RepoSource {
        path,
        range: GitSource {
            spec: format!("{rev}..HEAD"),
            base,
            head,
        },
    })
}

/// The repositories a `workspace` choice names, under `project`, each from
/// its base to its `HEAD` now. `Setup::check` first: this trusts the paths.
pub fn repos_chosen(project: &Path, choice: &Choice) -> Result<Vec<RepoSource>> {
    let mut repos = Vec::new();
    for repo in &choice.repos {
        let path = clean_path(&repo.path)?;
        let git = Repo::at(project.join(&path));
        let rev = repo.base.trim();
        let base = git.commit_id(rev)?;
        let head = git.commit_id("HEAD")?;
        repos.push(RepoSource {
            path,
            range: GitSource {
                spec: format!("{rev}..HEAD"),
                base,
                head,
            },
        });
    }
    repos.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(repos)
}

/// A repository's path as the review records it: relative, `/`-separated,
/// inside the project.
fn clean_path(path: &str) -> Result<String> {
    let path = path.trim().replace('\\', "/");
    let path = path.trim_matches('/').to_string();
    if path.is_empty()
        || Path::new(&path).is_absolute()
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        bail!(mf("setup.repo_path_invalid", &[("path", &path)]));
    }
    Ok(path)
}

fn commit_ref(repo: &Repo, id: &str) -> Result<CommitRef> {
    Ok(CommitRef {
        id: id.to_string(),
        short: id.chars().take(7).collect(),
        subject: repo.subject(id).unwrap_or_default(),
    })
}

/// A repository as the screen shows it. `Err` says, for a person, why it
/// can't be reviewed (not a repository; no commit yet).
fn describe_repo(repo: &Repo, path: String) -> std::result::Result<RepoInfo, String> {
    if !repo.exists() {
        return Err(m("setup.not_in_a_repo").to_string());
    }
    let Ok(head) = repo.commit_id("HEAD") else {
        return Err(m("setup.no_commits").to_string());
    };
    let describe = |id: &str| commit_ref(repo, id).map_err(|e| e.to_string());
    let default_branch = repo.default_branch();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut add = |rev: String, id: String, name: Name| -> std::result::Result<(), String> {
        if let Some(had) = candidates.iter_mut().find(|c| c.commit.id == id) {
            had.names.push(name);
            return Ok(());
        }
        candidates.push(Candidate {
            commit: describe(&id)?,
            commits: repo.count_commits(&id, &head).map_err(|e| e.to_string())?,
            files: repo
                .count_changed_files(&id, &head)
                .map_err(|e| e.to_string())?,
            names: vec![name],
            rev,
        });
        Ok(())
    };
    if let Some(branch) = &default_branch {
        if let Ok(fork) = repo.merge_base(branch, &head) {
            add(
                fork.clone(),
                fork,
                Name {
                    kind: "fork",
                    branch: Some(branch.clone()),
                },
            )?;
        }
        if let Ok(tip) = repo.commit_id(branch) {
            add(
                branch.clone(),
                tip,
                Name {
                    kind: "branch",
                    branch: Some(branch.clone()),
                },
            )?;
        }
    }
    add(
        "HEAD".to_string(),
        head.clone(),
        Name {
            kind: "head",
            branch: None,
        },
    )?;
    Ok(RepoInfo {
        path,
        branch: repo.current_branch(),
        head: describe(&head)?,
        default_branch,
        candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
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

    /// A repository at `dir` with `c1` on `main`, and, on `feature`, `c2`
    /// (both writing `a.txt`).
    fn repo_on_a_branch(dir: &Path) -> (String, String) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "c1"]);
        let c1 = git(dir, &["rev-parse", "HEAD"]);
        git(dir, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "c2"]);
        let c2 = git(dir, &["rev-parse", "HEAD"]);
        (c1, c2)
    }

    fn setup_in(dir: &Path) -> Setup {
        Setup {
            review: dir.join("r.diffnote"),
            project: dir.to_path_buf(),
            repo: Repo::at(dir),
            title: None,
            snapshot: None,
            create: Arc::new(|_| Ok(String::new())),
        }
    }

    #[test]
    fn in_a_repository_the_kind_is_git_and_the_bases_are_the_fork_the_branch_and_head() {
        let dir = tempfile::tempdir().unwrap();
        let (c1, c2) = repo_on_a_branch(dir.path());
        let d = setup_in(dir.path()).describe().unwrap();
        assert_eq!(d.kind, "git");
        assert!(d.kinds.git.ok && d.kinds.raw.ok && !d.kinds.workspace.ok);
        assert!(
            d.kinds
                .workspace
                .why
                .as_deref()
                .unwrap()
                .contains("見つかりません")
        );
        assert!(d.repos.is_empty());
        let git = d.git.unwrap();
        assert_eq!(git.path, "");
        assert_eq!(git.branch.as_deref(), Some("feature"));
        assert_eq!(git.default_branch.as_deref(), Some("main"));
        assert_eq!(
            (git.head.id.as_str(), git.head.subject.as_str()),
            (c2.as_str(), "c2")
        );
        // c1 is where feature left main, and main's tip: one candidate, both names.
        let names = |c: &Candidate| c.names.iter().map(|n| n.kind).collect::<Vec<_>>();
        assert_eq!(git.candidates.len(), 2);
        assert_eq!(git.candidates[0].commit.id, c1);
        assert_eq!(git.candidates[0].rev, c1);
        assert_eq!(names(&git.candidates[0]), ["fork", "branch"]);
        assert_eq!(git.candidates[0].names[0].branch.as_deref(), Some("main"));
        assert_eq!((git.candidates[0].commits, git.candidates[0].files), (1, 2));
        assert_eq!(git.candidates[1].rev, "HEAD");
        assert_eq!(names(&git.candidates[1]), ["head"]);
        assert_eq!((git.candidates[1].commits, git.candidates[1].files), (0, 0));
    }

    #[test]
    fn under_a_directory_of_repositories_the_kind_is_workspace_and_deeper_ones_are_named_by_hand() {
        let dir = tempfile::tempdir().unwrap();
        repo_on_a_branch(&dir.path().join("backend/repo-a"));
        repo_on_a_branch(&dir.path().join("mobile-app"));
        repo_on_a_branch(&dir.path().join("a/b/c/d/e/deep"));
        std::fs::write(dir.path().join("notes.md"), "# notes\n").unwrap();
        let setup = setup_in(dir.path());
        let d = setup.describe().unwrap();
        assert_eq!(d.kind, "workspace");
        assert!(!d.kinds.git.ok && d.kinds.workspace.ok);
        assert!(d.git.is_none());
        let paths: Vec<&str> = d.repos.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            ["backend/repo-a", "mobile-app"],
            "the deep one is past the depth"
        );
        assert_eq!(d.repos[0].candidates.len(), 2);
        let deep = setup.repo_at("a/b/c/d/e/deep/").unwrap();
        assert_eq!(deep.path, "a/b/c/d/e/deep");
        assert_eq!(deep.branch.as_deref(), Some("feature"));
        for bad in ["", "/abs", "../out", "a/../b", "no/such"] {
            assert!(setup.repo_at(bad).is_err(), "{bad}");
        }
        assert!(setup.repo_at("backend").is_err(), "not itself a repository");
    }

    #[test]
    fn elsewhere_the_kind_is_raw_and_the_files_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        let d = setup_in(dir.path()).describe().unwrap();
        assert_eq!(d.kind, "raw");
        assert!(!d.kinds.git.ok && !d.kinds.workspace.ok && d.kinds.raw.ok);
        let raw = setup_in(dir.path()).raw().unwrap();
        assert_eq!((raw.files, raw.bytes), (2, 8));
    }

    #[test]
    fn a_base_is_previewed_by_what_it_would_review_and_refused_when_it_is_no_commit() {
        let dir = tempfile::tempdir().unwrap();
        let (c1, _) = repo_on_a_branch(&dir.path().join("mobile-app"));
        let setup = setup_in(dir.path());
        let p = setup.preview("mobile-app", "main").unwrap();
        assert_eq!(
            (p.commit.id.as_str(), p.commits, p.files),
            (c1.as_str(), 1, 2)
        );
        assert!(setup.preview("mobile-app", "no-such").is_err());
        assert!(
            setup.preview("", "main").is_err(),
            "the directory itself is no repository"
        );
        assert!(setup.preview("nowhere", "main").is_err());
    }

    #[test]
    fn a_choice_is_checked_before_anything_is_made() {
        let dir = tempfile::tempdir().unwrap();
        let (c1, c2) = repo_on_a_branch(&dir.path().join("mobile-app"));
        repo_on_a_branch(&dir.path().join("backend/repo-a"));
        let setup = setup_in(dir.path());
        let choice = |json: &str| serde_json::from_str::<Choice>(json).unwrap();
        assert!(setup.check(&choice(r#"{"kind":"raw"}"#)).is_ok());
        assert!(
            setup
                .check(&choice(r#"{"kind":"git","base":"HEAD"}"#))
                .is_err(),
            "not in a repository"
        );
        assert!(
            setup.check(&choice(r#"{"kind":"workspace"}"#)).is_err(),
            "no repository chosen"
        );
        assert!(
            setup
                .check(&choice(
                    r#"{"kind":"workspace","repos":[{"path":"mobile-app","base":"main"}]}"#
                ))
                .is_ok()
        );
        assert!(
            setup
                .check(&choice(
                    r#"{"kind":"workspace","repos":[{"path":"mobile-app","base":"nope"}]}"#
                ))
                .is_err()
        );
        assert!(setup.check(&choice(r#"{"kind":"workspace","repos":[{"path":"mobile-app","base":"main"},{"path":"mobile-app/","base":"main"}]}"#)).is_err(), "twice");
        assert!(
            setup
                .check(&choice(
                    r#"{"kind":"workspace","repos":[{"path":"docs","base":"main"}]}"#
                ))
                .is_err()
        );
        assert!(setup.check(&choice(r#"{"kind":"other"}"#)).is_err());
        let long = "あ".repeat(201);
        assert!(
            setup
                .check(&choice(&format!(r#"{{"kind":"raw","title":"{long}"}}"#)))
                .is_err()
        );
        let repos = repos_chosen(dir.path(), &choice(r#"{"kind":"workspace","repos":[{"path":"mobile-app","base":"main"},{"path":"backend/repo-a","base":"HEAD"}]}"#))
            .unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].path, "backend/repo-a", "sorted");
        assert_eq!(repos[1].range.base, c1);
        assert_eq!(repos[1].range.head, c2);
        assert_eq!(repos[1].range.spec, "main..HEAD");
        assert_eq!(repos[0].range.base, repos[0].range.head);
    }

    #[test]
    fn a_repository_with_nothing_committed_is_left_out_and_said_so() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        let d = setup_in(dir.path()).describe().unwrap();
        assert!(!d.kinds.git.ok);
        assert!(d.kinds.git.why.as_deref().unwrap().contains("コミット"));
        assert_eq!(d.kind, "raw");
    }
}
