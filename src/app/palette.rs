//! `Ctrl-k`: every action by name, as in Linear. A command is the key that
//! does it, so the palette and the keys can never disagree; choosing one
//! sends that key.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::write::{Overlay, PickItem, PickList, Target};
use super::{App, Effect, Focus};

/// Where a command makes sense.
#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Anywhere,
    List,
    Detail,
    /// The selected issue, or the open one.
    Issue,
}

/// (name, key as shown, the key itself, where).
const COMMANDS: &[(&str, &str, &str, Scope)] = &[
    ("Create issue", "c", "c", Scope::Anywhere),
    ("Edit issue", "e", "e", Scope::Issue),
    ("Rename issue", "r", "r", Scope::Issue),
    ("Change status", "s", "s", Scope::Issue),
    ("Change priority", "p", "p", Scope::Issue),
    ("Assign to…", "a", "a", Scope::Issue),
    ("Assign to me", "i", "i", Scope::Issue),
    ("Change labels", "L", "L", Scope::Issue),
    ("Move to project", "P", "P", Scope::Issue),
    ("Move to cycle", "C", "C", Scope::Issue),
    ("Set estimate", "E", "E", Scope::Issue),
    ("Comment", "m", "m", Scope::Issue),
    ("Reply in a thread", "R", "R", Scope::Detail),
    ("Edit description in $EDITOR", "ctrl-e", "^e", Scope::Detail),
    ("Archive issue", "x", "x", Scope::Issue),
    ("Copy issue ID", "y", "y", Scope::Issue),
    ("Copy issue URL", "Y", "Y", Scope::Issue),
    ("Copy git branch name", "b", "b", Scope::Issue),
    ("Open in browser", "o", "o", Scope::Issue),
    ("Next issue", "J", "J", Scope::Detail),
    ("Previous issue", "K", "K", Scope::Detail),
    ("Images on / off", "I", "I", Scope::Detail),
    ("Filter or search", "/", "/", Scope::List),
    ("Filters", "f", "f", Scope::List),
    ("Clear filters", "F", "F", Scope::List),
    ("Linear's saved views", "V", "V", Scope::List),
    ("Change order", "D", "D", Scope::List),
    ("Refresh", "ctrl-r", "^r", Scope::Anywhere),
    ("Keyboard shortcuts", "?", "?", Scope::Anywhere),
    ("Quit", "ctrl-c", "^c", Scope::Anywhere),
];

/// The key a command's value stands for: `x`, or `^x` for Ctrl-x.
fn key_of(value: &str) -> Option<KeyEvent> {
    let mut chars = value.chars();
    match (chars.next()?, chars.next(), chars.next()) {
        ('^', Some(c), None) => Some(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)),
        (c, None, None) => Some(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
        _ => None,
    }
}

impl App {
    /// The commands that apply here and now.
    fn palette_items(&self) -> Vec<PickItem> {
        let issue = match (self.focus, &self.detail) {
            (Focus::Detail, Some(d)) => d.issue.is_some(),
            _ => self.selected_row().is_some(),
        };
        let item = |label: String, hint: &str, value: String| PickItem {
            label,
            value,
            color: None,
            hint: hint.into(),
            chosen: false,
        };
        let mut items = Vec::new();
        if let Some(u) = &self.undo {
            items.push(item(format!("Undo {}", u.label), "u", "u".into()));
        }
        items.extend(
            COMMANDS
                .iter()
                .filter(|(_, _, value, scope)| match scope {
                    Scope::Anywhere => true,
                    Scope::List => self.focus == Focus::List,
                    Scope::Detail => self.focus == Focus::Detail && issue,
                    Scope::Issue => issue && (!matches!(*value, "C" | "E") || self.at_least(0, 6)),
                })
                .filter(|(_, _, value, _)| *value != "R" || self.can_reply)
                .map(|(label, hint, value, _)| item((*label).into(), hint, (*value).into())),
        );
        for (i, v) in self.views.iter().enumerate().filter(|(i, _)| *i != self.view) {
            let hint = if i < 9 { format!("{}", i + 1) } else { String::new() };
            items.push(item(format!("Go to {}", v.name), &hint, format!("view:{i}")));
        }
        items
    }

    pub(super) fn open_palette(&mut self) -> Vec<Effect> {
        self.overlay = Some(Overlay::Pick(Box::new(PickList {
            title: "Command".into(),
            items: self.palette_items(),
            multi: false,
            cursor: 0,
            query: String::new(),
            target: Target::Command,
            waiting: None,
        })));
        Vec::new()
    }

    /// Run the chosen command as its key, where the palette was opened.
    pub(super) fn run_command(&mut self, item: PickItem) -> Vec<Effect> {
        if let Some(i) = item.value.strip_prefix("view:").and_then(|i| i.parse().ok()) {
            self.focus = Focus::List;
            return self.switch_view(i);
        }
        match key_of(&item.value) {
            Some(k) => self.key(k),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{ch, ctrl, drive, key, started};

    fn palette(app: &App) -> &PickList {
        match &app.overlay {
            Some(Overlay::Pick(p)) if p.target == Target::Command => p,
            other => panic!("no palette: {other:?}"),
        }
    }

    fn labels(app: &App) -> Vec<String> {
        palette(app).visible().into_iter().map(|(_, i)| i.label.clone()).collect()
    }

    fn type_in(app: &mut App, s: &str) {
        for c in s.chars() {
            drive(app, ch(c));
        }
    }

    #[test]
    fn ctrl_k_lists_what_applies_here() {
        let mut app = started();
        drive(&mut app, ctrl('k'));
        let all = labels(&app);
        assert!(all.contains(&"Change status".to_string()));
        assert!(all.contains(&"Go to Active".to_string()));
        assert!(!all.contains(&"Go to My issues".to_string()), "the view on screen");
        assert!(!all.contains(&"Next issue".to_string()), "detail only");
        assert!(!all.iter().any(|l| l.starts_with("Undo")), "nothing to undo");
        drive(&mut app, key(KeyCode::Esc));
        assert!(app.overlay.is_none());
        drive(&mut app, key(KeyCode::Enter));
        drive(&mut app, ctrl('k'));
        let all = labels(&app);
        assert!(all.contains(&"Next issue".to_string()));
        assert!(!all.contains(&"Filters".to_string()), "list only");
    }

    #[test]
    fn typing_narrows_by_words_and_keys() {
        let mut app = started();
        drive(&mut app, ctrl('k'));
        type_in(&mut app, "ch stat");
        assert_eq!(labels(&app), ["Change status"]);
        for _ in 0..7 {
            drive(&mut app, key(KeyCode::Backspace));
        }
        type_in(&mut app, "ctrl-r");
        assert_eq!(labels(&app), ["Refresh"]);
    }

    #[test]
    fn choosing_sends_the_key() {
        let mut app = started();
        drive(&mut app, ctrl('k'));
        type_in(&mut app, "copy issue id");
        assert_eq!(drive(&mut app, key(KeyCode::Enter)), [Effect::Copy("ENG-142".into())]);
        drive(&mut app, ctrl('k'));
        type_in(&mut app, "status");
        drive(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.overlay, Some(Overlay::Pick(p)) if p.target != Target::Command));
        drive(&mut app, key(KeyCode::Esc));
        drive(&mut app, ctrl('k'));
        type_in(&mut app, "go to active");
        drive(&mut app, key(KeyCode::Enter));
        assert_eq!(app.views[app.view].name, "Active");
        drive(&mut app, ctrl('k'));
        type_in(&mut app, "shortcuts");
        drive(&mut app, key(KeyCode::Enter));
        assert!(app.help);
        drive(&mut app, key(KeyCode::Esc));
        drive(&mut app, ctrl('k'));
        type_in(&mut app, "nothing like this");
        assert!(drive(&mut app, key(KeyCode::Enter)).is_empty());
        assert!(app.overlay.is_none());
    }

    #[test]
    fn old_linearctl_hides_cycle_and_estimate() {
        let mut app = started();
        app.describe = Some(
            crate::contract::Describe::from_json(include_str!(
                "../../tests/fixtures/linearctl-describe-0.1.0.json"
            ))
            .unwrap(),
        );
        app.can_reply = false;
        drive(&mut app, ctrl('k'));
        let all = labels(&app);
        assert!(!all.contains(&"Move to cycle".to_string()));
        assert!(!all.contains(&"Set estimate".to_string()));
    }

    #[test]
    fn keys_of_values() {
        assert_eq!(key_of("^e"), Some(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)));
        assert_eq!(key_of("x"), Some(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)));
        assert_eq!(key_of("xy"), None);
        assert_eq!(key_of(""), None);
    }
}
