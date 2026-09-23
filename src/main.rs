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
    #[command(about = m("cli.init.about"))]
    Init {
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true,
            help = m("cli.init.review")
        )]
        review: PathBuf,
        #[arg(value_name = "REV|DIR", help = m("cli.init.target"))]
        target: Option<String>,
        #[arg(long, help = m("cli.init.files"))]
        files: bool,
        #[arg(long, value_name = "DIR", help = m("cli.init.repo"))]
        repo: Option<PathBuf>,
        #[arg(long, value_name = "TITLE", help = m("cli.init.title"))]
        title: Option<String>,
        #[arg(long, value_enum, hide_possible_values = true, help = m("cli.init.snapshot"))]
        snapshot: Option<diffnote::bundle::SnapshotMode>,
    },
    #[command(about = m("cli.serve.about"))]
    Serve {
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true,
            help = m("cli.serve.review")
        )]
        review: PathBuf,
        #[arg(long, value_name = "PORT", default_value_t = 0, help = m("cli.serve.port"))]
        port: u16,
        #[arg(long, help = m("cli.serve.open"))]
        open: bool,
        #[arg(long, value_name = "DIR", help = m("cli.serve.repo"))]
        repo: Option<PathBuf>,
        #[arg(value_name = "REV|DIR", help = m("cli.serve.target"))]
        target: Option<String>,
        #[arg(long, value_name = "REV|DIR", help = m("cli.serve.base"))]
        base: Option<String>,
        #[arg(long, help = m("cli.serve.files"))]
        files: bool,
        #[arg(long, help = m("cli.serve.reopen"))]
        reopen: bool,
        #[arg(long, value_enum, hide_possible_values = true, help = m("cli.serve.snapshot"))]
        snapshot: Option<diffnote::bundle::SnapshotMode>,
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
        Cmd::Init {
            review,
            target,
            files,
            repo,
            title,
            snapshot,
        } => cmd_init(review, target, files, repo, title, snapshot),
        Cmd::Serve {
            review,
            port,
            open,
            repo,
            target,
            base,
            files,
            reopen,
            snapshot,
        } => cmd_serve(
            review,
            port,
            open,
            repo,
            Compare {
                target,
                base,
                files,
                reopen,
                snapshot,
            },
        ),
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

/// For `serve`, what it is given to compare: the
/// changes from the base to the target are recorded as a revision of the
/// bundle (made if there is none yet, for git), so that they can be reviewed in
/// the browser.
///
/// - A git bundle: the target is a commit (`HEAD` if none is named).
/// - A directory bundle: the target is the directory to compare with the base
///   snapshot. With none, nothing is added (`.` may be anywhere).
/// - No bundle yet: `base` (or the target's parent) starts it.
///
/// Nothing is written if the diff is empty or is already recorded (or if
/// `apply` is off: then it only says whether there is something to add).
fn add_revision(
    review_path: &Path,
    repo: &diffnote::git::Repo,
    base: Option<&str>,
    target: Option<&str>,
    files: bool,
    apply: bool,
    // What to keep, when this call is the one that makes the review. Refused
    // earlier if the review already exists, so it can only apply here.
    snapshot: Option<bundle::SnapshotMode>,
) -> Result<Option<String>> {
    let mut loaded = bundle::load(review_path)?;
    let explicit = base.is_some() || target.is_some();
    let mut fresh = FreshBundle(None);
    let exclude = [review_path.to_path_buf()];
    let directory_review = match loaded.source() {
        Some(diffnote::model::Source::Files { .. }) => true,
        Some(diffnote::model::Source::Git(_)) => false,
        None => explicit && files_mode(repo, files),
    };
    let input = if directory_review {
        if loaded.source().is_none() {
            let Some(base_dir) = base else {
                anyhow::bail!(m("main.needs_a_base"));
            };
            init_files(review_path, Path::new(base_dir), None, false)?;
            fresh = FreshBundle(Some(review_path.to_path_buf()));
            loaded = bundle::load(review_path)?;
        } else if let Some(base_dir) = base {
            check_files_base(&loaded, Path::new(base_dir), &exclude)?;
        }
        // The directory is only taken when it is named: `.` may be anywhere.
        let dir = match target {
            Some(dir) => dir,
            None if fresh.0.is_some() => ".",
            None => return Ok(None),
        };
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
    if input.diff_text.trim().is_empty() || loaded.revisions().any(|r| r.digest == input.digest) {
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
    let said = match &source {
        diffnote::model::Source::Git(g) => {
            let short = |id: &str| id[..id.len().min(10)].to_string();
            mf(
                "main.add_revision.recorded_git",
                &[("from", &short(&g.base)), ("to", &short(&g.head))],
            )
        }
        diffnote::model::Source::Files { .. } => m("main.add_revision.recorded_files").to_string(),
    };
    let is_git = matches!(source, diffnote::model::Source::Git(_));
    // The trail this head was reached by, read while the repository is at
    // hand: the review keeps it, since an exported page has nothing to ask.
    let commits = match &source {
        diffnote::model::Source::Git(g) => repo.log(&g.base, &g.head).unwrap_or_default(),
        diffnote::model::Source::Files { .. } => Vec::new(),
    };
    let mode = diffnote::record::pick_snapshot_mode(snapshot, loaded.snapshot_mode(), &source);
    let mut new_events = Vec::new();
    if loaded.events.is_empty() {
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
        &|| confirm_snapshot_size(mode, is_git, tree_size),
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

/// What `serve` is told to compare: the target, and (if there is no
/// bundle yet) the base to start from; `files` makes it a directory review.
struct Compare {
    target: Option<String>,
    base: Option<String>,
    files: bool,
    /// Skip the diff entirely: reopen the last recorded revision as-is
    /// (`--reopen`), so nothing new is added.
    reopen: bool,
    /// What a review made here keeps. Only when there is no review yet: the
    /// range belongs to the review, and is decided once (see `init`).
    snapshot: Option<bundle::SnapshotMode>,
}

fn cmd_serve(
    review: PathBuf,
    port: u16,
    open: bool,
    repo: Option<PathBuf>,
    compare: Compare,
) -> Result<()> {
    let Compare {
        target,
        base,
        files,
        reopen,
        snapshot,
    } = compare;
    if reopen && (target.is_some() || base.is_some() || files) {
        anyhow::bail!(m("main.reopen.conflicting_flags"));
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
        if files && asked == bundle::SnapshotMode::Changed {
            anyhow::bail!(m("main.snapshot.dir_changed_refused"));
        }
    }
    let explicit = target.is_some() || base.is_some();
    if !review.exists() {
        if reopen {
            anyhow::bail!(mf(
                "main.serve.reopen_no_bundle",
                &[("path", &review.display().to_string())]
            ));
        }
        if !explicit {
            anyhow::bail!(mf(
                "main.no_bundle",
                &[("path", &review.display().to_string())]
            ));
        }
    }
    // What was asked for is added to the review, so it can be reviewed here (a
    // failure to do what was asked stops; one to do what was not, only says so).
    let git = repo_of(repo.clone())?;
    // As the review was before any of this: what「保存せずに終了」goes back to.
    let before = std::fs::read(&review).ok();
    if reopen {
        if bundle::load(&review)?.revisions().next().is_none() {
            anyhow::bail!(mf(
                "main.serve.no_revision",
                &[("path", &review.display().to_string())]
            ));
        }
    } else {
        match add_revision(
            &review,
            &git,
            base.as_deref(),
            target.as_deref(),
            files,
            true,
            snapshot,
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
    if !review.exists() {
        anyhow::bail!(m("main.serve.no_diff_at_all"));
    }
    if bundle::load(&review)
        .ok()
        .is_some_and(|l| diffnote::html::view_model(&l).is_err())
    {
        println!("{}", m("main.serve.no_diff_yet"));
    }
    // The page's button: what was added to the target since (the base is
    // already the review's; a named commit doesn't move, `HEAD` does).
    // `--reopen` has none: nothing should be added in this session at all.
    let refresher: Option<diffnote::serve::Refresher> = if reopen {
        None
    } else {
        let (review, git, target) = (review.clone(), git.clone(), target.clone());
        Some(std::sync::Arc::new(move |apply| {
            // Later rounds add to a review that exists: the range is the
            // one it was made with.
            add_revision(&review, &git, None, target.as_deref(), files, apply, None)
        }))
    };
    let options = diffnote::serve::Options {
        review,
        port,
        author: None,
        repo,
        refresh: refresher,
        before: Some(before),
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
/// - With none, `base` starts it (`serve --base main feature`); without `base`,
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
            // What the base was called when it was set, for the tab's name.
            let label = if first.base == first.head {
                first.spec.clone()
            } else {
                first
                    .spec
                    .split("..")
                    .next()
                    .filter(|_| first.spec.contains(".."))
                    .map_or_else(
                        || first.base[..first.base.len().min(10)].to_string(),
                        str::to_string,
                    )
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
                diffnote::model::Source::Git(_) => None,
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

/// Whether a review that has no bundle yet is one of directories: when asked
/// for (`--files`), or when there is no git repository to make one from.
fn files_mode(repo: &diffnote::git::Repo, files: bool) -> bool {
    files || !repo.exists()
}

fn cmd_init(
    review_path: PathBuf,
    target: Option<String>,
    files: bool,
    repo: Option<PathBuf>,
    title: Option<String>,
    snapshot: Option<bundle::SnapshotMode>,
) -> Result<()> {
    if review_path.exists() {
        anyhow::bail!(mf(
            "main.init.already_exists",
            &[("path", &review_path.display().to_string())]
        ));
    }
    let repo = repo_of(repo)?;
    if !files && repo.exists() {
        return init_git(
            &review_path,
            &repo,
            target.as_deref().unwrap_or("HEAD"),
            title,
            snapshot.unwrap_or(bundle::SnapshotMode::Changed),
        );
    }
    // A directory review has nothing but its own snapshots to compare
    // against, so it always keeps the whole tree.
    if snapshot == Some(bundle::SnapshotMode::Changed) {
        anyhow::bail!(m("main.snapshot.dir_changed_refused"));
    }
    let dir = PathBuf::from(target.as_deref().unwrap_or("."));
    init_files(&review_path, &dir, title, true)
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

/// A new bundle (not on disk yet) with its title, if one is given.
fn fresh_bundle(review_path: &Path, title: Option<&str>) -> Result<bundle::Loaded> {
    let mut loaded = bundle::load(review_path)?;
    if let Some(title) = title {
        review::set_title(&mut loaded.settings, title);
    }
    Ok(loaded)
}

/// A git review that starts at a commit: the commit is the base, so that the
/// next `serve` reviews what has changed since. Nothing is stored beyond the
/// commit's id (git has the rest).
fn init_git(
    review_path: &Path,
    repo: &diffnote::git::Repo,
    rev: &str,
    title: Option<String>,
    snapshot: bundle::SnapshotMode,
) -> Result<()> {
    let commit = repo.commit_id(rev)?;
    let mut events = first_events();
    let empty_diff = String::new();
    let digest = digest(&empty_diff);
    events.push(Event::Revision(diffnote::model::Revision {
        id: Ulid::new(),
        created_at: OffsetDateTime::now_utc(),
        digest: digest.clone(),
        source: diffnote::model::Source::Git(diffnote::model::GitSource {
            base: commit.clone(),
            head: commit.clone(),
            spec: rev.to_string(),
        }),
        // What every revision of this bundle will be recorded with: the
        // mode belongs to the review, not to one revision of it.
        snapshot_mode: snapshot,
        files: Vec::new(),
        tree: Vec::new(),
        commits: Vec::new(),
    }));
    let additions = bundle::Additions {
        diff: Some((digest, empty_diff)),
        blobs: Vec::new(),
        commits: Vec::new(),
    };
    bundle::save(
        review_path,
        &fresh_bundle(review_path, title.as_deref())?,
        &events,
        &additions,
    )?;
    println!(
        "{}",
        mf(
            "main.init.git_done",
            &[
                ("rev", rev),
                ("commit", &commit[..commit.len().min(10)]),
                ("path", &review_path.display().to_string()),
            ]
        )
    );
    Ok(())
}

fn init_files(review_path: &Path, dir: &Path, title: Option<String>, say: bool) -> Result<()> {
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
    bundle::save(
        review_path,
        &fresh_bundle(review_path, title.as_deref())?,
        &events,
        &additions,
    )?;
    if say {
        println!(
            "{}",
            mf(
                "main.init.files_done",
                &[
                    ("count", &count.to_string()),
                    ("path", &review_path.display().to_string())
                ]
            )
        );
    }
    Ok(())
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
