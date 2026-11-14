use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::state::{AppInput, AppWindowInner, ToastRequest};
use crate::updater::whats_new::{ChangelogSummary, WhatsNewPending};

pub fn open_whats_new(inner: &AppWindowInner) {
    let tx_wn = inner.tx.clone();
    gtk4::glib::spawn_future_local(async move {
        let loaded = boxxy_sys_utils::runtime()
            .spawn_blocking(|| {
                let pending = crate::updater::whats_new::load_last_whats_new();
                let cached = crate::updater::whats_new::load_cached_changelog_summary(&pending);
                (pending, cached)
            })
            .await;

        if let Ok((pending, cached)) = loaded {
            let _ = tx_wn
                .send(AppInput::ShowWhatsNewDialog(pending, cached.map(Box::new)))
                .await;
        }
    });
}

pub fn start_whats_new_claw(inner: &mut AppWindowInner, prompt: String) {
    if !boxxy_claw_protocol::characters::has_available_character(None) {
        let toast = adw::Toast::new("No characters available to read What's New");
        toast.set_timeout(3);
        inner.toast_overlay.add_toast(toast);
        return;
    }
    if inner.tabs.is_empty() {
        super::tabs::new_tab(inner);
    }

    // Ensure we target an idle pane that does not already have a running Claw session.
    if inner.active_tab().is_some_and(|t| t.is_claw_active()) {
        super::tabs::new_tab(inner);
    }

    if let Some(tab) = inner.active_tab() {
        let _ = tab.start_claw_query_with_default_character(prompt);
    }
}

pub fn show_whats_new_dialog(
    inner: &AppWindowInner,
    pending: WhatsNewPending,
    summary: Option<ChangelogSummary>,
) {
    let pending_to_save = pending.clone();
    boxxy_sys_utils::runtime().spawn_blocking(move || {
        crate::updater::whats_new::save_last_whats_new(&pending_to_save);
    });

    let claw_ready = matches!(
        inner
            .current_settings
            .resolve_model_state(&inner.current_settings.claw_model),
        boxxy_preferences::config::ModelConfigState::Ready(_)
    );

    let builder = gtk4::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/whats_new_dialog.ui");
    let Some(dialog) = builder.object::<adw::Dialog>("whats_new_dialog") else {
        return;
    };

    if let Some(banner_picture) = builder.object::<gtk4::Picture>("banner_picture") {
        banner_picture.set_resource(Some("/dev/boxxy/BoxxyTerminal/icons/whats_new_banner.png"));
    }

    if let Some(hero_subtitle) = builder.object::<gtk4::Label>("hero_subtitle_label") {
        if claw_ready {
            hero_subtitle.set_label("Hot out of the compiler! Let Boxxy give you the tour.");
        } else {
            hero_subtitle
                .set_label("Hot out of the compiler! Copy the prompt below for your favorite AI.");
        }
    }

    let commits_stack = builder.object::<gtk4::Stack>("commits_stack");
    let detail_scrolled_for_back = builder.object::<gtk4::ScrolledWindow>("detail_scrolled");
    if let Some(back_btn) = builder.object::<gtk4::Button>("commit_back_btn") {
        let stack_weak = commits_stack
            .as_ref()
            .map(gtk4::prelude::ObjectExt::downgrade);
        let detail_sc_weak = detail_scrolled_for_back
            .as_ref()
            .map(gtk4::prelude::ObjectExt::downgrade);
        back_btn.connect_clicked(move |_| {
            if let Some(ref scw) = detail_sc_weak
                && let Some(sc) = scw.upgrade()
            {
                sc.set_min_content_height(-1);
                sc.set_height_request(-1);
            }
            if let Some(ref sw) = stack_weak
                && let Some(stack) = sw.upgrade()
            {
                stack.set_visible_child_name("list");
            }
        });
    }

    let state_rc = Rc::new(RefCell::new((pending.clone(), summary.clone())));
    let detail_idx_rc = Rc::new(Cell::new(0usize));

    let show_commit_detail: Rc<dyn Fn(usize)> = {
        let builder_c = builder.clone();
        let state_c = state_rc.clone();
        let idx_c = detail_idx_rc.clone();
        Rc::new(move |idx: usize| {
            let (_, ref cur_summary) = *state_c.borrow();
            let Some(s) = cur_summary.as_ref() else {
                return;
            };
            let total = s.commits.len();
            if idx >= total {
                return;
            }
            // The commit list displays `s.commits.iter().rev()`, so index `idx`
            // in display order maps to `s.commits[total - 1 - idx]`.
            let entry = &s.commits[total - 1 - idx];
            idx_c.set(idx);

            if let Some(lbl) = builder_c.object::<gtk4::Label>("detail_sha_label") {
                let escaped_sha = gtk4::glib::markup_escape_text(&entry.short_sha);
                let commit_url = format!(
                    "https://github.com/{}/{}/commit/{}",
                    crate::updater::REPO_OWNER,
                    crate::updater::REPO_NAME,
                    escaped_sha
                );
                lbl.set_markup(&format!(
                    "<a href=\"{commit_url}\"><span underline=\"none\">{escaped_sha}</span></a>"
                ));
                lbl.set_tooltip_text(Some(&format!("View commit {} on GitHub", entry.short_sha)));
            }
            if let Some(prev_btn) = builder_c.object::<gtk4::Button>("commit_prev_btn") {
                prev_btn.set_sensitive(idx > 0);
            }
            if let Some(next_btn) = builder_c.object::<gtk4::Button>("commit_next_btn") {
                next_btn.set_sensitive(idx + 1 < total);
            }
            if let Some(lbl) = builder_c.object::<gtk4::Label>("detail_message_label") {
                let mut lines = entry.message.lines();
                let first_line = lines.next().unwrap_or(&entry.message).trim();
                let rest = lines.collect::<Vec<_>>().join("\n").trim().to_string();
                let markup = if rest.is_empty() {
                    format!("<b>{}</b>", gtk4::glib::markup_escape_text(first_line))
                } else {
                    format!(
                        "<b>{}</b>\n\n<span alpha=\"85%\">{}</span>",
                        gtk4::glib::markup_escape_text(first_line),
                        gtk4::glib::markup_escape_text(&rest)
                    )
                };
                lbl.set_markup(&markup);
            }
            if let Some(sc) = builder_c.object::<gtk4::ScrolledWindow>("detail_scrolled") {
                sc.set_min_content_height(452);
                sc.set_height_request(452);
                sc.vadjustment().set_value(0.0);
            }
            if let Some(stack) = builder_c.object::<gtk4::Stack>("commits_stack") {
                stack.set_visible_child_name("detail");
            }
        })
    };

    if let Some(prev_btn) = builder.object::<gtk4::Button>("commit_prev_btn") {
        let idx_c = detail_idx_rc.clone();
        let show_c = show_commit_detail.clone();
        prev_btn.connect_clicked(move |_| {
            let cur = idx_c.get();
            if cur > 0 {
                show_c(cur - 1);
            }
        });
    }
    if let Some(next_btn) = builder.object::<gtk4::Button>("commit_next_btn") {
        let idx_c = detail_idx_rc.clone();
        let show_c = show_commit_detail.clone();
        next_btn.connect_clicked(move |_| {
            show_c(idx_c.get() + 1);
        });
    }

    let loading_rc = Rc::new(Cell::new(false));
    let read_btn = builder.object::<gtk4::Button>("read_btn");
    let read_btn_label = builder.object::<gtk4::Label>("read_btn_label");
    let copy_btn = builder.object::<gtk4::Button>("copy_prompt_btn");

    let update_action_buttons: Rc<dyn Fn()> = {
        let read_btn_c = read_btn.clone();
        let read_btn_label_c = read_btn_label.clone();
        let copy_btn_c = copy_btn.clone();
        let loading_c = loading_rc.clone();
        Rc::new(move || {
            let is_loading = loading_c.get();
            if let Some(ref cb) = copy_btn_c {
                cb.set_sensitive(!is_loading);
            }
            if let Some(ref rb) = read_btn_c {
                let chars_available =
                    boxxy_claw_protocol::characters::has_available_character(None);
                if is_loading {
                    rb.set_sensitive(false);
                    if let Some(ref lbl) = read_btn_label_c {
                        lbl.set_label("Read What's New");
                    }
                    rb.set_tooltip_text(Some("Fetching update details…"));
                } else if !claw_ready {
                    rb.set_sensitive(false);
                    if let Some(ref lbl) = read_btn_label_c {
                        lbl.set_label("Read What's New");
                    }
                    rb.set_tooltip_text(Some(
                        "Configure a Claw AI model in Preferences to let Boxxy read what's new",
                    ));
                } else if !chars_available {
                    rb.set_sensitive(false);
                    if let Some(ref lbl) = read_btn_label_c {
                        lbl.set_label("No Characters Free");
                    }
                    rb.set_tooltip_text(Some(
                        "All characters are currently active in other sessions",
                    ));
                } else {
                    rb.set_sensitive(true);
                    if let Some(ref lbl) = read_btn_label_c {
                        lbl.set_label("Read What's New");
                    }
                    rb.set_tooltip_text(None);
                }
            }
        })
    };

    let range_stack = builder.object::<gtk4::Stack>("commit_range_stack");
    let from_entry = builder.object::<gtk4::Entry>("from_commit_entry");
    let to_entry = builder.object::<gtk4::Entry>("to_commit_entry");
    let edit_range_btn = builder.object::<gtk4::Button>("edit_range_btn");
    let refresh_range_btn = builder.object::<gtk4::Button>("refresh_range_btn");

    if let Some(ref edit_btn) = edit_range_btn {
        let range_stack_c = range_stack.clone();
        let from_entry_c = from_entry.clone();
        let to_entry_c = to_entry.clone();
        let state_c = state_rc.clone();
        edit_btn.connect_clicked(move |_| {
            let (ref cur_pending, _) = *state_c.borrow();
            if let Some(ref fe) = from_entry_c {
                fe.set_text(&cur_pending.from_commit);
            }
            if let Some(ref te) = to_entry_c {
                te.set_text(&cur_pending.to_commit);
            }
            if let Some(ref st) = range_stack_c {
                st.set_visible_child_name("edit");
            }
            if let Some(ref fe) = from_entry_c {
                fe.grab_focus();
            }
        });
    }

    let fetch_and_populate: Rc<dyn Fn(WhatsNewPending, bool)> = {
        let builder_c = builder.clone();
        let range_stack_c = range_stack.clone();
        let state_c = state_rc.clone();
        let show_detail_c = show_commit_detail.clone();
        let loading_c = loading_rc.clone();
        let update_btns_c = update_action_buttons.clone();
        let tx_toast = inner.tx.clone();
        Rc::new(move |new_pending, show_error_toast| {
            if let Some(ref st) = range_stack_c {
                st.set_visible_child_name("display");
            }
            if let Some(commit_pill) = builder_c.object::<gtk4::Label>("commit_pill_label") {
                commit_pill.set_label(&format!(
                    "{} → {}",
                    new_pending.from_commit, new_pending.to_commit
                ));
            }
            loading_c.set(true);
            set_whats_new_loading_state(&builder_c, true);
            update_btns_c();

            let builder_async = builder_c.clone();
            let state_async = state_c.clone();
            let show_detail_async = show_detail_c.clone();
            let loading_async = loading_c.clone();
            let update_btns_async = update_btns_c.clone();
            let tx_toast_async = tx_toast.clone();
            gtk4::glib::spawn_future_local(async move {
                let p_fetch = new_pending.clone();
                let res = boxxy_sys_utils::runtime()
                    .spawn(async move {
                        let p_cache = p_fetch.clone();
                        let cached = tokio::task::spawn_blocking(move || {
                            crate::updater::whats_new::load_cached_changelog_summary(&p_cache)
                        })
                        .await
                        .ok()
                        .flatten();

                        if let Some(hit) = cached {
                            let p_save = p_fetch.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                crate::updater::whats_new::save_last_whats_new(&p_save);
                            })
                            .await;
                            return Ok(hit);
                        }

                        let fetched = crate::updater::whats_new::fetch_changelog_summary(
                            &p_fetch.from_commit,
                            &p_fetch.to_commit,
                        )
                        .await?;
                        if fetched.total_commits > 0 {
                            let p_save = p_fetch.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                crate::updater::whats_new::save_last_whats_new(&p_save);
                            })
                            .await;
                        }
                        Ok::<ChangelogSummary, anyhow::Error>(fetched)
                    })
                    .await;

                loading_async.set(false);
                match res {
                    Ok(Ok(new_summary)) if new_summary.total_commits > 0 => {
                        *state_async.borrow_mut() =
                            (new_pending.clone(), Some(new_summary.clone()));
                        populate_whats_new_summary(
                            &builder_async,
                            &new_pending,
                            Some(&new_summary),
                            &show_detail_async,
                        );
                    }
                    _ => {
                        {
                            let (ref prev_pending, ref prev_summary) = *state_async.borrow();
                            if prev_summary.is_some() {
                                populate_whats_new_summary(
                                    &builder_async,
                                    prev_pending,
                                    prev_summary.as_ref(),
                                    &show_detail_async,
                                );
                            } else {
                                set_whats_new_offline_state(&builder_async, &new_pending);
                            }
                        }
                        if show_error_toast {
                            let _ = tx_toast_async
                                .send(AppInput::ShowToast(ToastRequest::ephemeral(format!(
                                    "Could not fetch commits for {}...{}",
                                    new_pending.from_commit, new_pending.to_commit
                                ))))
                                .await;
                        }
                    }
                }
                update_btns_async();
            });
        })
    };

    let trigger_refresh: Rc<dyn Fn()> = {
        let from_entry_c = from_entry.clone();
        let to_entry_c = to_entry.clone();
        let fetch_c = fetch_and_populate.clone();
        let tx_toast = inner.tx.clone();
        Rc::new(move || {
            let from_val = from_entry_c
                .as_ref()
                .map(|e| e.text().trim().to_string())
                .unwrap_or_default();
            let to_val = to_entry_c
                .as_ref()
                .map(|e| e.text().trim().to_string())
                .unwrap_or_default();

            if from_val.is_empty() || to_val.is_empty() {
                return;
            }

            let (from_sanitized, from_clamped) =
                crate::updater::whats_new::sanitize_commit_ref(&from_val);
            let (to_sanitized, to_clamped) =
                crate::updater::whats_new::sanitize_commit_ref(&to_val);

            if from_sanitized.is_empty() || to_sanitized.is_empty() {
                let _ = tx_toast.send_blocking(AppInput::ShowToast(ToastRequest::ephemeral(
                    "Invalid commit reference",
                )));
                return;
            }

            if let Some(ref fe) = from_entry_c {
                fe.set_text(&from_sanitized);
            }
            if let Some(ref te) = to_entry_c {
                te.set_text(&to_sanitized);
            }

            if from_clamped || to_clamped {
                let _ =
                    tx_toast.send_blocking(AppInput::ShowToast(ToastRequest::ephemeral(format!(
                        "Commit range capped at {} commits",
                        crate::updater::whats_new::MAX_WHATS_NEW_COMMITS
                    ))));
            }

            let new_pending = WhatsNewPending {
                from_commit: from_sanitized,
                to_commit: to_sanitized,
            };
            fetch_c(new_pending, true);
        })
    };

    if let Some(ref refresh_btn) = refresh_range_btn {
        let trig = trigger_refresh.clone();
        refresh_btn.connect_clicked(move |_| trig());
    }
    if let Some(ref fe) = from_entry {
        let trig = trigger_refresh.clone();
        fe.connect_activate(move |_| trig());
    }
    if let Some(ref te) = to_entry {
        let trig = trigger_refresh.clone();
        te.connect_activate(move |_| trig());
    }

    if let Some(ref copy_btn) = copy_btn {
        let copy_icon = builder.object::<gtk4::Image>("copy_prompt_icon");
        let tx_copy = inner.tx.clone();
        let state_copy = state_rc.clone();
        copy_btn.connect_clicked(move |_| {
            let (ref cur_pending, ref cur_summary) = *state_copy.borrow();
            let clip_text = crate::updater::whats_new::build_clipboard_whats_new_prompt(
                cur_pending,
                cur_summary.as_ref(),
            );
            if let Some(display) = gtk4::gdk::Display::default() {
                display.clipboard().set_text(&clip_text);
            }
            if let Some(ref icon) = copy_icon {
                icon.set_icon_name(Some("boxxy-object-select-symbolic"));
            }
            let _ = tx_copy.send_blocking(AppInput::ShowToast(ToastRequest::ephemeral(
                "What's New prompt copied to clipboard",
            )));
        });
    }

    if let Some(commits_btn) = builder.object::<gtk4::Button>("commits_tile_btn") {
        let state_diff = state_rc.clone();
        commits_btn.connect_clicked(move |_| {
            let (ref cur_pending, _) = *state_diff.borrow();
            let compare_url = format!(
                "https://github.com/{}/{}/compare/{}...{}",
                crate::updater::REPO_OWNER,
                crate::updater::REPO_NAME,
                cur_pending.from_commit,
                cur_pending.to_commit,
            );
            let _ = gtk4::gio::AppInfo::launch_default_for_uri(
                &compare_url,
                None::<&gtk4::gio::AppLaunchContext>,
            );
        });
    }

    if let Some(contributors_btn) = builder.object::<gtk4::Button>("contributors_tile_btn") {
        contributors_btn.connect_clicked(move |_| {
            let contributors_url = format!(
                "https://github.com/{}/{}/graphs/contributors",
                crate::updater::REPO_OWNER,
                crate::updater::REPO_NAME,
            );
            let _ = gtk4::gio::AppInfo::launch_default_for_uri(
                &contributors_url,
                None::<&gtk4::gio::AppLaunchContext>,
            );
        });
    }

    if let Some(sponsor_btn) = builder.object::<gtk4::Button>("sponsor_tile_btn") {
        sponsor_btn.connect_clicked(move |_| {
            let sponsor_url = format!("https://github.com/sponsors/{}", crate::updater::REPO_OWNER);
            let _ = gtk4::gio::AppInfo::launch_default_for_uri(
                &sponsor_url,
                None::<&gtk4::gio::AppLaunchContext>,
            );
        });
    }

    if let Some(close_btn) = builder.object::<gtk4::Button>("close_btn") {
        let dialog_weak = dialog.downgrade();
        close_btn.connect_clicked(move |_| {
            if let Some(d) = dialog_weak.upgrade() {
                d.close();
            }
        });
    }

    if let Some(ref read_btn) = read_btn {
        update_action_buttons();

        // Reactively update the button if a character claim is released or taken while the dialog is open.
        let read_btn_weak = read_btn.downgrade();
        let update_btns_reg = update_action_buttons.clone();
        let mut rx_reg = boxxy_claw_protocol::characters::subscribe_registry_changes();
        gtk4::glib::spawn_future_local(async move {
            while rx_reg.changed().await.is_ok() {
                if read_btn_weak.upgrade().is_none() {
                    break;
                }
                update_btns_reg();
            }
        });

        let dialog_weak = dialog.downgrade();
        let tx_read = inner.tx.clone();
        let state_read = state_rc.clone();
        read_btn.connect_clicked(move |_| {
            if !boxxy_claw_protocol::characters::has_available_character(None) {
                return;
            }
            let (ref cur_pending, ref cur_summary) = *state_read.borrow();
            let prompt = crate::updater::whats_new::build_claw_whats_new_prompt(
                cur_pending,
                cur_summary.as_ref(),
            );
            let _ = tx_read.send_blocking(AppInput::StartWhatsNewClaw(prompt));
            if let Some(d) = dialog_weak.upgrade() {
                d.close();
            }
        });
    }

    if summary.is_some() {
        populate_whats_new_summary(&builder, &pending, summary.as_ref(), &show_commit_detail);
    } else {
        loading_rc.set(true);
        set_whats_new_loading_state(&builder, true);
        update_action_buttons();
    }

    {
        let tx_wn_close = inner.tx.clone();
        dialog.connect_closed(move |_| {
            let _ = tx_wn_close.send_blocking(AppInput::FocusActiveTerminal);
        });
    }

    dialog.present(Some(&inner.window));

    if summary.is_none() {
        fetch_and_populate(pending, false);
    }
}

fn set_whats_new_loading_state(builder: &gtk4::Builder, loading: bool) {
    if let Some(body_stack) = builder.object::<gtk4::Stack>("body_state_stack") {
        body_stack.set_visible_child_name(if loading { "loading" } else { "content" });
    }
    if let Some(spinner) = builder.object::<gtk4::Spinner>("body_loading_spinner") {
        spinner.set_visible(true);
        spinner.set_spinning(loading);
    }
    if loading && let Some(lbl) = builder.object::<gtk4::Label>("body_loading_label") {
        lbl.set_label("Fetching update details from GitHub…");
    }
    if let Some(edit_btn) = builder.object::<gtk4::Button>("edit_range_btn") {
        edit_btn.set_sensitive(!loading);
    }
    if let Some(refresh_btn) = builder.object::<gtk4::Button>("refresh_range_btn") {
        refresh_btn.set_sensitive(!loading);
    }
}

fn set_whats_new_offline_state(builder: &gtk4::Builder, pending: &WhatsNewPending) {
    if let Some(commit_pill) = builder.object::<gtk4::Label>("commit_pill_label") {
        commit_pill.set_label(&format!("{} → {}", pending.from_commit, pending.to_commit));
    }
    if let Some(body_stack) = builder.object::<gtk4::Stack>("body_state_stack") {
        body_stack.set_visible_child_name("loading");
    }
    if let Some(spinner) = builder.object::<gtk4::Spinner>("body_loading_spinner") {
        spinner.set_spinning(false);
        spinner.set_visible(false);
    }
    if let Some(lbl) = builder.object::<gtk4::Label>("body_loading_label") {
        lbl.set_label("Could not load commit details from GitHub.");
    }
    if let Some(edit_btn) = builder.object::<gtk4::Button>("edit_range_btn") {
        edit_btn.set_sensitive(true);
    }
    if let Some(refresh_btn) = builder.object::<gtk4::Button>("refresh_range_btn") {
        refresh_btn.set_sensitive(true);
    }
}

fn populate_whats_new_summary(
    builder: &gtk4::Builder,
    pending: &WhatsNewPending,
    summary: Option<&ChangelogSummary>,
    show_commit_detail: &Rc<dyn Fn(usize)>,
) {
    if let Some(commit_pill) = builder.object::<gtk4::Label>("commit_pill_label") {
        commit_pill.set_label(&format!("{} → {}", pending.from_commit, pending.to_commit));
    }

    if let Some(s) = summary
        && s.total_commits > 0
    {
        set_whats_new_loading_state(builder, false);
        if let Some(stats_card) = builder.object::<gtk4::Box>("stats_card") {
            stats_card.set_visible(true);
        }
        if let Some(commits_val) = builder.object::<gtk4::Label>("commits_val") {
            commits_val.set_label(&s.total_commits.to_string());
        }
        if let Some(commits_caption) = builder.object::<gtk4::Label>("commits_caption") {
            let word = if s.total_commits == 1 {
                "Commit Diff"
            } else {
                "Commits Diff"
            };
            commits_caption.set_label(word);
        }
        if let Some(files_val) = builder.object::<gtk4::Label>("files_val") {
            files_val.set_label(&s.files_changed.to_string());
        }
        if let Some(files_caption) = builder.object::<gtk4::Label>("files_caption") {
            let word = if s.files_changed == 1 {
                "File Touched"
            } else {
                "Files Touched"
            };
            files_caption.set_label(word);
        }
        if let Some(lines_val) = builder.object::<gtk4::Label>("lines_val") {
            lines_val.set_markup(&format!(
                "<span foreground=\"#4ade80\">+{}</span> <span alpha=\"50%\">/</span> <span foreground=\"#f87171\">-{}</span>",
                s.additions, s.deletions
            ));
        }
        let contributor_count = s.contributors.len().max(1);
        if let Some(contributors_val) = builder.object::<gtk4::Label>("contributors_val") {
            if s.contributors.is_empty() {
                contributors_val.set_label(&format!("👥 {contributor_count}"));
            } else {
                contributors_val.set_label(&contributor_count.to_string());
            }
        }
        if let Some(contributors_caption) = builder.object::<gtk4::Label>("contributors_caption") {
            let word = if contributor_count == 1 {
                "Contributor"
            } else {
                "Contributors"
            };
            contributors_caption.set_label(word);
        }
        if let Some(avatars_box) = builder.object::<gtk4::Box>("contributors_avatars_box") {
            while let Some(child) = avatars_box.first_child() {
                avatars_box.remove(&child);
            }
            let mut avatar_widgets = Vec::new();
            let mut needs_remote_fetch = false;
            for contributor in s.contributors.iter().take(5) {
                let avatar = adw::Avatar::builder()
                    .size(22)
                    .text(&contributor.login)
                    .show_initials(true)
                    .tooltip_text(format!("@{}", contributor.login))
                    .build();
                if let Some(ref bytes) = contributor.avatar_bytes {
                    let gbytes = gtk4::glib::Bytes::from(bytes);
                    if let Ok(texture) = gtk4::gdk::Texture::from_bytes(&gbytes) {
                        avatar.set_custom_image(Some(&texture));
                    }
                } else if contributor.avatar_url.is_some() {
                    needs_remote_fetch = true;
                }
                avatars_box.append(&avatar);
                avatar_widgets.push((contributor.login.clone(), avatar.downgrade()));
            }

            if needs_remote_fetch {
                let contributors_to_fetch: Vec<_> =
                    s.contributors.iter().take(5).cloned().collect();
                gtk4::glib::spawn_future_local(async move {
                    let fetched = boxxy_sys_utils::runtime()
                        .spawn(async move {
                            crate::updater::whats_new::fetch_contributor_avatars(
                                &contributors_to_fetch,
                            )
                            .await
                        })
                        .await
                        .unwrap_or_default();

                    for (login, bytes) in fetched {
                        let gbytes = gtk4::glib::Bytes::from_owned(bytes);
                        if let Ok(texture) = gtk4::gdk::Texture::from_bytes(&gbytes) {
                            for (w_login, w_avatar) in &avatar_widgets {
                                if w_login.eq_ignore_ascii_case(&login)
                                    && let Some(av) = w_avatar.upgrade()
                                {
                                    av.set_custom_image(Some(&texture));
                                }
                            }
                        }
                    }
                });
            }
        }
        if let Some(commits_scrolled) = builder.object::<gtk4::ScrolledWindow>("commits_scrolled") {
            commits_scrolled.vadjustment().set_value(0.0);
        }

        if let Some(commits_list) = builder.object::<gtk4::Box>("commits_list") {
            while let Some(child) = commits_list.first_child() {
                commits_list.remove(&child);
            }
            for (display_idx, entry) in s.commits.iter().rev().enumerate() {
                let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);

                let text_col = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
                text_col.set_hexpand(true);
                text_col.set_valign(gtk4::Align::Center);

                let title_label = gtk4::Label::builder()
                    .label(entry.title())
                    .halign(gtk4::Align::Start)
                    .valign(gtk4::Align::Center)
                    .hexpand(true)
                    .xalign(0.0)
                    .ellipsize(gtk4::pango::EllipsizeMode::End)
                    .single_line_mode(true)
                    .css_classes(["whats-new-commit-title"])
                    .build();

                let meta_label = gtk4::Label::builder()
                    .label(entry.meta_caption())
                    .halign(gtk4::Align::Start)
                    .valign(gtk4::Align::Center)
                    .xalign(0.0)
                    .css_classes(["whats-new-commit-meta"])
                    .build();

                text_col.append(&title_label);
                text_col.append(&meta_label);

                let chevron = gtk4::Image::builder()
                    .icon_name("boxxy-right-symbolic")
                    .valign(gtk4::Align::Center)
                    .css_classes(["whats-new-commit-arrow"])
                    .build();

                row_box.append(&text_col);
                row_box.append(&chevron);

                let row_btn = gtk4::Button::builder()
                    .child(&row_box)
                    .focus_on_click(false)
                    .css_classes(["flat", "whats-new-commit-row"])
                    .build();

                let show_c = show_commit_detail.clone();
                row_btn.connect_clicked(move |_| {
                    show_c(display_idx);
                });

                commits_list.append(&row_btn);
            }
        }
    } else if let Some(stats_card) = builder.object::<gtk4::Box>("stats_card") {
        stats_card.set_visible(false);
    }
}
