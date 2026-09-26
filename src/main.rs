use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// `println!` that stops quietly when the reader has gone (`diffnote config
/// get | head`), as other commands do, instead of panicking on the broken
/// pipe.
macro_rules! println {
    ($($arg:tt)*) => {{
        use std::io::Write;
        if let Err(e) = writeln!(std::io::stdout(), $($arg)*) {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
            eprintln!("{}", mf("main.macro.stdout_write_failed", &[("error", &e.to_string())]));
            std::process::exit(1);
        }
    }};
}

use diffnote::digest::digest;
use diffnote::messages::{m, mf};
use diffnote::model::Event;
use diffnote::{bundle, review};
use std::path::{Path, PathBuf};
use std::process::Command;
use time::OffsetDateTime;
use ulid::Ulid;

// CLI のヘルプ文は messages/ja.yaml にある(レビュー・編集しやすいよう一
// 箇所にまとめるため)。clap の derive 属性は `help`/`about` に任意の式
// (関数呼び出しも含む)を取れるので、doc コメントの代わりに `m("...")` で
// 引く。
#[derive(Debug, Parser)]
#[command(
    name = "diffnote",
    about = m("cli.about"),
    version,
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    next_help_heading = m("cli.heading_options"),
    subcommand_help_heading = m("cli.heading_commands"),
    subcommand_value_name = m("cli.heading_commands"),
    help_template = m("cli.help_template")
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
    #[arg(short = 'h', long, global = true, action = clap::ArgAction::Help, help = m("cli.help_flag"))]
    help: Option<bool>,
    #[arg(short = 'V', long, action = clap::ArgAction::Version, help = m("cli.version_flag"))]
    version: Option<bool>,
}

/// clap の組み込みの見出し(Usage など)を日本語にしたコマンド定義。
/// (`config get`/`set`/`unset` のように、何段ネストしていても効くように再帰する。)
fn command() -> clap::Command {
    use clap::CommandFactory;
    fn localize(cmd: clap::Command) -> clap::Command {
        cmd.help_template(m("cli.help_template"))
            .subcommand_help_heading(m("cli.heading_commands"))
            .subcommand_value_name(m("cli.heading_commands"))
            .mut_args(|a| {
                let heading = if a.is_positional() {
                    m("cli.heading_args")
                } else {
                    m("cli.heading_options")
                };
                a.help_heading(heading)
            })
            .mut_subcommands(localize)
    }
    localize(Cli::command())
}

#[derive(Debug, Subcommand)]
enum Cmd {
    #[command(about = m("cli.review.about"))]
    Review {
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true,
            help = m("cli.review.review")
        )]
        review: PathBuf,
        #[arg(value_name = "REV|DIR", help = m("cli.review.target"))]
        target: Option<String>,
        #[arg(long, value_name = "REV|DIR", help = m("cli.review.base"))]
        base: Option<String>,
        #[arg(long = "type", value_name = "TYPE", value_enum, hide_possible_values = true, help = m("cli.review.type"))]
        kind: Option<ReviewType>,
        #[arg(long, value_enum, hide_possible_values = true, help = m("cli.review.snapshot"))]
        snapshot: Option<diffnote::bundle::SnapshotMode>,
        #[arg(long, value_name = "TITLE", help = m("cli.review.title"))]
        title: Option<String>,
        #[command(flatten)]
        server: Server,
    },
    #[command(about = m("cli.open.about"))]
    Open {
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true,
            help = m("cli.open.review")
        )]
        review: PathBuf,
        #[command(flatten)]
        server: Server,
    },
    #[command(about = m("cli.export.about"))]
    Export {
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true,
            help = m("cli.export.review")
        )]
        review: PathBuf,
        #[arg(long, short, help = m("cli.export.output"))]
        output: Option<PathBuf>,
        #[arg(index = 1, value_name = "OUTPUT", help = m("cli.export.output_pos"))]
        output_pos: Option<PathBuf>,
        #[arg(long, value_name = m("cli.export.expand_limit_value_name"), default_value = "5000", value_parser = parse_expand_limit, help = m("cli.export.expand_limit"))]
        expand_limit: diffnote::html::ExpandLimit,
    },
    #[command(about = m("cli.config.about"))]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

/// How the page is served: the same for `review` and `open`.
#[derive(Debug, clap::Args)]
struct Server {
    #[arg(long, value_name = "PORT", default_value_t = 0, help = m("cli.server.port"))]
    port: u16,
    #[arg(long, help = m("cli.server.no_browser"))]
    no_browser: bool,
    #[arg(long, value_name = "DIR", help = m("cli.server.repo"))]
    repo: Option<PathBuf>,
}

/// What a review is of: commits of a git repository, several repositories
/// under one directory (`workspace`), or a directory's files as they are
/// (`raw`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ReviewType {
    Git,
    Workspace,
    Raw,
}

impl ReviewType {
    fn name(self) -> &'static str {
        match self {
            ReviewType::Git => "git",
            ReviewType::Workspace => "workspace",
            ReviewType::Raw => "raw",
        }
    }

    fn of(source: &diffnote::model::Source) -> Self {
        match source {
            diffnote::model::Source::Git(_) => ReviewType::Git,
            diffnote::model::Source::Workspace(_) => ReviewType::Workspace,
            diffnote::model::Source::Files { .. } => ReviewType::Raw,
        }
    }
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    #[command(about = m("cli.config.get.about"))]
    Get {
        #[arg(value_enum, hide_possible_values = true, help = m("cli.config.get.key"))]
        key: Option<ConfigKey>,
    },
    #[command(about = m("cli.config.set.about"))]
    Set {
        #[arg(value_enum, hide_possible_values = true, help = m("cli.config.set.key"))]
        key: ConfigKey,
        #[arg(help = m("cli.config.set.value"))]
        value: String,
    },
    #[command(about = m("cli.config.unset.about"))]
    Unset {
        #[arg(value_enum, hide_possible_values = true, help = m("cli.config.unset.key"))]
        key: ConfigKey,
    },
}

/// `diffnote config` で扱える設定項目(今のところ `author` のみ)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ConfigKey {
    Author,
}

fn main() -> Result<()> {
    let cli = {
        use clap::FromArgMatches;
        Cli::from_arg_matches(&command().get_matches())?
    };
    match cli.command {
        Cmd::Review {
            review,
            target,
            base,
            kind,
            snapshot,
            title,
            server,
        } => cmd_serve(
            review,
            server,
            Some(Compare {
                target,
                base,
                kind,
                snapshot,
                title,
            }),
        ),
        Cmd::Open { review, server } => cmd_serve(review, server, None),
        Cmd::Export {
            review,
            output,
            output_pos,
            expand_limit,
        } => {
            let output = match (output, output_pos) {
                (Some(o), None) | (None, Some(o)) => o,
                (Some(_), Some(_)) => {
                    anyhow::bail!(m("main.output_path_conflict"))
                }
                (None, None) => {
                    anyhow::bail!(m("main.output_path_missing"))
                }
            };
            cmd_export(review, output, expand_limit)
        }
        Cmd::Config { action } => cmd_config(action),
    }
}

fn cmd_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Get { key: Some(key) } => {
            let config = diffnote::user_config::load();
            match config_field(&config, key) {
                Some(value) => println!("{value}"),
                None => println!(
                    "{}",
                    mf("main.config.not_set", &[("key", config_key_name(key))])
                ),
            }
        }
        ConfigAction::Get { key: None } => {
            let config = diffnote::user_config::load();
            let mut any = false;
            for key in [ConfigKey::Author] {
                if let Some(value) = config_field(&config, key) {
                    println!(
                        "{}",
                        mf(
                            "main.config.entry",
                            &[("key", config_key_name(key)), ("value", value)]
                        )
                    );
                    any = true;
                }
            }
            if !any {
                println!("{}", m("main.config.none_set"));
            }
            if let Some(path) = diffnote::user_config::path() {
                println!(
                    "{}",
                    mf(
                        "main.config.file_path",
                        &[("path", &path.display().to_string())]
                    )
                );
            }
        }
        ConfigAction::Set { key, value } => {
            let value = value.trim();
            if value.is_empty() {
                anyhow::bail!(mf(
                    "main.config.empty_value_refused",
                    &[("key", config_key_name(key))]
                ));
            }
            let mut config = diffnote::user_config::load();
            set_config_field(&mut config, key, Some(value.to_string()));
            diffnote::user_config::save(&config)?;
            println!(
                "{}",
                mf(
                    "main.config.set_ok",
                    &[("key", config_key_name(key)), ("value", value)]
                )
            );
        }
        ConfigAction::Unset { key } => {
            let mut config = diffnote::user_config::load();
            set_config_field(&mut config, key, None);
            diffnote::user_config::save(&config)?;
            println!(
                "{}",
                mf("main.config.unset_ok", &[("key", config_key_name(key))])
            );
        }
    }
    Ok(())
}

fn config_key_name(key: ConfigKey) -> &'static str {
    match key {
        ConfigKey::Author => "author",
    }
}

fn config_field(config: &diffnote::user_config::UserConfig, key: ConfigKey) -> Option<&str> {
    match key {
        ConfigKey::Author => config.author.as_deref(),
    }
}

fn set_config_field(
    config: &mut diffnote::user_config::UserConfig,
    key: ConfigKey,
    value: Option<String>,
) {
    match key {
        ConfigKey::Author => config.author = value,
    }
}

/// For `review`, what it is given to compare: the
/// changes from the base to the target are recorded as a revision of the
/// bundle (made if there is none yet, for git), so that they can be reviewed in
/// the browser.
///
/// - A git bundle: the target is a commit (`HEAD` if none is named).
/// - A directory bundle: the target is the directory to compare with the base
///   snapshot (`.` if none is named, as git's is `HEAD`).
/// - No bundle yet: `base` (or the target's parent) starts it.
///
/// Nothing is written if the diff is empty or is already recorded (or if
/// `apply` is off: then it only says whether there is something to add).
/// What `add_revision` is asked to add, and how.
struct Ask<'a> {
    base: Option<&'a str>,
    target: Option<&'a str>,
    kind: Option<ReviewType>,
    /// A review of several repositories made by this call: which ones, each
    /// from where (as chosen on the first screen). `None`: the ones found
    /// under the directory, each from its default base.
    repos: Option<Vec<diffnote::model::RepoSource>>,
    /// What a review made by this call keeps. Refused earlier if the review
    /// already exists, so it can only apply here.
    snapshot: Option<bundle::SnapshotMode>,
    /// Its title, likewise.
    title: Option<&'a str>,
    /// Whether to record what is found (`false`: only say whether there is
    /// anything, and write nothing).
    apply: bool,
    /// Whether someone is at the terminal to answer a question (a full
    /// snapshot that is big). From the page, there is no one.
    terminal: bool,
}

/// Adds what `ask` names to the review at `review_path`, making the review
/// if there is none: what it did, in a line, or `None` when there was
/// nothing to add. A review made here is kept even with nothing to show yet
/// (its base is what it records; what comes later is reviewed from it).
fn add_revision(
    review_path: &Path,
    repo: &diffnote::git::Repo,
    ask: Ask,
) -> Result<Option<String>> {
    let Ask {
        base,
        target,
        kind,
        repos: chosen,
        snapshot,
        title,
        apply,
        terminal,
    } = ask;
    let mut loaded = bundle::load(review_path)?;
    let explicit = base.is_some() || target.is_some() || kind.is_some();
    let mut fresh = FreshBundle(None);
    let exclude = [review_path.to_path_buf()];
    let project = Path::new(".");
    let review_kind = match loaded.source() {
        Some(source) => {
            let is = ReviewType::of(source);
            // The type is the review's: asking for another is a mistake.
            if kind.is_some_and(|k| k != is) {
                anyhow::bail!(mf("main.type.mismatch", &[("type", is.name())]));
            }
            Some(is)
        }
        // (Repositories chosen by hand may all be deeper than they are
        // looked for: the choice is what counts.)
        None if chosen.is_some() => Some(ReviewType::Workspace),
        None if explicit => Some(kind_of(repo, project, kind)?),
        None => None,
    };
    let input = if review_kind == Some(ReviewType::Workspace) {
        if base.is_some() || target.is_some() {
            anyhow::bail!(m("main.workspace.no_target"));
        }
        let (repos, away) = workspace_range(&loaded, project, chosen)?;
        if !apply {
            // Only the heads: has any repository moved since the last revision?
            let last = loaded.revisions().last().and_then(|r| match &r.source {
                diffnote::model::Source::Workspace(w) => Some(w),
                _ => None,
            });
            // (Or the repositories themselves are not the last revision's:
            // one added or taken out since.)
            let moved = last.is_none_or(|w| {
                w.repos.len() != repos.len()
                    || repos.iter().any(|r| {
                        w.repos
                            .iter()
                            .find(|k| k.path == r.path)
                            .is_none_or(|k| k.range.head != r.range.head)
                    })
            });
            return Ok(moved.then(String::new));
        }
        // Said at once, whether or not anything is recorded this time.
        if !away.is_empty() {
            println!(
                "{}",
                mf(
                    "main.workspace.missing_repos",
                    &[("repos", &away.join(", "))],
                )
            );
        }
        // What a repository that isn't here was last recorded as: its part
        // of the last diff, and its files' versions, carried on as they are.
        let last = loaded
            .revisions()
            .last()
            .map(|r| (loaded.revision_diff(r).unwrap_or_default(), r.files.clone()));
        workspace_input(project, repos, last)?
    } else if review_kind == Some(ReviewType::Raw) {
        if loaded.source().is_none() {
            let Some(base_dir) = base else {
                anyhow::bail!(m("main.needs_a_base"));
            };
            let count = start_files(review_path, Path::new(base_dir), title)?;
            println!(
                "{}",
                mf(
                    "main.add_revision.started_files",
                    &[("count", &count.to_string())]
                )
            );
            fresh = FreshBundle(Some(review_path.to_path_buf()));
            loaded = bundle::load(review_path)?;
        } else if let Some(base_dir) = base {
            check_files_base(&loaded, Path::new(base_dir), &exclude)?;
        }
        // As a git review follows `HEAD`, a directory review follows where
        // it is run.
        let dir = target.unwrap_or(".");
        // Reading the directory is too much to do each time it is only looked at.
        if !apply {
            return Ok(None);
        }
        files_input(&loaded, Path::new(dir), &exclude)?
    } else {
        match loaded.source() {
            Some(_) => {
                if !explicit && !repo.exists() {
                    return Ok(None);
                }
            }
            None if !explicit => {
                anyhow::bail!(mf(
                    "main.no_bundle",
                    &[("path", &review_path.display().to_string())]
                ));
            }
            None => {}
        }
        let range = git_range(repo, &loaded, base, target)?;
        if !apply {
            // Only the commit ids (not the diff): is this one not yet recorded?
            let recorded = loaded.revisions().any(
                |r| matches!(&r.source, diffnote::model::Source::Git(g) if g.head == range.head),
            );
            return Ok((!recorded).then(String::new));
        }
        git_input(repo.clone(), range)?
    };
    // A review that has revisions already gets one only for something new.
    // (One made by this call, with nothing to add: kept, with its base.)
    // Of several repositories, where they stand is part of what is new: a
    // revision with nothing to show is still recorded when a repository was
    // added, taken out, or moved (see `workspace_input`), so that what
    // stands recorded is where they are.
    let first = loaded.revisions().next().is_none();
    let of_several = matches!(input.source, diffnote::model::Source::Workspace(_));
    let nothing_to_show = input.diff_text.trim().is_empty() && !of_several;
    if !first && (nothing_to_show || loaded.revisions().any(|r| r.digest == input.digest)) {
        fresh.keep();
        return Ok(None);
    }
    let Input {
        diff_text,
        files,
        new_files,
        base_files,
        source,
        digest,
        tree_size,
        head_some,
        head_all,
    } = input;
    let nothing_yet = diff_text.trim().is_empty();
    let said = match &source {
        // Only the base so far: what comes later is reviewed from it.
        diffnote::model::Source::Git(g) if nothing_yet => {
            let short = |id: &str| id[..id.len().min(10)].to_string();
            let named = g.spec.split_once("..").map_or(g.spec.as_str(), |(b, _)| b);
            mf(
                "main.add_revision.started_git",
                &[("base", named), ("commit", &short(&g.base))],
            )
        }
        diffnote::model::Source::Git(g) => {
            let short = |id: &str| id[..id.len().min(10)].to_string();
            mf(
                "main.add_revision.recorded_git",
                &[("from", &short(&g.base)), ("to", &short(&g.head))],
            )
        }
        diffnote::model::Source::Workspace(w) if nothing_yet && first => mf(
            "main.add_revision.started_workspace",
            &[("n", &w.repos.len().to_string())],
        ),
        diffnote::model::Source::Workspace(_) if nothing_yet => {
            m("main.add_revision.moved_workspace").to_string()
        }
        diffnote::model::Source::Workspace(w) => mf(
            "main.add_revision.recorded_workspace",
            &[("n", &w.repos.len().to_string())],
        ),
        diffnote::model::Source::Files { .. } => m("main.add_revision.recorded_files").to_string(),
    };
    let is_git = !matches!(source, diffnote::model::Source::Files { .. });
    // The trail this head was reached by, read while the repository is at
    // hand: the review keeps it, since an exported page has nothing to ask.
    let commits = match &source {
        diffnote::model::Source::Git(g) => repo.log(&g.base, &g.head).unwrap_or_default(),
        // Each repository's own trail, each commit said to be in it.
        diffnote::model::Source::Workspace(w) => w
            .repos
            .iter()
            .flat_map(|r| {
                let git = diffnote::git::Repo::at(project.join(&r.path));
                git.log(&r.range.base, &r.range.head)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |(id, mut about)| {
                        about.repo = Some(r.path.clone());
                        (id, about)
                    })
            })
            .collect(),
        diffnote::model::Source::Files { .. } => Vec::new(),
    };
    let mode = diffnote::record::pick_snapshot_mode(snapshot, loaded.snapshot_mode(), &source);
    let mut new_events = Vec::new();
    if loaded.events.is_empty() {
        // The review is made by this call: what it is called is set now.
        if let Some(title) = title {
            review::set_title(&mut loaded.settings, title);
        }
        new_events.push(Event::Meta {
            version: 1,
            created_at: OffsetDateTime::now_utc(),
            description: None,
            context_lines: 3,
        });
    }
    let additions = diffnote::record::record_session(
        &loaded,
        &mut new_events,
        diffnote::record::Capture {
            diff_text: &diff_text,
            diff_digest: &digest,
            source,
            files: &files,
            new_files: &new_files,
            base_files: &base_files,
            commits: &commits,
        },
        &|| confirm_snapshot_size(mode, is_git && terminal, tree_size),
        &*head_some,
        head_all,
    )?;
    let mut events = loaded.events.clone();
    events.extend(new_events);
    bundle::save(review_path, &loaded, &events, &additions)?;
    fresh.keep();
    Ok(Some(said))
}

/// The repository to work in: the one named by `--repo`, else the one the
/// current directory is in (not checked until it is used).
fn repo_of(dir: Option<PathBuf>) -> Result<diffnote::git::Repo> {
    match dir {
        Some(dir) => {
            let repo = diffnote::git::Repo::at(&dir);
            if !repo.exists() {
                anyhow::bail!(mf(
                    "main.repo_of.not_a_repo",
                    &[("dir", &dir.display().to_string())]
                ));
            }
            Ok(repo)
        }
        None => Ok(diffnote::git::Repo::current()),
    }
}

/// What `review` is told to compare: the target, and (if there is no
/// review yet) the base to start from and what the review is of.
struct Compare {
    target: Option<String>,
    base: Option<String>,
    kind: Option<ReviewType>,
    /// What a review made here keeps. Only when there is no review yet: the
    /// range belongs to the review, and is decided once.
    snapshot: Option<bundle::SnapshotMode>,
    /// Its title, likewise (afterwards, the page's 設定 changes it).
    title: Option<String>,
}

/// `review` (with what to compare) and `open` (without: the review as it is
/// recorded, nothing added) both serve the review in the browser.
fn cmd_serve(review: PathBuf, server: Server, compare: Option<Compare>) -> Result<()> {
    let Server {
        port,
        no_browser,
        repo,
    } = server;
    let open = !no_browser;
    let reopen = compare.is_none();
    let Compare {
        target,
        base,
        kind,
        snapshot,
        title,
    } = compare.unwrap_or(Compare {
        target: None,
        base: None,
        kind: None,
        snapshot: None,
        title: None,
    });
    if title.is_some() && review.exists() {
        anyhow::bail!(m("main.title.locked"));
    }
    // The range a review keeps is decided when it is made, and never again:
    // asking an existing review for another one is refused rather than
    // ignored. (Saying again what it already is, as with `--base`, is fine.)
    if let Some(asked) = snapshot {
        if let Some(mode) = bundle::load(&review).ok().and_then(|l| l.snapshot_mode())
            && mode != asked
        {
            anyhow::bail!(mf("main.snapshot.locked", &[("mode", mode_name(mode))]));
        }
        if kind == Some(ReviewType::Raw) && asked == bundle::SnapshotMode::Changed {
            anyhow::bail!(m("main.snapshot.dir_changed_refused"));
        }
    }
    let git = repo_of(repo.clone())?;
    let explicit = target.is_some() || base.is_some() || kind.is_some();
    if reopen && !review.exists() {
        anyhow::bail!(mf(
            "main.serve.open_no_bundle",
            &[("path", &review.display().to_string())]
        ));
    }
    // No review yet, and nothing said of what it is to be: the page asks
    // (the first screen), and makes the review from the answers.
    let first_screen = !reopen && !review.exists() && !explicit;
    // What was asked for is added to the review, so it can be reviewed here (a
    // failure to do what was asked stops; one to do what was not, only says so).
    // As the review was before any of this: what「保存せずに終了」goes back to.
    let before = std::fs::read(&review).ok();
    if reopen {
        if bundle::load(&review)?.revisions().next().is_none() {
            anyhow::bail!(mf(
                "main.serve.no_revision",
                &[("path", &review.display().to_string())]
            ));
        }
    } else if !first_screen {
        match add_revision(
            &review,
            &git,
            Ask {
                base: base.as_deref(),
                target: target.as_deref(),
                kind,
                repos: None,
                snapshot,
                title: title.as_deref(),
                apply: true,
                terminal: true,
            },
        ) {
            Ok(said) => {
                if let Some(said) = said {
                    println!("{said}");
                }
            }
            Err(e) if explicit => return Err(e),
            Err(e) => println!(
                "{}",
                mf("main.serve.refresh_failed", &[("error", &e.to_string())])
            ),
        }
    }
    if !first_screen {
        if !review.exists() {
            anyhow::bail!(m("main.serve.no_diff_at_all"));
        }
        if bundle::load(&review)
            .ok()
            .is_some_and(|l| diffnote::html::view_model(&l).is_err())
        {
            println!("{}", m("main.serve.no_diff_yet"));
        }
    }
    // The page's button: what was added to the target since (the base is
    // already the review's; a named commit doesn't move, `HEAD` does).
    // `open` has none: nothing should be added in this session at all.
    let refresher: Option<diffnote::serve::Refresher> = if reopen {
        None
    } else {
        let (review, git, target) = (review.clone(), git.clone(), target.clone());
        Some(std::sync::Arc::new(move |apply| {
            // Later rounds add to a review that exists: the range is the
            // one it was made with.
            add_revision(
                &review,
                &git,
                Ask {
                    base: None,
                    target: target.as_deref(),
                    kind,
                    repos: None,
                    snapshot: None,
                    title: None,
                    apply,
                    terminal: false,
                },
            )
        }))
    };
    let setup = first_screen.then(|| {
        let project = PathBuf::from(".");
        let (review_path, git_repo, at) = (review.clone(), git.clone(), project.clone());
        std::sync::Arc::new(diffnote::setup::Setup {
            review: review.clone(),
            project,
            repo: git.clone(),
            title: title.clone(),
            snapshot,
            create: std::sync::Arc::new(move |choice| {
                first_review(&review_path, &git_repo, &at, choice)
            }),
        })
    });
    let options = diffnote::serve::Options {
        review,
        port,
        author: None,
        repo,
        refresh: refresher,
        before: Some(before),
        setup,
    };
    diffnote::serve::run(&options, |url, notices| {
        for notice in notices {
            println!("{}", mf("main.notice_prefix", &[("notice", notice)]));
        }
        if open {
            println!("{}", mf("main.serve.opening_browser", &[("url", url)]));
        } else {
            println!("{}", mf("main.serve.at", &[("url", url)]));
        }
        println!("{}", m("main.serve.quit_hint"));
        if open && !open_in_browser(url) {
            println!("{}", m("main.serve.open_failed"));
        }
    })
}

/// Asks the system to open `url` in the default browser.
fn open_in_browser(url: &str) -> bool {
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("cmd", vec!["/C", "start", "", url])
    } else if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else {
        ("xdg-open", vec![url])
    };
    Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn parse_expand_limit(s: &str) -> Result<diffnote::html::ExpandLimit, String> {
    if s.eq_ignore_ascii_case("all") {
        return Ok(diffnote::html::ExpandLimit::All);
    }
    s.parse::<usize>()
        .map(diffnote::html::ExpandLimit::Lines)
        .map_err(|_| m("main.export.expand_limit_invalid").to_string())
}

fn cmd_export(
    review_path: PathBuf,
    output_path: PathBuf,
    limit: diffnote::html::ExpandLimit,
) -> Result<()> {
    let loaded = bundle::load(&review_path)?;
    let html = diffnote::html::render_export_with(&loaded, limit).with_context(|| {
        mf(
            "main.export.no_diff",
            &[("path", &review_path.display().to_string())],
        )
    })?;
    std::fs::write(&output_path, html).with_context(|| {
        mf(
            "main.export.write_failed",
            &[("path", &output_path.display().to_string())],
        )
    })?;
    println!(
        "{}",
        mf(
            "main.export.wrote",
            &[("path", &output_path.display().to_string())]
        )
    );
    Ok(())
}

/// Reads the head-side content of the given paths (those that exist).
type HeadSome = Box<dyn Fn(&[String]) -> Result<Vec<(String, Vec<u8>)>>>;
/// Reads the whole head tree.
type HeadAll = Box<dyn FnOnce() -> Result<Vec<(String, Vec<u8>)>>>;

/// What a round of review compares: the diff, plus everything needed to
/// record it as a `Revision`.
struct Input {
    diff_text: String,
    /// Per-file digests of the files the diff touches.
    files: Vec<diffnote::model::FileDigest>,
    /// Full head-side content of the files the diff touches.
    new_files: diffnote::files::Tree,
    /// The base-side counterparts, when the base isn't already in the bundle
    /// (git reviews; a directory review's base is its previous revision).
    base_files: diffnote::files::Tree,
    source: diffnote::model::Source,
    /// The revision's digest (see `Revision::digest`).
    digest: String,
    /// Total size of the tree a `full` snapshot would store.
    tree_size: u64,
    /// The head content of specific files (the ones comments refer to).
    head_some: HeadSome,
    /// The whole head tree (for a `full` snapshot).
    head_all: HeadAll,
}

/// What a git review compares: from the bundle's base up to the target.
///
/// - With a bundle, the base is the one its first revision has; `base`, if
///   given, must be that same commit. The target is `HEAD` if none is named.
/// - With none, `base` starts it (`review --base main feature`); without `base`,
///   the target alone is a commit's own changes (from its first parent).
fn git_range(
    repo: &diffnote::git::Repo,
    loaded: &bundle::Loaded,
    base: Option<&str>,
    target: Option<&str>,
) -> Result<diffnote::model::GitSource> {
    if target.is_some_and(|t| t.contains("..")) {
        anyhow::bail!(m("main.git_range.no_ranges"));
    }
    let first = loaded.revisions().next().map(|r| &r.source);
    match first {
        Some(diffnote::model::Source::Git(first)) => {
            if let Some(base) = base {
                let asked = repo.commit_id(base)?;
                if asked != first.base {
                    anyhow::bail!(mf(
                        "main.git_range.base_locked",
                        &[("base", &first.base[..first.base.len().min(10)])]
                    ));
                }
            }
            // What the base was called when it was set (`--base main`, or
            // a commit reviewed by itself, whose base is its parent).
            let label = match first.spec.split_once("..") {
                Some((base, _)) => base.to_string(),
                None if first.base == first.head => first.spec.clone(),
                None => first.base[..first.base.len().min(10)].to_string(),
            };
            let target = target.unwrap_or("HEAD");
            Ok(diffnote::model::GitSource {
                base: first.base.clone(),
                head: repo.commit_id(target)?,
                spec: format!("{label}..{target}"),
            })
        }
        Some(diffnote::model::Source::Files { .. }) => {
            anyhow::bail!(m("main.git_range.dir_bundle"))
        }
        Some(diffnote::model::Source::Workspace(_)) => {
            anyhow::bail!(m("main.workspace.no_target"))
        }
        None => match (base, target) {
            (Some(base), target) => repo.between(base, target.unwrap_or("HEAD")),
            (None, Some(target)) => repo.commit_range(target),
            (None, None) => anyhow::bail!(m("main.git_range.no_target")),
        },
    }
}

fn git_input(repo: diffnote::git::Repo, range: diffnote::model::GitSource) -> Result<Input> {
    let diff_text = repo.diff(&range)?;
    let parsed = diffnote::diff::parse(&diff_text).map_err(|e| anyhow::anyhow!("{e}"))?;
    let head_tree = repo.ls_tree(&range.head)?;
    let base_tree = repo.ls_tree(&range.base)?;
    let files = file_digests(&repo, &base_tree, &head_tree, &parsed)?;
    let touched_new: Vec<String> = parsed
        .files
        .iter()
        .filter_map(|f| f.new_path.clone())
        .collect();
    let touched_old: Vec<String> = parsed
        .files
        .iter()
        .filter_map(|f| f.old_path.clone())
        .collect();
    let new_files = repo
        .read_paths(&head_tree, &touched_new)?
        .into_iter()
        .collect();
    let base_files = repo
        .read_paths(&base_tree, &touched_old)?
        .into_iter()
        .collect();
    let tree_size = head_tree.iter().map(|e| e.size).sum();
    let repo = std::rc::Rc::new(repo);
    let head_tree = std::rc::Rc::new(head_tree);
    let (repo_all, tree_all) = (repo.clone(), head_tree.clone());
    Ok(Input {
        digest: digest(&diff_text),
        tree_size,
        source: diffnote::model::Source::Git(range),
        files,
        new_files,
        base_files,
        head_some: Box::new(move |paths| repo.read_paths(&head_tree, paths)),
        head_all: Box::new(move || {
            let all: Vec<String> = tree_all.iter().map(|e| e.path.clone()).collect();
            repo_all.read_paths(&tree_all, &all)
        }),
        diff_text,
    })
}

/// Compares `dir` with the bundle's base (its first snapshot).
fn files_input(loaded: &bundle::Loaded, dir: &Path, exclude: &[PathBuf]) -> Result<Input> {
    let first = loaded
        .revisions()
        .next()
        .context(m("main.files_input.no_snapshot"))?;
    let previous = loaded.tree_of(first);
    let current = diffnote::files::read_tree(dir, exclude)?;
    let digest = diffnote::files::tree_digest(&current);
    // The same as a revision already recorded: reopen the diff that revision
    // was reviewed with (so replies/resolves can still be added), rather than
    // compare it with the base again.
    let (diff_text, files, base) = match loaded.revisions().filter(|r| r.digest == digest).last() {
        Some(rev) => {
            let base = match &rev.source {
                diffnote::model::Source::Files { base } => base.clone(),
                _ => None,
            };
            (
                loaded.revision_diff(rev).unwrap_or_default(),
                rev.files.clone(),
                base,
            )
        }
        None => {
            let (text, files) = diffnote::files::diff_trees(&previous, &current);
            (text, files, Some(first.digest.clone()))
        }
    };
    let (some_tree, all_tree) = (current.clone(), current.clone());
    Ok(Input {
        digest,
        tree_size: current.values().map(|b| b.len() as u64).sum(),
        source: diffnote::model::Source::Files { base },
        files,
        new_files: current,
        base_files: Default::default(),
        head_some: Box::new(move |paths| {
            Ok(paths
                .iter()
                .filter_map(|p| Some((p.clone(), some_tree.get(p)?.clone())))
                .collect())
        }),
        head_all: Box::new(move || Ok(all_tree.into_iter().collect())),
        diff_text,
    })
}

/// `--base DIR` for a bundle that has a base already: it is fine if it is the
/// same content (the bundle's base can't change), an error if not.
fn check_files_base(loaded: &bundle::Loaded, dir: &Path, exclude: &[PathBuf]) -> Result<()> {
    let Some(first) = loaded.revisions().next() else {
        return Ok(());
    };
    let asked = diffnote::files::tree_digest(&diffnote::files::read_tree(dir, exclude)?);
    if asked != first.digest {
        anyhow::bail!(mf(
            "main.check_files_base.mismatch",
            &[("dir", &dir.display().to_string())]
        ));
    }
    Ok(())
}

/// A bundle made by this run: removed again if the run leaves nothing worth
/// keeping in it (no comment, no diff), so that looking doesn't leave one.
struct FreshBundle(Option<PathBuf>);

impl FreshBundle {
    fn keep(&mut self) {
        self.0 = None;
    }
}

impl Drop for FreshBundle {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// What a review that has no bundle yet is of: what was asked for
/// (`--type`), else what the directory is -- a git repository, one with
/// repositories under it (`workspace`), or neither (`raw`). Asking for what
/// the directory can't be (`git` outside a repository, `workspace` with none
/// under it) is a mistake, not a fallback.
fn kind_of(
    repo: &diffnote::git::Repo,
    project: &Path,
    kind: Option<ReviewType>,
) -> Result<ReviewType> {
    match kind {
        Some(ReviewType::Raw) => Ok(ReviewType::Raw),
        Some(ReviewType::Git) if !repo.exists() => anyhow::bail!(m("main.type.git_no_repo")),
        Some(ReviewType::Git) => Ok(ReviewType::Git),
        Some(ReviewType::Workspace) if diffnote::files::find_repos(project)?.is_empty() => {
            anyhow::bail!(m("main.type.workspace_no_repos"))
        }
        Some(ReviewType::Workspace) => Ok(ReviewType::Workspace),
        None if repo.exists() => Ok(ReviewType::Git),
        None if !diffnote::files::find_repos(project)?.is_empty() => Ok(ReviewType::Workspace),
        None => Ok(ReviewType::Raw),
    }
}

/// The repositories a review of several compares, each from its base to
/// its head now, and the ones that aren't where the review says they are
/// (kept as last recorded, so nothing of them is lost or read).
///
/// - With a bundle, the repositories and their bases are the review's now
///   (see `review::repos_of`); each head is its `HEAD` now.
/// - With none, the repositories are the ones `chosen` (on the first screen,
///   each from the commit chosen there), else those found under `project`,
///   each from where its work left the default branch (see
///   `Repo::default_base`).
fn workspace_range(
    loaded: &bundle::Loaded,
    project: &Path,
    chosen: Option<Vec<diffnote::model::RepoSource>>,
) -> Result<(Vec<diffnote::model::RepoSource>, Vec<String>)> {
    let short = |id: &str| id[..id.len().min(10)].to_string();
    let mut away = Vec::new();
    let first = loaded.revisions().next().map(|r| &r.source);
    let repos = match first {
        Some(diffnote::model::Source::Workspace(first)) => {
            let last = loaded
                .revisions()
                .last()
                .and_then(|r| match &r.source {
                    diffnote::model::Source::Workspace(w) => Some(w),
                    _ => None,
                })
                .unwrap_or(first);
            // The repositories as they are now: added or taken out on the
            // page since (a taken-out one is in no revision from here on).
            let known = review::repos_of(loaded).unwrap_or_default();
            let mut repos = Vec::new();
            for known in &known {
                let git = diffnote::git::Repo::at(project.join(&known.path));
                let head = if git.exists() {
                    git.commit_id(known.target())?
                } else {
                    away.push(known.path.clone());
                    last.repos
                        .iter()
                        .find(|r| r.path == known.path)
                        .map_or(known.range.head.clone(), |r| r.range.head.clone())
                };
                repos.push(diffnote::model::RepoSource {
                    path: known.path.clone(),
                    range: diffnote::model::GitSource {
                        base: known.range.base.clone(),
                        spec: format!("{}..{}", short(&known.range.base), known.target()),
                        head,
                    },
                    target: known.target.clone(),
                });
            }
            repos
        }
        Some(_) => anyhow::bail!(m("main.workspace.not_one")),
        None if chosen.is_some() => chosen.unwrap_or_default(),
        None => {
            let mut repos = Vec::new();
            for path in diffnote::files::find_repos(project)? {
                let git = diffnote::git::Repo::at(project.join(&path));
                let base = git.default_base()?;
                let head = git.commit_id("HEAD")?;
                repos.push(diffnote::model::RepoSource {
                    path,
                    range: diffnote::model::GitSource {
                        spec: format!("{}..HEAD", short(&base)),
                        base,
                        head,
                    },
                    target: None,
                });
            }
            repos
        }
    };
    Ok((repos, away))
}

/// One repository's part of a review of several: its diff, with every path
/// under its directory, and its trees the same way, so that the parts add
/// up as one review would.
struct RepoPart {
    repo: diffnote::git::Repo,
    text: String,
    head_tree: Vec<diffnote::git::TreeEntry>,
    base_tree: Vec<diffnote::git::TreeEntry>,
}

fn repo_part(project: &Path, source: &diffnote::model::RepoSource) -> Result<Option<RepoPart>> {
    let repo = diffnote::git::Repo::at(project.join(&source.path));
    if !repo.exists() {
        return Ok(None);
    }
    let under = |tree: Vec<diffnote::git::TreeEntry>| -> Vec<diffnote::git::TreeEntry> {
        tree.into_iter()
            .map(|e| diffnote::git::TreeEntry {
                path: format!("{}/{}", source.path, e.path),
                ..e
            })
            .collect()
    };
    Ok(Some(RepoPart {
        text: repo.diff_under(&source.range, &source.path)?,
        head_tree: under(repo.ls_tree(&source.range.head)?),
        base_tree: under(repo.ls_tree(&source.range.base)?),
        repo,
    }))
}

/// A repository of a review of several, with its head tree, to read from.
type RepoTree = (
    String,
    std::rc::Rc<diffnote::git::Repo>,
    std::rc::Rc<Vec<diffnote::git::TreeEntry>>,
);

/// The part of a review's diff (of several repositories) that is `repo`'s:
/// the files under its directory, verbatim.
fn part_of_diff(text: &str, repo: &str) -> String {
    let mine = format!("diff --git a/{repo}/");
    let mut out = String::new();
    let mut keep = false;
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            keep = line.starts_with(&mine);
        }
        if keep {
            out.push_str(line);
        }
    }
    out
}

fn workspace_input(
    project: &Path,
    repos: Vec<diffnote::model::RepoSource>,
    last: Option<(String, Vec<diffnote::model::FileDigest>)>,
) -> Result<Input> {
    let mut diff_text = String::new();
    let mut files = Vec::new();
    let mut new_files = diffnote::files::Tree::new();
    let mut base_files = diffnote::files::Tree::new();
    let mut tree_size = 0;
    let mut parts: Vec<RepoTree> = Vec::new();
    for source in &repos {
        let Some(part) = repo_part(project, source)? else {
            // Not here: as it was last recorded (its content is in the bundle).
            if let Some((text, digests)) = &last {
                diff_text.push_str(&part_of_diff(text, &source.path));
                let under = format!("{}/", source.path);
                files.extend(
                    digests
                        .iter()
                        .filter(|f| {
                            f.new_path
                                .as_deref()
                                .or(f.old_path.as_deref())
                                .is_some_and(|p| p.starts_with(&under))
                        })
                        .cloned(),
                );
            }
            continue;
        };
        let parsed = diffnote::diff::parse(&part.text).map_err(|e| anyhow::anyhow!("{e}"))?;
        files.extend(file_digests(
            &part.repo,
            &part.base_tree,
            &part.head_tree,
            &parsed,
        )?);
        let touched_new: Vec<String> = parsed
            .files
            .iter()
            .filter_map(|f| f.new_path.clone())
            .collect();
        let touched_old: Vec<String> = parsed
            .files
            .iter()
            .filter_map(|f| f.old_path.clone())
            .collect();
        new_files.extend(part.repo.read_paths(&part.head_tree, &touched_new)?);
        base_files.extend(part.repo.read_paths(&part.base_tree, &touched_old)?);
        tree_size += part.head_tree.iter().map(|e| e.size).sum::<u64>();
        diff_text.push_str(&part.text);
        parts.push((
            source.path.clone(),
            std::rc::Rc::new(part.repo),
            std::rc::Rc::new(part.head_tree),
        ));
    }
    let parts_some = parts.clone();
    // What tells one revision of several repositories from another: the
    // diff, and which repositories at which commits it is of (a repository
    // added or taken out is a new revision even where the diff is not).
    let stamp = repos.iter().fold(diff_text.clone(), |mut s, r| {
        s.push_str(&format!("\n{} {}", r.path, r.range.head));
        s
    });
    Ok(Input {
        digest: digest(&stamp),
        tree_size,
        source: diffnote::model::Source::Workspace(diffnote::model::WorkspaceSource { repos }),
        files,
        new_files,
        base_files,
        // Each path is read from the repository its directory names.
        head_some: Box::new(move |paths| {
            let mut out = Vec::new();
            for (dir, repo, tree) in &parts_some {
                let mine: Vec<String> = paths
                    .iter()
                    .filter(|p| p.starts_with(&format!("{dir}/")))
                    .cloned()
                    .collect();
                if !mine.is_empty() {
                    out.extend(repo.read_paths(tree, &mine)?);
                }
            }
            Ok(out)
        }),
        head_all: Box::new(move || {
            let mut out = Vec::new();
            for (_, repo, tree) in &parts {
                let all: Vec<String> = tree.iter().map(|e| e.path.clone()).collect();
                out.extend(repo.read_paths(tree, &all)?);
            }
            Ok(out)
        }),
        diff_text,
    })
}

/// Makes the review the first screen chose (see `crate::setup`): the same
/// as `review --base ...` would, from the page. What it did, in a line.
fn first_review(
    review: &Path,
    git: &diffnote::git::Repo,
    project: &Path,
    choice: &diffnote::setup::Choice,
) -> Result<String> {
    let title = choice.title.as_deref();
    let said = match choice.kind.as_str() {
        // The directory as it is, kept whole: what changes from here on is
        // what is reviewed.
        "raw" => {
            let count = start_files(review, project, title)?;
            return Ok(mf(
                "main.add_revision.started_files",
                &[("count", &count.to_string())],
            ));
        }
        "workspace" => add_revision(
            review,
            git,
            Ask {
                base: None,
                target: None,
                kind: Some(ReviewType::Workspace),
                repos: Some(diffnote::setup::repos_chosen(project, choice)?),
                snapshot: choice.snapshot,
                title,
                apply: true,
                terminal: false,
            },
        )?,
        _ => add_revision(
            review,
            git,
            Ask {
                base: choice.base.as_deref().map(str::trim),
                target: Some("HEAD"),
                kind: Some(ReviewType::Git),
                repos: None,
                snapshot: choice.snapshot,
                title,
                apply: true,
                terminal: false,
            },
        )?,
    };
    Ok(said.unwrap_or_else(|| m("setup.made").to_string()))
}

/// What a snapshot mode is called on the command line.
fn mode_name(mode: bundle::SnapshotMode) -> &'static str {
    match mode {
        bundle::SnapshotMode::Full => "full",
        bundle::SnapshotMode::Changed => "changed",
    }
}

/// The events every new bundle starts with: what it was made by.
fn first_events() -> Vec<Event> {
    vec![Event::Meta {
        version: 1,
        created_at: OffsetDateTime::now_utc(),
        description: None,
        context_lines: 3,
    }]
}

/// A new bundle with its title, if one is given.
fn fresh_bundle(title: Option<&str>) -> bundle::Loaded {
    let mut loaded = bundle::empty();
    if let Some(title) = title {
        review::set_title(&mut loaded.settings, title);
    }
    loaded
}

/// A directory review's first revision: `dir` as it is, kept whole (it is
/// what the next `review` compares the directory with). How many files.
fn start_files(review_path: &Path, dir: &Path, title: Option<&str>) -> Result<usize> {
    let tree = diffnote::files::read_tree(dir, &[review_path.to_path_buf()])?;
    let digest = diffnote::files::tree_digest(&tree);
    let size: u64 = tree.values().map(|b| b.len() as u64).sum();
    confirm_snapshot_size(bundle::SnapshotMode::Full, false, size);
    let mut events = first_events();
    events.push(Event::Revision(diffnote::model::Revision {
        id: Ulid::new(),
        created_at: OffsetDateTime::now_utc(),
        digest: digest.clone(),
        source: diffnote::model::Source::Files { base: None },
        snapshot_mode: bundle::SnapshotMode::Full,
        files: Vec::new(),
        commits: Vec::new(),
        tree: tree
            .iter()
            .map(|(path, bytes)| diffnote::record::tree_file(path, bytes))
            .collect(),
    }));
    let count = tree.len();
    let additions = bundle::Additions {
        diff: Some((digest, String::new())),
        blobs: tree.into_values().collect(),
        commits: Vec::new(),
    };
    bundle::save(review_path, &fresh_bundle(title), &events, &additions)?;
    Ok(count)
}

/// The per-file digests of every file `diff` touches: old side read from
/// the base tree, new side from the head tree (each `None` when that side
/// doesn't exist, e.g. an added or deleted file).
fn file_digests(
    repo: &diffnote::git::Repo,
    base_tree: &[diffnote::git::TreeEntry],
    head_tree: &[diffnote::git::TreeEntry],
    diff: &diffnote::diff::UnifiedDiff,
) -> Result<Vec<diffnote::model::FileDigest>> {
    let find = |tree: &'_ [diffnote::git::TreeEntry], path: &Option<String>| {
        path.as_deref()
            .and_then(|p| tree.iter().find(|e| e.path == p))
            .map(|e| e.oid.clone())
    };
    let wanted: Vec<(Option<String>, Option<String>)> = diff
        .files
        .iter()
        .map(|f| (find(base_tree, &f.old_path), find(head_tree, &f.new_path)))
        .collect();
    let oids: Vec<&str> = wanted
        .iter()
        .flat_map(|(o, n)| [o, n])
        .flatten()
        .map(String::as_str)
        .collect();
    let mut blobs = repo.read_blobs(&oids)?.into_iter();
    let mut next = |present: &Option<String>| present.as_ref().and_then(|_| blobs.next());
    let mut out = Vec::new();
    for (f, (old_oid, new_oid)) in diff.files.iter().zip(&wanted) {
        let old = next(old_oid).map(digest);
        let new = next(new_oid).map(digest);
        out.push(diffnote::model::FileDigest {
            old_path: f.old_path.clone(),
            new_path: f.new_path.clone(),
            old,
            new,
        });
    }
    Ok(out)
}

/// Asks about the size of a `full` snapshot, if it is big. A git review
/// can switch to `changed` (git has the rest); a directory review needs its
/// full tree to compare the next round against, so it is only told.
fn confirm_snapshot_size(
    mode: bundle::SnapshotMode,
    is_git: bool,
    tree_size: u64,
) -> bundle::SnapshotMode {
    if mode != bundle::SnapshotMode::Full {
        return mode;
    }
    let Some(warning) = diffnote::record::full_snapshot_warning(tree_size) else {
        return mode;
    };
    eprintln!("{}", mf("main.warning_prefix", &[("warning", &warning)]));
    if !is_git {
        eprintln!("{}", m("main.snapshot.ignore_hint"));
        return mode;
    }
    eprint!("{}", m("main.snapshot.confirm_prompt"));
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok();
    if answer.trim().eq_ignore_ascii_case("y") {
        bundle::SnapshotMode::Changed
    } else {
        mode
    }
}
