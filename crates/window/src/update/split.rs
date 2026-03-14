use crate::state::AppWindowInner;

pub fn split_vertical(inner: &mut AppWindowInner) {
    if let Some(page) = inner.tab_view.selected_page() {
        let child = page.child();
        if let Some(pos) = inner.tabs.iter().position(|c| c.controller.widget() == &child) {
            inner.tabs[pos].controller.split_vertical();
        }
    }
}

pub fn split_horizontal(inner: &mut AppWindowInner) {
    if let Some(page) = inner.tab_view.selected_page() {
        let child = page.child();
        if let Some(pos) = inner.tabs.iter().position(|c| c.controller.widget() == &child) {
            inner.tabs[pos].controller.split_horizontal();
        }
    }
}

pub fn close_split(inner: &mut AppWindowInner) {
    if let Some(page) = inner.tab_view.selected_page() {
        let child = page.child();
        if let Some(pos) = inner.tabs.iter().position(|c| c.controller.widget() == &child) {
            inner.tabs[pos].controller.close_split();
        }
    }
}

pub fn toggle_maximize(inner: &mut AppWindowInner) {
    if let Some(page) = inner.tab_view.selected_page() {
        let child = page.child();
        if let Some(pos) = inner.tabs.iter().position(|c| c.controller.widget() == &child) {
            inner.tabs[pos].controller.toggle_maximize();
        }
    }
}

pub fn focus(inner: &mut AppWindowInner, direction: boxxy_terminal::Direction) {
    if let Some(page) = inner.tab_view.selected_page() {
        let child = page.child();
        if let Some(pos) = inner.tabs.iter().position(|c| c.controller.widget() == &child) {
            inner.tabs[pos].controller.focus(direction);
        }
    }
}

pub fn swap(inner: &mut AppWindowInner, direction: boxxy_terminal::Direction) {
    if let Some(page) = inner.tab_view.selected_page() {
        let child = page.child();
        if let Some(pos) = inner.tabs.iter().position(|c| c.controller.widget() == &child) {
            inner.tabs[pos].controller.swap(direction);
        }
    }
}