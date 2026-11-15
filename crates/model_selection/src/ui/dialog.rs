use crate::models::{
    CharacterModelTarget, ModelAssignmentConfig, ModelProvider, OpenAiCompatibleEndpoint,
};
use crate::ui::selector::{
    OllamaTagsCache, SelectorLayout, SelectorOptions, SingleModelSelector,
};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

type VoidCallbackSlot = Rc<RefCell<Option<Rc<dyn Fn() + 'static>>>>;
type ProviderChangeSlot = Rc<RefCell<Option<Rc<dyn Fn(Option<ModelProvider>)>>>>;
type AssignmentTuple = (
    Option<ModelProvider>,
    HashMap<String, ModelProvider>,
    Option<ModelProvider>,
);
type DoneCallback = Rc<
    dyn Fn(Option<ModelProvider>, HashMap<String, ModelProvider>, Option<ModelProvider>),
>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OverrideBadgeState {
    Default,
    Custom,
    Broken,
}

struct CharacterRowWidgets {
    target: CharacterModelTarget,
    row: gtk::ListBoxRow,
    badge: gtk::Label,
}

fn set_badge_state(badge: &gtk::Label, state: OverrideBadgeState) {
    badge.remove_css_class("model-badge-default");
    badge.remove_css_class("model-badge-custom");
    badge.remove_css_class("model-badge-broken");
    match state {
        OverrideBadgeState::Default => {
            badge.set_label("Default");
            badge.add_css_class("model-badge-default");
        }
        OverrideBadgeState::Custom => {
            badge.set_label("Custom");
            badge.add_css_class("model-badge-custom");
        }
        OverrideBadgeState::Broken => {
            badge.set_label("Broken");
            badge.add_css_class("model-badge-broken");
        }
    }
}

fn first_line_duties(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Boxxy Claw Character")
        .to_string()
}

fn load_avatar_async(
    avatar: &adw::Avatar,
    path_opt: Option<&PathBuf>,
    tex_cache: &Rc<RefCell<HashMap<PathBuf, gdk::Texture>>>,
) {
    let Some(path) = path_opt else {
        avatar.set_custom_image(None::<&gdk::Paintable>);
        return;
    };
    if let Some(cached) = tex_cache.borrow().get(path) {
        avatar.set_custom_image(Some(cached));
        return;
    }
    avatar.set_custom_image(None::<&gdk::Paintable>);
    let path_owned = path.clone();
    let avatar_weak = avatar.downgrade();
    let cache_rc = tex_cache.clone();
    glib::spawn_future_local(async move {
        let file = gio::File::for_path(&path_owned);
        if let Ok((bytes, _)) = file.load_contents_future().await {
            let gbytes = glib::Bytes::from_owned(bytes);
            if let Ok(tex) = gdk::Texture::from_bytes(&gbytes) {
                cache_rc.borrow_mut().insert(path_owned, tex.clone());
                if let Some(av) = avatar_weak.upgrade() {
                    av.set_custom_image(Some(&tex));
                }
            }
        }
    });
}

#[derive(Clone)]
pub struct GlobalModelSelectorDialog {
    dialog: adw::Dialog,
    pub claw_selector: SingleModelSelector,
    pub memory_selector: SingleModelSelector,
    on_open_apis: VoidCallbackSlot,
}

impl GlobalModelSelectorDialog {
    pub fn new<F>(
        init_claw: Option<ModelProvider>,
        init_memory: Option<ModelProvider>,
        ollama_url: String,
        api_keys: HashMap<String, String>,
        on_done: F,
    ) -> Self
    where
        F: Fn(Option<ModelProvider>, Option<ModelProvider>) + 'static,
    {
        Self::new_with_custom(
            init_claw,
            init_memory,
            ollama_url,
            api_keys,
            Vec::new(),
            on_done,
        )
    }

    pub fn new_with_custom<F>(
        init_claw: Option<ModelProvider>,
        init_memory: Option<ModelProvider>,
        ollama_url: String,
        api_keys: HashMap<String, String>,
        custom_endpoints: Vec<OpenAiCompatibleEndpoint>,
        on_done: F,
    ) -> Self
    where
        F: Fn(Option<ModelProvider>, Option<ModelProvider>) + 'static,
    {
        let cfg = ModelAssignmentConfig {
            default_claw_model: init_claw,
            character_models: HashMap::new(),
            memory_model: init_memory,
            broken_default_notice: None,
            initial_selected_character_id: None,
        };
        Self::new_with_characters(
            cfg,
            Vec::new(),
            ollama_url,
            api_keys,
            custom_endpoints,
            move |default_claw, _char_models, memory| {
                on_done(default_claw, memory);
            },
        )
    }

    #[allow(clippy::too_many_lines)]
    pub fn new_with_characters<F>(
        config: ModelAssignmentConfig,
        characters: Vec<CharacterModelTarget>,
        ollama_url: String,
        api_keys: HashMap<String, String>,
        custom_endpoints: Vec<OpenAiCompatibleEndpoint>,
        on_done: F,
    ) -> Self
    where
        F: Fn(Option<ModelProvider>, HashMap<String, ModelProvider>, Option<ModelProvider>)
            + 'static,
    {
        let builder =
            gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/model_selection_dialog.ui");
        let dialog: adw::Dialog = builder.object("model_selection_dialog").unwrap();
        let default_selector_slot: gtk::Box = builder.object("default_selector_slot").unwrap();
        let broken_default_banner: gtk::Box = builder.object("broken_default_banner").unwrap();
        let broken_default_lbl: gtk::Label = builder.object("broken_default_lbl").unwrap();
        let overrides_section: gtk::Box = builder.object("overrides_section").unwrap();
        let overrides_locked_lbl: gtk::Label = builder.object("overrides_locked_lbl").unwrap();
        let overrides_split: gtk::Box = builder.object("overrides_split").unwrap();
        let character_list: gtk::ListBox = builder.object("character_list").unwrap();
        let character_detail_card: gtk::Box = builder.object("character_detail_card").unwrap();
        let detail_avatar: adw::Avatar = builder.object("detail_avatar").unwrap();
        let detail_name_lbl: gtk::Label = builder.object("detail_name_lbl").unwrap();
        let detail_duties_lbl: gtk::Label = builder.object("detail_duties_lbl").unwrap();
        let use_default_switch: gtk::Switch = builder.object("use_default_switch").unwrap();
        let char_selector_slot: gtk::Box = builder.object("char_selector_slot").unwrap();
        let detail_footer_lbl: gtk::Label = builder.object("detail_footer_lbl").unwrap();
        let dreams_use_default_switch: gtk::Switch =
            builder.object("dreams_use_default_switch").unwrap();
        let dreams_selector_slot: gtk::Box = builder.object("dreams_selector_slot").unwrap();
        let apis_link_lbl: gtk::Label = builder.object("apis_link_lbl").unwrap();
        let reset_overrides_btn: gtk::Button = builder.object("reset_overrides_btn").unwrap();
        let done_btn: gtk::Button = builder.object("done_btn").unwrap();

        apis_link_lbl.set_cursor_from_name(Some("pointer"));

        let shared_ollama = OllamaTagsCache::new(ollama_url.clone());
        let draft_overrides: Rc<RefCell<HashMap<String, ModelProvider>>> =
            Rc::new(RefCell::new(config.character_models));
        let selected_char_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let syncing_detail = Rc::new(Cell::new(false));
        let syncing_dreams = Rc::new(Cell::new(false));
        let avatar_textures: Rc<RefCell<HashMap<PathBuf, gdk::Texture>>> =
            Rc::new(RefCell::new(HashMap::new()));

        let initial_broken_notice = config.broken_default_notice;
        if let Some(ref msg) = initial_broken_notice {
            broken_default_lbl.set_label(msg);
            broken_default_banner.set_visible(true);
        }

        // 1. Mandatory Global Default Selector (3-column Horizontal)
        let default_change_slot: VoidCallbackSlot = Rc::new(RefCell::new(None));
        let default_change_hook = default_change_slot.clone();
        let claw_selector = SingleModelSelector::new_with_options(
            config.default_claw_model.clone(),
            ollama_url.clone(),
            api_keys.clone(),
            custom_endpoints.clone(),
            SelectorOptions {
                layout: SelectorLayout::Horizontal,
                allow_none: true,
                ollama_cache: Some(shared_ollama.clone()),
            },
            move |_| {
                if let Some(cb) = default_change_hook.borrow().as_ref().cloned() {
                    cb();
                }
            },
        );
        default_selector_slot.append(claw_selector.widget());

        // 2. Shared Character Override Selector (Vertical, allow_none = false)
        let char_change_slot: ProviderChangeSlot = Rc::new(RefCell::new(None));
        let char_change_hook = char_change_slot.clone();
        let char_selector = SingleModelSelector::new_with_options(
            config.default_claw_model.clone(),
            ollama_url.clone(),
            api_keys.clone(),
            custom_endpoints.clone(),
            SelectorOptions {
                layout: SelectorLayout::Vertical,
                allow_none: false,
                ollama_cache: Some(shared_ollama.clone()),
            },
            move |mp| {
                if let Some(cb) = char_change_hook.borrow().as_ref().cloned() {
                    cb(mp);
                }
            },
        );
        char_selector_slot.append(char_selector.widget());

        // 3. System-Wide Dreams Selector (Vertical, allow_none = false)
        let dreams_has_custom = config.memory_model.is_some();
        let dreams_seed = config
            .memory_model
            .clone()
            .or_else(|| config.default_claw_model.clone());
        let memory_selector = SingleModelSelector::new_with_options(
            dreams_seed,
            ollama_url,
            api_keys,
            custom_endpoints,
            SelectorOptions {
                layout: SelectorLayout::Vertical,
                allow_none: false,
                ollama_cache: Some(shared_ollama),
            },
            |_| {},
        );
        dreams_selector_slot.append(memory_selector.widget());

        syncing_dreams.set(true);
        dreams_use_default_switch.set_active(!dreams_has_custom);
        memory_selector.set_sensitive(dreams_has_custom);
        syncing_dreams.set(false);

        // Build Character List Rows
        let mut char_rows: Vec<CharacterRowWidgets> = Vec::with_capacity(characters.len());
        if characters.is_empty() {
            overrides_split.set_visible(false);
        } else {
            for target in characters {
                let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                row_box.set_valign(gtk::Align::Center);

                let av = adw::Avatar::new(34, Some(&target.display_name), true);
                av.set_valign(gtk::Align::Center);
                load_avatar_async(&av, target.avatar_path.as_ref(), &avatar_textures);
                row_box.append(&av);

                let name_lbl = gtk::Label::builder()
                    .label(&target.display_name)
                    .halign(gtk::Align::Start)
                    .valign(gtk::Align::Center)
                    .hexpand(true)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build();
                row_box.append(&name_lbl);

                let badge = gtk::Label::builder().valign(gtk::Align::Center).build();
                let badge_state = match draft_overrides.borrow().get(&target.id) {
                    None => OverrideBadgeState::Default,
                    Some(mp) if char_selector.has_provider_named(mp.provider_name()) => {
                        OverrideBadgeState::Custom
                    }
                    Some(_) => OverrideBadgeState::Broken,
                };
                set_badge_state(&badge, badge_state);
                row_box.append(&badge);

                let list_row = gtk::ListBoxRow::new();
                list_row.set_child(Some(&row_box));
                character_list.append(&list_row);

                char_rows.push(CharacterRowWidgets {
                    target,
                    row: list_row,
                    badge,
                });
            }
        }
        let char_rows = Rc::new(char_rows);

        let compute_badge_for = {
            let char_sel = char_selector.clone();
            move |mp_opt: Option<&ModelProvider>| -> OverrideBadgeState {
                match mp_opt {
                    None => OverrideBadgeState::Default,
                    Some(mp) if char_sel.has_provider_named(mp.provider_name()) => {
                        OverrideBadgeState::Custom
                    }
                    Some(_) => OverrideBadgeState::Broken,
                }
            }
        };

        let refresh_all_badges: Rc<dyn Fn()> = {
            let char_rows = char_rows.clone();
            let drafts = draft_overrides.clone();
            let compute_badge_for = compute_badge_for.clone();
            let reset_btn = reset_overrides_btn.clone();
            Rc::new(move || {
                let map = drafts.borrow();
                for cr in char_rows.iter() {
                    let st = compute_badge_for(map.get(&cr.target.id));
                    set_badge_state(&cr.badge, st);
                }
                reset_btn.set_sensitive(!map.is_empty());
            })
        };

        let sync_detail_panel: Rc<dyn Fn()> = {
            let char_rows = char_rows.clone();
            let sel_id = selected_char_id.clone();
            let drafts = draft_overrides.clone();
            let syncing = syncing_detail.clone();
            let claw_sel = claw_selector.clone();
            let char_sel = char_selector.clone();
            let detail_card = character_detail_card;
            let detail_av = detail_avatar;
            let detail_name = detail_name_lbl;
            let detail_duties = detail_duties_lbl;
            let use_def_sw = use_default_switch.clone();
            let footer_lbl = detail_footer_lbl.clone();
            let tex_cache = avatar_textures;

            Rc::new(move || {
                let current_id = sel_id.borrow().clone();
                let Some(id) = current_id else {
                    detail_card.set_sensitive(false);
                    return;
                };
                let Some(cr) = char_rows.iter().find(|r| r.target.id == id) else {
                    detail_card.set_sensitive(false);
                    return;
                };

                syncing.set(true);
                detail_card.set_sensitive(true);
                detail_av.set_text(Some(&cr.target.display_name));
                load_avatar_async(&detail_av, cr.target.avatar_path.as_ref(), &tex_cache);
                detail_name.set_label(&cr.target.display_name);
                detail_duties.set_label(&first_line_duties(&cr.target.duties));

                let custom_opt = drafts.borrow().get(&id).cloned();
                match custom_opt {
                    None => {
                        use_def_sw.set_active(true);
                        char_sel.set_model_provider(claw_sel.get_current_provider());
                        char_sel.set_sensitive(false);
                        footer_lbl.remove_css_class("warning");
                        footer_lbl.set_label("Using the global default model.");
                    }
                    Some(mp) => {
                        use_def_sw.set_active(false);
                        let prov_available = char_sel.has_provider_named(mp.provider_name());
                        char_sel.set_model_provider(Some(mp.clone()));
                        char_sel.set_sensitive(true);
                        if prov_available {
                            footer_lbl.remove_css_class("warning");
                            footer_lbl.set_label(&format!(
                                "Only {} will use these settings.",
                                cr.target.display_name
                            ));
                        } else {
                            footer_lbl.add_css_class("warning");
                            footer_lbl.set_label(&format!(
                                "Previous provider ({}) was removed — pick a new model or enable Use default model.",
                                mp.provider_name()
                            ));
                        }
                    }
                }
                syncing.set(false);
            })
        };

        let sync_global_validation: Rc<dyn Fn()> = {
            let claw_sel = claw_selector.clone();
            let overrides_sec = overrides_section;
            let locked_lbl = overrides_locked_lbl;
            let broken_banner = broken_default_banner;
            let done = done_btn.clone();
            let initial_broken = initial_broken_notice.is_some();
            let sync_detail = sync_detail_panel.clone();
            let dreams_sw = dreams_use_default_switch.clone();
            let mem_sel = memory_selector.clone();
            let syncing_dreams_c = syncing_dreams.clone();

            Rc::new(move || {
                let def_model = claw_sel.get_current_provider();
                let has_default = def_model.is_some();

                overrides_sec.set_sensitive(has_default);
                locked_lbl.set_visible(!has_default && claw_sel.has_providers());
                if has_default {
                    broken_banner.set_visible(false);
                } else if initial_broken {
                    broken_banner.set_visible(true);
                }

                // Mandatory Default rule: cannot click Done without a valid Default model.
                done.set_sensitive(has_default);

                sync_detail();

                if dreams_sw.is_active() {
                    syncing_dreams_c.set(true);
                    mem_sel.set_model_provider(def_model);
                    mem_sel.set_sensitive(false);
                    syncing_dreams_c.set(false);
                }
            })
        };

        *default_change_slot.borrow_mut() = Some(sync_global_validation.clone());

        // Character List Row Selection
        {
            let char_rows_sel = char_rows.clone();
            let sel_id = selected_char_id.clone();
            let sync_detail = sync_detail_panel.clone();
            character_list.connect_row_selected(move |_, opt_row| {
                let next_id = opt_row.and_then(|row| {
                    char_rows_sel
                        .iter()
                        .find(|cr| &cr.row == row)
                        .map(|cr| cr.target.id.clone())
                });
                *sel_id.borrow_mut() = next_id;
                sync_detail();
            });
        }

        // Character "Use default model" Switch
        {
            let syncing = syncing_detail.clone();
            let sel_id = selected_char_id.clone();
            let drafts = draft_overrides.clone();
            let claw_sel = claw_selector.clone();
            let char_sel = char_selector.clone();
            let refresh_badges = refresh_all_badges.clone();
            let sync_detail = sync_detail_panel.clone();

            use_default_switch.connect_active_notify(move |sw| {
                if syncing.get() {
                    return;
                }
                let Some(id) = sel_id.borrow().clone() else {
                    return;
                };
                if sw.is_active() {
                    drafts.borrow_mut().remove(&id);
                } else {
                    let seed = claw_sel
                        .get_current_provider()
                        .or_else(|| char_sel.get_current_provider());
                    char_sel.set_model_provider(seed);
                    if let Some(chosen) = char_sel.get_current_provider() {
                        drafts.borrow_mut().insert(id, chosen);
                    }
                }
                refresh_badges();
                sync_detail();
            });
        }

        // Character Override Dropdown Changes
        // IMPORTANT: Do NOT call `sync_detail()` / `char_sel.set_model_provider()` here!
        // `char_sel` is currently inside `GtkDropDown`'s `notify::selected` handler while its
        // popover list is closing; splicing `model_list` synchronously re-entrantly wedges GTK's grab.
        {
            let syncing = syncing_detail;
            let sel_id = selected_char_id;
            let drafts = draft_overrides.clone();
            let use_def_sw = use_default_switch;
            let refresh_badges = refresh_all_badges.clone();
            let char_rows_lbl = char_rows.clone();
            let footer_lbl = detail_footer_lbl;

            *char_change_slot.borrow_mut() = Some(Rc::new(move |mp_opt| {
                if syncing.get() || use_def_sw.is_active() {
                    return;
                }
                let Some(id) = sel_id.borrow().clone() else {
                    return;
                };
                if let Some(mp) = mp_opt {
                    drafts.borrow_mut().insert(id.clone(), mp);
                    refresh_badges();
                    footer_lbl.remove_css_class("warning");
                    if let Some(cr) = char_rows_lbl.iter().find(|r| r.target.id == id) {
                        footer_lbl.set_label(&format!(
                            "Only {} will use these settings.",
                            cr.target.display_name
                        ));
                    }
                }
            }));
        }

        // Dreams "Use default character model" Switch
        {
            let syncing_d = syncing_dreams;
            let claw_sel = claw_selector.clone();
            let mem_sel = memory_selector.clone();
            dreams_use_default_switch.connect_active_notify(move |sw| {
                if syncing_d.get() {
                    return;
                }
                if sw.is_active() {
                    mem_sel.set_model_provider(claw_sel.get_current_provider());
                    mem_sel.set_sensitive(false);
                } else {
                    if mem_sel.get_current_provider().is_none() {
                        mem_sel.set_model_provider(claw_sel.get_current_provider());
                    }
                    mem_sel.set_sensitive(true);
                }
            });
        }

        // Reset All Overrides Button
        {
            let drafts = draft_overrides.clone();
            let refresh_badges = refresh_all_badges.clone();
            let sync_detail = sync_detail_panel;
            reset_overrides_btn.connect_clicked(move |_| {
                drafts.borrow_mut().clear();
                refresh_badges();
                sync_detail();
            });
        }

        // Pre-select the active pane's character if present, else index 0
        if !char_rows.is_empty() {
            let target_row = config
                .initial_selected_character_id
                .as_deref()
                .and_then(|wanted| char_rows.iter().find(|cr| cr.target.id == wanted))
                .or_else(|| char_rows.first())
                .map(|cr| cr.row.clone());
            character_list.select_row(target_row.as_ref());
        }
        refresh_all_badges();
        sync_global_validation();

        // Escape closes dialog cleanly
        {
            let key_ctrl = gtk::EventControllerKey::new();
            let d_esc = dialog.clone();
            key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
                if keyval == gdk::Key::Escape {
                    d_esc.close();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
            dialog.add_controller(key_ctrl);
        }

        let on_done_rc: DoneCallback = Rc::new(on_done);
        let on_open_apis: VoidCallbackSlot = Rc::new(RefCell::new(None));

        let collect_valid_payload = {
            let claw_sel = claw_selector.clone();
            let mem_sel = memory_selector.clone();
            let dreams_sw = dreams_use_default_switch;
            let drafts = draft_overrides;
            move || -> Option<AssignmentTuple> {
                // Enforce mandatory Default model invariant: never emit overrides without Default.
                let def = claw_sel.get_current_provider()?;
                let mem = if dreams_sw.is_active() {
                    None
                } else {
                    mem_sel.get_current_provider()
                };
                let overrides = drafts.borrow().clone();
                Some((Some(def), overrides, mem))
            }
        };

        {
            let d_clone = dialog.clone();
            let done_cb = on_done_rc.clone();
            let collect = collect_valid_payload.clone();
            done_btn.connect_clicked(move |_| {
                if let Some((def, overrides, mem)) = collect() {
                    done_cb(def, overrides, mem);
                    d_clone.close();
                }
            });
        }

        {
            let gesture = gtk::GestureClick::new();
            let lbl_weak = apis_link_lbl.downgrade();
            let d_clone = dialog.clone();
            let done_cb = on_done_rc;
            let collect = collect_valid_payload;
            let open_apis_cb = on_open_apis.clone();
            gesture.connect_released(move |_, _, _, _| {
                if let Some((def, overrides, mem)) = collect() {
                    done_cb(def, overrides, mem);
                }
                let root_opt = lbl_weak.upgrade().and_then(|l| l.root());
                d_clone.close();
                if let Some(cb) = open_apis_cb.borrow().as_ref().cloned() {
                    cb();
                } else if let Some(root) = root_opt {
                    let _ = gtk::prelude::WidgetExt::activate_action(&root, "win.open-apis", None);
                }
            });
            apis_link_lbl.add_controller(gesture);
        }

        Self {
            dialog,
            claw_selector,
            memory_selector,
            on_open_apis,
        }
    }

    pub fn set_on_open_apis<FA: Fn() + 'static>(&self, cb: FA) {
        *self.on_open_apis.borrow_mut() = Some(Rc::new(cb));
    }

    pub fn dialog(&self) -> &adw::Dialog {
        &self.dialog
    }

    pub fn present(&self, parent: Option<&impl IsA<gtk::Widget>>) {
        self.dialog.present(parent);
    }
}
