use crate::state::AppWindowInner;
use boxxy_broadcast::{BroadcastEvent, StoredBroadcastArticle, SyncTrigger};
use boxxy_viewer::StructuredViewer;
use chrono::{DateTime, Utc};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const DEFAULT_HERO_SUBTITLE: &str =
    "Release highlights, feature announcements, and community updates.";
const OFFLINE_HERO_SUBTITLE: &str = "Offline — showing saved news.";
const PUBLIC_NEWS_ARCHIVE_URL: &str = "https://boxxy.dev/news/";

const CATEGORY_STYLE_CLASSES: [&str; 4] = [
    "is-feature",
    "is-community",
    "is-security",
    "is-maintenance",
];

fn category_style_class(category: &str) -> Option<&'static str> {
    match category.trim().to_ascii_lowercase().as_str() {
        "feature" => Some("is-feature"),
        "community" => Some("is-community"),
        "security" => Some("is-security"),
        "maintenance" => Some("is-maintenance"),
        _ => None,
    }
}

fn apply_category_badge(label: &gtk::Label, category: &str) {
    for cls in CATEGORY_STYLE_CLASSES {
        label.remove_css_class(cls);
    }
    if let Some(cls) = category_style_class(category) {
        label.add_css_class(cls);
    }
    let display = category.trim();
    label.set_label(&if display.is_empty() {
        "NEWS".to_string()
    } else {
        display.to_ascii_uppercase()
    });
}

fn format_relative_date(rfc3339: &str) -> String {
    let Ok(dt) = DateTime::parse_from_rfc3339(rfc3339.trim()) else {
        return String::new();
    };
    let utc_dt = dt.with_timezone(&Utc);
    let secs = Utc::now().signed_duration_since(utc_dt).num_seconds();
    if secs < 0 {
        dt.format("%b %-d").to_string()
    } else if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 86_400 * 14 {
        let days = secs / 86_400;
        if days == 1 {
            "yesterday".to_string()
        } else {
            format!("{days}d ago")
        }
    } else {
        dt.format("%b %-d").to_string()
    }
}

fn format_full_date(rfc3339: &str) -> String {
    DateTime::parse_from_rfc3339(rfc3339.trim())
        .map(|dt| dt.format("%B %-d, %Y").to_string())
        .unwrap_or_default()
}

fn launch_https_uri(widget: &impl IsA<gtk::Widget>, url: &str) {
    let trimmed = url.trim();
    if !trimmed.starts_with("https://") {
        return;
    }
    let parent_win = widget
        .root()
        .and_then(|r| r.downcast::<gtk::Window>().ok());
    let launcher = gtk::UriLauncher::new(trimmed);
    gtk::glib::spawn_future_local(async move {
        let _ = launcher.launch_future(parent_win.as_ref()).await;
    });
}

struct NewsDialogState {
    articles: Vec<StoredBroadcastArticle>,
    active_article_id: Option<String>,
}

/// Handle stored on `AppWindowInner` while the `Boxxy News` dialog is open so toast
/// `"Read"` clicks or Command Palette activations focus/navigate the existing dialog.
#[derive(Clone)]
pub struct BoxxyNewsDialogHandle {
    dialog_weak: gtk::glib::WeakRef<adw::Dialog>,
    open_article_fn: Rc<dyn Fn(&str)>,
    show_list_fn: Rc<dyn Fn()>,
}

impl std::fmt::Debug for BoxxyNewsDialogHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxxyNewsDialogHandle")
            .finish_non_exhaustive()
    }
}

impl BoxxyNewsDialogHandle {
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.dialog_weak.upgrade().is_some()
    }

    pub fn navigate_to_list(&self) {
        (self.show_list_fn)();
    }

    pub fn navigate_to_article(&self, article_id: &str) {
        (self.open_article_fn)(article_id);
    }
}

type RebuildCallback = Rc<dyn Fn()>;

pub fn open_broadcasts(inner_ref: &Rc<RefCell<AppWindowInner>>, inner: &mut AppWindowInner) {
    if let Some(ref handle) = inner.open_news_dialog
        && handle.is_alive()
    {
        handle.navigate_to_list();
        return;
    }
    present_news_dialog(inner_ref, inner, None);
}

pub fn open_broadcast_article(
    inner_ref: &Rc<RefCell<AppWindowInner>>,
    inner: &mut AppWindowInner,
    article_id: String,
) {
    super::broadcast::teardown_toast_if_matching(inner, &article_id);

    if let Some(ref handle) = inner.open_news_dialog
        && handle.is_alive()
    {
        handle.navigate_to_article(&article_id);
        return;
    }
    present_news_dialog(inner_ref, inner, Some(article_id));
}

#[allow(clippy::too_many_lines)]
fn present_news_dialog(
    inner_ref: &Rc<RefCell<AppWindowInner>>,
    inner: &mut AppWindowInner,
    initial_article_id: Option<String>,
) {
    let builder = gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/boxxy_news_dialog.ui");
    let Some(dialog) = builder.object::<adw::Dialog>("boxxy_news_dialog") else {
        return;
    };

    let news_stack = builder.object::<gtk::Stack>("news_stack");
    let list_state_stack = builder.object::<gtk::Stack>("list_state_stack");
    let news_list = builder.object::<gtk::Box>("news_list");
    let news_status_label = builder.object::<gtk::Label>("news_status_label");
    let refresh_btn = builder.object::<gtk::Button>("news_refresh_btn");
    let detail_scrolled = builder.object::<gtk::ScrolledWindow>("detail_scrolled");
    let detail_cta_btn = builder.object::<gtk::Button>("detail_cta_btn");
    let detail_cta_label = builder.object::<gtk::Label>("detail_cta_label");
    let detail_open_web_btn = builder.object::<gtk::Button>("detail_open_web_btn");

    let viewer = StructuredViewer::new_remote_safe();
    if let Some(slot) = builder.object::<gtk::Box>("detail_viewer_slot") {
        slot.append(viewer.widget());
    }

    let state_rc = Rc::new(RefCell::new(NewsDialogState {
        articles: Vec::new(),
        active_article_id: initial_article_id.clone(),
    }));
    let pending_initial_open = Rc::new(RefCell::new(initial_article_id));
    let active_cta_url: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let active_web_slug: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let is_refreshing = Rc::new(Cell::new(false));

    let update_offline_caption: Rc<dyn Fn(bool)> = {
        let lbl_opt = news_status_label;
        Rc::new(move |online: bool| {
            if let Some(ref lbl) = lbl_opt {
                lbl.set_label(if online {
                    DEFAULT_HERO_SUBTITLE
                } else {
                    OFFLINE_HERO_SUBTITLE
                });
            }
        })
    };
    update_offline_caption(boxxy_broadcast::last_sync_succeeded());

    let rebuild_rows_holder: Rc<RefCell<Option<RebuildCallback>>> = Rc::new(RefCell::new(None));

    let show_article_detail: Rc<dyn Fn(&str)> = {
        let builder_c = builder.clone();
        let state_c = state_rc.clone();
        let viewer_c = viewer;
        let cta_btn_c = detail_cta_btn.clone();
        let cta_lbl_c = detail_cta_label;
        let cta_url_c = active_cta_url.clone();
        let web_slug_c = active_web_slug.clone();
        let inner_weak = Rc::downgrade(inner_ref);
        let rebuild_holder = rebuild_rows_holder.clone();

        Rc::new(move |target_id: &str| {
            let (idx, article, total) = {
                let mut st = state_c.borrow_mut();
                let Some(pos) = st.articles.iter().position(|a| a.id == target_id) else {
                    return;
                };
                let was_unread = st.articles[pos].is_unread();
                if was_unread {
                    st.articles[pos].disposition = "read".to_string();
                }
                st.active_article_id = Some(target_id.to_string());
                (pos, st.articles[pos].clone(), st.articles.len())
            };

            if let Some(rc) = inner_weak.upgrade()
                && let Ok(mut guard) = rc.try_borrow_mut()
            {
                super::broadcast::teardown_toast_if_matching(&mut guard, &article.id);
            }

            let id_to_mark = article.id.clone();
            gtk::glib::spawn_future_local(async move {
                let _ = boxxy_sys_utils::runtime()
                    .spawn(async move { boxxy_broadcast::mark_article_read(&id_to_mark).await })
                    .await;
            });

            if let Some(ref rebuild) = *rebuild_holder.borrow() {
                rebuild();
            }

            if let Some(cat_lbl) = builder_c.object::<gtk::Label>("detail_category_badge") {
                apply_category_badge(&cat_lbl, &article.category);
            }
            if let Some(date_lbl) = builder_c.object::<gtk::Label>("detail_date_label") {
                date_lbl.set_label(&format_full_date(&article.published_at));
            }
            if let Some(title_lbl) = builder_c.object::<gtk::Label>("detail_title_label") {
                title_lbl.set_label(&article.title);
            }
            if let Some(sum_lbl) = builder_c.object::<gtk::Label>("detail_summary_label") {
                sum_lbl.set_label(&article.summary);
            }

            viewer_c.set_content(&article.body_md);

            *web_slug_c.borrow_mut() = article.slug.clone();

            match (&article.action_label, &article.action_url) {
                (Some(lbl), Some(url)) if url.starts_with("https://") => {
                    *cta_url_c.borrow_mut() = Some(url.clone());
                    if let Some(ref l) = cta_lbl_c {
                        l.set_label(lbl);
                    }
                    if let Some(ref b) = cta_btn_c {
                        b.set_tooltip_text(Some(url));
                        b.set_visible(true);
                    }
                }
                _ => {
                    *cta_url_c.borrow_mut() = None;
                    if let Some(ref b) = cta_btn_c {
                        b.set_visible(false);
                    }
                }
            }

            if let Some(prev_btn) = builder_c.object::<gtk::Button>("detail_prev_btn") {
                prev_btn.set_sensitive(idx > 0);
            }
            if let Some(next_btn) = builder_c.object::<gtk::Button>("detail_next_btn") {
                next_btn.set_sensitive(idx + 1 < total);
            }
            if let Some(ref sc) = builder_c.object::<gtk::ScrolledWindow>("detail_scrolled") {
                sc.vadjustment().set_value(0.0);
            }
            if let Some(stack) = builder_c.object::<gtk::Stack>("news_stack") {
                stack.set_visible_child_name("detail");
            }
        })
    };

    let rebuild_rows: Rc<dyn Fn()> = {
        let list_opt = news_list;
        let list_stack_opt = list_state_stack.clone();
        let state_c = state_rc.clone();
        let open_detail = show_article_detail.clone();

        Rc::new(move || {
            let Some(ref list_box) = list_opt else {
                return;
            };
            while let Some(child) = list_box.first_child() {
                list_box.remove(&child);
            }

            let articles = state_c.borrow().articles.clone();
            if articles.is_empty() {
                if let Some(ref st) = list_stack_opt {
                    st.set_visible_child_name("empty");
                }
                return;
            }

            for article in &articles {
                let row_btn = build_article_row(article, &open_detail);
                list_box.append(&row_btn);
            }

            if let Some(ref st) = list_stack_opt {
                st.set_visible_child_name("content");
            }
        })
    };
    *rebuild_rows_holder.borrow_mut() = Some(rebuild_rows.clone());

    let show_list_page: Rc<dyn Fn()> = {
        let stack_opt = news_stack.clone();
        let state_c = state_rc.clone();
        let detail_sc_opt = detail_scrolled;
        Rc::new(move || {
            state_c.borrow_mut().active_article_id = None;
            if let Some(ref sc) = detail_sc_opt {
                sc.vadjustment().set_value(0.0);
            }
            if let Some(ref stack) = stack_opt {
                stack.set_visible_child_name("list");
            }
        })
    };

    if let Some(back_btn) = builder.object::<gtk::Button>("detail_back_btn") {
        let show_list = show_list_page.clone();
        back_btn.connect_clicked(move |_| show_list());
    }

    // Escape on `detail` returns to `list` rather than dismissing the entire dialog immediately.
    {
        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let stack_weak = news_stack.as_ref().map(ObjectExt::downgrade);
        let show_list = show_list_page.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape
                && let Some(ref sw) = stack_weak
                && let Some(stack) = sw.upgrade()
                && stack.visible_child_name().as_deref() == Some("detail")
            {
                show_list();
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
        dialog.add_controller(key_ctrl);
    }

    if let Some(prev_btn) = builder.object::<gtk::Button>("detail_prev_btn") {
        let state_c = state_rc.clone();
        let show_detail = show_article_detail.clone();
        prev_btn.connect_clicked(move |_| {
            let prev_id = {
                let st = state_c.borrow();
                let cur_id = st.active_article_id.as_deref();
                cur_id
                    .and_then(|id| st.articles.iter().position(|a| a.id == id))
                    .filter(|&idx| idx > 0)
                    .map(|idx| st.articles[idx - 1].id.clone())
            };
            if let Some(id) = prev_id {
                show_detail(&id);
            }
        });
    }

    if let Some(next_btn) = builder.object::<gtk::Button>("detail_next_btn") {
        let state_c = state_rc.clone();
        let show_detail = show_article_detail.clone();
        next_btn.connect_clicked(move |_| {
            let next_id = {
                let st = state_c.borrow();
                let cur_id = st.active_article_id.as_deref();
                cur_id
                    .and_then(|id| st.articles.iter().position(|a| a.id == id))
                    .filter(|&idx| idx + 1 < st.articles.len())
                    .map(|idx| st.articles[idx + 1].id.clone())
            };
            if let Some(id) = next_id {
                show_detail(&id);
            }
        });
    }

    if let Some(open_archive_btn) = builder.object::<gtk::Button>("open_web_archive_btn") {
        open_archive_btn.connect_clicked(|btn| {
            launch_https_uri(btn, PUBLIC_NEWS_ARCHIVE_URL);
        });
    }

    if let Some(ref open_web_btn) = detail_open_web_btn {
        let slug_c = active_web_slug;
        open_web_btn.connect_clicked(move |btn| {
            let slug = slug_c.borrow().clone();
            let url = if slug.is_empty() {
                PUBLIC_NEWS_ARCHIVE_URL.to_string()
            } else {
                format!("{PUBLIC_NEWS_ARCHIVE_URL}#{slug}")
            };
            launch_https_uri(btn, &url);
        });
    }

    if let Some(ref cta_btn) = detail_cta_btn {
        let cta_url_c = active_cta_url;
        cta_btn.connect_clicked(move |btn| {
            let url_opt = cta_url_c.borrow().clone();
            if let Some(url) = url_opt {
                launch_https_uri(btn, &url);
            }
        });
    }

    for close_id in ["close_btn", "detail_bottom_close_btn"] {
        if let Some(btn) = builder.object::<gtk::Button>(close_id) {
            let d_weak = dialog.downgrade();
            btn.connect_clicked(move |_| {
                if let Some(d) = d_weak.upgrade() {
                    d.close();
                }
            });
        }
    }

    let apply_articles: Rc<dyn Fn(Vec<StoredBroadcastArticle>)> = {
        let state_c = state_rc;
        let rebuild = rebuild_rows;
        let open_detail = show_article_detail.clone();
        let pending_init = pending_initial_open;
        let show_list = show_list_page.clone();

        Rc::new(move |articles: Vec<StoredBroadcastArticle>| {
            let active_before = state_c.borrow().active_article_id.clone();
            state_c.borrow_mut().articles = articles;
            rebuild();

            if let Some(init_id) = pending_init.borrow_mut().take() {
                open_detail(&init_id);
            } else if let Some(ref cur_id) = active_before {
                let still_exists = state_c.borrow().articles.iter().any(|a| a.id == *cur_id);
                if still_exists {
                    open_detail(cur_id);
                } else {
                    show_list();
                }
            }
        })
    };

    if let Some(ref btn) = refresh_btn {
        let refreshing_c = is_refreshing.clone();
        let apply_c = apply_articles.clone();
        let update_caption_c = update_offline_caption.clone();
        let list_stack_c = list_state_stack.clone();
        let btn_weak = btn.downgrade();

        btn.connect_clicked(move |_| {
            if refreshing_c.replace(true) {
                return;
            }
            if let Some(b) = btn_weak.upgrade() {
                b.set_sensitive(false);
            }

            let refreshing_done = refreshing_c.clone();
            let apply_done = apply_c.clone();
            let caption_done = update_caption_c.clone();
            let btn_done = btn_weak.clone();
            let stack_done = list_stack_c.clone();

            gtk::glib::spawn_future_local(async move {
                let sync_res = boxxy_sys_utils::runtime()
                    .spawn(async move { boxxy_broadcast::sync_now(SyncTrigger::ManualRefresh).await })
                    .await;

                refreshing_done.set(false);
                if let Some(b) = btn_done.upgrade() {
                    b.set_sensitive(true);
                }

                match sync_res {
                    Ok(Ok(fresh)) => {
                        caption_done(true);
                        apply_done(fresh);
                    }
                    _ => {
                        caption_done(false);
                        if let Ok(Ok(cached)) = boxxy_sys_utils::runtime()
                            .spawn(async move { boxxy_broadcast::list_cached_articles().await })
                            .await
                        {
                            apply_done(cached);
                        } else if let Some(ref st) = stack_done {
                            st.set_visible_child_name("empty");
                        }
                    }
                }
            });
        });
    }

    // Live subscription while dialog is open
    {
        let dialog_weak = dialog.downgrade();
        let apply_live = apply_articles.clone();
        let caption_live = update_offline_caption.clone();
        let mut rx_ev = boxxy_broadcast::subscribe_events();
        gtk::glib::spawn_future_local(async move {
            loop {
                let sync_ok = match rx_ev.recv().await {
                    Ok(BroadcastEvent::Updated { sync_succeeded }) => sync_succeeded,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        boxxy_broadcast::last_sync_succeeded()
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if dialog_weak.upgrade().is_none() {
                    break;
                }
                caption_live(sync_ok);
                if let Ok(Ok(rows)) = boxxy_sys_utils::runtime()
                    .spawn(async move { boxxy_broadcast::list_cached_articles().await })
                    .await
                    && dialog_weak.upgrade().is_some()
                {
                    apply_live(rows);
                }
            }
        });
    }

    if let Some(ref st) = list_state_stack {
        st.set_visible_child_name("loading");
    }

    {
        let inner_weak_close = Rc::downgrade(inner_ref);
        dialog.connect_closed(move |_| {
            if let Some(rc) = inner_weak_close.upgrade()
                && let Ok(mut guard) = rc.try_borrow_mut()
            {
                guard.open_news_dialog = None;
            }
        });
    }

    inner.open_news_dialog = Some(BoxxyNewsDialogHandle {
        dialog_weak: dialog.downgrade(),
        open_article_fn: show_article_detail,
        show_list_fn: show_list_page,
    });

    dialog.present(Some(&inner.window));

    // Step 1: Load local SQLite cache immediately (works 100% offline in ~0ms),
    // Step 2: Trigger background `sync_if_stale()` if last sync was `>= 60s` ago.
    let dialog_weak_init = dialog.downgrade();
    let apply_init = apply_articles;
    let caption_init = update_offline_caption;
    gtk::glib::spawn_future_local(async move {
        let cached_opt = boxxy_sys_utils::runtime()
            .spawn(async move { boxxy_broadcast::list_cached_articles().await.ok() })
            .await
            .ok()
            .flatten();

        if dialog_weak_init.upgrade().is_none() {
            return;
        }

        let had_cached = cached_opt.as_ref().is_some_and(|v| !v.is_empty());
        if let Some(rows) = cached_opt
            && !rows.is_empty()
        {
            apply_init(rows);
        }

        let stale_res = boxxy_sys_utils::runtime()
            .spawn(async move { boxxy_broadcast::sync_if_stale().await })
            .await;

        if dialog_weak_init.upgrade().is_none() {
            return;
        }

        match stale_res {
            Ok(Ok(fresh)) => {
                caption_init(true);
                apply_init(fresh);
            }
            _ => {
                caption_init(false);
                if !had_cached {
                    apply_init(Vec::new());
                }
            }
        }
    });
}

fn build_article_row(
    article: &StoredBroadcastArticle,
    on_select: &Rc<dyn Fn(&str)>,
) -> gtk::Button {
    let row_btn = gtk::Button::new();
    row_btn.add_css_class("flat");
    row_btn.add_css_class("boxxy-news-row");
    row_btn.set_hexpand(true);

    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    outer.set_hexpand(true);
    outer.set_valign(gtk::Align::Center);

    let main_col = gtk::Box::new(gtk::Orientation::Vertical, 5);
    main_col.set_hexpand(true);
    main_col.set_valign(gtk::Align::Center);

    let top_meta = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top_meta.set_valign(gtk::Align::Center);

    if article.is_unread() {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("boxxy-news-unread-dot");
        dot.set_valign(gtk::Align::Center);
        dot.set_tooltip_text(Some("Unread"));
        top_meta.append(&dot);
    }

    let cat_pill = gtk::Label::new(None);
    cat_pill.add_css_class("boxxy-news-cat-pill");
    cat_pill.set_valign(gtk::Align::Center);
    apply_category_badge(&cat_pill, &article.category);
    top_meta.append(&cat_pill);

    let date_lbl = gtk::Label::new(Some(&format_relative_date(&article.published_at)));
    date_lbl.add_css_class("dim-label");
    date_lbl.add_css_class("caption");
    date_lbl.add_css_class("monospace");
    date_lbl.set_halign(gtk::Align::End);
    date_lbl.set_hexpand(true);
    top_meta.append(&date_lbl);

    let title_lbl = gtk::Label::new(Some(&article.title));
    title_lbl.add_css_class("boxxy-news-row-title");
    title_lbl.set_xalign(0.0);
    title_lbl.set_halign(gtk::Align::Fill);
    title_lbl.set_hexpand(true);
    title_lbl.set_single_line_mode(true);
    title_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let summary_lbl = gtk::Label::new(Some(&article.summary));
    summary_lbl.add_css_class("boxxy-news-row-summary");
    summary_lbl.set_xalign(0.0);
    summary_lbl.set_halign(gtk::Align::Fill);
    summary_lbl.set_hexpand(true);
    summary_lbl.set_wrap(true);
    summary_lbl.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    summary_lbl.set_lines(2);
    summary_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);

    main_col.append(&top_meta);
    main_col.append(&title_lbl);
    main_col.append(&summary_lbl);

    let chevron = gtk::Image::from_icon_name("boxxy-right-symbolic");
    chevron.add_css_class("boxxy-news-chevron");
    chevron.set_pixel_size(14);
    chevron.set_valign(gtk::Align::Center);

    outer.append(&main_col);
    outer.append(&chevron);
    row_btn.set_child(Some(&outer));

    let id_click = article.id.clone();
    let cb = on_select.clone();
    row_btn.connect_clicked(move |_| {
        cb(&id_click);
    });

    row_btn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gtk4::test]
    fn boxxy_news_dialog_ui_parses_with_list_and_detail_pages() {
        let _ = adw::init();

        let builder = gtk::Builder::from_string(include_str!(
            "../../../../resources/ui/boxxy_news_dialog.ui"
        ));
        let dialog: adw::Dialog = builder
            .object("boxxy_news_dialog")
            .expect("boxxy_news_dialog must exist");
        assert_eq!(dialog.content_width(), 680);
        assert!(dialog.follows_content_size());

        let news_stack: gtk::Stack = builder.object("news_stack").expect("news_stack");
        assert!(news_stack.child_by_name("list").is_some());
        assert!(news_stack.child_by_name("detail").is_some());

        let list_state_stack: gtk::Stack =
            builder.object("list_state_stack").expect("list_state_stack");
        assert!(list_state_stack.child_by_name("loading").is_some());
        assert!(list_state_stack.child_by_name("empty").is_some());
        assert!(list_state_stack.child_by_name("content").is_some());

        for required_btn in [
            "news_refresh_btn",
            "open_web_archive_btn",
            "close_btn",
            "detail_back_btn",
            "detail_prev_btn",
            "detail_next_btn",
            "detail_open_web_btn",
            "detail_bottom_close_btn",
            "detail_cta_btn",
        ] {
            assert!(
                builder.object::<gtk::Button>(required_btn).is_some(),
                "missing button '{required_btn}' in boxxy_news_dialog.ui"
            );
        }

        assert!(builder.object::<gtk::Box>("news_list").is_some());
        assert!(builder.object::<gtk::Box>("detail_viewer_slot").is_some());
    }
}
