use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{Application, ApplicationWindow, ScrolledWindow, PolicyType};
use boxxy_vte::terminal::TerminalWidget;

#[tokio::main]
async fn main() {
    env_logger::init();

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
        // Because TerminalWidget now implements gtk::Scrollable, the
        // ScrolledWindow below will hand our vadjustment/hadjustment properties
        // to us directly and will NOT wrap us in a Viewport.
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

        // Spawn the user's default shell.  Do this after present() so that the
        // widget already has a real pixel allocation and the initial PTY resize
        // reflects the true terminal dimensions.
        terminal.spawn_async(None, &[]);
        
        let term_clone = terminal.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            log::info!("Simulating Kitty APC sequence directly into shell...");
            if let Some(backend) = term_clone.imp().backend.borrow().as_ref() {
                backend.write_to_pty(b"printf \"\\x1b_Ga=T,f=100;test\\x1b\\\\\"\n".to_vec());
            }
            glib::ControlFlow::Break
        });
    });

    app.run();
}