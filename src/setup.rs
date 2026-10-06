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

/// One commit a repository can be reviewed from, and how many files its
/// tree holds (what is compared from it is said apart: see [`Span`]).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// What to ask for it by (`HEAD`, a branch, or the id itself).
    pub rev: String,
    #[serde(flatten)]
    pub commit: CommitRef,
    /// The names it goes by (one commit may be all three).
    pub names: Vec<Name>,
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
    /// The base a review of it starts from, which leaves it something to
    /// review: where the work left the default branch -- or, where that is
    /// `HEAD` itself (the default branch is what is checked out), the
    /// commit before `HEAD`.
    pub suggested: Suggested,
}

/// See [`RepoInfo::suggested`].
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Suggested {
    #[serde(flatten)]
    pub commit: CommitRef,
    /// `fork` (where the work left the default branch), `previous` (the
    /// commit before `HEAD`), or `head` (`HEAD` itself: nothing came before).
    pub why: &'static str,
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

/// A commit typed in as a base: which, and how many files its tree holds.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    #[serde(flatten)]
    pub commit: CommitRef,
    pub files: usize,
}

/// What comparing from a base up to a target takes in: the target, how many
/// commits, how many files differ, and which commits they are (newest first,
/// as many as [`SPAN_IDS`]: what the graph marks).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Span {
    #[serde(flatten)]
    pub commit: CommitRef,
    pub commits: usize,
    pub files: usize,
    pub ids: Vec<String>,
    /// The base, as an id (what was asked for may be a name: `HEAD~3`).
    pub from: String,
}

/// The most commits of a span named one by one.
pub const SPAN_IDS: usize = 1000;

/// A name a commit of the graph goes by.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GraphRef {
    pub name: String,
    /// `head` (`HEAD` itself), `branch`, `remote` (a remote-tracking
    /// branch) or `tag`.
    pub kind: &'static str,
}

/// One commit of the graph, with where it is drawn: its column (`lane`),
/// the columns whose lines come down into it from the row above (`up`),
/// the columns its lines go on down in to its parents (`down`), and the
/// columns whose lines pass by it (`through`).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    #[serde(flatten)]
    pub commit: CommitRef,
    pub author: String,
    /// When it was committed (RFC 3339, UTC): the page says it in the
    /// reader's time.
    pub at: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<GraphRef>,
    pub lane: usize,
    pub up: Vec<usize>,
    pub down: Vec<usize>,
    pub through: Vec<usize>,
}

/// The history of a repository as the first screen draws it: the commits of
/// its local branches and `HEAD`, newest first, as far back as the work on
/// `HEAD` left the default branch (and a few before) or four weeks,
/// whichever goes further.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    pub rows: Vec<GraphRow>,
    /// How many columns the lines take.
    pub lanes: usize,
    /// Whether there are older commits than these (asked for with a limit).
    pub more: bool,
}

/// How many commits before the fork point the graph shows.
const GRAPH_BEFORE_FORK: usize = 10;
/// How far back in time the graph goes at least.
const GRAPH_DAYS: i64 = 28;
/// What it shows when neither says anything (no default branch, and nothing
/// lately), and the most it shows unless asked for more.
const GRAPH_LEAST: usize = 20;
const GRAPH_MOST: usize = 500;

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
    /// What to compare up to, each time (`HEAD` if not said).
    #[serde(default)]
    pub target: Option<String>,
}

/// A target as it is kept: `None` for `HEAD` (said or not), else trimmed.
pub fn target_of(target: Option<&str>) -> Option<String> {
    target
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "HEAD")
        .map(str::to_string)
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

    /// What reviewing `path` (empty: the one repository) from `base` up to
    /// `target` would take in.
    pub fn span(&self, path: &str, base: &str, target: &str) -> Result<Span> {
        span_of(&self.repo_of(path)?, base, target)
    }

    /// The history of `path` (empty: the one repository) to choose from.
    pub fn graph(&self, path: &str, limit: Option<usize>) -> Result<Graph> {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        graph_of(&self.repo_of(path)?, limit, now)
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
                    let git = Repo::at(self.project.join(&path));
                    git.commit_id(repo.base.trim())
                        .map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
                    if let Some(target) = target_of(repo.target.as_deref()) {
                        git.commit_id(&target)
                            .map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
                    }
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
    Ok(Preview {
        commit: commit_ref(repo, &id)?,
        files: repo.ls_tree(&id)?.len(),
    })
}

/// What comparing `repo` from `base` up to `target` takes in.
pub fn span_of(repo: &Repo, base: &str, target: &str) -> Result<Span> {
    let from = repo.commit_id(base)?;
    let to = repo.commit_id(target)?;
    Ok(Span {
        commit: commit_ref(repo, &to)?,
        commits: repo.count_commits(&from, &to)?,
        files: repo.count_changed_files(&from, &to)?,
        ids: repo.commit_ids(&from, &to, SPAN_IDS)?,
        from,
    })
}

/// The graph of `repo` (see [`Graph`]): as far as it goes of itself, or, with
/// a `limit`, that many commits. `now` is the time it is (seconds).
pub fn graph_of(repo: &Repo, limit: Option<usize>, now: i64) -> Result<Graph> {
    let asked = limit.unwrap_or(GRAPH_MOST).clamp(1, 5000);
    let history = repo.history(asked + 1)?;
    let shown = match limit {
        Some(n) => n.min(history.len()),
        None => {
            let head = repo.commit_id("HEAD")?;
            let fork = repo
                .default_branch()
                .and_then(|b| repo.merge_base(&b, &head).ok());
            let to_fork = fork
                .and_then(|f| history.iter().position(|e| e.id == f))
                .map_or(0, |i| i + 1 + GRAPH_BEFORE_FORK);
            let since = now - GRAPH_DAYS * 24 * 60 * 60;
            let to_date = history
                .iter()
                .rposition(|e| e.at >= since)
                .map_or(0, |i| i + 1);
            let n = to_fork.max(to_date);
            (if n == 0 { GRAPH_LEAST } else { n })
                .min(asked)
                .min(history.len())
        }
    };
    let more = history.len() > shown;
    let head = repo.commit_id("HEAD").ok();
    let mut names: std::collections::HashMap<String, Vec<GraphRef>> = Default::default();
    if let Some(head) = &head {
        names.entry(head.clone()).or_default().push(GraphRef {
            name: "HEAD".into(),
            kind: "head",
        });
    }
    for (id, name, kind) in repo.refs()? {
        names.entry(id).or_default().push(GraphRef { name, kind });
    }
    let entries = &history[..shown];
    let placed = lay_out(entries);
    let lanes = placed.iter().map(|p| p.width).max().unwrap_or(0);
    let rows = entries
        .iter()
        .zip(placed)
        .map(|(e, p)| GraphRow {
            commit: CommitRef {
                id: e.id.clone(),
                short: e.id.chars().take(7).collect(),
                subject: e.subject.clone(),
            },
            author: e.author.clone(),
            at: time::OffsetDateTime::from_unix_timestamp(e.at)
                .ok()
                .and_then(|t| {
                    t.format(&time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .unwrap_or_default(),
            refs: names.remove(&e.id).unwrap_or_default(),
            lane: p.lane,
            up: p.up,
            down: p.down,
            through: p.through,
        })
        .collect();
    Ok(Graph { rows, lanes, more })
}

/// Where one commit of the graph is drawn (see [`GraphRow`]), and how many
/// columns are taken at its row.
#[derive(Debug, PartialEq, Eq)]
struct Placed {
    lane: usize,
    up: Vec<usize>,
    down: Vec<usize>,
    through: Vec<usize>,
    width: usize,
}

/// The columns of a graph, newest commit first: each column waits for the
/// commit its line goes down to. A commit takes the first column waiting for
/// it (or a free one: a tip nothing came down to), and its first parent goes
/// on in that column -- or in one to its left already waiting for it; one to
/// its right waits too, so that lines meet in the leftmost. Each other parent
/// goes in a column already waiting for it, or a free one. The columns of the
/// others go straight past.
fn lay_out(entries: &[crate::git::LogEntry]) -> Vec<Placed> {
    let mut waiting: Vec<Option<&str>> = Vec::new();
    let mut out = Vec::new();
    let free = |waiting: &mut Vec<Option<&str>>| -> usize {
        match waiting.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                waiting.push(None);
                waiting.len() - 1
            }
        }
    };
    for e in entries {
        let up: Vec<usize> = waiting
            .iter()
            .enumerate()
            .filter(|(_, w)| **w == Some(e.id.as_str()))
            .map(|(i, _)| i)
            .collect();
        let through: Vec<usize> = waiting
            .iter()
            .enumerate()
            .filter(|(_, w)| w.is_some_and(|w| w != e.id))
            .map(|(i, _)| i)
            .collect();
        let lane = match up.first() {
            Some(&i) => i,
            None => free(&mut waiting),
        };
        for &i in &up {
            waiting[i] = None;
        }
        let mut down = Vec::new();
        for (k, parent) in e.parents.iter().enumerate() {
            let already = waiting.iter().position(|w| *w == Some(parent.as_str()));
            let at = match already {
                Some(i) if k > 0 || i < lane => i,
                _ if k == 0 => lane,
                _ => free(&mut waiting),
            };
            waiting[at] = Some(parent.as_str());
            if !down.contains(&at) {
                down.push(at);
            }
        }
        while waiting.last().is_some_and(Option::is_none) {
            waiting.pop();
        }
        let width = up
            .iter()
            .chain(&down)
            .chain(&through)
            .chain(std::iter::once(&lane))
            .max()
            .map_or(1, |m| m + 1);
        out.push(Placed {
            lane,
            up,
            down,
            through,
            width,
        });
    }
    out
}

/// One repository to add to a review of several, from `rev` up to
/// `target` (`HEAD` if none): as it is recorded (its target now is what
/// the next revision compares).
pub fn repo_from(
    project: &Path,
    path: &str,
    rev: &str,
    target: Option<&str>,
) -> Result<RepoSource> {
    let path = clean_path(path)?;
    if !project.join(&path).join(".git").exists() {
        bail!(mf("setup.repo_not_found", &[("path", &path)]));
    }
    let git = Repo::at(project.join(&path));
    let rev = rev.trim();
    if rev.is_empty() {
        bail!(mf("setup.repo_base_missing", &[("path", &path)]));
    }
    let target = target_of(target);
    let base = git.commit_id(rev)?;
    let head = git.commit_id(target.as_deref().unwrap_or("HEAD"))?;
    Ok(RepoSource {
        path,
        range: GitSource {
            spec: format!("{rev}..{}", target.as_deref().unwrap_or("HEAD")),
            base,
            head,
        },
        target,
    })
}

/// The repositories a `workspace` choice names, under `project`, each from
/// its base to its `HEAD` now. `Setup::check` first: this trusts the paths.
pub fn repos_chosen(project: &Path, choice: &Choice) -> Result<Vec<RepoSource>> {
    let mut repos = Vec::new();
    for repo in &choice.repos {
        repos.push(repo_from(
            project,
            &repo.path,
            &repo.base,
            repo.target.as_deref(),
        )?);
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
            files: repo.ls_tree(&id).map_err(|e| e.to_string())?.len(),
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
    // Where the work left the default branch, unless that is `HEAD` itself
    // (nothing to review from there): then the commit before it.
    let fork = default_branch
        .as_deref()
        .and_then(|b| repo.merge_base(b, &head).ok())
        .filter(|f| *f != head);
    let (suggested, why) = match fork {
        Some(fork) => (fork, "fork"),
        None => match repo.commit_id(&format!("{head}~1")) {
            Ok(previous) => (previous, "previous"),
            Err(_) => (head.clone(), "head"),
        },
    };
    Ok(RepoInfo {
        path,
        branch: repo.current_branch(),
        head: describe(&head)?,
        default_branch,
        candidates,
        suggested: Suggested {
            commit: describe(&suggested)?,
            why,
        },
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

    /// A commit at `dir` of a file `name` written `text`, committed at `at`
    /// (seconds since 1970): its id.
    fn commit_at(dir: &Path, name: &str, text: &str, at: i64) -> String {
        std::fs::write(dir.join(name), text).unwrap();
        git(dir, &["add", "-A"]);
        let out = Command::new("git")
            .current_dir(dir)
            .env("GIT_COMMITTER_DATE", format!("@{at} +0000"))
            .env("GIT_AUTHOR_DATE", format!("@{at} +0000"))
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(["commit", "-q", "-m", text.trim()])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        git(dir, &["rev-parse", "HEAD"])
    }

    fn entry(id: &str, parents: &[&str]) -> crate::git::LogEntry {
        crate::git::LogEntry {
            id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: String::new(),
            at: 0,
            subject: String::new(),
        }
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
    fn a_line_of_commits_is_one_column() {
        let placed = lay_out(&[entry("c", &["b"]), entry("b", &["a"]), entry("a", &[])]);
        let lanes: Vec<_> = placed
            .iter()
            .map(|p| (p.lane, p.up.clone(), p.down.clone()))
            .collect();
        assert_eq!(
            lanes,
            [
                (0, vec![], vec![0]),
                (0, vec![0], vec![0]),
                (0, vec![0], vec![])
            ]
        );
        assert!(placed.iter().all(|p| p.through.is_empty() && p.width == 1));
    }

    #[test]
    fn a_branch_takes_a_column_of_its_own_until_it_is_merged() {
        // m merges f into c; f and c both come from b.
        //   m      lane 0, down to c (0) and f (1)
        //   f      lane 1, c's line passes by at 0
        //   c      lane 0, f's line passes by at 1
        //   b      lane 0, both come into it
        let placed = lay_out(&[
            entry("m", &["c", "f"]),
            entry("f", &["b"]),
            entry("c", &["b"]),
            entry("b", &[]),
        ]);
        assert_eq!((placed[0].lane, placed[0].down.clone()), (0, vec![0, 1]));
        assert_eq!(
            (
                placed[1].lane,
                placed[1].up.clone(),
                placed[1].through.clone()
            ),
            (1, vec![1], vec![0])
        );
        assert_eq!((placed[2].lane, placed[2].through.clone()), (0, vec![1]));
        assert_eq!(
            (placed[3].lane, placed[3].up.clone()),
            (0, vec![0, 1]),
            "both lines end in it"
        );
        assert_eq!(placed.iter().map(|p| p.width).max(), Some(2));
    }

    #[test]
    fn a_tip_nothing_came_down_to_starts_a_column_of_its_own() {
        // Two branches' tips, newest first: `x` (from b) is no one's parent.
        let placed = lay_out(&[entry("c", &["b"]), entry("x", &["b"]), entry("b", &[])]);
        assert_eq!((placed[1].lane, placed[1].up.clone()), (1, vec![]));
        assert_eq!(
            placed[1].down,
            vec![0],
            "its parent is already waited for in column 0"
        );
        assert_eq!(placed[2].up, vec![0]);
    }

    #[test]
    fn the_graph_goes_back_past_the_fork_or_four_weeks_whichever_is_further() {
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = dir.path().join("r");
        std::fs::create_dir_all(&repo_dir).unwrap();
        git(&repo_dir, &["init", "-q", "-b", "main"]);
        let day = 24 * 60 * 60;
        let now = 1_800_000_000;
        // 60 old commits on main (a year ago), then a branch of 3 recent ones.
        let mut ids = Vec::new();
        for n in 0..60 {
            ids.push(commit_at(
                &repo_dir,
                "a.txt",
                &format!("main {n}\n"),
                now - 365 * day + n,
            ));
        }
        git(&repo_dir, &["checkout", "-q", "-b", "feature"]);
        for n in 0..3 {
            commit_at(&repo_dir, "a.txt", &format!("feature {n}\n"), now - day + n);
        }
        let repo = Repo::at(&repo_dir);
        let graph = graph_of(&repo, None, now).unwrap();
        // The 3, the fork, and 10 before it; the 4 weeks have only the 3.
        assert_eq!(graph.rows.len(), 3 + 1 + GRAPH_BEFORE_FORK);
        assert!(graph.more);
        assert!(
            graph.rows[0].refs.iter().any(|r| r.kind == "head")
                && graph.rows[0].refs.iter().any(|r| r.name == "feature")
        );
        assert!(
            graph.rows[3]
                .refs
                .iter()
                .any(|r| r.name == "main" && r.kind == "branch")
        );
        assert_eq!(graph.rows[3].commit.id, ids[59]);
        // Asked for more: as many as that.
        let more = graph_of(&repo, Some(40), now).unwrap();
        assert_eq!(more.rows.len(), 40);
        let all = graph_of(&repo, Some(1000), now).unwrap();
        assert_eq!(all.rows.len(), 63);
        assert!(!all.more);
        // Everything lately: four weeks of commits, more than the fork asks for.
        let busy = graph_of(&repo, None, now - 365 * day + 27 * day).unwrap();
        assert_eq!(
            busy.rows.len(),
            63,
            "all of them are within the four weeks then"
        );
    }

    #[test]
    fn the_suggested_base_leaves_something_to_review() {
        let dir = tempfile::tempdir().unwrap();
        let (c1, _) = repo_on_a_branch(&dir.path().join("r"));
        let r = dir.path().join("r");
        // On `feature`: where it left main.
        let info = describe_repo(&Repo::at(&r), String::new()).unwrap();
        assert_eq!(
            (info.suggested.commit.id.as_str(), info.suggested.why),
            (c1.as_str(), "fork")
        );
        // On `main` itself: the commit before `HEAD`.
        git(&r, &["checkout", "-q", "main"]);
        std::fs::write(r.join("c.txt"), "c\n").unwrap();
        git(&r, &["add", "-A"]);
        git(&r, &["commit", "-q", "-m", "c3"]);
        let info = describe_repo(&Repo::at(&r), String::new()).unwrap();
        assert_eq!(
            (info.suggested.commit.id.as_str(), info.suggested.why),
            (c1.as_str(), "previous")
        );
        // One commit, nothing before it: `HEAD`.
        let lone = dir.path().join("lone");
        std::fs::create_dir_all(&lone).unwrap();
        git(&lone, &["init", "-q", "-b", "main"]);
        std::fs::write(lone.join("a.txt"), "a\n").unwrap();
        git(&lone, &["add", "-A"]);
        git(&lone, &["commit", "-q", "-m", "c1"]);
        assert_eq!(
            describe_repo(&Repo::at(&lone), String::new())
                .unwrap()
                .suggested
                .why,
            "head"
        );
    }

    #[test]
    fn a_span_says_how_many_commits_and_files_and_which_commits() {
        let dir = tempfile::tempdir().unwrap();
        let (c1, c2) = repo_on_a_branch(dir.path());
        let span = span_of(&Repo::at(dir.path()), &c1, &c2).unwrap();
        assert_eq!((span.commits, span.files), (1, 2), "a.txt and b.txt");
        assert_eq!(span.ids, vec![c2.clone()]);
        let none = span_of(&Repo::at(dir.path()), &c2, &c2).unwrap();
        assert_eq!((none.commits, none.files, none.ids.len()), (0, 0, 0));
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
        assert_eq!(git.candidates[0].files, 1, "c1's tree: a.txt");
        assert_eq!(git.candidates[1].rev, "HEAD");
        assert_eq!(names(&git.candidates[1]), ["head"]);
        assert_eq!(git.candidates[1].files, 2, "c2's tree: a.txt, b.txt");
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
        assert_eq!((p.commit.id.as_str(), p.files), (c1.as_str(), 1));
        let s = setup.span("mobile-app", "main", "HEAD").unwrap();
        assert_eq!((s.commit.subject.as_str(), s.commits), ("c2", 1));
        assert_eq!(setup.span("mobile-app", "main", "main").unwrap().commits, 0);
        assert!(setup.span("mobile-app", "main", "no-such").is_err());
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
        assert_eq!(repos[0].target, None, "HEAD, said or not, is none");
        // A target of its own: what each round compares up to.
        let with = repos_chosen(dir.path(), &choice(r#"{"kind":"workspace","repos":[{"path":"mobile-app","base":"main","target":"main"}]}"#)).unwrap();
        assert_eq!(with[0].target.as_deref(), Some("main"));
        assert_eq!(with[0].target(), "main");
        assert_eq!(with[0].range.head, c1, "up to main, not HEAD");
        assert_eq!(with[0].range.spec, "main..main");
        assert!(setup.check(&choice(r#"{"kind":"workspace","repos":[{"path":"mobile-app","base":"main","target":"nowhere"}]}"#)).is_err(), "a target that is no commit");
        assert_eq!(target_of(Some(" HEAD ")), None);
        assert_eq!(target_of(Some("")), None);
        assert_eq!(target_of(Some(" v1 ")), Some("v1".to_string()));
        assert!(repo_from(dir.path(), "mobile-app", "main", Some("nowhere")).is_err());
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
