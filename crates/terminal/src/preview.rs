use gtk4::prelude::*;
use gtk4 as gtk;
use std::rc::Rc;
use std::cell::RefCell;

#[derive(Clone)]
pub struct ImagePreviewPopover {
    popover: gtk::Popover,
    picture: gtk::Picture,
    video_stream: gtk::MediaFile,
    stack: gtk::Stack,
    pending_task: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    hide_task: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    active_uri: Rc<RefCell<Option<String>>>,
    is_pinned: Rc<RefCell<bool>>,
}

impl Default for ImagePreviewPopover {
    fn default() -> Self {
        Self::new()
    }
}

impl ImagePreviewPopover {
    pub fn new() -> Self {
        let settings = boxxy_preferences::Settings::load();
        
        let popover = gtk::Popover::builder()
            .has_arrow(true)
            .autohide(false) // We control visibility manually based on hover
            .can_target(true) // Allow popover to receive events to prevent flickering when mouse enters it
            .build();
        popover.add_css_class("preview-popover");

        let picture = gtk::Picture::builder()
            .can_shrink(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
            
        let video_stream = gtk::MediaFile::new();
        video_stream.set_loop(true);

        let video_pic = gtk::Picture::builder()
            .can_shrink(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .paintable(&video_stream)
            .build();
            
        // Provide a minimum size so the popover doesn't collapse to 0x0 before the video metadata loads.
        video_pic.set_size_request(300, 200);

        let controls = gtk::MediaControls::builder()
            .halign(gtk::Align::Fill)
            .media_stream(&video_stream)
            .build();

        let video = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .build();
        video.append(&video_pic);
        video.append(&controls);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::None)
            .hhomogeneous(false)
            .vhomogeneous(false)
            .build();
        stack.add_named(&picture, Some("image"));
        stack.add_named(&video, Some("video"));
            
        // Constrain max size via Clamp so large images don't cover the whole screen
        let clamp = libadwaita::Clamp::builder()
            .maximum_size(settings.preview_max_width)
            .child(&stack)
            .build();
            
        popover.set_child(Some(&clamp));

        let is_pinned = Rc::new(RefCell::new(false));
        let is_pinned_clone = is_pinned.clone();
        let active_uri = Rc::new(RefCell::new(None));
        let active_uri_clone = active_uri.clone();
        
        let picture_clone = picture.clone();
        let video_stream_clone = video_stream.clone();

        popover.connect_closed(move |_| {
            *is_pinned_clone.borrow_mut() = false;
            *active_uri_clone.borrow_mut() = None;
            picture_clone.set_file(None::<&gtk::gio::File>);
            video_stream_clone.set_file(None::<&gtk::gio::File>);
        });

        let instance = Self {
            popover,
            picture,
            video_stream,
            stack,
            pending_task: Rc::new(RefCell::new(None)),
            hide_task: Rc::new(RefCell::new(None)),
            active_uri,
            is_pinned,
        };

        // Add motion controller to the clamp to keep preview alive while hovering the popover
        let popover_motion_ctrl = gtk::EventControllerMotion::new();
        let instance_clone_enter = instance.clone();
        popover_motion_ctrl.connect_enter(move |_, _, _| {
            instance_clone_enter.cancel_hide();
        });
        let instance_clone_leave = instance.clone();
        popover_motion_ctrl.connect_leave(move |_| {
            instance_clone_leave.hide_preview_if_not_pinned();
        });
        clamp.add_controller(popover_motion_ctrl);

        instance
    }

    pub fn popover(&self) -> &gtk::Popover {
        &self.popover
    }

    pub fn show_preview(&self, uri: &str, x: f64, y: f64, is_hover: bool) {
        if is_hover && *self.is_pinned.borrow() {
            // Do not override a pinned preview with a hover preview
            return;
        }

        self.cancel_hide();

        // If we are already showing or pending this exact URI, don't restart the timer.
        // This prevents flashing when moving the mouse inside a single link.
        if let Some(ref current) = *self.active_uri.borrow()
            && current == uri {
                if !is_hover {
                    // Upgrade to pinned
                    *self.is_pinned.borrow_mut() = true;
                    self.popover.set_autohide(true);
                }
                return;
            }

        self.cancel_pending();
        *self.active_uri.borrow_mut() = Some(uri.to_string());

        // Trim common enclosure characters from the URI before checking the extension.
        // This helps with URIs that might contain literal quotes from the shell.
        let uri_trimmed = uri.trim_matches(|c| c == '\'' || c == '"');
        let uri_lower = uri_trimmed.to_lowercase();
        
        let is_image = uri_lower.ends_with(".png")
            || uri_lower.ends_with(".jpg")
            || uri_lower.ends_with(".jpeg")
            || uri_lower.ends_with(".gif")
            || uri_lower.ends_with(".webp")
            || uri_lower.ends_with(".bmp")
            || uri_lower.ends_with(".svg");
            
        let is_video = uri_lower.ends_with(".mp4")
            || uri_lower.ends_with(".webm")
            || uri_lower.ends_with(".mkv")
            || uri_lower.ends_with(".avi")
            || uri_lower.ends_with(".mov");

        if !is_image && !is_video {
            return;
        }

        let file = gtk::gio::File::for_uri(uri_trimmed);

        let popover_clone = self.popover.clone();
        let picture_clone = self.picture.clone();
        let video_stream_clone = self.video_stream.clone();
        let stack_clone = self.stack.clone();

        // Shift pointing rectangle slightly so it doesn't block the mouse
        let rect = gtk::gdk::Rectangle::new(x as i32, (y - 10.0).max(0.0) as i32, 1, 1);

        let timeout_ms = if is_hover { 300 } else { 0 };
        let is_pinned_clone = self.is_pinned.clone();

        // Debounce: Wait 300ms before showing the preview for hover, 0ms for click
        let pending_task_clone = self.pending_task.clone();
        let task_id = gtk::glib::timeout_add_local(std::time::Duration::from_millis(timeout_ms), move || {
            if is_image {
                stack_clone.set_visible_child_name("image");
                picture_clone.set_file(Some(&file));
                video_stream_clone.set_file(None::<&gtk::gio::File>);
            } else if is_video {
                stack_clone.set_visible_child_name("video");
                video_stream_clone.set_file(Some(&file));
                video_stream_clone.play();
                picture_clone.set_file(None::<&gtk::gio::File>);
            }            
            
            popover_clone.set_autohide(!is_hover);
            if !is_hover {
                *is_pinned_clone.borrow_mut() = true;
            }

            popover_clone.set_pointing_to(Some(&rect));
            popover_clone.popup();
            
            // Clear the stored source ID so we don't try to remove it later
            *pending_task_clone.borrow_mut() = None;
            
            gtk::glib::ControlFlow::Break
        });

        *self.pending_task.borrow_mut() = Some(task_id);
    }

    pub fn hide_preview_if_not_pinned(&self) {
        if !*self.is_pinned.borrow() {
            self.cancel_hide();
            
            let popover_clone = self.popover.clone();
            let active_uri_clone = self.active_uri.clone();
            let picture_clone = self.picture.clone();
            let video_stream_clone = self.video_stream.clone();
            let hide_task_clone = self.hide_task.clone();
            
            let task_id = gtk::glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
                *active_uri_clone.borrow_mut() = None;
                popover_clone.popdown();
                picture_clone.set_file(None::<&gtk::gio::File>);
                video_stream_clone.set_file(None::<&gtk::gio::File>);
                *hide_task_clone.borrow_mut() = None;
                gtk::glib::ControlFlow::Break
            });
            *self.hide_task.borrow_mut() = Some(task_id);
        }
    }

    pub fn hide_preview(&self) {
        self.cancel_pending();
        self.cancel_hide();
        *self.active_uri.borrow_mut() = None;
        self.popover.popdown();
        // Unload the image and video to save memory
        self.picture.set_file(None::<&gtk::gio::File>);
        self.video_stream.set_file(None::<&gtk::gio::File>);
    }

    fn cancel_pending(&self) {
        if let Some(source_id) = self.pending_task.borrow_mut().take() {
            source_id.remove();
        }
    }
    
    fn cancel_hide(&self) {
        if let Some(source_id) = self.hide_task.borrow_mut().take() {
            source_id.remove();
        }
    }
}
