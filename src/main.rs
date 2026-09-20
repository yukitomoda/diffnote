use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
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
        sub.help_template(HELP_TEMPLATE)
                        .mut_args(|a| {
                {
                    let heading = if a.is_positional() { "引数" } else { "オプション" };
                    a.help_heading(heading)
                }
            })
    })
}

const HELP_TEMPLATE: &str = "{about}\n\n使い方: {usage}\n\n{all-args}";

#[derive(Debug, Subcommand)]
enum Cmd {
    /// git を使わないディレクトリのスナップショットを新しいレビューバンドルに保存する。以降の `edit` で、その時点からの変更をレビューできる。git のレビューでは不要。
    Init {
        /// 作成するレビューバンドル(.diffnote、zip 形式)のパス。省略時は ./.diffnote。
        #[arg(short = 'f', long = "file", default_value = ".diffnote", hide_default_value = true)]
        review: PathBuf,
        /// スナップショットを取るディレクトリ。省略時はカレントディレクトリ。`.diffnoteignore`(なければ`.gitignore`)に一致するファイルは含めない。
        #[arg(value_name = "DIR", default_value = ".", hide_default_value = true)]
        dir: PathBuf,
    },
    /// レビュー対象を $EDITOR で開いてコメントを書き、レビューバンドルに追記する(git のレビューでは、バンドルがなければ作成する)。
    Edit {
        /// レビューバンドル(.diffnote、zip 形式)のパス。省略時は ./.diffnote。
        #[arg(short = 'f', long = "file", default_value = ".diffnote", hide_default_value = true)]
        review: PathBuf,
        /// git のレビュー: git 自身が解決するコミット指定。`A..B` または `A B`(A → B)、`A...B`(A と B のマージベース → B)、単一のコミット(その第一親 → そのコミット)。コミット済みの内容だけをレビューする。ディレクトリのレビュー(`init` で作ったバンドル): 引数は多くても 1 つで、バンドルの最後のスナップショットと比べるディレクトリ(省略時はカレント)。
        #[arg(value_name = "REV|DIR", num_args = 0..)]
        targets: Vec<String>,
        /// 新しい差分を初めて見て、かつこの回で何かを追加したときに、バンドルへ保存する内容。`changed`(差分が触れた全ファイルの両側と、コメントが参照する全ファイル)か、`full`(それに加えて head 全体のツリー)。省略時は、バンドルにすでに決まっているモード、なければ git のレビューでは `changed`(残りは git が持っている)。ディレクトリのレビューは常に全体を保存するので、そこで `--snapshot changed` を指定するとエラーになる。
        #[arg(long, value_enum, hide_possible_values = true)]
        snapshot: Option<diffnote::bundle::SnapshotMode>,
        /// ファイル(`PATH`)またはその一部の行(`PATH:START-END`、`PATH:LINE`)をバッファに入れる。差分が触れていない箇所にもコメントを書ける。レビューの head 側の内容が対象。繰り返し指定できる。差分がすでに表示している箇所の前後 3 行は、重ねて追加されない。
        #[arg(long = "show", value_name = "PATH[:START[-END]]")]
        show: Vec<String>,
    },
    /// レビューバンドルに保存されたスレッドと返信を表示する。
    Show {
        /// レビューバンドル(.diffnote)のパス。省略時は ./.diffnote。
        #[arg(short = 'f', long = "file", default_value = ".diffnote", hide_default_value = true)]
        review: PathBuf,
    },
    /// レビューバンドルを、単体で開ける HTML ファイルに書き出す。
    Export {
        /// レビューバンドル(.diffnote)のパス。省略時は ./.diffnote。
        #[arg(short = 'f', long = "file", default_value = ".diffnote", hide_default_value = true)]
        review: PathBuf,
        /// 出力する HTML のパス。例: `-o out.html`。
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// -o/--output の代わりに位置引数で渡す出力パス。例: `diffnote export out.html`。
        #[arg(index = 1, value_name = "OUTPUT")]
        output_pos: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = {
        use clap::FromArgMatches;
        Cli::from_arg_matches(&command().get_matches())?
    };
    match cli.command {
        Cmd::Init { review, dir } => cmd_init(review, dir),
        Cmd::Edit {
            review,
            targets,
            snapshot,
            show,
        } => cmd_edit(review, targets, snapshot, show),
        Cmd::Show { review } => cmd_show(review),
        Cmd::Export {
            review,
            output,
            output_pos,
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
            cmd_export(review, output)
        }
    }
}

fn cmd_export(review_path: PathBuf, output_path: PathBuf) -> Result<()> {
    let loaded = bundle::load(&review_path)?;
    let html = diffnote::html::render_bundle(&loaded).with_context(|| {
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

fn git_input(targets: &[String]) -> Result<Input> {
    let repo = diffnote::git::Repo::current();
    let range = repo.resolve_range(targets)?;
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
    let new_files = repo.read_paths(&head_tree, &touched_new)?.into_iter().collect();
    let base_files = repo.read_paths(&base_tree, &touched_old)?.into_iter().collect();
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

/// Compares `dir` with the bundle's last recorded snapshot.
fn files_input(loaded: &bundle::Loaded, dir: &Path, exclude: &[PathBuf]) -> Result<Input> {
    let previous = loaded
        .revisions()
        .last()
        .map(|r| loaded.tree_of(r))
        .context("バンドルに、比べる対象のスナップショットがありません")?;
    let current = diffnote::files::read_tree(dir, exclude)?;
    let digest = diffnote::files::tree_digest(&current);
    // Unchanged since the last recorded snapshot: reopen the diff that
    // revision was reviewed with (so replies/resolves can still be added),
    // rather than the empty diff against itself.
    let latest = loaded.latest_revision();
    let (diff_text, files, base) = match latest {
        Some((rev, text)) if rev.digest == digest => {
            let base = match &rev.source {
                diffnote::model::Source::Files { base } => base.clone(),
                diffnote::model::Source::Git(_) => None,
            };
            (text, rev.files.clone(), base)
        }
        _ => {
            let (text, files) = diffnote::files::diff_trees(&previous, &current);
            (text, files, latest.map(|(rev, _)| rev.digest.clone()))
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

fn cmd_init(review_path: PathBuf, dir: PathBuf) -> Result<()> {
    if review_path.exists() {
        anyhow::bail!("{} はすでに存在します", review_path.display());
    }
    let tree = diffnote::files::read_tree(&dir, std::slice::from_ref(&review_path))?;
    let digest = diffnote::files::tree_digest(&tree);
    let size: u64 = tree.values().map(|b| b.len() as u64).sum();
    confirm_snapshot_size(bundle::SnapshotMode::Full, false, size);
    let events = vec![
        Event::Meta {
            version: 1,
            created_at: OffsetDateTime::now_utc(),
            description: None,
            context_lines: 3,
        },
        Event::Revision(diffnote::model::Revision {
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
        }),
    ];
    let count = tree.len();
    let additions = bundle::Additions {
        diff: Some((digest, String::new())),
        blobs: tree.into_values().collect(),
    };
    bundle::save(
        &review_path,
        &bundle::load(&review_path)?,
        &events,
        &additions,
    )?;
    println!("{count} 個のファイルを {} に保存しました", review_path.display());
    Ok(())
}

fn cmd_edit(
    review_path: PathBuf,
    targets: Vec<String>,
    snapshot_override: Option<bundle::SnapshotMode>,
    show_specs: Vec<String>,
) -> Result<()> {
    let shows: Vec<diffnote::show::Show> = show_specs
        .iter()
        .map(|spec| {
            diffnote::show::parse(spec).map_err(|e| anyhow::anyhow!("--show {spec}: {e}"))
        })
        .collect::<Result<_>>()?;
    let loaded = bundle::load(&review_path)?;
    let input = match loaded.source() {
        Some(diffnote::model::Source::Files { .. }) => {
            if snapshot_override == Some(bundle::SnapshotMode::Changed) {
                anyhow::bail!(
                    "ディレクトリのレビューでは `--snapshot changed` は指定できません。git のように\
                    残りを読み出す手段がなく、次の edit で比べるためにツリー全体が必要なので、\
                    常に全体を保存します。レビューに不要なものは `.diffnoteignore` で除外して\
                    ください。"
                );
            }
            if targets.len() > 1 {
                anyhow::bail!("ディレクトリのレビューが受け取る引数は、ディレクトリ 1 つまでです");
            }
            let dir = PathBuf::from(targets.first().map_or(".", String::as_str));
            files_input(
                &loaded,
                &dir,
                &[review_path.clone(), draft_path_for(&review_path)],
            )?
        }
        Some(diffnote::model::Source::Git(_)) => git_input(&targets)?,
        None if targets.is_empty() => anyhow::bail!(
            "レビューするコミットを指定してください(例: `diffnote edit HEAD~3..HEAD`)。\
            git を使わないディレクトリをレビューするには、先に `diffnote init` を実行してください"
        ),
        None => git_input(&targets)?,
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
    if diff_text.trim().is_empty() {
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
    let anchor_files: Vec<diffnote::model::FileDigest> = files
        .iter()
        .cloned()
        .chain(synthetic_files)
        .collect();

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
    let status = Command::new(&editor)
        .arg(&temp_path)
        .status()
        .with_context(|| format!("エディタ '{editor}' を起動できませんでした($EDITOR を設定してください)"))?;

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

    if existing_events.is_empty() && parsed.items.is_empty() {
        println!("コメントは追加されませんでした");
        return Ok(());
    }

    let author = resolve_author();
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
                    let new_anchor = diffnote::create::build_anchor(scope, &parsed.diff, &anchor_files, &revisions)?;
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
                let comment_anchor = diffnote::create::build_anchor(scope, &parsed.diff, &anchor_files, &revisions)?;
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
                        anyhow::anyhow!("内部エラー: 不明なスレッド参照です")
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
            head.as_ref().or(base.as_ref()).map_or("?", |f| f.file.as_str())
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

fn resolve_author() -> String {
    if let Ok(output) = Command::new("git").args(["config", "user.email"]).output()
        && output.status.success()
    {
        let email = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !email.is_empty() {
            return email;
        }
    }
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn default_editor() -> String {
    std::env::var("EDITOR").unwrap_or_else(|_| {
        if cfg!(windows) {
            "notepad".to_string()
        } else {
            "vi".to_string()
        }
    })
}
