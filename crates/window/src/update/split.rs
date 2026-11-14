use crate::state::AppWindowInner;

pub fn split_vertical(inner: &mut AppWindowInner) {
    if let Some(tab) = inner.active_tab() {
        tab.controller.split_vertical(None);
    }
}

pub fn split_horizontal(inner: &mut AppWindowInner) {
    if let Some(tab) = inner.active_tab() {
        tab.controller.split_horizontal(None);
    }
}

pub fn close_split(inner: &mut AppWindowInner) {
    if let Some(tab) = inner.active_tab() {
        tab.controller.close_split();
    }
}

pub fn toggle_maximize(inner: &mut AppWindowInner) {
    if let Some(tab) = inner.active_tab() {
        tab.controller.toggle_maximize();
    }
}

pub fn focus(inner: &mut AppWindowInner, direction: boxxy_terminal::Direction) {
    if let Some(tab) = inner.active_tab() {
        tab.controller.focus(direction);
    }
}

pub fn swap(inner: &mut AppWindowInner, direction: boxxy_terminal::Direction) {
    if let Some(tab) = inner.active_tab() {
        tab.controller.swap(direction);
    }
}

pub fn move_pane_to_split(
    inner: &mut AppWindowInner,
    source_pane_id: String,
    target_pane_id: String,
    direction: boxxy_terminal::Direction,
) {
    if source_pane_id == target_pane_id {
        return;
    }

    let Some(target_tab_idx) = inner
        .tabs
        .iter()
        .position(|t| t.controller.has_pane(&target_pane_id))
    else {
        return;
    };

    let Some(source_tab_idx) = inner
        .tabs
        .iter()
        .position(|t| t.controller.has_pane(&source_pane_id))
    else {
        return;
    };

    if source_tab_idx == target_tab_idx {
        inner.tabs[target_tab_idx].controller.move_pane_to_split(
            &source_pane_id,
            &target_pane_id,
            direction,
        );
        super::tabs::focus_active_terminal(inner);
        return;
    }

    let Some(extracted) = inner.tabs[source_tab_idx]
        .controller
        .take_pane(&source_pane_id)
    else {
        return;
    };

    inner.tabs[target_tab_idx]
        .controller
        .insert_pane_split(extracted, &target_pane_id, direction);

    let target_widget = inner.tabs[target_tab_idx].controller.widget().clone();
    let target_page = inner.tab_view.page(&target_widget);
    inner.tab_view.set_selected_page(&target_page);

    if inner.tabs[source_tab_idx].controller.pane_count() == 0 {
        let removed_tc = inner.tabs.remove(source_tab_idx);
        removed_tc.stop_working_anim();
        let src_page = inner.tab_view.page(removed_tc.controller.widget());
        inner.tab_view.close_page(&src_page);
    }

    super::tab_bar_sync::sync_tab_titles(inner);
    super::tabs::focus_active_terminal(inner);
}

pub fn drop_pane_on_tab(
    inner: &mut AppWindowInner,
    source_pane_id: String,
    target_page_key: usize,
) {
    let Some(target_tab_idx) = inner.tab_pos_by_widget_key(target_page_key) else {
        return;
    };

    if inner.tabs[target_tab_idx]
        .controller
        .has_pane(&source_pane_id)
    {
        return;
    }

    let target_pane_id = inner.tabs[target_tab_idx].controller.active_pane_id();
    move_pane_to_split(
        inner,
        source_pane_id,
        target_pane_id,
        boxxy_terminal::Direction::Right,
    );
}
