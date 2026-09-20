//! Who a comment is by: `--author`, else git's `user.name`, else its
//! `user.email`, else the login name from the environment.

use std::process::Command;

/// The first of the given values that isn't blank, or `unknown`.
pub fn pick(
    explicit: Option<&str>,
    git_name: Option<&str>,
    git_email: Option<&str>,
    login: Option<&str>,
) -> String {
    [explicit, git_name, git_email, login]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn git_config(key: &str) -> Option<String> {
    let output = Command::new("git").args(["config", key]).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The author to record, from `--author` (if given) and this machine's
/// settings.
pub fn resolve(explicit: Option<&str>) -> String {
    let login = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).ok();
    // `--author` needs no git at all.
    if let Some(explicit) = explicit.filter(|e| !e.trim().is_empty()) {
        return explicit.trim().to_string();
    }
    pick(
        None,
        git_config("user.name").as_deref(),
        git_config("user.email").as_deref(),
        login.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_explicit_author_beats_everything() {
        assert_eq!(pick(Some("Me"), Some("Name"), Some("e@x"), Some("login")), "Me");
    }

    #[test]
    fn git_name_comes_before_email_and_login() {
        assert_eq!(pick(None, Some("Yuki T"), Some("e@x"), Some("login")), "Yuki T");
        assert_eq!(pick(None, None, Some("e@x"), Some("login")), "e@x");
        assert_eq!(pick(None, None, None, Some("login")), "login");
        assert_eq!(pick(None, None, None, None), "unknown");
    }

    #[test]
    fn blank_values_are_skipped_and_the_result_is_trimmed() {
        assert_eq!(pick(Some("  "), Some(""), Some(" e@x "), Some("login")), "e@x");
        assert_eq!(pick(Some(" Me "), None, None, None), "Me");
    }

    #[test]
    fn an_explicit_author_is_used_without_asking_git() {
        assert_eq!(resolve(Some("  Someone Else ")), "Someone Else");
    }
}
