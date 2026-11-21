use libadwaita::prelude::*;

use crate::state::{AppInput, AppWindowInner};

fn resolve_target_pane_id(inner: &AppWindowInner, pane_id: Option<String>) -> Option<String> {
    pane_id.or_else(|| inner.active_tab().map(|t| t.controller.active_pane_id()))
}

pub fn set_claw_active(inner: &mut AppWindowInner, active: bool, pane_id: Option<String>) {
    let Some(id) = resolve_target_pane_id(inner, pane_id) else {
        inner.claw_active = active;
        return;
    };

    let mut found = false;
    for tab in &inner.tabs {
        if tab.controller.set_claw_active_for_pane(&id, active) {
            found = true;
            break;
        }
    }

    if found
        && inner
            .active_tab()
            .is_some_and(|t| t.controller.active_pane_id() == id)
    {
        inner.claw_active = active;
    }
    super::tab_bar_sync::sync_tab_running_icons(inner);
}

pub fn set_claw_active_global(inner: &mut AppWindowInner, active: bool) {
    inner.claw_active = active;
    for tab in &inner.tabs {
        tab.controller.set_claw_active(active);
    }
    super::tab_bar_sync::sync_tab_running_icons(inner);
}

pub fn cancel_task(inner: &AppWindowInner, task_id: uuid::Uuid, pane_id: Option<String>) {
    let Some(id) = resolve_target_pane_id(inner, pane_id) else {
        return;
    };
    for tab in &inner.tabs {
        if tab.controller.cancel_task_by_id(&id, task_id) {
            break;
        }
    }
    super::tab_bar_sync::sync_tab_running_icons(inner);
    if let Some(tc) = inner.active_tab() {
        inner.claw.update_tasks(tc.get_pending_tasks());
    }
}

pub fn toggle_pause_task(inner: &AppWindowInner, task_id: uuid::Uuid, pane_id: Option<String>) {
    let Some(id) = resolve_target_pane_id(inner, pane_id) else {
        return;
    };
    for tab in &inner.tabs {
        if tab.controller.toggle_pause_task_by_id(&id, task_id) {
            break;
        }
    }
    if let Some(tc) = inner.active_tab() {
        inner.claw.update_tasks(tc.get_pending_tasks());
    }
}

pub fn cancel_all_tasks(inner: &AppWindowInner, pane_id: Option<String>) {
    let Some(id) = resolve_target_pane_id(inner, pane_id) else {
        return;
    };
    for tab in &inner.tabs {
        if tab.controller.cancel_all_tasks_by_id(&id) {
            break;
        }
    }
    super::tab_bar_sync::sync_tab_running_icons(inner);
    if let Some(tc) = inner.active_tab() {
        inner.claw.update_tasks(tc.get_pending_tasks());
    }
}

pub fn clear_claw_history(inner: &AppWindowInner, pane_id: Option<String>) {
    let Some(id) = resolve_target_pane_id(inner, pane_id) else {
        return;
    };
    for tab in &inner.tabs {
        if tab.controller.soft_clear_claw_history(&id) {
            break;
        }
    }
}

pub fn resume_claw_session(
    inner: &AppWindowInner,
    session_id: &str,
    override_character_id: Option<String>,
) {
    if let Some(tc) = inner.active_tab() {
        tc.controller
            .resume_session(session_id, override_character_id);
    }
}

pub fn open_model_selection(inner: &AppWindowInner, initial_character_id: Option<String>) {
    let tx_apis = inner.tx.clone();
    let tx_model_close = inner.tx.clone();

    let cached_catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
    let catalog: Vec<boxxy_claw_protocol::characters::CharacterInfo> = if cached_catalog.is_empty()
    {
        boxxy_claw_protocol::character_loader::load_characters().unwrap_or_default()
    } else {
        (**cached_catalog).clone()
    };

    let chars_dir = boxxy_claw_protocol::character_loader::get_characters_dir().ok();
    let characters: Vec<boxxy_model_selection::CharacterModelTarget> = catalog
        .into_iter()
        .filter(|c| !boxxy_claw_protocol::characters::is_incognito_character(&c.config.id))
        .map(|info| {
            let avatar_path = info
                .has_avatar
                .then(|| {
                    chars_dir
                        .as_ref()
                        .map(|d| d.join(&info.config.name).join("AVATAR.png"))
                })
                .flatten();
            let display_name = boxxy_claw_ui::resolve_character_display_name(&info);
            boxxy_model_selection::CharacterModelTarget {
                id: info.config.id,
                display_name,
                duties: info.config.duties,
                color: info.config.color,
                avatar_path,
            }
        })
        .collect();

    let active_char_id = initial_character_id
        .filter(|id| !id.is_empty() && !boxxy_claw_protocol::characters::is_incognito_character(id))
        .or_else(|| {
            inner
                .active_tab()
                .map(|t| t.controller.active_pane_id())
                .and_then(|pane_id| {
                    boxxy_claw_protocol::characters::CLAIMS_CACHE
                        .load()
                        .iter()
                        .find(|claim| claim.holder_id == pane_id)
                        .map(|claim| claim.character_id.clone())
                })
                .filter(|id| !boxxy_claw_protocol::characters::is_incognito_character(id))
        });

    let config = boxxy_model_selection::ModelAssignmentConfig {
        default_claw_model: inner.current_settings.claw_model.clone(),
        character_models: inner.current_settings.character_models.clone(),
        memory_model: inner.current_settings.memory_model.clone(),
        broken_default_notice: inner.current_settings.default_model_broken_reason(),
        initial_selected_character_id: active_char_id,
    };

    let dialog = boxxy_model_selection::GlobalModelSelectorDialog::new_with_characters(
        config,
        characters,
        inner.current_settings.ollama_base_url.clone(),
        inner.current_settings.get_effective_api_keys(),
        inner.current_settings.openai_compatible_endpoints.clone(),
        move |claw_provider, character_models, memory_provider| {
            boxxy_preferences::Settings::update(|settings| {
                settings.claw_model = claw_provider.clone();
                settings.character_models = character_models.clone();
                settings.memory_model = memory_provider.clone();
            });
        },
    );
    dialog.set_on_open_apis(move || {
        let _ = tx_apis.send_blocking(AppInput::OpenApisPreferences);
    });
    dialog.dialog().connect_closed(move |_| {
        let _ = tx_model_close.send_blocking(AppInput::FocusActiveTerminal);
    });
    if boxxy_preferences::is_chatgpt_oauth_signed_in() {
        let dialog_refresh = dialog.clone();
        gtk4::glib::spawn_future_local(async move {
            let res = boxxy_sys_utils::runtime()
                .spawn(async { boxxy_preferences::refresh_chatgpt_account_models().await })
                .await;
            if matches!(res, Ok(Ok(_))) {
                dialog_refresh.refresh_chatgpt_models();
            }
        });
    }
    dialog.present(Some(&inner.window));
}

pub fn reload_engine(inner: &mut AppWindowInner) {
    boxxy_preferences::Settings::reload();
    let reloaded = (*boxxy_preferences::Settings::load()).clone();
    super::window_state::settings_changed(inner, reloaded);
    inner.claw.refresh_past_sessions();
    super::window_state::refresh_claw_maintenance_status(inner.tx.clone());
}
