use boxxy_vte::terminal::TerminalWidget;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, PolicyType, ScrolledWindow};

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("tracing", log::LevelFilter::Warn)
        .filter_module("sqlx", log::LevelFilter::Warn)
        .init();

    let app = Application::builder()
        .application_id("org.boxxy.vte.sandbox")
        .build();

    app.connect_activate(|app| {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Boxxy VTE Sandbox")
            .default_width(900)
            .default_height(650)
            .build();

        // Instantiate our custom GObject widget.
        // Because TerminalWidget implements gtk::Scrollable, the
        // ScrolledWindow below hands our vadjustment/hadjustment properties
        // to us directly and does NOT wrap us in a Viewport.
        let terminal = TerminalWidget::new();

        // Only show a vertical scrollbar (terminals don't scroll horizontally).
        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Always)
            .hexpand(true)
            .vexpand(true)
            .child(&terminal)
            .build();

        window.set_child(Some(&scrolled));
        window.present();

        // Spawn the user's default shell after present() so that the widget
        // already has a real pixel allocation.
        terminal.spawn_async(None, &[]);

        let term_clone = terminal.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            log::info!("Simulating Kitty APC sequence directly into shell...");
            term_clone.write_all(b"printf \"\\x1b_Ga=T,f=100;test\\x1b\\\\\"\n".to_vec());
            glib::ControlFlow::Break
        });
    });

    app.run();
}
