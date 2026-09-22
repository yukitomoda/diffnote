use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// `println!` that stops quietly when the reader has gone (`diffnote show |
/// head`), as other commands do, instead of panicking on the broken pipe.
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
use diffnote::model::{Anchor, Event};
use diffnote::{annotation, bundle, review};
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
    },
    #[command(about = m("cli.edit.about"))]
    Edit {
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true,
            help = m("cli.edit.review")
        )]
        review: PathBuf,
        #[arg(value_name = "REV|DIR", help = m("cli.edit.target"))]
        target: Option<String>,
        #[arg(long, value_name = "REV|DIR", help = m("cli.edit.base"))]
        base: Option<String>,
        #[arg(long, help = m("cli.edit.files"))]
        files: bool,
        #[arg(long, value_name = "DIR", help = m("cli.edit.repo"))]
        repo: Option<PathBuf>,
        #[arg(long, value_enum, hide_possible_values = true, help = m("cli.edit.snapshot"))]
        snapshot: Option<diffnote::bundle::SnapshotMode>,
        #[arg(long = "show", value_name = "PATH[:START[-END]]", help = m("cli.edit.show"))]
        show: Vec<String>,
        #[arg(long, value_name = "TITLE", help = m("cli.edit.title"))]
        title: Option<String>,
        #[arg(long, help = m("cli.edit.reopen"))]
        reopen: bool,
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
        #[arg(long, help = m("cli.serve.no_open"))]
        no_open: bool,
        #[arg(long, value_name = "TITLE", help = m("cli.serve.title"))]
        title: Option<String>,
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
    },
    #[command(about = m("cli.show.about"))]
    Show {
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true,
            help = m("cli.show.review")
        )]
        review: PathBuf,
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
        } => cmd_init(review, target, files, repo, title),
        Cmd::Edit {
            review,
            target,
            base,
            files,
            repo,
            snapshot,
            show,
            title,
            reopen,
        } => cmd_edit(
            review,
            Compare {
                target,
                base,
                files,
                reopen,
            },
            repo,
            snapshot,
            show,
            title,
        ),
        Cmd::Show { review } => cmd_show(review),
        Cmd::Serve {
            review,
            port,
            no_open,
            title,
            repo,
            target,
            base,
            files,
            reopen,
        } => cmd_serve(
            review,
            port,
            no_open,
            title,
            repo,
            Compare {
                target,
                base,
                files,
                reopen,
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

/// For `serve`, the counterpart of what `edit` does with what it is given: the
/// changes from the base to the target are recorded as a revision of the
/// bundle (made if there is none yet, for git), so that they can be reviewed in
/// the browser.
///
/// - A git bundle: the target is a commit (`HEAD` if none is named).
/// - A directory bundle: the target is the directory to compare with the base
///   snapshot. With none, nothing is added (`.` may be anywhere).
/// - No bundle yet: as for `edit`, `base` (or the target's parent) starts it.
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
) -> Result<Option<String>> {
    let mut loaded = bundle::load(review_path)?;
    let explicit = base.is_some() || target.is_some();
    let mut fresh = FreshBundle(None);
    let exclude = [review_path.to_path_buf(), draft_path_for(review_path)];
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
                    "main.add_revision.no_bundle",
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
    let mode = diffnote::record::pick_snapshot_mode(None, loaded.snapshot_mode(), &source);
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

/// What `edit` and `serve` are told to compare: the target, and (if there is no
/// bundle yet) the base to start from; `files` makes it a directory review.
struct Compare {
    target: Option<String>,
    base: Option<String>,
    files: bool,
    /// Skip the diff entirely: reopen the last recorded revision as-is
    /// (`--reopen`), so nothing new is added.
    reopen: bool,
}

fn cmd_serve(
    review: PathBuf,
    port: u16,
    no_open: bool,
    title: Option<String>,
    repo: Option<PathBuf>,
    compare: Compare,
) -> Result<()> {
    let Compare {
        target,
        base,
        files,
        reopen,
    } = compare;
    if reopen && (target.is_some() || base.is_some() || files) {
        anyhow::bail!(m("main.reopen.conflicting_flags"));
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
                "main.serve.no_bundle",
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
    if let Some(title) = title.as_deref() {
        let mut loaded = bundle::load(&review)?;
        if review::set_title(&mut loaded.settings, title) {
            let events = loaded.events.clone();
            let none = bundle::Additions::default();
            bundle::save(&review, &loaded, &events, &none)?;
            println!("{}", m("main.title_set"));
        }
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
            add_revision(&review, &git, None, target.as_deref(), files, apply)
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
        println!("{}", mf("main.serve.opening_browser", &[("url", url)]));
        println!("{}", m("main.serve.quit_hint"));
        if !no_open && !open_in_browser(url) {
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

/// What one edit session reviews: the diff, plus everything needed to
/// record it as a `Revision` if the session ends up adding anything.
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
/// - With none, `base` starts it (`edit --base main feature`); without `base`,
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

/// `--reopen`: the last recorded revision, exactly as stored, with nothing
/// diffed and nothing read from git or the filesystem. Its digest already
/// matches that revision, so `record_session` adds no new one -- only, if
/// this session's comments reach a file that revision didn't need, a `Pin`
/// for it (read from what the bundle already has).
fn reopen_input(loaded: &bundle::Loaded) -> Result<Input> {
    let rev = loaded
        .revisions()
        .last()
        .context(m("main.reopen.input_no_revision"))?;
    let diff_text = loaded.revision_diff(rev).unwrap_or_default();
    let tree = loaded.tree_of(rev);
    let all_tree = tree.clone();
    Ok(Input {
        digest: rev.digest.clone(),
        tree_size: 0,
        source: rev.source.clone(),
        files: rev.files.clone(),
        new_files: Default::default(),
        base_files: Default::default(),
        head_some: Box::new(move |paths| {
            Ok(paths
                .iter()
                .filter_map(|p| Some((p.clone(), tree.get(p)?.clone())))
                .collect())
        }),
        head_all: Box::new(move || Ok(all_tree.clone().into_iter().collect())),
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
        );
    }
    let dir = PathBuf::from(target.as_deref().unwrap_or("."));
    init_files(&review_path, &dir, title, true)
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
/// next `edit` reviews what has changed since. Nothing is stored beyond the
/// commit's id (git has the rest).
fn init_git(
    review_path: &Path,
    repo: &diffnote::git::Repo,
    rev: &str,
    title: Option<String>,
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
        snapshot_mode: bundle::SnapshotMode::Changed,
        files: Vec::new(),
        tree: Vec::new(),
    }));
    let additions = bundle::Additions {
        diff: Some((digest, empty_diff)),
        blobs: Vec::new(),
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
        tree: tree
            .iter()
            .map(|(path, bytes)| diffnote::record::tree_file(path, bytes))
            .collect(),
    }));
    let count = tree.len();
    let additions = bundle::Additions {
        diff: Some((digest, String::new())),
        blobs: tree.into_values().collect(),
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

fn cmd_edit(
    review_path: PathBuf,
    compare: Compare,
    repo: Option<PathBuf>,
    snapshot_override: Option<bundle::SnapshotMode>,
    show_specs: Vec<String>,
    title: Option<String>,
) -> Result<()> {
    let Compare {
        target,
        base,
        files,
        reopen,
    } = compare;
    if reopen && (target.is_some() || base.is_some() || files) {
        anyhow::bail!(m("main.reopen.conflicting_flags"));
    }
    if reopen && snapshot_override.is_some() {
        anyhow::bail!(m("main.reopen.edit_conflicts_snapshot"));
    }
    if reopen && !show_specs.is_empty() {
        anyhow::bail!(m("main.reopen.edit_conflicts_show"));
    }
    let shows: Vec<diffnote::show::Show> = show_specs
        .iter()
        .map(|spec| {
            diffnote::show::parse(spec).map_err(|e| {
                anyhow::anyhow!(mf(
                    "main.edit.show_parse_failed",
                    &[("spec", spec), ("error", &e.to_string())]
                ))
            })
        })
        .collect::<Result<_>>()?;
    let mut loaded = bundle::load(&review_path)?;
    let repo = repo_of(repo)?;
    let mut fresh = FreshBundle(None);
    // A directory review: the bundle says so, or (with no bundle) --files or
    // the lack of a repository does.
    let directory_review = match loaded.source() {
        Some(diffnote::model::Source::Files { .. }) => true,
        Some(diffnote::model::Source::Git(_)) => false,
        None => files_mode(&repo, files),
    };
    let input = if reopen {
        reopen_input(&loaded)?
    } else if directory_review {
        if snapshot_override == Some(bundle::SnapshotMode::Changed) {
            anyhow::bail!(m("main.edit.dir_snapshot_changed_refused"));
        }
        let exclude = [review_path.clone(), draft_path_for(&review_path)];
        if loaded.source().is_none() {
            // No bundle: the base directory starts it, as `init` would.
            let Some(base_dir) = base.as_deref() else {
                anyhow::bail!(m("main.needs_a_base"));
            };
            init_files(&review_path, Path::new(base_dir), None, false)?;
            fresh = FreshBundle(Some(review_path.clone()));
            loaded = bundle::load(&review_path)?;
        } else if let Some(base_dir) = base.as_deref() {
            check_files_base(&loaded, Path::new(base_dir), &exclude)?;
        }
        let dir = PathBuf::from(target.as_deref().unwrap_or("."));
        files_input(&loaded, &dir, &exclude)?
    } else {
        let range = git_range(&repo, &loaded, base.as_deref(), target.as_deref())?;
        git_input(repo, range)?
    };
    let Input {
        diff_text,
        files,
        new_files,
        base_files,
        source,
        digest: diff_digest,
        tree_size,
        head_some,
        head_all,
    } = input;
    let author = diffnote::author::resolve(None);
    let title_set = title
        .as_deref()
        .is_some_and(|t| review::set_title(&mut loaded.settings, t));
    if diff_text.trim().is_empty() {
        // Nothing to review, but a title can still be given to a review that exists.
        if title_set && !loaded.events.is_empty() {
            let events = loaded.events.clone();
            let none = bundle::Additions::default();
            bundle::save(&review_path, &loaded, &events, &none)?;
            fresh.keep();
            println!("{}", m("main.title_set"));
            return Ok(());
        }
        println!("{}", m("main.edit.no_diff"));
        return Ok(());
    }
    let parsed_diff = diffnote::diff::parse(&diff_text).map_err(|e| anyhow::anyhow!("{e}"))?;

    let revisions = source.revisions(&diff_digest);
    let existing_events = &loaded.events;
    let existing_threads = review::build_threads(existing_events);

    // Fresh review: write the diff verbatim, exactly as `git diff` produced
    // it. annotation::render_for_edit reconstructs it line-by-line instead,
    // which is fine for round-trip but an unnecessary risk (e.g. a possible
    // trailing-newline mismatch) when there's nothing to interleave anyway.
    // The files existing threads refer to that this diff doesn't touch: read
    // at the head, so those threads can be placed too.
    let touched_now: std::collections::HashSet<&str> = files
        .iter()
        .flat_map(|f| [f.old_path.as_deref(), f.new_path.as_deref()])
        .flatten()
        .collect();
    let untouched_existing: Vec<String> =
        diffnote::record::referenced_files(existing_events.iter())
            .into_iter()
            .chain(shows.iter().map(|s| s.path.clone()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|p| !touched_now.contains(p.as_str()))
            .collect();
    let head_of_untouched = if untouched_existing.is_empty() {
        Vec::new()
    } else {
        (head_some)(&untouched_existing)?
    };
    let tree_now: Vec<diffnote::model::TreeFile> = head_of_untouched
        .iter()
        .map(|(p, b)| diffnote::record::tree_file(p, b))
        .collect();
    let mut blobs = loaded.blobs();
    for bytes in new_files
        .values()
        .chain(base_files.values())
        .chain(head_of_untouched.iter().map(|(_, b)| b))
    {
        blobs.add(bytes);
    }
    bundle::link_revision_files(&mut blobs, &files);
    let versions = diffnote::anchor::ViewVersions {
        files: &files,
        tree: &tree_now,
    };
    let show_wants = diffnote::show::wants(&shows, &versions, &blobs)
        .map_err(|e| anyhow::anyhow!("--show {e}"))?;
    let (temp_text, synthetic_files) = if existing_threads.is_empty() && show_wants.is_empty() {
        (diff_text.clone(), Vec::new())
    } else {
        annotation::render_for_edit(
            &diff_text,
            &parsed_diff,
            &versions,
            &blobs,
            &existing_threads,
            &show_wants,
        )
    };

    // Comments written in a file the diff doesn't touch (shown for the
    // threads on it) are anchored to that file's version at the head.
    let anchor_files: Vec<diffnote::model::FileDigest> =
        files.iter().cloned().chain(synthetic_files).collect();

    let temp_dir = tempfile::tempdir().context(m("main.edit.temp_dir_failed"))?;
    let temp_path = temp_dir.path().join("review.diff");

    // If a previous session's edits failed to parse (or the editor itself
    // exited non-zero), they were saved here instead of being lost -- reopen
    // that instead of a fresh render so the user can just fix the mistake.
    let draft_path = draft_path_for(&review_path);
    let initial_text = match std::fs::read_to_string(&draft_path) {
        Ok(draft) => {
            println!(
                "{}",
                mf(
                    "main.edit.resuming_draft",
                    &[("path", &draft_path.display().to_string())]
                )
            );
            draft
        }
        Err(_) => temp_text.clone(),
    };
    std::fs::write(&temp_path, &initial_text).context(m("main.edit.temp_write_failed"))?;

    let editor = default_editor();
    let words = diffnote::editor::command_words(&editor);
    let (program, args) = words.split_first().context(m("main.edit.editor_empty"))?;
    let status = Command::new(program)
        .args(args)
        .arg(&temp_path)
        .status()
        .with_context(|| mf("main.edit.editor_launch_failed", &[("editor", &editor)]))?;

    let annotated = std::fs::read_to_string(&temp_path).context(m("main.edit.read_back_failed"))?;

    if !status.success() {
        save_draft(&draft_path, &annotated)?;
        anyhow::bail!(mf(
            "main.edit.editor_failed",
            &[
                ("editor", &editor),
                ("path", &draft_path.display().to_string()),
            ]
        ));
    }

    let parsed = match annotation::parse(&annotated) {
        Ok(parsed) => parsed,
        Err(e) => {
            save_draft(&draft_path, &annotated)?;
            anyhow::bail!(mf(
                "main.edit.parse_failed",
                &[
                    ("error", &e.to_string()),
                    ("path", &draft_path.display().to_string()),
                ]
            ));
        }
    };

    for warning in &parsed.warnings {
        eprintln!(
            "{}",
            mf("main.warning_prefix", &[("warning", &warning.to_string())])
        );
    }

    // Parsing succeeded, so nothing here is at risk of being lost anymore --
    // any draft from an earlier failed attempt is now stale.
    let _ = std::fs::remove_file(&draft_path);

    if existing_events.is_empty() && parsed.items.is_empty() && !title_set {
        println!("{}", m("main.edit.no_comments"));
        return Ok(());
    }

    let mut new_events = Vec::new();
    if existing_events.is_empty() {
        new_events.push(Event::Meta {
            version: 1,
            created_at: OffsetDateTime::now_utc(),
            description: None,
            context_lines: 3,
        });
    }

    let mut thread_ids: Vec<Ulid> = Vec::new();
    for item in &parsed.items {
        match item {
            annotation::Item::NewThread {
                scope,
                body,
                directives,
                ..
            } => {
                if let Some(pos) = directives
                    .iter()
                    .position(|d| matches!(d, annotation::Directive::Reanchor(_)))
                {
                    if directives.len() != 1 || body.is_some() {
                        anyhow::bail!(m("main.edit.reanchor_conflict"));
                    }
                    let annotation::Directive::Reanchor(id_str) = &directives[pos] else {
                        unreachable!()
                    };
                    let target_id = Ulid::from_string(id_str).map_err(|_| {
                        anyhow::anyhow!(mf("main.edit.reanchor_bad_id", &[("id", id_str)]))
                    })?;
                    let new_anchor = diffnote::create::build_anchor(
                        scope,
                        &parsed.diff,
                        &anchor_files,
                        &revisions,
                    )?;
                    new_events.push(Event::Reanchor {
                        parent: target_id,
                        author: author.clone(),
                        created_at: OffsetDateTime::now_utc(),
                        anchor: new_anchor,
                    });
                    continue;
                }

                let id = Ulid::new();
                thread_ids.push(id);
                let comment_anchor =
                    diffnote::create::build_anchor(scope, &parsed.diff, &anchor_files, &revisions)?;
                new_events.push(Event::Comment {
                    id,
                    parent: None,
                    author: author.clone(),
                    created_at: OffsetDateTime::now_utc(),
                    anchor: Some(comment_anchor),
                    body: body.clone().unwrap_or_default(),
                });
                for directive in directives {
                    push_simple_directive(&mut new_events, directive, id, &author)?;
                }
            }
            annotation::Item::Reply {
                target,
                body,
                directives,
            } => {
                let target_id = match target {
                    annotation::ThreadRef::New(tid) => *thread_ids.get(tid.0).ok_or_else(|| {
                        anyhow::anyhow!(m("main.edit.internal_unknown_thread_ref"))
                    })?,
                    annotation::ThreadRef::Existing(ulid) => *ulid,
                };
                if let Some(body) = body {
                    let id = Ulid::new();
                    new_events.push(Event::Comment {
                        id,
                        parent: Some(target_id),
                        author: author.clone(),
                        created_at: OffsetDateTime::now_utc(),
                        anchor: None,
                        body: body.clone(),
                    });
                }
                for directive in directives {
                    push_simple_directive(&mut new_events, directive, target_id, &author)?;
                }
            }
        }
    }

    // (A title given is something, as a comment is.)
    if new_events.is_empty() && !title_set {
        println!("{}", m("main.edit.no_changes"));
        return Ok(());
    }

    // Only record a not-yet-seen diff when this session actually produced
    // something -- an idle "opened it, looked, closed it" pass shouldn't
    // grow the bundle.
    // Only asked about (if big) when a new revision is actually recorded.
    let picked_mode =
        diffnote::record::pick_snapshot_mode(snapshot_override, loaded.snapshot_mode(), &source);
    let is_git = matches!(source, diffnote::model::Source::Git(_));
    let additions = diffnote::record::record_session(
        &loaded,
        &mut new_events,
        diffnote::record::Capture {
            diff_text: &diff_text,
            diff_digest: &diff_digest,
            source,
            files: &files,
            new_files: &new_files,
            base_files: &base_files,
        },
        &|| confirm_snapshot_size(picked_mode, is_git, tree_size),
        &*head_some,
        head_all,
    )?;

    let comment_count = new_events
        .iter()
        .filter(|e| matches!(e, Event::Comment { .. }))
        .count();
    let mut all_events = loaded.events.clone();
    all_events.extend(new_events.iter().cloned());
    bundle::save(&review_path, &loaded, &all_events, &additions)?;
    fresh.keep();
    if title_set {
        println!("{}", m("main.title_set"));
    }
    println!(
        "{}",
        mf(
            "main.edit.saved",
            &[
                ("comments", &comment_count.to_string()),
                ("events", &new_events.len().to_string()),
                ("path", &review_path.display().to_string()),
            ]
        )
    );
    Ok(())
}

fn cmd_show(review_path: PathBuf) -> Result<()> {
    let loaded = bundle::load(&review_path)?;
    let settings = loaded.settings;
    let events = loaded.events;
    if events.is_empty() {
        println!(
            "{}",
            mf(
                "main.show.empty",
                &[("path", &review_path.display().to_string())]
            )
        );
        return Ok(());
    }
    // The settings that are not what they would be anyway.
    let default = diffnote::model::Settings::default();
    if let Some(title) = review::title(&settings) {
        println!("{}", mf("main.show.setting_title", &[("title", title)]));
    }
    if settings.ignore_whitespace {
        println!("{}", m("main.show.setting_ignore_whitespace"));
    }
    if settings.attachment_limit != default.attachment_limit {
        println!(
            "{}",
            mf(
                "main.show.setting_attachment_limit",
                &[("bytes", &settings.attachment_limit.to_string())]
            )
        );
    }
    for event in &events {
        match event {
            Event::Meta { context_lines, .. } => {
                println!(
                    "{}",
                    mf(
                        "main.show.meta",
                        &[("context_lines", &context_lines.to_string())]
                    )
                );
            }
            Event::Revision(r) => {
                println!(
                    "{}",
                    mf(
                        "main.show.revision",
                        &[
                            ("id", &r.id.to_string()),
                            ("digest", &r.digest),
                            (
                                "target",
                                match &r.source {
                                    diffnote::model::Source::Git(g) => g.spec.as_str(),
                                    diffnote::model::Source::Files { .. } =>
                                        m("main.show.dir_marker"),
                                }
                            ),
                            ("mode", &format!("{:?}", r.snapshot_mode)),
                        ]
                    )
                );
            }
            Event::Title { title, author, .. } => {
                println!(
                    "{}",
                    mf("main.show.title", &[("title", title), ("author", author)])
                );
            }
            Event::IgnoreWhitespace { value, author, .. } => {
                let state = if *value {
                    m("main.show.ignore_whitespace_on")
                } else {
                    m("main.show.ignore_whitespace_off")
                };
                println!(
                    "{}",
                    mf(
                        "main.show.ignore_whitespace_change",
                        &[("state", state), ("author", author)]
                    )
                );
            }
            Event::Pin { revision, files } => {
                println!(
                    "{}",
                    mf(
                        "main.show.pin",
                        &[
                            ("revision", &revision.to_string()),
                            ("count", &files.len().to_string()),
                        ]
                    )
                );
            }
            Event::Comment {
                id,
                parent: None,
                author,
                body,
                anchor,
                ..
            } => {
                println!(
                    "{}",
                    mf(
                        "main.show.new_comment",
                        &[
                            ("id", &id.to_string()),
                            ("where", &describe_anchor(anchor.as_ref())),
                            ("author", author),
                        ]
                    )
                );
                print_body(body);
            }
            Event::Comment {
                id,
                parent: Some(parent),
                author,
                body,
                ..
            } => {
                println!(
                    "{}",
                    mf(
                        "main.show.reply",
                        &[
                            ("id", &id.to_string()),
                            ("parent", &parent.to_string()),
                            ("author", author),
                        ]
                    )
                );
                print_body(body);
            }
            Event::Resolve { parent, author, .. } => {
                println!(
                    "{}",
                    mf(
                        "main.show.resolve",
                        &[("parent", &parent.to_string()), ("author", author)]
                    )
                );
            }
            Event::Reopen { parent, author, .. } => {
                println!(
                    "{}",
                    mf(
                        "main.show.reopen",
                        &[("parent", &parent.to_string()), ("author", author)]
                    )
                );
            }
            Event::Reanchor {
                parent,
                author,
                anchor,
                ..
            } => {
                println!(
                    "{}",
                    mf(
                        "main.show.reanchor",
                        &[
                            ("parent", &parent.to_string()),
                            ("where", &describe_anchor(Some(anchor))),
                            ("author", author),
                        ]
                    )
                );
            }
        }
    }
    Ok(())
}

fn print_body(body: &str) {
    for line in body.lines() {
        println!("        | {line}");
    }
}

fn describe_anchor(anchor: Option<&Anchor>) -> String {
    let span = |s: &diffnote::model::LineRange| {
        let (a, b) = (s.start, s.end());
        if s.is_empty() {
            mf(
                "main.anchor.insert_position",
                &[("file", &s.file), ("line", &a.to_string())],
            )
        } else if a == b {
            mf(
                "main.anchor.single_line",
                &[("file", &s.file), ("line", &a.to_string())],
            )
        } else {
            mf(
                "main.anchor.range",
                &[
                    ("file", &s.file),
                    ("start", &a.to_string()),
                    ("end", &b.to_string()),
                ],
            )
        }
    };
    match anchor {
        None => m("main.anchor.unknown").to_string(),
        Some(Anchor::Global { .. }) => m("main.anchor.global").to_string(),
        Some(Anchor::File { base, head }) => mf(
            "main.anchor.whole_file",
            &[(
                "file",
                head.as_ref()
                    .or(base.as_ref())
                    .map_or(m("main.anchor.unknown"), |f| f.file.as_str()),
            )],
        ),
        Some(Anchor::Span { base, head }) => match (base, head) {
            (Some(b), Some(h)) if !b.is_empty() && !h.is_empty() => mf(
                "main.anchor.moved",
                &[("head", &span(h)), ("base", &span(b))],
            ),
            (_, Some(h)) if !h.is_empty() => span(h),
            (Some(b), _) => mf("main.anchor.deleted", &[("span", &span(b))]),
            _ => m("main.anchor.unknown").to_string(),
        },
    }
}

/// Handles `resolve`/`reopen` only -- `reanchor` needs extra
/// context (the position/target-thread bookkeeping) that only the caller
/// has, so `cmd_edit` special-cases those before ever reaching here.
fn push_simple_directive(
    events: &mut Vec<Event>,
    directive: &annotation::Directive,
    thread_id: Ulid,
    author: &str,
) -> Result<()> {
    use annotation::Directive;
    match directive {
        Directive::Resolve => events.push(Event::Resolve {
            parent: thread_id,
            author: author.to_string(),
            created_at: OffsetDateTime::now_utc(),
        }),
        Directive::Reopen => events.push(Event::Reopen {
            parent: thread_id,
            author: author.to_string(),
            created_at: OffsetDateTime::now_utc(),
        }),
        Directive::Reanchor(_) => {
            anyhow::bail!(mf(
                "main.push_directive.internal_error",
                &[("directive", &format!("{directive:?}"))]
            ));
        }
    }
    Ok(())
}

/// Where an edit session's unsaved buffer is parked if it can't be
/// committed to the bundle (parse failure, or the editor itself exiting
/// non-zero) -- a sibling of the review bundle, not inside the temp dir
/// that gets deleted when `cmd_edit` returns.
fn draft_path_for(review_path: &Path) -> PathBuf {
    let mut name = review_path.file_name().unwrap_or_default().to_os_string();
    name.push(".draft");
    review_path.with_file_name(name)
}

fn save_draft(draft_path: &Path, content: &str) -> Result<()> {
    std::fs::write(draft_path, content).with_context(|| {
        mf(
            "main.save_draft.failed",
            &[("path", &draft_path.display().to_string())],
        )
    })
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
/// full tree to compare the next edit against, so it is only told.
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

fn default_editor() -> String {
    // An empty $EDITOR is as good as none.
    std::env::var("EDITOR")
        .ok()
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        })
}
