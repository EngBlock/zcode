//! Switch Branch… and New Branch… (Integrate menu): local branches, then
//! remote ones (checking one out makes a local branch that tracks it), and
//! typing a name that isn't there offers to create it from `HEAD`.

use gpui_kit::assets::IconName;
use gpui_kit::*;

use super::Workspace;
use crate::fuzzy::fuzzy_match;
use crate::git::Branch;
use crate::picker::{Picker, PickerDelegate, PickerItem};

enum Choice {
    Switch(Branch),
    Create(String),
}

pub struct BranchDelegate {
    workspace: WeakEntity<Workspace>,
    branches: Vec<Branch>,
    /// New Branch…: only creating.
    create_only: bool,
    choices: Vec<Choice>,
    items: Vec<PickerItem>,
}

impl BranchDelegate {
    pub fn new(workspace: WeakEntity<Workspace>, branches: Vec<Branch>, create_only: bool) -> Self {
        Self { workspace, branches, create_only, choices: vec![], items: vec![] }
    }
}

impl PickerDelegate for BranchDelegate {
    fn placeholder(&self) -> SharedString {
        if self.create_only { "New branch name".into() } else { "Switch to branch, or type a new branch name".into() }
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn empty_message(&self) -> SharedString {
        if self.create_only { "Type the new branch’s name".into() } else { "No branches".into() }
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let query = query.trim().to_string();
        self.choices.clear();
        self.items.clear();
        let exists = self.branches.iter().any(|b| b.name == query);
        if !query.is_empty() && !exists {
            self.items.push(PickerItem {
                label: format!("New Branch “{query}”").into(),
                detail: Some("from the current commit".into()),
                icon: Some(IconName::GitBranchPlus),
                ..Default::default()
            });
            self.choices.push(Choice::Create(query.clone()));
        }
        if self.create_only {
            return;
        }
        let names: Vec<&str> = self.branches.iter().map(|b| b.name.as_str()).collect();
        let mut found = fuzzy_match(&query, &names, false, 200);
        // Local branches first, as listed.
        found.sort_by_key(|m| (self.branches[m.index].remote, m.index));
        for m in found {
            let branch = &self.branches[m.index];
            self.items.push(PickerItem {
                label: branch.name.clone().into(),
                detail: branch.upstream.clone().map(Into::into).or(branch.remote.then(|| "remote".into())),
                icon: Some(IconName::GitBranch),
                positions: m.positions,
                checked: branch.current,
                ..Default::default()
            });
            self.choices.push(Choice::Switch(branch.clone()));
        }
    }

    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let choice = match self.choices.get(index) {
            Some(Choice::Switch(branch)) if branch.current => return true,
            Some(Choice::Switch(branch)) => Choice::Switch(branch.clone()),
            Some(Choice::Create(name)) => Choice::Create(name.clone()),
            None => return false,
        };
        // After the picker closes, so the notification lands in the window.
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |ws, cx| match choice {
                    Choice::Switch(branch) => ws.switch_branch(branch, window, cx),
                    Choice::Create(name) => ws.create_branch(name, window, cx),
                })
                .ok();
        });
        true
    }
}

impl Workspace {
    /// Switch Branch… (`create_only`: New Branch…).
    pub fn show_branch_picker(&mut self, create_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.git.root.is_none() {
            self.notify_error("The project isn’t in a Git repository.".into(), window, cx);
            return;
        }
        let delegate = BranchDelegate::new(cx.weak_entity(), self.git.branches.clone(), create_only);
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }
}
