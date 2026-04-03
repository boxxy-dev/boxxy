use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

/// A utility to bind a GObject property to a UI element asynchronously.
/// This is specifically designed for streaming text (like LLM tokens) from background threads.
pub fn bind_property_async<O, W, F>(
    obj: &O,
    property_name: &str,
    widget: &W,
    update_fn: F,
) -> glib::SignalHandlerId
where
    O: glib::object::ObjectExt,
    W: glib::object::ObjectExt + Clone + 'static,
    F: Fn(&W, String) + 'static,
{
    // Use a channel to bridge property updates to the main thread safely.
    // connect_notify requires Send + Sync, but GTK widgets are !Send.
    let (tx, rx) = async_channel::unbounded::<String>();

    let prop_name = property_name.to_string();
    let handler_id = obj.connect_notify(Some(property_name), move |o, _| {
        let val = o.property::<String>(&prop_name);
        let _ = tx.send_blocking(val);
    });

    let w_clone = widget.clone();
    gtk::glib::spawn_future_local(async move {
        while let Ok(val) = rx.recv().await {
            update_fn(&w_clone, val);
        }
    });

    handler_id
}

/// Safe wrapper for storing arbitrary data on GObjects using Quarks.
pub trait ObjectExtSafe: glib::object::ObjectExt {
    fn set_safe_data<T: 'static>(&self, key: &str, data: T) {
        let quark = glib::Quark::from_str(key);
        unsafe {
            self.set_qdata(quark, data);
        }
    }

    fn get_safe_data<T: 'static>(&self, key: &str) -> Option<&T> {
        let quark = glib::Quark::from_str(key);
        unsafe { self.qdata::<T>(quark).map(|p| p.as_ref()) }
    }

    fn steal_safe_data<T: 'static>(&self, key: &str) -> Option<T> {
        let quark = glib::Quark::from_str(key);
        unsafe { self.steal_qdata::<T>(quark) }
    }
}

impl<T: glib::object::ObjectExt> ObjectExtSafe for T {}
