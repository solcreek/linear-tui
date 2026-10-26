//! Editing long text (comments, descriptions) in the user's editor.

use std::process::Command;

/// Edit `text` in `$VISUAL`, else `$EDITOR`, else vi; `None` when the
/// editor fails.
pub fn edit(text: &str) -> Option<String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    edit_with(&editor, text)
}

/// Edit `text` with `editor` (a command line; the file is its last
/// argument) in a private temporary file, removed afterwards.
pub fn edit_with(editor: &str, text: &str) -> Option<String> {
    let words = shlex::split(editor).filter(|w| !w.is_empty())?;
    let path = std::env::temp_dir().join(format!("linear-tui-{}.md", uuid::Uuid::new_v4()));
    let mut opts = std::fs::File::options();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()))
        .ok()?;
    let ok = Command::new(&words[0])
        .args(&words[1..])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    let out = if ok {
        std::fs::read_to_string(&path).ok()
    } else {
        None
    };
    let _ = std::fs::remove_file(&path);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_editor_that_changes_nothing_gives_the_text_back() {
        assert_eq!(edit_with("true", "hello\n").as_deref(), Some("hello\n"));
    }

    #[test]
    fn an_editor_that_writes() {
        // The file is the last argument: sh -c SCRIPT FILE sees it as $0.
        let out = edit_with("sh -c 'printf edited > \"$0\"'", "old");
        assert_eq!(out.as_deref(), Some("edited"));
    }

    #[test]
    fn a_failing_or_missing_editor_is_none() {
        assert_eq!(edit_with("false", "x"), None);
        assert_eq!(edit_with("/nonexistent/editor", "x"), None);
        assert_eq!(edit_with("", "x"), None);
        assert_eq!(edit_with("'unbalanced", "x"), None);
    }

    #[test]
    fn the_file_is_private_and_removed() {
        // The "editor" replaces the text with the file's permission bits, as
        // `ls -l` prints them on macOS and Linux alike.
        let script = "m=$(ls -l \"$0\" | cut -c1-10); printf %s \"$m\" > \"$0\"";
        let editor = format!("sh -c {}", shlex::try_quote(script).unwrap());
        assert_eq!(edit_with(&editor, "x").as_deref(), Some("-rw-------"));
        let seen = edit_with("sh -c 'printf %s \"$0\" > \"$0\"'", "").unwrap();
        assert!(seen.contains("linear-tui-"), "{seen}");
        assert!(!std::path::Path::new(&seen).exists(), "removed after");
    }

    #[test]
    fn edit_reads_the_environment() {
        // VISUAL wins over EDITOR; both unset would start vi, so set one.
        // SAFETY: tests in this module do not read these variables elsewhere.
        unsafe {
            std::env::set_var("VISUAL", "true");
        }
        assert_eq!(edit("kept").as_deref(), Some("kept"));
        unsafe {
            std::env::remove_var("VISUAL");
            std::env::set_var("EDITOR", "false");
        }
        assert_eq!(edit("x"), None);
        unsafe {
            std::env::remove_var("EDITOR");
        }
    }
}
