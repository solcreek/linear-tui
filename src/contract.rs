//! The linearctl contract: what `linearctl describe --json` says exists.
//!
//! The TUI builds argv for linearctl; this module answers whether a given
//! command line names a real command and only flags that command accepts.
//! It also checks every `linearctl ...` invocation written in a Markdown
//! doc, so the design cannot drift from the CLI it depends on.

use std::collections::{HashMap, HashSet};
use std::process::Command;

use serde_json::Value;

/// One command path's accepted flags, and whether it has subcommands.
#[derive(Debug, Clone)]
struct Entry {
    flags: HashSet<String>,
    has_children: bool,
}

/// The command tree from `linearctl describe --json`, flattened by path.
#[derive(Debug, Clone)]
pub struct Describe {
    /// The linearctl version that produced it.
    pub version: String,
    global_flags: HashSet<String>,
    table: HashMap<Vec<String>, Entry>,
}

impl Describe {
    /// Parse the JSON `linearctl describe --json` prints.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| format!("describe: {e}"))?;
        let version = root["version"].as_str().unwrap_or_default().to_string();
        let global_flags = flag_names(&root["flags"])?;
        let mut table = HashMap::new();
        walk(&root["commands"], &[], &global_flags, &mut table)?;
        Ok(Self {
            version,
            global_flags,
            table,
        })
    }

    /// Run `linearctl describe --json` with the given binary.
    pub fn from_linearctl(program: &str) -> Result<Self, String> {
        Self::from_command(Command::new(program).args(["describe", "--json"]))
    }

    /// Run a prepared command and parse what it prints.
    pub fn from_command(cmd: &mut Command) -> Result<Self, String> {
        let out = cmd
            .output()
            .map_err(|e| format!("cannot run {:?}: {e}", cmd.get_program()))?;
        if !out.status.success() {
            return Err(format!("{:?} failed: {}", cmd.get_program(), out.status));
        }
        Self::from_json(&String::from_utf8_lossy(&out.stdout))
    }

    /// Problems with one command line; empty means it is valid.
    ///
    /// Positional words are not checked. Synopsis brackets (`[--team KEY]`)
    /// are ignored, and `linearctl …` in prose names no command.
    pub fn check_invocation(&self, text: &str) -> Vec<String> {
        let Some(argv) = shlex::split(text) else {
            return vec!["cannot parse: unbalanced quotes".into()];
        };
        self.check_argv(&argv[1.min(argv.len())..]) // drop "linearctl"
    }

    /// Problems with an argv as the TUI passes it (no program name).
    pub fn check_argv(&self, argv: &[String]) -> Vec<String> {
        let mut path: Vec<String> = Vec::new();
        let mut i = 0;
        while i < argv.len() {
            path.push(argv[i].clone());
            if self.table.contains_key(&path) {
                i += 1;
            } else {
                path.pop();
                break;
            }
        }
        let next = argv.get(i).map(String::as_str).unwrap_or("");
        if next == "…" || next == "..." {
            return Vec::new();
        }
        let positional = !next.is_empty() && !next.starts_with('-');
        let flags = match self.table.get(&path) {
            None if positional => return vec![format!("unknown command: {next}")],
            None => &self.global_flags,
            Some(e) if e.has_children && positional => {
                return vec![format!("unknown command: {} {next}", path.join(" "))];
            }
            Some(e) => &e.flags,
        };
        let mut problems = Vec::new();
        for tok in &argv[i..] {
            let tok = tok.trim_matches(|c| matches!(c, '[' | ']' | '(' | ')'));
            let Some(rest) = tok.strip_prefix("--") else {
                continue;
            };
            let name = rest.split('=').next().unwrap_or_default();
            if !name.is_empty() && !flags.contains(name) {
                let cmd = std::iter::once("linearctl").chain(path.iter().map(String::as_str));
                problems.push(format!(
                    "unknown flag --{name} on '{}'",
                    cmd.collect::<Vec<_>>().join(" ")
                ));
            }
        }
        problems
    }

    /// Check every invocation in a Markdown doc.
    /// Returns how many were found and one message per problem.
    pub fn check_markdown(&self, markdown: &str) -> (usize, Vec<String>) {
        let found = invocations(markdown);
        let problems = found
            .iter()
            .flat_map(|(line, text)| {
                self.check_invocation(text)
                    .into_iter()
                    .map(move |p| format!("line {line}: {p}: {text}"))
            })
            .collect();
        (found.len(), problems)
    }
}

fn flag_names(flags: &Value) -> Result<HashSet<String>, String> {
    let list = flags.as_array().ok_or("describe: flags is not a list")?;
    list.iter()
        .map(|f| {
            f["name"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "describe: flag without a name".to_string())
        })
        .collect()
}

fn walk(
    commands: &Value,
    path: &[String],
    inherited: &HashSet<String>,
    table: &mut HashMap<Vec<String>, Entry>,
) -> Result<(), String> {
    let list = commands.as_array().ok_or("describe: commands is not a list")?;
    for c in list {
        let name = c["name"].as_str().ok_or("describe: command without a name")?;
        let flags: HashSet<String> = inherited.union(&flag_names(&c["flags"])?).cloned().collect();
        let children = &c["commands"];
        let has_children = children.as_array().is_some_and(|a| !a.is_empty());
        let aliases = c["aliases"].as_array().map(Vec::as_slice).unwrap_or_default();
        let names = std::iter::once(name).chain(aliases.iter().filter_map(Value::as_str));
        for n in names {
            let mut p = path.to_vec();
            p.push(n.to_string());
            table.insert(
                p.clone(),
                Entry {
                    flags: flags.clone(),
                    has_children,
                },
            );
            if has_children {
                walk(children, &p, &flags, table)?;
            }
        }
    }
    Ok(())
}

/// Every `linearctl ...` call in a Markdown doc, with its line number.
///
/// Calls are taken from inline code spans and fenced code blocks; a span
/// holding a pipeline or a sequence is split at `|`, `;`, `&&` and `→`.
/// Calls under a heading containing "(proposed)" are skipped, down to the
/// next heading of the same or a higher level: they do not exist yet.
pub fn invocations(markdown: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut exempt_level: Option<usize> = None;
    for (n, line) in markdown.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && let Some((level, title)) = heading(line) {
            if exempt_level.is_some_and(|e| level <= e) {
                exempt_level = None;
            }
            if exempt_level.is_none() && title.contains("(proposed)") {
                exempt_level = Some(level);
            }
            continue;
        }
        if exempt_level.is_some() {
            continue;
        }
        let spans: Vec<&str> = if in_fence { vec![line] } else { code_spans(line) };
        for span in spans {
            for part in span.split(['|', ';', '→']).flat_map(|p| p.split("&&")) {
                let part = part.trim();
                if part == "linearctl" || part.starts_with("linearctl ") {
                    out.push((n + 1, part.to_string()));
                }
            }
        }
    }
    out
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|&b| b == b'#').count();
    let rest = &line[level..];
    ((1..=6).contains(&level) && rest.starts_with(char::is_whitespace)).then(|| (level, rest.trim()))
}

fn code_spans(line: &str) -> Vec<&str> {
    line.split('`').skip(1).step_by(2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A two-level tree shaped like `linearctl describe`, small enough to read.
    const DESCRIBE: &str = r#"{
      "version": "9.9.9",
      "flags": [{"name": "json"}, {"name": "timeout"}],
      "commands": [
        {"name": "issue", "aliases": [], "flags": [], "commands": [
          {"name": "show", "aliases": ["view"], "commands": [],
           "flags": [{"name": "comments"}, {"name": "signed-urls"}]},
          {"name": "list", "aliases": [], "commands": [],
           "flags": [{"name": "limit"}, {"name": "fields"}]}
        ]},
        {"name": "describe", "aliases": [], "flags": [], "commands": []}
      ]
    }"#;

    fn describe() -> Describe {
        Describe::from_json(DESCRIBE).unwrap()
    }

    fn problems(doc: &str) -> (usize, Vec<String>) {
        describe().check_markdown(doc)
    }

    fn texts(doc: &str) -> Vec<String> {
        invocations(doc).into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn inline_spans_and_fences() {
        let doc = "Run `linearctl issue list --limit 5` now.\n```\nlinearctl describe\n```\n";
        assert_eq!(
            texts(doc),
            ["linearctl issue list --limit 5", "linearctl describe"]
        );
    }

    #[test]
    fn ignores_other_code() {
        assert!(invocations("`cargo test` and `ls`\n").is_empty());
    }

    #[test]
    fn splits_pipelines_and_sequences() {
        let doc = "`echo x | linearctl describe --json` `linearctl describe && linearctl issue list`\n";
        assert_eq!(
            texts(doc),
            [
                "linearctl describe --json",
                "linearctl describe",
                "linearctl issue list"
            ]
        );
    }

    #[test]
    fn proposed_section_is_exempt_until_a_sibling_heading() {
        let doc = "## Gaps (proposed)\n`linearctl watch --json`\n\
                   ### detail\n`linearctl serve --stdio`\n\
                   ## Next\n`linearctl describe`\n";
        assert_eq!(texts(doc), ["linearctl describe"]);
    }

    #[test]
    fn heading_inside_a_fence_is_not_a_heading() {
        let doc = "## Gaps (proposed)\n```\n# not a heading\nlinearctl watch\n```\n";
        assert!(invocations(doc).is_empty());
    }

    #[test]
    fn hashes_without_a_space_are_not_a_heading() {
        let doc = "## Gaps (proposed)\n#hashtag\n`linearctl watch`\n";
        assert!(invocations(doc).is_empty());
    }

    #[test]
    fn valid() {
        assert_eq!(
            problems("`linearctl issue show ENG-1 --comments=0 --json`"),
            (1, vec![])
        );
    }

    #[test]
    fn alias_resolves() {
        assert!(
            problems("`linearctl issue view ENG-1 --signed-urls 300`")
                .1
                .is_empty()
        );
    }

    #[test]
    fn global_flag_on_a_leaf() {
        assert!(problems("`linearctl issue list --timeout 5`").1.is_empty());
    }

    #[test]
    fn unknown_flag() {
        let (found, p) = problems("`linearctl issue list --sort priority`");
        assert_eq!(found, 1);
        assert!(
            p[0].contains("unknown flag --sort on 'linearctl issue list'"),
            "{p:?}"
        );
    }

    #[test]
    fn flag_of_a_sibling_is_rejected() {
        assert!(problems("`linearctl issue list --comments 3`").1[0].contains("unknown flag --comments"));
    }

    #[test]
    fn unknown_subcommand() {
        assert!(problems("`linearctl issue frob ENG-1`").1[0].contains("unknown command: issue frob"));
    }

    #[test]
    fn unknown_top_level_command() {
        assert!(problems("`linearctl watch --json`").1[0].contains("unknown command: watch"));
    }

    #[test]
    fn group_followed_by_a_flag() {
        assert!(problems("`linearctl issue --json`").1.is_empty());
    }

    #[test]
    fn bare_global_flags() {
        assert!(problems("`linearctl --json`").1.is_empty());
        assert!(problems("`linearctl`").1.is_empty());
        assert!(problems("`linearctl --nope`").1[0].contains("--nope on 'linearctl'"));
    }

    #[test]
    fn synopsis_brackets() {
        assert!(
            problems("`linearctl issue list [--limit N] [--fields F]`")
                .1
                .is_empty()
        );
        assert!(problems("`linearctl issue list [--bogus]`").1[0].contains("--bogus"));
    }

    #[test]
    fn a_bare_double_dash_is_not_a_flag() {
        assert!(problems("`linearctl issue list --`").1.is_empty());
    }

    #[test]
    fn ellipsis_is_a_placeholder() {
        assert!(
            problems("`linearctl … --json` and `linearctl issue ...`")
                .1
                .is_empty()
        );
    }

    #[test]
    fn unparseable() {
        assert!(problems("`linearctl issue list \"open`").1[0].contains("cannot parse"));
    }

    #[test]
    fn reports_line_numbers() {
        assert!(problems("\n\n`linearctl issue list --nope`").1[0].starts_with("line 3:"));
    }

    #[test]
    fn version_is_kept() {
        assert_eq!(describe().version, "9.9.9");
    }

    #[test]
    fn malformed_describe() {
        let err = |s| Describe::from_json(s).unwrap_err();
        assert!(err("not json").starts_with("describe:"));
        assert!(err(r#"{"flags": 1, "commands": []}"#).contains("flags is not a list"));
        assert!(err(r#"{"flags": [{}], "commands": []}"#).contains("flag without a name"));
        assert!(err(r#"{"flags": [], "commands": 1}"#).contains("commands is not a list"));
        assert!(err(r#"{"flags": [], "commands": [{}]}"#).contains("command without a name"));
    }

    #[test]
    fn from_command_reads_stdout() {
        let d = Describe::from_command(Command::new("printf").arg("%s").arg(DESCRIBE)).unwrap();
        assert_eq!(d.version, "9.9.9");
    }

    #[test]
    fn from_command_failures() {
        assert!(
            Describe::from_command(&mut Command::new("false"))
                .unwrap_err()
                .contains("failed")
        );
        let missing = Describe::from_linearctl("/nonexistent/linearctl").unwrap_err();
        assert!(missing.starts_with("cannot run"), "{missing}");
    }
}
