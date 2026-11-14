pub mod cache;
pub mod parser;
pub mod registry;
pub mod renderer;
pub mod widget;

pub use parser::blocks::{ContentBlock, GalleryItem, ListItem, TableAlignment, TableCell};
pub use parser::markdown::{
    escape_pango, escape_pango_attr, normalize_fence_language, parse_markdown, strip_pango_markup,
    write_escaped_pango, write_escaped_pango_attr,
};
pub use registry::ViewerRegistry;
pub use renderer::code::CodeBlockWidget;
pub use renderer::{BlockRenderer, RenderContext};
pub use widget::StructuredViewer;

/// Runs GTK widget unit tests sequentially on a single dedicated OS thread that owns `gtk4::init()`.
/// Prevents `g_static_resource_init` races and GTK main-thread affinity segfaults under parallel `cargo test`.
#[cfg(test)]
pub(crate) fn run_gtk_test<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    type Task = Box<dyn FnOnce() + Send>;
    type TaskResult = Result<(), Box<dyn std::any::Any + Send>>;
    type GtkJobMsg = (Task, std::sync::mpsc::SyncSender<TaskResult>);
    static GTK_THREAD: std::sync::LazyLock<
        std::sync::Mutex<std::sync::mpsc::SyncSender<GtkJobMsg>>,
    > = std::sync::LazyLock::new(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<GtkJobMsg>(1);
        std::thread::Builder::new()
            .name("boxxy-viewer-gtk-test".to_string())
            .spawn(move || {
                let gtk_ok = gtk4::init().is_ok();
                while let Ok((job, done_tx)) = rx.recv() {
                    let res = if gtk_ok {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                    } else {
                        Ok(())
                    };
                    let _ = done_tx.send(res);
                }
            })
            .expect("spawn gtk test thread");
        std::sync::Mutex::new(tx)
    });

    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    GTK_THREAD
        .lock()
        .expect("gtk test mutex")
        .send((Box::new(f), done_tx))
        .expect("send gtk test job");
    if let Err(panic_payload) = done_rx.recv().expect("gtk test completion") {
        std::panic::resume_unwind(panic_payload);
    }
}
