//! Clickable circular [`EditableAvatar`] widget and single-flight file-chooser launcher.

use super::AvatarCropDialog;
use adw::prelude::*;
use boxxy_assets::image::AvatarOutput;
use gtk4 as gtk;
use gtk4::gio;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// RAII guard that resets an `in_flight` flag when dropped — whether the user applies an avatar,
/// encounters a decode error, or cancels the file chooser / crop dialog.
struct InFlightGuard(Rc<Cell<bool>>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Clickable circular avatar widget with edit and optional reset badges.
///
/// Both the bottom-right pencil badge and top-right hover `×` badge protrude ~35%
/// outside the avatar circle so the image itself remains unobstructed. Clicking opens
/// a file chooser followed by [`AvatarCropDialog`], returning 256×256 PNG bytes.
#[derive(Clone)]
pub struct EditableAvatar {
    container: gtk::Overlay,
    button: gtk::Button,
    edit_btn: gtk::Button,
    remove_btn: gtk::Button,
    avatar: adw::Avatar,
    picker_in_flight: Rc<Cell<bool>>,
}

impl EditableAvatar {
    #[must_use]
    pub fn new(size: i32, text: Option<&str>, show_initials: bool) -> Self {
        let avatar = adw::Avatar::new(size, text, show_initials);

        let button = gtk::Button::new();
        button.add_css_class("circular");
        button.add_css_class("editable-avatar-btn");
        button.set_valign(gtk::Align::Center);
        button.set_halign(gtk::Align::Center);
        button.set_focus_on_click(false);
        button.set_tooltip_text(Some("Click to change avatar"));
        button.update_property(&[gtk::accessible::Property::Label("Change avatar")]);
        button.set_cursor_from_name(Some("pointer"));
        button.set_child(Some(&avatar));

        let container = gtk::Overlay::new();
        container.add_css_class("editable-avatar-container");
        container.set_valign(gtk::Align::Center);
        container.set_halign(gtk::Align::Center);
        container.set_child(Some(&button));

        // Bottom-right pencil badge button (protrudes ~35% outside the avatar circle)
        let edit_btn = gtk::Button::new();
        edit_btn.add_css_class("circular");
        edit_btn.add_css_class("editable-avatar-badge");
        edit_btn.set_halign(gtk::Align::End);
        edit_btn.set_valign(gtk::Align::End);
        edit_btn.set_focus_on_click(false);
        edit_btn.set_tooltip_text(Some("Click to change avatar"));
        edit_btn.update_property(&[gtk::accessible::Property::Label("Edit avatar")]);
        edit_btn.set_cursor_from_name(Some("pointer"));

        let pencil_icon = gtk::Image::from_icon_name("boxxy-document-edit-symbolic");
        pencil_icon.set_pixel_size(13);
        pencil_icon.set_halign(gtk::Align::Center);
        pencil_icon.set_valign(gtk::Align::Center);
        edit_btn.set_child(Some(&pencil_icon));

        container.add_overlay(&edit_btn);

        // Top-right `×` remove/reset badge button (displayed when `removable` is enabled)
        let remove_btn = gtk::Button::new();
        remove_btn.add_css_class("circular");
        remove_btn.add_css_class("editable-avatar-remove-badge");
        remove_btn.set_halign(gtk::Align::End);
        remove_btn.set_valign(gtk::Align::Start);
        remove_btn.set_focus_on_click(false);
        remove_btn.set_tooltip_text(Some("Reset avatar"));
        remove_btn.update_property(&[gtk::accessible::Property::Label("Reset avatar")]);
        remove_btn.set_cursor_from_name(Some("pointer"));
        remove_btn.set_visible(false);

        let remove_icon = gtk::Image::from_icon_name("boxxy-cross-small-symbolic");
        remove_icon.set_pixel_size(14);
        remove_icon.set_halign(gtk::Align::Center);
        remove_icon.set_valign(gtk::Align::Center);
        remove_btn.set_child(Some(&remove_icon));

        container.add_overlay(&remove_btn);

        Self {
            container,
            button,
            edit_btn,
            remove_btn,
            avatar,
            picker_in_flight: Rc::new(Cell::new(false)),
        }
    }

    /// Underlying `adw::Avatar` for setting custom textures or fallback initials.
    #[must_use]
    pub const fn avatar(&self) -> &adw::Avatar {
        &self.avatar
    }

    /// Top-level widget to add to rows or containers.
    #[must_use]
    pub const fn widget(&self) -> &gtk::Overlay {
        &self.container
    }

    /// Controls whether the top-right `×` reset/remove badge is displayed.
    pub fn set_removable(&self, removable: bool) {
        self.remove_btn.set_visible(removable);
    }

    /// Customises the tooltip and accessible label on the top-right `×` reset/remove badge.
    pub fn set_remove_tooltip(&self, tooltip: &str) {
        self.remove_btn.set_tooltip_text(Some(tooltip));
        self.remove_btn
            .update_property(&[gtk::accessible::Property::Label(tooltip)]);
    }

    /// Connects the click handler (on both the avatar and the pencil badge) to open the
    /// file chooser and square crop dialog, guarded against concurrent duplicate dialogs.
    pub fn connect_avatar_cropped<F>(&self, dialog_title: &str, on_cropped: F)
    where
        F: Fn(Result<AvatarOutput, String>) + 'static,
    {
        let title = dialog_title.to_string();
        let on_cropped = Rc::new(on_cropped);

        let launch_once = {
            let in_flight = self.picker_in_flight.clone();
            Rc::new(
                move |btn: &gtk::Button,
                      title: &str,
                      cb: Rc<dyn Fn(Result<AvatarOutput, String>)>| {
                    if in_flight.replace(true) {
                        return;
                    }
                    let guard = InFlightGuard(in_flight.clone());
                    prompt_and_crop_avatar(btn, title, move |res| {
                        let _keep_until_done = guard;
                        cb(res);
                    });
                },
            )
        };

        let title_main = title.clone();
        let cb_main = on_cropped.clone();
        let launch_main = launch_once.clone();
        self.button.connect_clicked(move |btn| {
            launch_main(btn, &title_main, cb_main.clone());
        });

        let title_edit = title;
        let cb_edit = on_cropped;
        let launch_edit = launch_once;
        self.edit_btn.connect_clicked(move |btn| {
            launch_edit(btn, &title_edit, cb_edit.clone());
        });
    }

    /// Connects the click handler for the top-right `×` reset/remove badge.
    pub fn connect_avatar_removed<F>(&self, on_removed: F)
    where
        F: Fn() + 'static,
    {
        self.remove_btn.connect_clicked(move |_| {
            on_removed();
        });
    }
}

/// Opens a file chooser for PNG/JPEG/WebP images, presents [`AvatarCropDialog`] once a file
/// is chosen, and decodes the image + backdrop off the GTK main thread.
///
/// `on_result` is invoked with `Ok(AvatarOutput)` when the user clicks **Apply Avatar** or
/// `Err(String)` if reading/decoding/encoding fails; if the user cancels the file chooser or
/// closes the crop dialog without applying, `on_result` is dropped without being called.
pub fn prompt_and_crop_avatar<W, F>(parent: &W, dialog_title: &str, on_result: F)
where
    W: IsA<gtk::Widget>,
    F: FnOnce(Result<AvatarOutput, String>) + 'static,
{
    let window = parent.root().and_downcast::<gtk::Window>();
    let dialog = gtk::FileDialog::new();
    dialog.set_title(dialog_title);

    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Images (PNG, JPEG, WebP)"));
    filter.add_mime_type("image/png");
    filter.add_mime_type("image/jpeg");
    filter.add_mime_type("image/webp");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));

    let parent_widget: gtk::Widget = parent.clone().upcast();
    let title = dialog_title.to_string();
    let on_result = Rc::new(RefCell::new(Some(on_result)));

    dialog.open(window.as_ref(), None::<&gio::Cancellable>, move |result| {
        let Ok(file) = result else {
            // User cancelled the file chooser; dropping `on_result` releases any in-flight guard.
            return;
        };

        if let Some(cb) = on_result.borrow_mut().take() {
            AvatarCropDialog::present_from_file(&parent_widget, &title, file, cb);
        }
    });
}
