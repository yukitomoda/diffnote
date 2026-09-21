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
            eprintln!("標準出力に書き込めませんでした: {e}");
            std::process::exit(1);
        }
    }};
}

use diffnote::digest::digest;
use diffnote::model::{Anchor, Event};
use diffnote::{annotation, bundle, review};
use std::path::{Path, PathBuf};
use std::process::Command;
use time::OffsetDateTime;
use ulid::Ulid;

/// diffnote: git の差分やディレクトリにローカルでコメントし、レビューをファイルで共有する。
#[derive(Debug, Parser)]
#[command(name = "diffnote", version, disable_help_flag = true, disable_version_flag = true, disable_help_subcommand = true, next_help_heading = "オプション", subcommand_help_heading = "コマンド", subcommand_value_name = "コマンド", help_template = HELP_TEMPLATE)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
    /// ヘルプを表示する
    #[arg(short = 'h', long, global = true, action = clap::ArgAction::Help)]
    help: Option<bool>,
    /// バージョンを表示する
    #[arg(short = 'V', long, action = clap::ArgAction::Version)]
    version: Option<bool>,
}

/// clap の組み込みの見出し(Usage など)を日本語にしたコマンド定義。
fn command() -> clap::Command {
    use clap::CommandFactory;
    Cli::command().mut_subcommands(|sub| {
        sub.help_template(HELP_TEMPLATE).mut_args(|a| {
            let heading = if a.is_positional() {
                "引数"
            } else {
                "オプション"
            };
            a.help_heading(heading)
        })
    })
}

const HELP_TEMPLATE: &str = "{about}\n\n使い方: {usage}\n\n{all-args}";

#[derive(Debug, Subcommand)]
enum Cmd {
    /// 最初のレビューバンドルを作る。git のリポジトリの中(`.git` がある)なら、指定したコミット(既定は HEAD)を基準として記録し、そうでなければ、ディレクトリの今の状態を基準として保存する。以降の `edit`(引数なし)で、その基準からの変更をレビューできる。
    Init {
        /// 作成するレビューバンドル(.diffnote、zip 形式)のパス。省略時は ./.diffnote。
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true
        )]
        review: PathBuf,
        /// git のリポジトリの中では、基準にするコミット(HEAD、ブランチ名、タグ、コミット ID など。省略時は HEAD)。それ以外では、スナップショットを取るディレクトリ(省略時は `.`。`.diffnoteignore`(なければ`.gitignore`)に一致するファイルは含めない)。
        #[arg(value_name = "REV|DIR")]
        target: Option<String>,
        /// git のリポジトリの中でも、ファイルのスナップショット(ディレクトリのレビュー)を作る。
        #[arg(long)]
        files: bool,
        /// 基準のコミットを探すリポジトリ。省略時は、実行したディレクトリのリポジトリ。
        #[arg(long, value_name = "DIR")]
        repo: Option<PathBuf>,
        /// レビューのタイトル。エクスポートの見出しに使われる(省略できる)。
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// コメントなどの作者名。省略時は git の user.name、なければ user.email、なければ環境のユーザー名。
        #[arg(long, value_name = "NAME")]
        author: Option<String>,
    },
    /// レビュー対象を $EDITOR で開いてコメントを書き、レビューバンドルに追記する(git のレビューでは、バンドルがなければ作成する)。
    Edit {
        /// レビューバンドル(.diffnote、zip 形式)のパス。省略時は ./.diffnote。
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true
        )]
        review: PathBuf,
        /// 比較対象。git のレビュー: ベース(`init` で決めたコミット)と比べるコミット(HEAD、ブランチ名、タグ、コミット ID など。省略時は HEAD)。範囲(`A..B`)は指定できません。バンドルがなく `--base` もないときは、そのコミットの第一親をベースにします(そのコミット自身の変更のレビュー)。ディレクトリのレビュー(`init` で作ったバンドル): ベースのスナップショットと比べるディレクトリ(省略時はカレント)。
        #[arg(value_name = "REV|DIR")]
        target: Option<String>,
        /// ベース(比較の起点): git のレビューではコミット、ディレクトリのレビューではディレクトリ。まだバンドルがないときに指定でき、`init BASE` してから `edit` するのと同じ意味になります。バンドルがあるときは、そのベースと同じものしか指定できません。
        #[arg(long, value_name = "REV|DIR")]
        base: Option<String>,
        /// git のリポジトリの中でも、ディレクトリのレビューにする(まだバンドルがないときだけ意味があります)。
        #[arg(long)]
        files: bool,
        /// git のレビューの対象のリポジトリ。コミットの解決と、ファイルの読み出しに使う。省略時は、実行したディレクトリのリポジトリ。
        #[arg(long, value_name = "DIR")]
        repo: Option<PathBuf>,
        /// 新しい差分を初めて見て、かつこの回で何かを追加したときに、バンドルへ保存する内容。`changed`(差分が触れた全ファイルの両側と、コメントが参照する全ファイル)か、`full`(それに加えて head 全体のツリー)。省略時は、バンドルにすでに決まっているモード、なければ git のレビューでは `changed`(残りは git が持っている)。ディレクトリのレビューは常に全体を保存するので、そこで `--snapshot changed` を指定するとエラーになる。
        #[arg(long, value_enum, hide_possible_values = true)]
        snapshot: Option<diffnote::bundle::SnapshotMode>,
        /// ファイル(`PATH`)またはその一部の行(`PATH:START-END`、`PATH:LINE`)をバッファに入れる。差分が触れていない箇所にもコメントを書ける。レビューの head 側の内容が対象。繰り返し指定できる。差分がすでに表示している箇所の前後 3 行は、重ねて追加されない。
        #[arg(long = "show", value_name = "PATH[:START[-END]]")]
        show: Vec<String>,
        /// レビューのタイトルを設定する。エクスポートの見出しに使われる。すでにあるタイトルを変えるときにも使い、空文字列(`--title ""`)で取り消す。
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// コメントなどの作者名。省略時は git の user.name、なければ user.email、なければ環境のユーザー名。
        #[arg(long, value_name = "NAME")]
        author: Option<String>,
    },
    /// レビューをブラウザで開き、返信や解決をその画面で行う(自分のパソコンからだけ接続できる)。
    Serve {
        /// レビューバンドル(.diffnote)のパス。省略時は ./.diffnote。
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true
        )]
        review: PathBuf,
        /// 待ち受けるポート。省略時は空いているものを自動で選ぶ。
        #[arg(long, value_name = "PORT", default_value_t = 0)]
        port: u16,
        /// ブラウザを自動で開かない(URL だけを表示する)。
        #[arg(long)]
        no_open: bool,
        /// 返信などの作者名の既定値。省略時は git の user.name、なければ user.email、なければ環境のユーザー名。画面でも変えられます(その起動の間だけ)。
        #[arg(long, value_name = "NAME")]
        author: Option<String>,
        /// レビューのタイトルを設定する(`edit --title` と同じ)。画面でも変えられます。
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// git のレビューを作ったリポジトリ。バンドルに保存されていないファイルを、コミットから開くために使う。省略時は、起動したディレクトリ。
        #[arg(long, value_name = "DIR")]
        repo: Option<PathBuf>,
        /// レビューに加える比較対象。`edit` と同じ指定です。git のレビュー: ベースと比べるコミット(省略時は HEAD)。ディレクトリのレビュー: ベースのスナップショットと比べるディレクトリ(省略時は何も加えません)。すでに記録された差分や、空の差分は加えません。
        #[arg(value_name = "REV|DIR")]
        target: Option<String>,
        /// ベース(比較の起点)。まだバンドルがないときに指定できます(`edit --base` と同じ)。
        #[arg(long, value_name = "REV|DIR")]
        base: Option<String>,
        /// git のリポジトリの中でも、ディレクトリのレビューにする(まだバンドルがないときだけ意味があります)。
        #[arg(long)]
        files: bool,
    },
    /// レビューバンドルに保存されたスレッドと返信を表示する。
    Show {
        /// レビューバンドル(.diffnote)のパス。省略時は ./.diffnote。
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true
        )]
        review: PathBuf,
    },
    /// レビューバンドルを、単体で開ける HTML ファイルに書き出す。
    Export {
        /// レビューバンドル(.diffnote)のパス。省略時は ./.diffnote。
        #[arg(
            short = 'f',
            long = "file",
            default_value = ".diffnote",
            hide_default_value = true
        )]
        review: PathBuf,
        /// 出力する HTML のパス。例: `-o out.html`。
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// -o/--output の代わりに位置引数で渡す出力パス。例: `diffnote export out.html`。
        #[arg(index = 1, value_name = "OUTPUT")]
        output_pos: Option<PathBuf>,
        /// 差分が省略している行を、HTML に埋め込む行数の上限(全ファイル・全リビジョンの合計)。
        /// 埋め込んだ分は、HTML を開いたあとで展開できます。`0` で埋め込まず、`all` で全部埋め込みます。
        #[arg(long, value_name = "行数|all", default_value = "5000", value_parser = parse_expand_limit)]
        expand_limit: diffnote::html::ExpandLimit,
    },
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
            author,
        } => cmd_init(review, target, files, repo, title, author),
        Cmd::Edit {
            review,
            target,
            base,
            files,
            repo,
            snapshot,
            show,
            title,
            author,
        } => cmd_edit(
            review,
            Compare {
                target,
                base,
                files,
            },
            repo,
            snapshot,
            show,
            title,
            author,
        ),
        Cmd::Show { review } => cmd_show(review),
        Cmd::Serve {
            review,
            port,
            no_open,
            author,
            title,
            repo,
            target,
            base,
            files,
        } => cmd_serve(
            review,
            port,
            no_open,
            author,
            title,
            repo,
            Compare {
                target,
                base,
                files,
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
                    anyhow::bail!(
                        "出力パスは位置引数か -o/--output のどちらか一方で指定してください"
                    )
                }
                (None, None) => {
                    anyhow::bail!("出力パスがありません(位置引数か -o/--output で指定してください)")
                }
            };
            cmd_export(review, output, expand_limit)
        }
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
/// Nothing is written if the diff is empty or is already recorded.
fn add_revision(
    review_path: &Path,
    repo: &diffnote::git::Repo,
    base: Option<&str>,
    target: Option<&str>,
    files: bool,
) -> Result<()> {
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
                anyhow::bail!("{NEEDS_A_BASE}");
            };
            init_files(review_path, Path::new(base_dir), None, None, false)?;
            fresh = FreshBundle(Some(review_path.to_path_buf()));
            loaded = bundle::load(review_path)?;
        } else if let Some(base_dir) = base {
            check_files_base(&loaded, Path::new(base_dir), &exclude)?;
        }
        // The directory is only taken when it is named: `.` may be anywhere.
        let dir = match target {
            Some(dir) => dir,
            None if fresh.0.is_some() => ".",
            None => return Ok(()),
        };
        files_input(&loaded, Path::new(dir), &exclude)?
    } else {
        match loaded.source() {
            Some(_) => {
                if !explicit && !repo.exists() {
                    return Ok(());
                }
            }
            None if !explicit => {
                anyhow::bail!(
                    "{} がありません。先に `diffnote init` でレビューを作るか、レビューするコミットを指定してください(例: `diffnote serve HEAD`)",
                    review_path.display()
                );
            }
            None => {}
        }
        git_input(repo.clone(), git_range(repo, &loaded, base, target)?)?
    };
    if input.diff_text.trim().is_empty() || loaded.revisions().any(|r| r.digest == input.digest) {
        return Ok(());
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
            format!("差分を記録しました: {}..{}", short(&g.base), short(&g.head))
        }
        diffnote::model::Source::Files { .. } => "ディレクトリの変更を記録しました".to_string(),
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
    println!("{said}");
    Ok(())
}

/// The repository to work in: the one named by `--repo`, else the one the
/// current directory is in (not checked until it is used).
fn repo_of(dir: Option<PathBuf>) -> Result<diffnote::git::Repo> {
    match dir {
        Some(dir) => {
            let repo = diffnote::git::Repo::at(&dir);
            if !repo.exists() {
                anyhow::bail!("{} は git リポジトリではありません", dir.display());
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
}

fn cmd_serve(
    review: PathBuf,
    port: u16,
    no_open: bool,
    author: Option<String>,
    title: Option<String>,
    repo: Option<PathBuf>,
    compare: Compare,
) -> Result<()> {
    let Compare {
        target,
        base,
        files,
    } = compare;
    let explicit = target.is_some() || base.is_some();
    if !review.exists() && !explicit {
        anyhow::bail!(
            "{} がありません。先に `diffnote init` か `diffnote edit` でレビューを作るか、レビューするコミットを指定してください(例: `diffnote serve HEAD`)",
            review.display()
        );
    }
    // What was asked for is added to the review, so it can be reviewed here (a
    // failure to do what was asked stops; one to do what was not, only says so).
    let git = repo_of(repo.clone())?;
    match add_revision(&review, &git, base.as_deref(), target.as_deref(), files) {
        Ok(()) => {}
        Err(e) if explicit => return Err(e),
        Err(e) => println!("注意: 最新の差分を記録できませんでした: {e}"),
    }
    if !review.exists() {
        anyhow::bail!("レビューする差分がありません(バンドルは作りませんでした)");
    }
    if let Some(title) = title.as_deref() {
        let loaded = bundle::load(&review)?;
        let by = diffnote::author::resolve(author.as_deref());
        if let Some(event) = review::title_change(&loaded.events, title, &by) {
            let mut events = loaded.events.clone();
            events.push(event);
            let none = bundle::Additions {
                diff: None,
                blobs: Vec::new(),
            };
            bundle::save(&review, &loaded, &events, &none)?;
            println!("タイトルを設定しました");
        }
    }
    if bundle::load(&review)
        .ok()
        .is_some_and(|l| diffnote::html::view_model(&l).is_err())
    {
        println!(
            "注意: レビューする差分がまだありません。基準のあとにコミットを重ねてから、もう一度 `diffnote serve` を起動してください(比較対象は `diffnote serve <コミット>` でも指定できます)"
        );
    }
    let options = diffnote::serve::Options {
        review,
        port,
        author,
        repo,
    };
    diffnote::serve::run(&options, |url, notices| {
        for notice in notices {
            println!("注意: {notice}");
        }
        println!("ブラウザで開きます: {url}");
        println!(
            "終了するには、この画面で Ctrl+C を押すか、ブラウザの「終了」ボタンを押してください"
        );
        if !no_open && !open_in_browser(url) {
            println!("ブラウザを自動で開けませんでした。上の URL を、ブラウザに貼り付けてください");
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
        .map_err(|_| "行数(0 以上の整数)か `all` を指定してください".to_string())
}

fn cmd_export(
    review_path: PathBuf,
    output_path: PathBuf,
    limit: diffnote::html::ExpandLimit,
) -> Result<()> {
    let loaded = bundle::load(&review_path)?;
    let html = diffnote::html::render_export_with(&loaded, limit).with_context(|| {
        format!(
            "{} にはまだ記録された差分がありません。先に `diffnote edit` を実行してください",
            review_path.display()
        )
    })?;
    std::fs::write(&output_path, html)
        .with_context(|| format!("{} を書き込めませんでした", output_path.display()))?;
    println!("{} を書き出しました", output_path.display());
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

const NO_RANGES: &str = "範囲(`A..B`、`A B`)は指定できません。比較の起点(ベース)は、`diffnote init` か `--base` で決め、比較対象は 1 つだけ指定してください";

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
        anyhow::bail!("{NO_RANGES}");
    }
    let first = loaded.revisions().next().map(|r| &r.source);
    match first {
        Some(diffnote::model::Source::Git(first)) => {
            if let Some(base) = base {
                let asked = repo.commit_id(base)?;
                if asked != first.base {
                    anyhow::bail!(
                        "このバンドルのベースは {} です。ベースは変えられません(別のベースでレビューするときは、新しいバンドルを作ってください)",
                        &first.base[..first.base.len().min(10)]
                    );
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
            anyhow::bail!(
                "このバンドルはディレクトリのレビューです(git のコミットは指定できません)"
            )
        }
        None => match (base, target) {
            (Some(base), target) => repo.between(base, target.unwrap_or("HEAD")),
            (None, Some(target)) => repo.commit_range(target),
            (None, None) => anyhow::bail!(
                "レビューするコミットを指定してください(例: `diffnote edit HEAD`)。基準になる状態を先に決めるには、`diffnote init` を実行してください"
            ),
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
        .context("バンドルに、比べる対象のスナップショットがありません")?;
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
        anyhow::bail!(
            "{} の内容は、このバンドルのベースと違います。ベースは変えられません(別のベースでレビューするときは、新しいバンドルを作ってください)",
            dir.display()
        );
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

const NEEDS_A_BASE: &str = "ディレクトリのレビューを始めるには、`diffnote init` でベースを決めるか、`--base DIR` でベースのディレクトリを指定してください";

fn cmd_init(
    review_path: PathBuf,
    target: Option<String>,
    files: bool,
    repo: Option<PathBuf>,
    title: Option<String>,
    author: Option<String>,
) -> Result<()> {
    if review_path.exists() {
        anyhow::bail!("{} はすでに存在します", review_path.display());
    }
    let repo = repo_of(repo)?;
    if !files && repo.exists() {
        return init_git(
            &review_path,
            &repo,
            target.as_deref().unwrap_or("HEAD"),
            title,
            author,
        );
    }
    let dir = PathBuf::from(target.as_deref().unwrap_or("."));
    init_files(&review_path, &dir, title, author, true)
}

/// The events every new bundle starts with: what it was made by, and a title.
fn first_events(title: Option<&str>, author: Option<&str>) -> Vec<Event> {
    let mut events = vec![Event::Meta {
        version: 1,
        created_at: OffsetDateTime::now_utc(),
        description: None,
        context_lines: 3,
    }];
    if let Some(title) = title {
        events.extend(review::title_change(
            &events,
            title,
            &diffnote::author::resolve(author),
        ));
    }
    events
}

/// A git review that starts at a commit: the commit is the base, so that the
/// next `edit` reviews what has changed since. Nothing is stored beyond the
/// commit's id (git has the rest).
fn init_git(
    review_path: &Path,
    repo: &diffnote::git::Repo,
    rev: &str,
    title: Option<String>,
    author: Option<String>,
) -> Result<()> {
    let commit = repo.commit_id(rev)?;
    let mut events = first_events(title.as_deref(), author.as_deref());
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
        &bundle::load(review_path)?,
        &events,
        &additions,
    )?;
    println!(
        "{rev}({}) を基準として {} を作成しました。`diffnote edit` で、ここから今の HEAD までの変更をレビューできます",
        &commit[..commit.len().min(10)],
        review_path.display()
    );
    Ok(())
}

fn init_files(
    review_path: &Path,
    dir: &Path,
    title: Option<String>,
    author: Option<String>,
    say: bool,
) -> Result<()> {
    let tree = diffnote::files::read_tree(dir, &[review_path.to_path_buf()])?;
    let digest = diffnote::files::tree_digest(&tree);
    let size: u64 = tree.values().map(|b| b.len() as u64).sum();
    confirm_snapshot_size(bundle::SnapshotMode::Full, false, size);
    let mut events = first_events(title.as_deref(), author.as_deref());
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
        &bundle::load(review_path)?,
        &events,
        &additions,
    )?;
    if say {
        println!(
            "{count} 個のファイルを {} に保存しました",
            review_path.display()
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
    author: Option<String>,
) -> Result<()> {
    let Compare {
        target,
        base,
        files,
    } = compare;
    let shows: Vec<diffnote::show::Show> = show_specs
        .iter()
        .map(|spec| diffnote::show::parse(spec).map_err(|e| anyhow::anyhow!("--show {spec}: {e}")))
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
    let input = if directory_review {
        if snapshot_override == Some(bundle::SnapshotMode::Changed) {
            anyhow::bail!(
                "ディレクトリのレビューでは `--snapshot changed` は指定できません。git のように\
                残りを読み出す手段がなく、次の edit で比べるためにツリー全体が必要なので、\
                常に全体を保存します。レビューに不要なものは `.diffnoteignore` で除外して\
                ください。"
            );
        }
        let exclude = [review_path.clone(), draft_path_for(&review_path)];
        if loaded.source().is_none() {
            // No bundle: the base directory starts it, as `init` would.
            let Some(base_dir) = base.as_deref() else {
                anyhow::bail!("{NEEDS_A_BASE}");
            };
            init_files(&review_path, Path::new(base_dir), None, None, false)?;
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
    let author = diffnote::author::resolve(author.as_deref());
    let title_event = title
        .as_deref()
        .and_then(|t| review::title_change(&loaded.events, t, &author));
    if diff_text.trim().is_empty() {
        // Nothing to review, but a title can still be given to a review that exists.
        if let Some(event) = title_event
            && !loaded.events.is_empty()
        {
            let mut all_events = loaded.events.clone();
            all_events.push(event);
            let none = bundle::Additions {
                diff: None,
                blobs: Vec::new(),
            };
            bundle::save(&review_path, &loaded, &all_events, &none)?;
            fresh.keep();
            println!("タイトルを設定しました");
            return Ok(());
        }
        println!("レビューする変更がありません(差分が空です)");
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

    let temp_dir = tempfile::tempdir().context("一時ディレクトリを作れませんでした")?;
    let temp_path = temp_dir.path().join("review.diff");

    // If a previous session's edits failed to parse (or the editor itself
    // exited non-zero), they were saved here instead of being lost -- reopen
    // that instead of a fresh render so the user can just fix the mistake.
    let draft_path = draft_path_for(&review_path);
    let initial_text = match std::fs::read_to_string(&draft_path) {
        Ok(draft) => {
            println!(
                "前回の編集がうまく保存できなかったときの下書きから再開します: {}",
                draft_path.display()
            );
            draft
        }
        Err(_) => temp_text.clone(),
    };
    std::fs::write(&temp_path, &initial_text)
        .context("一時ファイルにコメント用のバッファを書き込めませんでした")?;

    let editor = default_editor();
    let words = diffnote::editor::command_words(&editor);
    let (program, args) = words
        .split_first()
        .context("$EDITOR が空です。エディタのコマンドを設定してください")?;
    let status = Command::new(program)
        .args(args)
        .arg(&temp_path)
        .status()
        .with_context(|| {
            format!("エディタ '{editor}' を起動できませんでした($EDITOR を設定してください)")
        })?;

    let annotated =
        std::fs::read_to_string(&temp_path).context("編集したファイルを読み戻せませんでした")?;

    if !status.success() {
        save_draft(&draft_path, &annotated)?;
        anyhow::bail!(
            "エディタ '{editor}' が異常終了しました。編集内容は {} に保存しました。\
            直してから `diffnote edit` を再実行すると続きから再開できます",
            draft_path.display()
        );
    }

    let parsed = match annotation::parse(&annotated) {
        Ok(parsed) => parsed,
        Err(e) => {
            save_draft(&draft_path, &annotated)?;
            anyhow::bail!(
                "編集内容を解釈できませんでした: {e}\n\n編集内容は {} に保存しました。\
                誤りを直して `diffnote edit` を再実行すると、続きから再開できます。",
                draft_path.display()
            );
        }
    };

    for warning in &parsed.warnings {
        eprintln!("警告: {warning}");
    }

    // Parsing succeeded, so nothing here is at risk of being lost anymore --
    // any draft from an earlier failed attempt is now stale.
    let _ = std::fs::remove_file(&draft_path);

    if existing_events.is_empty() && parsed.items.is_empty() && title_event.is_none() {
        println!("コメントは追加されませんでした");
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
    let title_set = title_event.is_some();
    new_events.extend(title_event);

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
                        anyhow::bail!(
                            "'>!reanchor' は、同じブロックの中でコメント本文や他のディレクティブと併用できません"
                        );
                    }
                    let annotation::Directive::Reanchor(id_str) = &directives[pos] else {
                        unreachable!()
                    };
                    let target_id = Ulid::from_string(id_str).map_err(|_| {
                        anyhow::anyhow!("'>!reanchor {id_str}': スレッド ID として正しくありません")
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
                    annotation::ThreadRef::New(tid) => *thread_ids
                        .get(tid.0)
                        .ok_or_else(|| anyhow::anyhow!("内部エラー: 不明なスレッド参照です"))?,
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

    if new_events.is_empty() {
        println!("変更はありません");
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
        println!("タイトルを設定しました");
    }
    println!(
        "コメント {comment_count} 件({} 件のイベント)を {} に書き込みました",
        new_events.len(),
        review_path.display()
    );
    Ok(())
}

fn cmd_show(review_path: PathBuf) -> Result<()> {
    let loaded = bundle::load(&review_path)?;
    let events = loaded.events;
    if events.is_empty() {
        println!("{} は空です", review_path.display());
        return Ok(());
    }
    for event in &events {
        match event {
            Event::Meta { context_lines, .. } => {
                println!("[メタ] context_lines={context_lines}");
            }
            Event::Revision(r) => {
                println!(
                    "[リビジョン] {} digest={} 対象={} スナップショット={:?}",
                    r.id,
                    r.digest,
                    match &r.source {
                        diffnote::model::Source::Git(g) => g.spec.as_str(),
                        diffnote::model::Source::Files { .. } => "(ディレクトリ)",
                    },
                    r.snapshot_mode
                );
            }
            Event::Title { title, author, .. } => {
                println!("[タイトル] {title} -- {author}");
            }
            Event::Pin { revision, files } => {
                println!("[固定] リビジョン {revision}: {} 個のファイル", files.len());
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
                    "[{id}] 新規  {} -- {author}",
                    describe_anchor(anchor.as_ref())
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
                println!("[{id}] 返信 -> {parent} -- {author}");
                print_body(body);
            }
            Event::Resolve { parent, author, .. } => {
                println!("      解決 {parent} -- {author}");
            }
            Event::Reopen { parent, author, .. } => {
                println!("      再オープン {parent} -- {author}");
            }
            Event::Reanchor {
                parent,
                author,
                anchor,
                ..
            } => {
                println!(
                    "      再アンカー {parent} -> {} -- {author}",
                    describe_anchor(Some(anchor))
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
            format!("{}:{a} (挿入位置)", s.file)
        } else if a == b {
            format!("{}:{a}", s.file)
        } else {
            format!("{}:{a}-{b}", s.file)
        }
    };
    match anchor {
        None => "?".to_string(),
        Some(Anchor::Global { .. }) => "差分全体".to_string(),
        Some(Anchor::File { base, head }) => format!(
            "ファイル全体: {}",
            head.as_ref()
                .or(base.as_ref())
                .map_or("?", |f| f.file.as_str())
        ),
        Some(Anchor::Span { base, head }) => match (base, head) {
            (Some(b), Some(h)) if !b.is_empty() && !h.is_empty() => {
                format!("{} <- {}", span(h), span(b))
            }
            (_, Some(h)) if !h.is_empty() => span(h),
            (Some(b), _) => format!("{} (削除)", span(b)),
            _ => "?".to_string(),
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
            anyhow::bail!("内部エラー: {directive:?} は呼び出し側で処理されるはずです");
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
    std::fs::write(draft_path, content)
        .with_context(|| format!("下書きを {} に保存できませんでした", draft_path.display()))
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
    eprintln!("警告: {warning}");
    if !is_git {
        eprintln!("レビューに不要なものは `.diffnoteignore` で除外してください。");
        return mode;
    }
    eprint!("代わりに `changed`(この差分が触れるファイルだけ)にしますか? [y/N] ");
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
