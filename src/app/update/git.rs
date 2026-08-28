//! Git panel keyboard handling: zone focus, buttons, and the commit input.

use super::*;

use crate::app::model::GitZone;

/// Validates the commit message and returns a commit Cmd (optimistically clears the message).
pub(super) fn git_commit(model: &mut Model) -> Vec<Cmd> {
    let g = &mut model.sidebar.git;
    let msg = g.commit.content().trim().to_string();
    if msg.is_empty() {
        model.notify("Commit message is empty".to_string());
        return Vec::new();
    }
    if g.staged.is_empty() {
        model.notify("No staged changes".to_string());
        return Vec::new();
    }
    g.commit.clear();
    model.focus = Focus::Sidebar;
    model.sidebar.git.zone = GitZone::Files;
    vec![Cmd::GitCommit(msg)]
}

/// Whether the keyboard is currently inside the Git panel (either on the commit
/// box or on one of its lists/buttons). Guards the panel's own shortcuts so they
/// stay inert in the Files/Search/Themes/Settings panels.
pub(super) fn in_git_panel(model: &Model) -> bool {
    model.sidebar.active == Panel::Git
        && model.layout.sidebar_open
        && matches!(model.focus, Focus::Sidebar | Focus::GitCommit)
}

/// Moves the keyboard `delta` zones through the panel (Tab / Shift+Tab).
///
/// The commit box is the one zone that needs the app-level focus too, so typing
/// is routed into its text input; every other zone stays on `Focus::Sidebar`.
pub(super) fn git_cycle_zone(model: &mut Model, delta: isize) -> Vec<Cmd> {
    // Without a repository the panel is a single "No git repository" line: there
    // are no buttons or commit box to tab to.
    if !in_git_panel(model) || !model.sidebar.git.is_repo {
        return Vec::new();
    }
    let zone = model.sidebar.git.zone.step(delta);
    set_git_zone(model, zone);
    Vec::new()
}

/// Focuses `zone`, keeping the app-level focus in sync with it.
pub(super) fn set_git_zone(model: &mut Model, zone: GitZone) {
    model.sidebar.git.zone = zone;
    if zone == GitZone::Message {
        model.focus = Focus::GitCommit;
        model.sidebar.git.commit.cursor_to_end();
    } else {
        model.focus = Focus::Sidebar;
    }
}

/// Enter in the Git panel: presses the focused button, or opens the selected
/// change as a diff tab when the change list has the focus.
pub(super) fn git_zone_activate(model: &mut Model) -> Vec<Cmd> {
    match model.sidebar.git.zone {
        GitZone::Files => activate_selection(model),
        // Enter inside the commit box is a newline (handled by the input widget),
        // so this is only reachable defensively.
        GitZone::Message => Vec::new(),
        zone => git_button(model, zone),
    }
}

/// Runs a Git panel button, honouring the same enabled/disabled rules the
/// rendering uses — a disabled button does nothing when "pressed".
pub(super) fn git_button(model: &mut Model, zone: GitZone) -> Vec<Cmd> {
    let g = &model.sidebar.git;
    match zone {
        GitZone::Fetch => {
            if !g.has_remote {
                return Vec::new();
            }
            model.show_toast("Fetching…");
            vec![Cmd::GitFetch]
        }
        GitZone::Pull => {
            if !g.has_upstream {
                return Vec::new();
            }
            model.show_toast("Pulling…");
            vec![Cmd::GitPull]
        }
        GitZone::Push => {
            if !g.can_push() {
                return Vec::new();
            }
            model.show_toast("Pushing…");
            vec![Cmd::GitPush]
        }
        GitZone::Uncommit => {
            if !g.can_undo_commit() {
                return Vec::new();
            }
            model.notify("Undoing last commit…".to_string());
            vec![Cmd::GitUndoLastCommit]
        }
        GitZone::Commit => git_commit(model),
        GitZone::Message | GitZone::Files => Vec::new(),
    }
}

/// `a` on the selected change: stage it, or unstage it when it is already staged.
pub(super) fn git_toggle_stage(model: &mut Model) -> Vec<Cmd> {
    if !in_git_panel(model) {
        return Vec::new();
    }
    let g = &model.sidebar.git;
    let Some((entry, staged)) = g.entry_at(g.selected) else {
        return Vec::new();
    };
    let rel = entry.rel.clone();
    if staged {
        vec![Cmd::GitUnstage(rel)]
    } else {
        vec![Cmd::GitStage(rel)]
    }
}

/// `r` on the selected change: confirm, then restore the file to its committed
/// state (the same destructive action as the row's ↺ button).
pub(super) fn git_revert_entry(model: &mut Model) -> Vec<Cmd> {
    if !in_git_panel(model) {
        return Vec::new();
    }
    let g = &model.sidebar.git;
    let Some((entry, staged)) = g.entry_at(g.selected) else {
        return Vec::new();
    };
    // Reverting restores the working tree to the *index*, so a staged change has
    // to be unstaged first or the revert would be a no-op.
    if staged {
        model.notify("Unstage it first (a), then revert".to_string());
        return Vec::new();
    }
    let rel = entry.rel.clone();
    model.dialog = Some(Dialog::ask(
        "Revert changes".to_string(),
        format!("Changes in '{rel}' will be reverted. This cannot be undone. Are you sure?"),
        DialogAction::GitRevert(rel),
    ));
    Vec::new()
}
