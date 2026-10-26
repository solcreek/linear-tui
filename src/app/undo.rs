//! Undo: `u` reverts the last write, one level deep. Each write that can be
//! reverted records the linearctl call that reverts it; the call goes
//! through the same dry run and `--yes` as any quick edit.

use super::write::{Purpose, Text};
use super::{App, Effect, Focus, Request};
use crate::ctl::{self, Mode};
use crate::model::WriteResult;

/// The call that reverts a write.
#[derive(Debug, Clone, PartialEq)]
pub enum UndoStep {
    /// `issue update` back to the old values (and old description).
    Update {
        issue: String,
        flags: Vec<(String, String)>,
        text: Option<Text>,
    },
    Unarchive {
        issue: String,
    },
    /// For an issue just created: Linear has no delete; archive it.
    Archive {
        issue: String,
    },
    DeleteComment {
        issue: String,
        id: String,
    },
}

/// What `u` would undo, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct Undo {
    /// For the notice: "ENG-142: status → Done".
    pub label: String,
    pub step: UndoStep,
}

impl UndoStep {
    fn argv(&self, mode: Mode) -> Vec<String> {
        match self {
            UndoStep::Update { issue, flags, .. } => {
                let f: Vec<(&str, &str)> = flags.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                ctl::update_argv(issue, &f, mode)
            }
            UndoStep::Unarchive { issue } => ctl::unarchive_argv(issue, mode),
            UndoStep::Archive { issue } => ctl::archive_argv(issue, mode),
            UndoStep::DeleteComment { id, .. } => ctl::delete_comment_argv(id, mode),
        }
    }
}

impl App {
    /// Keep `step` as what `u` undoes, if this linearctl can run it.
    pub(super) fn remember_undo(&mut self, label: String, step: UndoStep) {
        let runnable =
            self.describe.as_ref().is_none_or(|d| d.check_argv(&step.argv(Mode::DryRun)).is_empty());
        self.undo = runnable.then_some(Undo { label, step });
    }

    pub(super) fn start_undo(&mut self) -> Vec<Effect> {
        let Some(u) = self.undo.clone() else {
            self.notice = Some("Nothing to undo.".into());
            return Vec::new();
        };
        let argv = u.step.argv(Mode::DryRun);
        let text = match &u.step {
            UndoStep::Update { text, .. } => text.clone(),
            _ => None,
        };
        let purpose = Purpose::Undo { label: u.label, step: u.step };
        vec![Effect::Run(Request::Write { purpose, argv, text, summary: Vec::new() })]
    }

    pub(super) fn on_undone(&mut self, label: String, step: UndoStep, r: WriteResult) -> Vec<Effect> {
        self.undo = None;
        self.notice = Some(format!("Undone: {label}"));
        match step {
            UndoStep::Update { issue, .. } => {
                self.apply_row(r.issue);
                self.refresh_detail(&issue)
            }
            UndoStep::Unarchive { issue } => {
                let mut fx = self.refresh_detail(&issue);
                fx.extend(self.load_view());
                fx
            }
            UndoStep::Archive { issue } => {
                self.rows.retain(|row| row.identifier != issue);
                self.reselect(None);
                self.cache.remove(&issue);
                if self.detail.as_ref().is_some_and(|d| d.id == issue) {
                    self.detail = None;
                    self.focus = Focus::List;
                }
                Vec::new()
            }
            UndoStep::DeleteComment { issue, id } => {
                if let Some(i) = self.detail.as_mut().filter(|d| d.id == issue).and_then(|d| d.issue.as_mut())
                {
                    i.comments.retain(|c| c.id != id);
                }
                self.cache.remove(&issue);
                Vec::new()
            }
        }
    }
}
