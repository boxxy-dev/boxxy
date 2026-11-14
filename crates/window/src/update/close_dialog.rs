use boxxy_claw_protocol::{PendingTaskInfo, TaskType};
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    Cancel,
    Terminate,
    RunInBackground,
}

/// Presents the unified 3-button close protection dialog (`Cancel`, `Terminate` / `Terminate All`,
/// `Run in Background`) listing active OS child processes, scheduled AI tasks, and active AI sessions.
pub async fn prompt_close_active_work(
    window: &adw::ApplicationWindow,
    heading: &str,
    body: &str,
    terminate_label: &str,
    running_apps: Vec<(u32, String)>,
    pending_tasks: Vec<PendingTaskInfo>,
    active_claw_panes: Vec<(String, String)>,
) -> CloseDecision {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .build();

    let group = adw::PreferencesGroup::new();
    for (pid, comm) in running_apps {
        let row = adw::ActionRow::builder()
            .title(&comm)
            .subtitle(format!("Process {pid}"))
            .use_markup(false)
            .focusable(false)
            .build();
        group.add(&row);
    }

    let mut panes_with_tasks = HashSet::new();
    for task in pending_tasks {
        panes_with_tasks.insert(task.pane_id.clone());
        let type_str = match task.task_type {
            TaskType::Notification => "Reminder",
            TaskType::Command => "Scheduled Command",
            TaskType::Query => "Scheduled Query",
        };
        let agent_name = if task.character_name.is_empty() {
            "AI Agent"
        } else {
            &task.character_name
        };
        let row = adw::ActionRow::builder()
            .title(&task.payload)
            .subtitle(format!("{type_str} by {agent_name}"))
            .use_markup(false)
            .focusable(false)
            .build();
        group.add(&row);
    }

    for (pane_id, agent_name) in active_claw_panes {
        if !panes_with_tasks.contains(&pane_id) {
            let row = adw::ActionRow::builder()
                .title(&agent_name)
                .subtitle("Active AI Agent Session")
                .use_markup(false)
                .focusable(false)
                .build();
            group.add(&row);
        }
    }

    let scrolled = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .max_content_height(250)
        .propagate_natural_height(true)
        .child(&group)
        .build();

    let extra_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
    extra_vbox.set_width_request(440);
    extra_vbox.append(&scrolled);

    let actions_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    actions_row.set_homogeneous(true);

    let cancel_btn = gtk4::Button::builder()
        .label("Cancel")
        .height_request(38)
        .build();
    let terminate_btn = gtk4::Button::builder()
        .label(terminate_label)
        .css_classes(["destructive-action"])
        .height_request(38)
        .build();
    let bg_btn = gtk4::Button::builder()
        .label("Run in Background")
        .css_classes(["suggested-action"])
        .height_request(38)
        .build();

    let chosen = Rc::new(Cell::new(CloseDecision::Cancel));
    {
        let c = chosen.clone();
        let d = dialog.downgrade();
        cancel_btn.connect_clicked(move |_| {
            c.set(CloseDecision::Cancel);
            if let Some(dlg) = d.upgrade() {
                dlg.close();
            }
        });
    }
    {
        let c = chosen.clone();
        let d = dialog.downgrade();
        terminate_btn.connect_clicked(move |_| {
            c.set(CloseDecision::Terminate);
            if let Some(dlg) = d.upgrade() {
                dlg.close();
            }
        });
    }
    {
        let c = chosen.clone();
        let d = dialog.downgrade();
        bg_btn.connect_clicked(move |_| {
            c.set(CloseDecision::RunInBackground);
            if let Some(dlg) = d.upgrade() {
                dlg.close();
            }
        });
    }

    actions_row.append(&cancel_btn);
    actions_row.append(&terminate_btn);
    actions_row.append(&bg_btn);
    extra_vbox.append(&actions_row);

    dialog.set_extra_child(Some(&extra_vbox));

    let _ = dialog.choose_future(Some(window)).await;
    chosen.get()
}
