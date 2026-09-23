//! diffnote 専用の、OS のユーザーごとの設定(`author` など)。バンドルの
//! `settings.json`(レビューごとに効く)と違い、この設定はマシン上のこの
//! ユーザーが `init`/`serve` する、すべてのバンドルに効く。
//!
//! OS のユーザー設定ディレクトリの `diffnote/config.json` に保存する
//! (`DIFFNOTE_CONFIG_DIR` を設定すれば、そのディレクトリを使う。テストや、
//! 置き場所を変えたい場合のため)。

use crate::messages::{m, mf};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserConfig {
    /// コメントなどの作者名の既定値。`--author` や、バンドルにひもづく
    /// 何かより優先度は低いが、git の設定より高い(git の `user.name` は
    /// リポジトリごとに違うことがあるが、これは一貫させるためのもの)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

/// 設定ファイルの場所。決めようがなければ(`HOME` が読めないなど)`None`。
pub fn path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DIFFNOTE_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("config.json"));
    }
    Some(dirs::config_dir()?.join("diffnote").join("config.json"))
}

/// 今の設定。ファイルがない、壊れている、読めないときは既定値(何も設定
/// されていない状態)を返す(ユーザー設定が読めないせいで `serve`
/// 自体が失敗することはない)。
pub fn load() -> UserConfig {
    path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 保存する。設定ディレクトリがなければ作る。
pub fn save(config: &UserConfig) -> Result<()> {
    let path = path().context(m("user_config.save_dir_failed"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| {
            mf(
                "user_config.create_dir_failed",
                &[("path", &dir.display().to_string())],
            )
        })?;
    }
    let json = serde_json::to_vec_pretty(config).context(m("user_config.encode_failed"))?;
    std::fs::write(&path, json).with_context(|| {
        mf(
            "user_config.write_failed",
            &[("path", &path.display().to_string())],
        )
    })?;
    Ok(())
}

/// テスト用: `DIFFNOTE_CONFIG_DIR` を一時ディレクトリに向けて `f` を呼び、
/// 実行前の値に戻す。この crate のどのテストも(`serve` のテストのように、
/// `load`/`save` を間接に呼ぶものも含め)、この関数越しでなければ触れては
/// いけない -- そうでないと、このマシンの実際の設定ファイルを読み書きして
/// しまう。環境変数はプロセス全体で共有なので、プロセス全体のロックを取る
/// (テストどうしが並行に動いても競合しない)。
#[cfg(test)]
pub(crate) fn with_test_config_dir<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let before = std::env::var_os("DIFFNOTE_CONFIG_DIR");
    // SAFETY: the environment change is confined to this call by the lock above.
    unsafe {
        std::env::set_var("DIFFNOTE_CONFIG_DIR", dir.path());
    }
    let result = f(dir.path());
    unsafe {
        match &before {
            Some(v) => std::env::set_var("DIFFNOTE_CONFIG_DIR", v),
            None => std::env::remove_var("DIFFNOTE_CONFIG_DIR"),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_nothing_saved_yet_the_config_is_empty() {
        with_test_config_dir(|_| {
            assert_eq!(load(), UserConfig::default());
        });
    }

    #[test]
    fn what_is_saved_is_what_load_returns_next() {
        with_test_config_dir(|dir| {
            save(&UserConfig {
                author: Some("山田 太郎".into()),
            })
            .unwrap();
            assert_eq!(
                load(),
                UserConfig {
                    author: Some("山田 太郎".into())
                }
            );
            assert!(dir.join("config.json").exists());
        });
    }

    #[test]
    fn a_broken_file_is_treated_as_no_config_rather_than_an_error() {
        with_test_config_dir(|dir| {
            std::fs::write(dir.join("config.json"), b"not json").unwrap();
            assert_eq!(load(), UserConfig::default());
        });
    }
}
