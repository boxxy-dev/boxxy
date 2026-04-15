use crate::config::Settings;
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

pub fn setup_agents_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
    let claw_on_by_default_switch: adw::SwitchRow =
        builder.object("claw_on_by_default_switch").unwrap();
    let web_search_by_default_switch: adw::SwitchRow =
        builder.object("web_search_by_default_switch").unwrap();
    let proactive_by_default_switch: adw::SwitchRow =
        builder.object("proactive_by_default_switch").unwrap();
    let hide_agent_badge_switch: adw::SwitchRow =
        builder.object("hide_agent_badge_switch").unwrap();
    let enable_file_tools_switch: adw::SwitchRow =
        builder.object("enable_file_tools_switch").unwrap();
    let enable_system_tools_switch: adw::SwitchRow =
        builder.object("enable_system_tools_switch").unwrap();
    let enable_dangerous_tools_switch: adw::SwitchRow =
        builder.object("enable_dangerous_tools_switch").unwrap();
    let enable_web_tools_switch: adw::SwitchRow =
        builder.object("enable_web_tools_switch").unwrap();
    let enable_web_search_switch: adw::SwitchRow =
        builder.object("enable_web_search_switch").unwrap();
    let enable_os_context_switch: adw::SwitchRow =
        builder.object("enable_os_context_switch").unwrap();
    let enable_clipboard_tools_switch: adw::SwitchRow =
        builder.object("enable_clipboard_tools_switch").unwrap();
    let enable_auto_dreaming_switch: adw::SwitchRow =
        builder.object("enable_auto_dreaming_switch").unwrap();

    let group_agent_general: adw::PreferencesGroup = builder.object("group_agent_general").unwrap();
    let group_agent_dreaming: adw::PreferencesGroup = builder.object("group_agent_dreaming").unwrap();
    let group_agent_toolbox: adw::PreferencesGroup = builder.object("group_agent_toolbox").unwrap();

    // Initialize values
    claw_on_by_default_switch.set_active(settings_rc.borrow().claw_on_by_default);
    web_search_by_default_switch.set_active(settings_rc.borrow().web_search_on_by_default);
    proactive_by_default_switch.set_active(
        settings_rc.borrow().claw_auto_diagnosis_mode
            == crate::config::ClawAutoDiagnosisMode::Proactive,
    );
    hide_agent_badge_switch.set_active(settings_rc.borrow().hide_agent_badge);
    enable_auto_dreaming_switch.set_active(settings_rc.borrow().enable_auto_dreaming);
    enable_file_tools_switch.set_active(settings_rc.borrow().enable_file_tools);
    enable_system_tools_switch.set_active(settings_rc.borrow().enable_system_tools);
    enable_dangerous_tools_switch.set_active(settings_rc.borrow().enable_dangerous_tools);
    enable_web_tools_switch.set_active(settings_rc.borrow().enable_web_tools);
    enable_web_search_switch.set_active(settings_rc.borrow().enable_web_search);
    enable_os_context_switch.set_active(settings_rc.borrow().enable_os_context);
    enable_clipboard_tools_switch.set_active(settings_rc.borrow().enable_clipboard_tools);

    // Connect signals
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    claw_on_by_default_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.claw_on_by_default != row.is_active() {
            s.claw_on_by_default = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    web_search_by_default_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.web_search_on_by_default != row.is_active() {
            s.web_search_on_by_default = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    proactive_by_default_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        let target_mode = if row.is_active() {
            crate::config::ClawAutoDiagnosisMode::Proactive
        } else {
            crate::config::ClawAutoDiagnosisMode::Lazy
        };
        if s.claw_auto_diagnosis_mode != target_mode {
            s.claw_auto_diagnosis_mode = target_mode;
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    hide_agent_badge_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.hide_agent_badge != row.is_active() {
            s.hide_agent_badge = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_auto_dreaming_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_auto_dreaming != row.is_active() {
            s.enable_auto_dreaming = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_file_tools_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_file_tools != row.is_active() {
            s.enable_file_tools = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_system_tools_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_system_tools != row.is_active() {
            s.enable_system_tools = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_dangerous_tools_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_dangerous_tools != row.is_active() {
            s.enable_dangerous_tools = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_web_tools_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_web_tools != row.is_active() {
            s.enable_web_tools = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_web_search_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_web_search != row.is_active() {
            s.enable_web_search = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_os_context_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_os_context != row.is_active() {
            s.enable_os_context = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_clipboard_tools_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_clipboard_tools != row.is_active() {
            s.enable_clipboard_tools = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let claw_on_by_default_switch_clone = claw_on_by_default_switch.clone();
    let web_search_by_default_switch_clone = web_search_by_default_switch.clone();
    let proactive_by_default_switch_clone = proactive_by_default_switch.clone();
    let hide_agent_badge_switch_clone = hide_agent_badge_switch.clone();
    let enable_file_tools_switch_clone = enable_file_tools_switch.clone();
    let enable_system_tools_switch_clone = enable_system_tools_switch.clone();
    let enable_dangerous_tools_switch_clone = enable_dangerous_tools_switch.clone();
    let enable_web_tools_switch_clone = enable_web_tools_switch.clone();
    let enable_os_context_switch_clone = enable_os_context_switch.clone();
    let enable_clipboard_tools_switch_clone = enable_clipboard_tools_switch.clone();
    let enable_auto_dreaming_switch_clone = enable_auto_dreaming_switch.clone();

    Box::new(move |query: &str| {
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let ag1 = match_row(
            claw_on_by_default_switch_clone.upcast_ref(),
            "boxxyclaw on by default start automatically new terminal",
        );
        let ag_web_default = match_row(
            web_search_by_default_switch_clone.upcast_ref(),
            "web search on by default allowed automatically",
        );
        let ag_proactive = match_row(
            proactive_by_default_switch_clone.upcast_ref(),
            "proactive mode by default start boxxyclaw background analysis lazy",
        );
        let ag2 = match_row(
            hide_agent_badge_switch_clone.upcast_ref(),
            "hide agent identity badge top right corner",
        );
        let ag_dream = match_row(
            enable_auto_dreaming_switch_clone.upcast_ref(),
            "enable auto-dreaming memory consolidation durable facts patterns",
        );
        let ag3 = match_row(
            enable_file_tools_switch_clone.upcast_ref(),
            "enable file tools read write list delete search",
        );
        let ag4 = match_row(
            enable_system_tools_switch_clone.upcast_ref(),
            "enable system tools monitoring list processes",
        );
        let ag5 = match_row(
            enable_dangerous_tools_switch_clone.upcast_ref(),
            "enable dangerous tools terminate kill processes",
        );
        let ag6 = match_row(
            enable_web_tools_switch_clone.upcast_ref(),
            "enable web tools fetch content documentation",
        );
        let ag_search = match_row(
            enable_web_search_switch.upcast_ref(),
            "enable web search tools tavily providers",
        );
        let ag_os = match_row(
            enable_os_context_switch_clone.upcast_ref(),
            "enable location and time context injection environment",
        );
        let ag7 = match_row(
            enable_clipboard_tools_switch_clone.upcast_ref(),
            "enable clipboard tools read write copy paste",
        );

        group_agent_general.set_visible(ag1 || ag_web_default || ag_proactive || ag2);
        group_agent_dreaming.set_visible(ag_dream);
        group_agent_toolbox.set_visible(ag3 || ag4 || ag5 || ag6 || ag_search || ag_os || ag7);

        group_agent_general.is_visible() || group_agent_dreaming.is_visible() || group_agent_toolbox.is_visible()
    })
}
