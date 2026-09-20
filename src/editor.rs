//! `$EDITOR` as a command: a program and its arguments (`code --wait`,
//! `"C:\Program Files\Vim\vim.exe" -u NONE`).

/// The words of the command to run, without the file to open. A value that
/// is itself an existing file is the program (a path with spaces needs no
/// quotes); otherwise it is split on whitespace, with `"…"` and `'…'`
/// keeping words together. A backslash is an ordinary character, so Windows
/// paths work unquoted.
pub fn command_words(editor: &str) -> Vec<String> {
    if std::path::Path::new(editor).is_file() {
        return vec![editor.to_string()];
    }
    split(editor)
}

fn split(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        command_words(s)
    }

    #[test]
    fn a_bare_program_is_one_word() {
        assert_eq!(words("vim"), ["vim"]);
        assert_eq!(words("  vim  "), ["vim"]);
    }

    #[test]
    fn arguments_are_split_off() {
        assert_eq!(words("code --wait"), ["code", "--wait"]);
        assert_eq!(words("emacsclient -t  -a  ''"), ["emacsclient", "-t", "-a", ""]);
    }

    #[test]
    fn quotes_keep_a_word_together() {
        assert_eq!(
            words(r#""C:\Program Files\Vim\vim.exe" -u NONE"#),
            [r"C:\Program Files\Vim\vim.exe", "-u", "NONE"]
        );
        assert_eq!(words("'my editor' --flag"), ["my editor", "--flag"]);
        assert_eq!(words(r#"ed "a 'b' c""#), ["ed", "a 'b' c"]);
    }

    #[test]
    fn backslashes_are_kept_as_they_are() {
        assert_eq!(words(r"C:\tools\ed.cmd --wait"), [r"C:\tools\ed.cmd", "--wait"]);
    }

    #[test]
    fn an_unclosed_quote_takes_the_rest_as_one_word() {
        assert_eq!(words(r#"ed "a b"#), ["ed", "a b"]);
    }

    #[test]
    fn nothing_is_no_words() {
        assert!(words("").is_empty());
        assert!(words("   ").is_empty());
    }

    #[test]
    fn a_value_that_is_an_existing_file_is_taken_whole_even_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("my editor.exe");
        std::fs::write(&path, "").unwrap();
        let text = path.to_str().unwrap();
        assert_eq!(words(text), [text]);
        // With an argument after it, it is not a file: quote the program.
        assert_eq!(words(&format!("\"{text}\" --wait")), [text, "--wait"]);
    }
}
