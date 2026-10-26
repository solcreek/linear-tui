//! `~/.config/linear-tui/config.toml`: the views, as `issue list` flags.
//!
//! ```toml
//! [[view]]
//! name = "Triage"
//! args = ["--team", "ENG", "--state-type", "triage"]
//! ```
//!
//! Views in the file replace the built-in ones. The filter panel appends
//! views to the file, leaving the rest of it (comments included) alone.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::app::View;

#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    view: Vec<ViewEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewEntry {
    name: String,
    #[serde(default)]
    args: Vec<String>,
}

/// `$XDG_CONFIG_HOME/linear-tui/config.toml`, else `~/.config/...`.
pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("linear-tui").join("config.toml"))
}

/// The views a config file defines; none when it does not exist.
pub fn load(path: &Path) -> Result<Vec<View>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let file: File = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    file.view
        .into_iter()
        .map(|v| {
            if v.name.trim().is_empty() {
                return Err(format!("{}: a view needs a name", path.display()));
            }
            Ok(View { name: v.name, args: v.args })
        })
        .collect()
}

/// Add a view at the end of the file, creating it (and its directory).
pub fn append_view(path: &Path, view: &View) -> Result<(), String> {
    let quote = |s: &str| toml::Value::String(s.to_string()).to_string();
    let args: Vec<String> = view.args.iter().map(|a| quote(a)).collect();
    let block = format!("\n[[view]]\nname = {}\nargs = [{}]\n", quote(&view.name), args.join(", "));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut f = std::fs::File::options()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    std::io::Write::write_all(&mut f, block.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("linear-tui-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("linear-tui").join("config.toml")
    }

    #[test]
    fn no_file_is_no_views() {
        assert_eq!(load(Path::new("/nonexistent/config.toml")).unwrap(), []);
    }

    #[test]
    fn reads_views() {
        let p = tmp("read");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            "# mine\n[[view]]\nname = \"Bugs\"\nargs = [\"--label\", \"Bug\"]\n\n[[view]]\nname = \"Everything\"\n",
        )
        .unwrap();
        let v = load(&p).unwrap();
        assert_eq!(v, [View::new("Bugs", &["--label", "Bug"]), View::new("Everything", &[])]);
        std::fs::remove_dir_all(p.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn bad_files_say_where() {
        let p = tmp("bad");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        for (text, needle) in [
            ("[[view]]\nargs = []\n", "missing field `name`"),
            ("[[view]]\nname = \" \"\n", "a view needs a name"),
            ("[[view]]\nname = \"x\"\nsort = \"y\"\n", "unknown field `sort`"),
            ("not toml at all", "config.toml"),
        ] {
            std::fs::write(&p, text).unwrap();
            let e = load(&p).unwrap_err();
            assert!(e.contains("config.toml") && e.contains(needle), "{text}: {e}");
        }
        std::fs::remove_dir_all(p.parent().unwrap().parent().unwrap()).unwrap();
        let unreadable = std::env::temp_dir();
        assert!(load(&unreadable).is_err(), "a directory is not a file");
    }

    #[test]
    fn appends_keep_what_was_there() {
        let p = tmp("append");
        append_view(&p, &View::new("Mine \"now\"", &["--assignee", "me"])).unwrap();
        std::fs::write(&p, format!("# keep me\n{}", std::fs::read_to_string(&p).unwrap())).unwrap();
        append_view(&p, &View::new("Second", &[])).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("# keep me\n"));
        assert_eq!(
            load(&p).unwrap(),
            [View::new("Mine \"now\"", &["--assignee", "me"]), View::new("Second", &[])]
        );
        std::fs::remove_dir_all(p.parent().unwrap().parent().unwrap()).unwrap();
        assert!(append_view(Path::new("/nonexistent-root-dir/x/config.toml"), &View::new("x", &[])).is_err());
    }

    #[test]
    fn the_path_follows_xdg() {
        // SAFETY: no other test in this module reads these variables.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", "/x/cfg") };
        assert_eq!(path().unwrap(), PathBuf::from("/x/cfg/linear-tui/config.toml"));
        unsafe { std::env::set_var("XDG_CONFIG_HOME", "") };
        assert!(path().unwrap().ends_with(".config/linear-tui/config.toml"));
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    }
}
