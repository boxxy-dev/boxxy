use crate::claw_host::ClawHost;
use crate::msgbar::MsgBarComponent;
use boxxy_claw_protocol::characters::{CharacterInfo, is_incognito_character};
use boxxy_preferences::config::{ModelConfigState, Settings, ThinkingLevel};
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const CARD_WIDTH: i32 = 248;
const PORTRAIT_HEIGHT: i32 = 116;

mod imp {
    #[allow(clippy::wildcard_imports)]
    use super::*;

    pub struct PortraitBanner {
        pub(super) banner_w: Cell<i32>,
        pub(super) banner_h: Cell<i32>,
    }

    impl Default for PortraitBanner {
        fn default() -> Self {
            Self {
                banner_w: Cell::new(CARD_WIDTH),
                banner_h: Cell::new(PORTRAIT_HEIGHT),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PortraitBanner {
        const NAME: &'static str = "BoxxyClawPortraitBanner";
        type Type = super::PortraitBanner;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for PortraitBanner {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for PortraitBanner {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            // ConstantSize prevents GtkPicture from bubbling its 256x256 intrinsic
            // texture height up to the card, preserving the wide banner aspect ratio.
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let w = self.banner_w.get().max(120);
            let h = self.banner_h.get().max(64);
            if let Some(child) = self.obj().first_child() {
                match orientation {
                    gtk::Orientation::Horizontal => {
                        let _ = child.measure(gtk::Orientation::Horizontal, -1);
                    }
                    gtk::Orientation::Vertical => {
                        let _ = child.measure(gtk::Orientation::Vertical, w);
                    }
                    _ => {}
                }
            }
            match orientation {
                gtk::Orientation::Vertical => (h, h, -1, -1),
                _ => (w, w, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                let _ = child.measure(gtk::Orientation::Vertical, width);
                child.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct PortraitBanner(ObjectSubclass<imp::PortraitBanner>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PortraitBanner {
    fn new(w: i32, h: i32, child: &impl IsA<gtk::Widget>) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().banner_w.set(w);
        obj.imp().banner_h.set(h);
        obj.set_overflow(gtk::Overflow::Hidden);
        child.set_parent(&obj);
        obj
    }
}

#[derive(Clone, PartialEq, Eq)]
struct CardStamp {
    id: String,
    full_name: String,
    duties: String,
    color: String,
    has_avatar: bool,
    is_in_use: bool,
    has_custom_model: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct CardModelSummary {
    model_text: String,
    thinking_level: Option<ThinkingLevel>,
    is_warn: bool,
}

struct CardSlot {
    id: String,
    btn: gtk::Button,
    check_badge: gtk::Image,
    model_lbl: gtk::Label,
    thinking_lbl: gtk::Label,
}

#[derive(Clone)]
pub(crate) struct CharacterPicker {
    scroll: gtk::ScrolledWindow,
    cards_wrap: adw::WrapBox,
    models_link_lbl: gtk::Label,
    cards: Rc<RefCell<Vec<CardSlot>>>,
    last_stamp: Rc<RefCell<Vec<CardStamp>>>,
}

impl CharacterPicker {
    pub(crate) fn from_builder(
        builder: &gtk::Builder,
        host: &Rc<dyn ClawHost>,
        selected_character: &Rc<RefCell<String>>,
    ) -> Self {
        let scroll: gtk::ScrolledWindow = builder.object("character_picker_scroll").unwrap();
        let cards_wrap: adw::WrapBox = builder.object("character_cards_wrap").unwrap();
        let models_link_lbl: gtk::Label = builder.object("picker_models_link_lbl").unwrap();
        let characters_link_lbl: gtk::Label = builder.object("picker_characters_link_lbl").unwrap();

        models_link_lbl.set_cursor_from_name(Some("pointer"));
        let click = gtk::GestureClick::new();
        let host_for_models = host.clone();
        let selected_for_models = selected_character.clone();
        click.connect_released(move |_, _, _, _| {
            let current = selected_for_models.borrow().clone();
            let target =
                (!current.is_empty() && !is_incognito_character(&current)).then_some(current);
            host_for_models.open_model_selection(target);
        });
        models_link_lbl.add_controller(click);

        characters_link_lbl.set_cursor_from_name(Some("pointer"));
        let chars_click = gtk::GestureClick::new();
        let chars_lbl_weak = characters_link_lbl.downgrade();
        chars_click.connect_released(move |_, _, x, y| {
            if let Some(w) = chars_lbl_weak.upgrade()
                && w.contains(x, y)
                && let Some(root) = w.root()
            {
                let _ = gtk::prelude::WidgetExt::activate_action(
                    &root,
                    "win.open-preferences-page",
                    Some(&"characters".to_variant()),
                );
            }
        });
        characters_link_lbl.add_controller(chars_click);

        Self {
            scroll,
            cards_wrap,
            models_link_lbl,
            cards: Rc::new(RefCell::new(Vec::new())),
            last_stamp: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        self.scroll.set_visible(visible);
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.scroll.is_visible()
    }

    pub(crate) fn set_max_content_height(&self, height: i32) {
        self.scroll.set_max_content_height(height);
    }

    pub(crate) fn refresh<F: Fn() + 'static>(
        &self,
        host_id: &str,
        selected_character: &Rc<RefCell<String>>,
        msg_bar: &Rc<MsgBarComponent>,
        on_selected_focus: F,
    ) {
        if !self.scroll.is_visible() {
            return;
        }

        let (registry, claims, _) = boxxy_claw_protocol::characters::load_coherent_registry();
        let settings = Settings::load();

        let is_taken = |id: &str| -> bool {
            claims
                .iter()
                .any(|claim| claim.character_id == id && claim.holder_id != host_id)
        };

        // Ensure `selected_character` points to an available character when possible.
        {
            let mut pending = selected_character.borrow_mut();
            let exists_in_catalog = registry.iter().any(|c| c.config.id == *pending);
            if pending.is_empty() || !exists_in_catalog || is_taken(&pending) {
                if let Some(info) = boxxy_claw_protocol::characters::find_first_available_in(
                    &registry,
                    &claims,
                    Some(host_id),
                ) {
                    pending.clone_from(&info.config.id);
                } else if (pending.is_empty() || !exists_in_catalog)
                    && let Some(first) = registry.first()
                {
                    pending.clone_from(&first.config.id);
                }
            }
        }

        let pending = selected_character.borrow().clone();
        if !pending.is_empty() {
            *msg_bar.character_id.borrow_mut() = Some(pending.clone());
        }

        let next_stamp: Vec<CardStamp> = registry
            .iter()
            .filter(|info| !is_incognito_character(&info.config.id))
            .map(|info| CardStamp {
                id: info.config.id.clone(),
                full_name: full_character_title(info),
                duties: first_line_duties(&info.config.duties),
                color: info.config.color.clone(),
                has_avatar: info.has_avatar,
                is_in_use: is_taken(&info.config.id),
                has_custom_model: settings.has_custom_character_model(&info.config.id),
            })
            .collect();

        let structure_changed = *self.last_stamp.borrow() != next_stamp;
        if structure_changed {
            *self.last_stamp.borrow_mut() = next_stamp;
            let focus_cb: Rc<dyn Fn()> = Rc::new(on_selected_focus);
            self.rebuild_cards(
                &registry,
                &settings,
                &pending,
                is_taken,
                selected_character,
                msg_bar,
                &focus_cb,
            );
        } else {
            self.sync_cards_in_place(&settings, &pending);
        }

        self.sync_models_link(&settings, &pending);
    }

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    fn rebuild_cards<F>(
        &self,
        registry: &[CharacterInfo],
        settings: &Settings,
        pending: &str,
        is_taken: F,
        selected_character: &Rc<RefCell<String>>,
        msg_bar: &Rc<MsgBarComponent>,
        on_selected_focus: &Rc<dyn Fn()>,
    ) where
        F: Fn(&str) -> bool,
    {
        while let Some(child) = self.cards_wrap.first_child() {
            self.cards_wrap.remove(&child);
        }

        let mut built_slots = Vec::with_capacity(registry.len());

        for info in registry {
            if is_incognito_character(&info.config.id) {
                continue;
            }

            let full_name = full_character_title(info);
            let role_text = first_line_duties(&info.config.duties);
            let is_current = info.config.id == pending;
            let is_in_use = is_taken(&info.config.id);
            let has_custom = settings.has_custom_character_model(&info.config.id);

            let btn = gtk::Button::new();
            btn.set_width_request(CARD_WIDTH);
            btn.set_valign(gtk::Align::Fill);
            btn.set_overflow(gtk::Overflow::Hidden);
            // Keep keyboard focus on the message bar during mouse clicks so
            // `.boxxy-msgbar:focus-within` never flickers off/on between press and release.
            btn.set_focus_on_click(false);
            btn.add_css_class("character-btn");
            btn.add_css_class("claw-char-card");

            if let Some(color_cls) = boxxy_claw_ui::character_btn_color_class(&info.config.color) {
                btn.add_css_class(&color_cls);
            }
            if is_in_use {
                btn.set_sensitive(false);
                btn.add_css_class("in-use");
                btn.set_tooltip_text(Some("Active in another terminal pane"));
            }
            if is_current {
                btn.add_css_class("selected-character");
            }

            let card_vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
            card_vbox.set_vexpand(true);

            // Wide landscape portrait banner (248x116 ConstantSize so 256x256 avatars never square-inflate)
            let portrait_overlay = gtk::Overlay::new();
            portrait_overlay.set_overflow(gtk::Overflow::Hidden);
            portrait_overlay.add_css_class("claw-char-portrait");

            if let Some(texture) = boxxy_claw_ui::load_character_avatar_texture(info) {
                let pic = gtk::Picture::for_paintable(&texture);
                pic.set_content_fit(gtk::ContentFit::Cover);
                pic.set_can_shrink(true);
                pic.set_hexpand(true);
                pic.set_vexpand(true);
                portrait_overlay.set_child(Some(&pic));
            } else {
                let fallback_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
                fallback_box.set_hexpand(true);
                fallback_box.set_vexpand(true);
                fallback_box.set_halign(gtk::Align::Fill);
                fallback_box.set_valign(gtk::Align::Fill);
                fallback_box.add_css_class("claw-char-portrait-fallback");

                let av = adw::Avatar::new(56, Some(&full_name), true);
                av.set_halign(gtk::Align::Center);
                av.set_valign(gtk::Align::Center);
                av.set_vexpand(true);
                fallback_box.append(&av);
                portrait_overlay.set_child(Some(&fallback_box));
            }

            // Selected checkmark badge (direct GtkImage centers the 14px SVG inside its 24x24 circle)
            let check_badge = gtk::Image::from_icon_name("boxxy-object-select-symbolic");
            check_badge.set_pixel_size(14);
            check_badge.set_halign(gtk::Align::End);
            check_badge.set_valign(gtk::Align::Start);
            check_badge.set_margin_top(8);
            check_badge.set_margin_end(8);
            check_badge.set_visible(is_current && !is_in_use);
            check_badge.set_can_target(false);
            check_badge.add_css_class("claw-char-check-badge");
            portrait_overlay.add_overlay(&check_badge);

            if is_in_use {
                let inuse_lbl = gtk::Label::new(Some("In use"));
                inuse_lbl.set_halign(gtk::Align::End);
                inuse_lbl.set_valign(gtk::Align::Start);
                inuse_lbl.set_margin_top(8);
                inuse_lbl.set_margin_end(8);
                inuse_lbl.set_can_target(false);
                inuse_lbl.add_css_class("claw-char-inuse-badge");
                portrait_overlay.add_overlay(&inuse_lbl);
            }

            if has_custom {
                let custom_lbl = gtk::Label::new(Some("Custom"));
                custom_lbl.set_halign(gtk::Align::Start);
                custom_lbl.set_valign(gtk::Align::End);
                custom_lbl.set_margin_bottom(7);
                custom_lbl.set_margin_start(8);
                custom_lbl.set_can_target(false);
                custom_lbl.add_css_class("claw-char-custom-pill");
                portrait_overlay.add_overlay(&custom_lbl);
            }

            let banner = PortraitBanner::new(CARD_WIDTH, PORTRAIT_HEIGHT, &portrait_overlay);
            card_vbox.append(&banner);

            // Lower text body
            let body_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
            body_box.set_margin_top(10);
            body_box.set_margin_bottom(10);
            body_box.set_margin_start(10);
            body_box.set_margin_end(10);
            body_box.set_vexpand(true);

            let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            name_row.set_halign(gtk::Align::Center);
            name_row.set_valign(gtk::Align::Center);

            let dot_lbl = gtk::Label::new(Some("●"));
            dot_lbl.set_valign(gtk::Align::Center);
            dot_lbl.add_css_class("claw-char-color-dot");
            name_row.append(&dot_lbl);

            let name_lbl = gtk::Label::builder()
                .label(&full_name)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(22)
                .valign(gtk::Align::Center)
                .css_classes(["claw-char-card-name"])
                .build();
            name_row.append(&name_lbl);
            body_box.append(&name_row);

            let role_lbl = gtk::Label::builder()
                .label(&role_text)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .lines(2)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .justify(gtk::Justification::Center)
                .max_width_chars(28)
                .vexpand(true)
                .valign(gtk::Align::Start)
                .css_classes(["claw-char-card-role"])
                .build();
            body_box.append(&role_lbl);

            // Model + Thinking level capsule row
            let model_row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            model_row.set_halign(gtk::Align::Center);
            model_row.set_valign(gtk::Align::Center);
            model_row.set_margin_top(3);

            let summary = resolve_card_model_summary(settings, &info.config.id);
            let model_lbl = gtk::Label::builder()
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(22)
                .valign(gtk::Align::Center)
                .css_classes(["claw-char-card-model"])
                .build();
            let thinking_lbl = gtk::Label::builder()
                .valign(gtk::Align::Center)
                .css_classes(["claw-char-card-thinking"])
                .build();
            apply_card_model_widgets(&model_lbl, &thinking_lbl, &summary);

            model_row.append(&model_lbl);
            model_row.append(&thinking_lbl);
            body_box.append(&model_row);

            card_vbox.append(&body_box);
            btn.set_child(Some(&card_vbox));

            let picker_for_click = self.clone();
            let selected_rc = selected_character.clone();
            let msg_bar_for_click = msg_bar.clone();
            let focus_cb = on_selected_focus.clone();
            let char_id = info.config.id.clone();
            btn.connect_clicked(move |_| {
                selected_rc.borrow_mut().clone_from(&char_id);
                *msg_bar_for_click.character_id.borrow_mut() = Some(char_id.clone());
                let live_settings = Settings::load();
                picker_for_click.sync_cards_in_place(&live_settings, &char_id);
                picker_for_click.sync_models_link(&live_settings, &char_id);
                focus_cb();
            });

            self.cards_wrap.append(&btn);
            built_slots.push(CardSlot {
                id: info.config.id.clone(),
                btn,
                check_badge,
                model_lbl,
                thinking_lbl,
            });
        }

        *self.cards.borrow_mut() = built_slots;
    }

    fn sync_cards_in_place(&self, settings: &Settings, pending: &str) {
        for slot in self.cards.borrow().iter() {
            let is_selected = slot.id == pending;
            let is_in_use = slot.btn.has_css_class("in-use");
            if is_selected {
                slot.btn.add_css_class("selected-character");
            } else {
                slot.btn.remove_css_class("selected-character");
            }
            slot.check_badge.set_visible(is_selected && !is_in_use);

            let summary = resolve_card_model_summary(settings, &slot.id);
            apply_card_model_widgets(&slot.model_lbl, &slot.thinking_lbl, &summary);
        }
    }

    fn sync_models_link(&self, settings: &Settings, pending: &str) {
        let effective = settings.resolve_character_model(pending);
        let state = settings.resolve_model_state(&effective);
        let broken_default = settings.default_model_broken_reason();

        if !matches!(state, ModelConfigState::Ready(_)) || broken_default.is_some() {
            self.models_link_lbl.remove_css_class("accent");
            self.models_link_lbl.add_css_class("warning");
            if matches!(state, ModelConfigState::Unconfigured) {
                self.models_link_lbl
                    .set_label("Models Selection · Set up model");
            } else {
                self.models_link_lbl.set_label("Models Selection");
            }
            if let Some(reason) = broken_default {
                self.models_link_lbl.set_tooltip_text(Some(&format!(
                    "Default model issue: {reason} — Click to configure models"
                )));
            } else {
                self.models_link_lbl
                    .set_tooltip_text(Some("Configure default and character-specific AI models"));
            }
        } else {
            self.models_link_lbl.remove_css_class("warning");
            self.models_link_lbl.add_css_class("accent");
            self.models_link_lbl.set_label("Models Selection");
            self.models_link_lbl
                .set_tooltip_text(Some("Configure default and character-specific AI models"));
        }
    }
}

fn apply_card_model_widgets(
    model_lbl: &gtk::Label,
    thinking_lbl: &gtk::Label,
    summary: &CardModelSummary,
) {
    model_lbl.set_label(&summary.model_text);
    if summary.is_warn {
        model_lbl.add_css_class("warning");
    } else {
        model_lbl.remove_css_class("warning");
    }

    if let Some(level) = summary.thinking_level {
        thinking_lbl.set_visible(true);
        thinking_lbl.set_label(level.short_label());
        if level == ThinkingLevel::None {
            thinking_lbl.add_css_class("thinking-off");
            thinking_lbl.remove_css_class("thinking-active");
        } else {
            thinking_lbl.remove_css_class("thinking-off");
            thinking_lbl.add_css_class("thinking-active");
        }
    } else {
        thinking_lbl.set_visible(false);
    }
}

/// Resolves the full human-readable character name for cards and headers.
///
/// Prefers the prettified directory slug when `display_name` is a short first-name prefix
/// (e.g. `name = "niko-una"`, `display_name = "niko"` -> `"Niko Una"`), while respecting
/// custom multi-word or distinct `display_name` overrides.
#[must_use]
fn full_character_title(info: &CharacterInfo) -> String {
    let from_slug = boxxy_claw_ui::prettify_character_name(&info.config.name);
    let from_disp = boxxy_claw_ui::prettify_character_name(&info.config.display_name);
    let is_slug_prefix = from_slug.len() > from_disp.len()
        && from_slug
            .get(..from_disp.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&from_disp))
        && from_slug.as_bytes().get(from_disp.len()) == Some(&b' ');
    if from_disp.is_empty() || is_slug_prefix {
        from_slug
    } else {
        from_disp
    }
}

#[must_use]
fn first_line_duties(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Boxxy Claw Character")
        .to_string()
}

#[must_use]
fn resolve_card_model_summary(settings: &Settings, character_id: &str) -> CardModelSummary {
    let effective = settings.resolve_character_model(character_id);
    match settings.resolve_model_state(&effective) {
        ModelConfigState::Ready(mp) => CardModelSummary {
            model_text: mp.display_model_name(),
            thinking_level: mp.effective_thinking_level(),
            is_warn: false,
        },
        ModelConfigState::Unconfigured => CardModelSummary {
            model_text: "No model set".to_string(),
            thinking_level: None,
            is_warn: true,
        },
        ModelConfigState::MissingApiKey { provider_name } => CardModelSummary {
            model_text: if provider_name == "ChatGPT" {
                "Not signed in".to_string()
            } else {
                "Missing API key".to_string()
            },
            thinking_level: None,
            is_warn: true,
        },
        ModelConfigState::ProviderUnavailable { .. } => CardModelSummary {
            model_text: "Unavailable".to_string(),
            thinking_level: None,
            is_warn: true,
        },
        ModelConfigState::OllamaOffline => CardModelSummary {
            model_text: "Ollama offline".to_string(),
            thinking_level: None,
            is_warn: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_claw_protocol::characters::CharacterConfig;

    fn sample_character(
        slug: &str,
        display: &str,
        duties: &str,
        personality: &str,
    ) -> CharacterInfo {
        CharacterInfo::new(
            CharacterConfig {
                id: "00000000-0000-4000-8000-000000000099".to_string(),
                name: slug.to_string(),
                display_name: display.to_string(),
                color: "#7B61FF".to_string(),
                duties: duties.to_string(),
                personality: personality.to_string(),
            },
            false,
        )
    }

    #[test]
    fn full_character_title_expands_first_name_slug_prefixes_and_preserves_custom_names() {
        let expanded = sample_character("alpha-prime", "alpha", "Kernel Helper", "");
        assert_eq!(full_character_title(&expanded), "Alpha Prime");

        let single = sample_character("beta", "beta", "Build Guard", "");
        assert_eq!(full_character_title(&single), "Beta");

        let custom = sample_character("slot-one", "Captain Nova", "Ops Lead", "");
        assert_eq!(full_character_title(&custom), "Captain Nova");

        let partial_word = sample_character("alphabet", "alpha", "Parser", "");
        assert_eq!(full_character_title(&partial_word), "Alpha");
    }

    #[test]
    fn first_line_duties_trims_and_provides_fallback() {
        assert_eq!(
            first_line_duties("\n  Systems Architect  \nSecondary line"),
            "Systems Architect"
        );
        assert_eq!(first_line_duties("   \n  "), "Boxxy Claw Character");
    }
}
