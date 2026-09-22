//! Makes sure the page in `ui/dist` is there, and was built from the `ui/src`
//! on disk, before `src/html.rs` embeds it in the binary.
//!
//! Nothing is built here: the page is built with node, the binary with cargo,
//! and `mise run build` runs the two in that order (see ui/README.md). This
//! only catches the case where cargo is run by itself -- by hand, or by an
//! editor -- with a stale page on disk, which would otherwise be embedded
//! without a word. `ui/build.mjs` writes what it built from into
//! `ui/dist/.sources`, and this works the same sum out again.
//! `DIFFNOTE_SKIP_UI_CHECK=1` turns the whole check off.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const UI: &str = "ui";
const BUILT: [&str; 4] = ["export.js", "serve.js", "style.css", ".sources"];
/// Beside `src/`: what the bundle is built with, and what it is built from.
const ALSO: [&str; 3] = ["build.mjs", "package.json", "package-lock.json"];

fn main() {
    // The sources, and the directories themselves: a file added to or taken out
    // of one changes its own time, and nothing else's.
    for dir in ["ui/src", "ui/dist"] {
        println!("cargo:rerun-if-changed={dir}");
        for path in files(Path::new(dir)) {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    for name in ALSO {
        println!("cargo:rerun-if-changed={UI}/{name}");
    }
    println!("cargo:rerun-if-env-changed=DIFFNOTE_SKIP_UI_CHECK");
    if std::env::var_os("DIFFNOTE_SKIP_UI_CHECK").is_some() {
        return;
    }

    let missing: Vec<String> = BUILT
        .iter()
        .map(|name| format!("{UI}/dist/{name}"))
        .filter(|path| !Path::new(path).is_file())
        .collect();
    if !missing.is_empty() {
        fail(&format!("{} がありません", missing.join(", ")));
    }
    let built = std::fs::read_to_string(format!("{UI}/dist/.sources")).unwrap_or_default();
    if built.trim() != sources() {
        fail("ui/dist が ui/src と合っていません");
    }
}

fn fail(what: &str) -> ! {
    println!("cargo:warning={what}");
    println!("cargo:warning=`mise run build` で画面をビルドしてください(ui/README.md)");
    std::process::exit(1);
}

/// The same sum over the same files as `ui/build.mjs` works out: each path (as
/// `ui/` sees it, with `/`), then its bytes, in the order of the paths.
fn sources() -> String {
    let mut paths: Vec<String> = files(Path::new(UI).join("src").as_path())
        .iter()
        // The tests of the page are not in any bundle.
        .filter(|path| !path.components().any(|c| c.as_os_str() == "test"))
        .map(|path| relative(path.as_path()))
        .collect();
    paths.extend(ALSO.iter().map(|name| (*name).to_string()));
    paths.sort();

    let mut sum = Sha256::new();
    for path in paths {
        sum.update(path.as_bytes());
        sum.update(b"\0");
        sum.update(std::fs::read(Path::new(UI).join(&path)).unwrap_or_default());
        sum.update(b"\0");
    }
    format!("{:x}", sum.finalize())
}

fn relative(path: &Path) -> String {
    path.strip_prefix(UI)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.push(path);
        }
    }
    found
}
