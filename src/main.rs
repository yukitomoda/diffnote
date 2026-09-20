use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use diffnote::digest::digest;
use diffnote::model::{Anchor, Event};
use diffnote::{annotation, bundle, review};
use std::path::{Path, PathBuf};
use std::process::Command;
use time::OffsetDateTime;
use ulid::Ulid;

/// diffnote: comment on a git diff locally, share the review as a file.
#[derive(Debug, Parser)]
#[command(name = "diffnote", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Snapshot a plain directory (no git) into a new review bundle, so later
    /// `edit` runs can review what changed since. Not needed for git reviews.
    Init {
        /// Path of the diffnote review bundle to create (.diffnote, a zip).
        #[arg(short = 'f', long = "file", default_value = ".diffnote")]
        review: PathBuf,
        /// The directory to snapshot. Files matched by `.diffnoteignore`
        /// (or, if there is none, `.gitignore`) are left out.
        #[arg(value_name = "DIR", default_value = ".")]
        dir: PathBuf,
    },
    /// Open what is being reviewed for annotation in $EDITOR, appending new
    /// comments to a review bundle (created if missing, for git reviews).
    Edit {
        /// Path to the diffnote review bundle (.diffnote, a zip).
        #[arg(short = 'f', long = "file", default_value = ".diffnote")]
        review: PathBuf,
        /// Git review: the commits, resolved by git itself -- `A..B` or
        /// `A B` (A -> B), `A...B` (merge-base of A and B -> B), or a single
        /// commit (its first parent -> it). Only committed content is
        /// reviewed. Directory review (bundle made by `init`): at most one
        /// argument, the directory to compare with the bundle's last
        /// snapshot (default: the current directory).
        #[arg(value_name = "REV|DIR", num_args = 0..)]
        targets: Vec<String>,
        /// What to keep in the bundle the first time a new diff digest is
        /// seen and this session actually adds something: `changed` (both
        /// sides of every touched file, plus every file a comment refers to)
        /// or `full` (+ the whole head tree). Defaults to the bundle's own
        /// previously-established mode if it has one, otherwise `full`.
        /// Directory reviews always keep the full tree.
        #[arg(long, value_enum)]
        snapshot: Option<diffnote::bundle::SnapshotMode>,
    },
    /// Print the threads/replies stored in a review bundle.
    Show {
        /// Path to the diffnote review bundle (.diffnote).
        #[arg(short = 'f', long = "file", default_value = ".diffnote")]
        review: PathBuf,
    },
    /// Render a review bundle to a self-contained HTML file.
    Export {
        /// Path to the diffnote review bundle (.diffnote).
        #[arg(short = 'f', long = "file", default_value = ".diffnote")]
        review: PathBuf,
        /// Output HTML path, e.g. `-o out.html`.
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Output HTML path, given positionally instead of via -o/--output,
        /// e.g. `diffnote export out.html`.
        #[arg(index = 1, value_name = "OUTPUT")]
        output_pos: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Init { review, dir } => cmd_init(review, dir),
        Cmd::Edit {
            review,
            targets,
            snapshot,
        } => cmd_edit(review, targets, snapshot),
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
                        "pass the output path either positionally or via -o/--output, not both"
                    )
                }
                (None, None) => {
                    anyhow::bail!("missing output path (pass it positionally or via -o/--output)")
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
            "{} has no captured diff yet; run `diffnote edit` first",
            review_path.display()
        )
    })?;
    std::fs::write(&output_path, html)
        .with_context(|| format!("failed to write {}", output_path.display()))?;
    println!("Wrote {}", output_path.display());
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
    /// `Some` when only one snapshot mode makes sense for this source.
    forced_snapshot_mode: Option<bundle::SnapshotMode>,
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
        forced_snapshot_mode: None,
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
        .context("the bundle has no snapshot to compare with")?;
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
        forced_snapshot_mode: Some(bundle::SnapshotMode::Full),
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
        anyhow::bail!("{} already exists", review_path.display());
    }
    let tree = diffnote::files::read_tree(&dir, std::slice::from_ref(&review_path))?;
    let digest = diffnote::files::tree_digest(&tree);
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
    println!("Snapshotted {count} file(s) into {}", review_path.display());
    Ok(())
}

fn cmd_edit(
    review_path: PathBuf,
    targets: Vec<String>,
    snapshot_override: Option<bundle::SnapshotMode>,
) -> Result<()> {
    let loaded = bundle::load(&review_path)?;
    let input = match loaded.source() {
        Some(diffnote::model::Source::Files { .. }) => {
            if targets.len() > 1 {
                anyhow::bail!("a directory review takes at most one argument: the directory");
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
            "specify the commit(s) to review, e.g. `diffnote edit HEAD~3..HEAD`; \
            to review a plain directory instead, run `diffnote init` first"
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
        forced_snapshot_mode,
        tree_size,
        head_some,
        head_all,
    } = input;
    if diff_text.trim().is_empty() {
        println!("No changes to review (diff is empty).");
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
    let temp_text = if existing_threads.is_empty() {
        diff_text.clone()
    } else {
        annotation::render_for_edit(
            &diff_text,
            &parsed_diff,
            &diffnote::anchor::ViewVersions {
                files: &files,
                tree: &tree_now,
            },
            &blobs,
            &existing_threads,
        )
    };

    let temp_dir = tempfile::tempdir().context("failed to create a temp directory")?;
    let temp_path = temp_dir.path().join("review.diff");

    // If a previous session's edits failed to parse (or the editor itself
    // exited non-zero), they were saved here instead of being lost -- reopen
    // that instead of a fresh render so the user can just fix the mistake.
    let draft_path = draft_path_for(&review_path);
    let initial_text = match std::fs::read_to_string(&draft_path) {
        Ok(draft) => {
            println!(
                "Resuming a draft saved after a previous edit didn't save cleanly: {}",
                draft_path.display()
            );
            draft
        }
        Err(_) => temp_text.clone(),
    };
    std::fs::write(&temp_path, &initial_text)
        .context("failed to write the temp annotation file")?;

    let editor = default_editor();
    let status = Command::new(&editor)
        .arg(&temp_path)
        .status()
        .with_context(|| format!("failed to launch editor '{editor}' (set $EDITOR)"))?;

    let annotated =
        std::fs::read_to_string(&temp_path).context("failed to read back the annotated file")?;

    if !status.success() {
        save_draft(&draft_path, &annotated)?;
        anyhow::bail!(
            "editor '{editor}' exited with a failure status; your edits were saved to {} \
            -- fix them and rerun `diffnote edit` to resume, aborting",
            draft_path.display()
        );
    }

    let parsed = match annotation::parse(&annotated) {
        Ok(parsed) => parsed,
        Err(e) => {
            save_draft(&draft_path, &annotated)?;
            anyhow::bail!(
                "failed to parse your edits: {e}\n\nYour changes were saved to {} -- fix the \
                error and run `diffnote edit` again to resume where you left off.",
                draft_path.display()
            );
        }
    };

    for warning in &parsed.warnings {
        eprintln!("warning: {warning}");
    }

    // Parsing succeeded, so nothing here is at risk of being lost anymore --
    // any draft from an earlier failed attempt is now stale.
    let _ = std::fs::remove_file(&draft_path);

    if existing_events.is_empty() && parsed.items.is_empty() {
        println!("No comments added.");
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
                            "'>!reanchor' cannot be combined with comment text or other directives in the same block"
                        );
                    }
                    let annotation::Directive::Reanchor(id_str) = &directives[pos] else {
                        unreachable!()
                    };
                    let target_id = Ulid::from_string(id_str).map_err(|_| {
                        anyhow::anyhow!("'>!reanchor {id_str}': not a valid thread id")
                    })?;
                    let new_anchor = diffnote::create::build_anchor(scope, &parsed.diff, &files, &revisions)?;
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
                let comment_anchor = diffnote::create::build_anchor(scope, &parsed.diff, &files, &revisions)?;
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
                        anyhow::anyhow!("internal error: unknown thread reference")
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
        println!("No changes.");
        return Ok(());
    }

    // Only record a not-yet-seen diff when this session actually produced
    // something -- an idle "opened it, looked, closed it" pass shouldn't
    // grow the bundle.
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
        &|| {
            forced_snapshot_mode.unwrap_or_else(|| {
                resolve_snapshot_mode(snapshot_override, loaded.snapshot_mode(), tree_size)
            })
        },
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
        "Wrote {comment_count} comment(s) ({} event(s) total) to {}",
        new_events.len(),
        review_path.display()
    );
    Ok(())
}

fn cmd_show(review_path: PathBuf) -> Result<()> {
    let loaded = bundle::load(&review_path)?;
    let events = loaded.events;
    if events.is_empty() {
        println!("{} is empty.", review_path.display());
        return Ok(());
    }
    for event in &events {
        match event {
            Event::Meta { context_lines, .. } => {
                println!("[meta] context_lines={context_lines}");
            }
            Event::Revision(r) => {
                println!(
                    "[revision] {} digest={} source={} snapshot={:?}",
                    r.id,
                    r.digest,
                    match &r.source {
                        diffnote::model::Source::Git(g) => g.spec.as_str(),
                        diffnote::model::Source::Files { .. } => "(directory)",
                    },
                    r.snapshot_mode
                );
            }
            Event::Pin { revision, files } => {
                println!("[pin] revision {revision}: {} file(s)", files.len());
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
                    "[{id}] NEW  {} -- {author}",
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
                println!("[{id}] REPLY -> {parent} -- {author}");
                print_body(body);
            }
            Event::Resolve { parent, author, .. } => {
                println!("      RESOLVE {parent} -- {author}");
            }
            Event::Reopen { parent, author, .. } => {
                println!("      REOPEN {parent} -- {author}");
            }
            Event::Reanchor {
                parent,
                author,
                anchor,
                ..
            } => {
                println!(
                    "      REANCHOR {parent} -> {} -- {author}",
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
        Some(Anchor::Global { .. }) => "diff全体".to_string(),
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
            anyhow::bail!("internal error: {directive:?} should have been handled by the caller");
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
        .with_context(|| format!("failed to save draft to {}", draft_path.display()))
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

/// Bytes at which a `full` snapshot is considered "big enough to ask
/// about" -- 30 MB, per the user's own threshold.
const FULL_SNAPSHOT_WARN_BYTES: u64 = 30 * 1024 * 1024;

/// Picks the `SnapshotMode` for a newly-captured digest. In priority
/// order: an explicit `--snapshot` always wins; otherwise the bundle's own
/// previously-established mode (so an existing bundle's behavior never
/// silently changes just because the CLI's own default did); otherwise
/// `Full`.
///
/// If the mode lands on `Full` (however it got there) and the head tree is
/// at least `FULL_SNAPSHOT_WARN_BYTES`, warns and offers to use `Changed`
/// instead.
fn resolve_snapshot_mode(
    explicit: Option<bundle::SnapshotMode>,
    stored: Option<bundle::SnapshotMode>,
    tree_size: u64,
) -> bundle::SnapshotMode {
    let mode = explicit.or(stored).unwrap_or(bundle::SnapshotMode::Full);
    if mode != bundle::SnapshotMode::Full {
        return mode;
    }

    let size = tree_size;
    if size < FULL_SNAPSHOT_WARN_BYTES {
        return mode;
    }

    eprintln!(
        "warning: a `full` snapshot of the reviewed tree would be about {:.1} MB.",
        size as f64 / (1024.0 * 1024.0)
    );
    eprint!("Use `changed` instead (only the files this diff touches)? [y/N] ");
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
