mod ingest;
pub mod tokens;

pub use tokens::{
    AtomicKeyAction, AtomicKeyOp, Attachment, AttachmentPayload, CharSpan,
    MAX_ATTACHMENT_FILE_BYTES, MAX_ATTACHMENTS_PER_MESSAGE, MAX_TEXT_SNIPPET_BYTES,
    MAX_TOTAL_ATTACHMENT_BYTES, PASTE_INLINE_MAX_CHARS, PASTED_IMAGE_LABEL, TEXT_SNIPPET_LABEL,
    TokenKind,
};

use ingest::{
    FileReadReject, build_attachment_file_filters, file_display_label_from_path, format_limit_mb,
    has_supported_image_magic, has_unsupported_image_or_media_extension, is_probably_binary,
    is_supported_image_path, parse_candidate_image_paths, read_and_encode_local_image,
    read_gio_file_bounded, resolve_gio_file_display_name,
};
use tokens::{
    allocate_next_token, build_payload_from_attachments, evaluate_atomic_token_key,
    find_active_token_char_spans, hover_tooltip_for_offset, normalize_pasted_text,
    should_collapse_pasted_text, snap_cursor_out_of_token, sync_attachments_with_buffer_text,
};

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

type NotifyHook = Rc<RefCell<Option<Rc<dyn Fn(String)>>>>;

/// RAII guard resetting `picker_in_flight` when dropped (on open, error, or cancel).
struct InFlightGuard(Rc<Cell<bool>>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// RAII guard resetting `in_sync` so nested buffer callbacks never re-enter.
struct SyncGuard(Rc<Cell<bool>>);

impl Drop for SyncGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileIngestSource {
    Picker,
    Drop,
}

#[derive(Clone)]
pub struct AttachmentManager {
    pub attachments: Rc<RefCell<Vec<Attachment>>>,
    stashed: Rc<RefCell<Vec<Attachment>>>,
    tracked_inline_ids: Rc<RefCell<HashSet<String>>>,
    entry_ref: Rc<RefCell<Option<glib::WeakRef<gtk::TextView>>>>,
    notify_hook: NotifyHook,
    picker_in_flight: Rc<Cell<bool>>,
    in_sync: Rc<Cell<bool>>,
    generation: Rc<Cell<u64>>,
}

impl Default for AttachmentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AttachmentManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            attachments: Rc::new(RefCell::new(Vec::new())),
            stashed: Rc::new(RefCell::new(Vec::new())),
            tracked_inline_ids: Rc::new(RefCell::new(HashSet::new())),
            entry_ref: Rc::new(RefCell::new(None)),
            notify_hook: Rc::new(RefCell::new(None)),
            picker_in_flight: Rc::new(Cell::new(false)),
            in_sync: Rc::new(Cell::new(false)),
            generation: Rc::new(Cell::new(0)),
        }
    }

    /// Binds the owning `GtkTextView` for inline `[Image N · file]`, `[File N · file]`, and
    /// `[Pasted ~N lines]` token insertion, `Backspace` / Undo stash sync, and hover tooltips.
    pub fn bind_entry(&self, entry: &gtk::TextView) {
        *self.entry_ref.borrow_mut() = Some(entry.downgrade());

        entry.set_has_tooltip(true);
        let mgr_for_tip = self.clone();
        entry.connect_query_tooltip(move |tv, x, y, keyboard_mode, tooltip| {
            if keyboard_mode {
                return false;
            }
            if mgr_for_tip.attachments.borrow().is_empty() {
                return false;
            }
            let (bx, by) = tv.window_to_buffer_coords(gtk::TextWindowType::Widget, x, y);
            let Some(iter) = tv.iter_at_location(bx, by) else {
                return false;
            };
            let char_offset = usize::try_from(iter.offset().max(0)).unwrap_or(0);
            let buf = tv.buffer();
            let text = buf
                .text(&buf.start_iter(), &buf.end_iter(), false)
                .to_string();
            let active = mgr_for_tip.attachments.borrow();
            if let Some(tip_text) = hover_tooltip_for_offset(&text, &active, char_offset) {
                tooltip.set_text(Some(&tip_text));
                return true;
            }
            false
        });
    }

    fn bound_entry(&self) -> Option<gtk::TextView> {
        self.entry_ref
            .borrow()
            .as_ref()
            .and_then(glib::WeakRef::upgrade)
    }

    /// Runs `f` while suppressing automatic buffer-to-attachment stash sync.
    pub fn with_sync_suppressed<R>(&self, f: impl FnOnce() -> R) -> R {
        let was_sync = self.in_sync.replace(true);
        let out = f();
        self.in_sync.set(was_sync);
        out
    }

    /// Registers a callback (typically routed to `ClawHost::notify`) for surfacing user-visible
    /// toasts when an attached, pasted, or dropped file is oversized or unsupported.
    pub fn set_notify_hook<F: Fn(String) + 'static>(&self, hook: F) {
        *self.notify_hook.borrow_mut() = Some(Rc::new(hook));
    }

    fn notify(&self, message: impl Into<String>) {
        if let Some(cb) = self.notify_hook.borrow().as_ref().cloned() {
            cb(message.into());
        }
    }

    pub fn clear(&self) {
        self.generation
            .set(self.generation.get().wrapping_add(1));
        self.attachments.borrow_mut().clear();
        self.stashed.borrow_mut().clear();
        self.tracked_inline_ids.borrow_mut().clear();
    }

    #[must_use]
    pub fn get_attachments(&self) -> Vec<Attachment> {
        self.attachments.borrow().clone()
    }

    pub fn load_attachments(&self, attachments: Vec<Attachment>) {
        let current_text = self.bound_entry().map_or_else(String::new, |entry| {
            let buf = entry.buffer();
            buf.text(&buf.start_iter(), &buf.end_iter(), false)
                .to_string()
        });
        self.load_attachments_for_text(attachments, &current_text);
    }

    /// Restores attachments for a recalled `MsgHistory` entry. Any pre-token legacy entry
    /// whose token was not yet in `text` appends its inline `[...]` tag at the end of `entry`.
    pub fn load_attachments_for_text(&self, attachments: Vec<Attachment>, text: &str) {
        self.clear();
        if attachments.is_empty() {
            return;
        }

        let mut legacy_tokens_to_append: Vec<String> = Vec::new();
        {
            let mut active = self.attachments.borrow_mut();
            let mut tracked = self.tracked_inline_ids.borrow_mut();
            for mut att in attachments {
                if att.id.is_empty() {
                    att.id = uuid::Uuid::new_v4().to_string();
                }
                if att.token.is_empty() {
                    let kind = TokenKind::from_attachment(&att.label, att.is_image);
                    att.token =
                        allocate_next_token(kind, &att.label, &att.content, &active, &[], text);
                }
                if text.contains(&att.token) {
                    tracked.insert(att.id.clone());
                } else if self.bound_entry().is_some() {
                    tracked.insert(att.id.clone());
                    legacy_tokens_to_append.push(att.token.clone());
                }
                active.push(att);
            }
        }

        if !legacy_tokens_to_append.is_empty()
            && let Some(entry) = self.bound_entry()
        {
            let _guard = SyncGuard(self.in_sync.clone());
            self.in_sync.set(true);
            insert_tokens_at_cursor(&entry, &legacy_tokens_to_append);
        }
    }

    /// Adds an attachment and, when an input `GtkTextView` is bound, inserts its inline token
    /// (`[Image N · file]`, `[Image N]`, `[File N · file]`, or `[Pasted ~N lines]`) at the cursor.
    #[allow(clippy::needless_pass_by_value)]
    pub fn add_attachment(&self, label: String, content: String, is_image: bool) {
        let bound = self.bound_entry();
        self.add_attachment_at_cursor(label, content, is_image, None, bound.as_ref());
    }

    fn add_attachment_at_cursor(
        &self,
        label: String,
        content: String,
        is_image: bool,
        raw_bytes_hint: Option<usize>,
        entry: Option<&gtk::TextView>,
    ) {
        // 1. Reuse existing token if identical content + label is already attached or stashed
        //    (matches OpenCode's `promptAttachmentLabel` duplicate-paste reuse).
        if let Some(existing_token) = {
            let active = self.attachments.borrow();
            active
                .iter()
                .find(|a| a.is_image == is_image && a.label == label && a.content == content)
                .map(|a| a.token.clone())
        } {
            if let Some(tv) = entry
                && !existing_token.is_empty()
            {
                self.insert_inline_token(tv, &existing_token);
            }
            return;
        }

        if let Some(restored) = {
            let mut stashed = self.stashed.borrow_mut();
            stashed
                .iter()
                .position(|a| a.is_image == is_image && a.label == label && a.content == content)
                .map(|idx| stashed.remove(idx))
        } {
            let token = restored.token.clone();
            if entry.is_some() && !token.is_empty() {
                self.tracked_inline_ids
                    .borrow_mut()
                    .insert(restored.id.clone());
            }
            self.attachments.borrow_mut().push(restored);
            if let Some(tv) = entry
                && !token.is_empty()
            {
                self.insert_inline_token(tv, &token);
            }
            return;
        }

        // 2. Enforce per-message attachment count and combined byte budget limits.
        let (active_count, current_total_bytes) = {
            let active = self.attachments.borrow();
            let total: usize = active.iter().map(Attachment::estimated_raw_bytes).sum();
            (active.len(), total)
        };
        if active_count >= MAX_ATTACHMENTS_PER_MESSAGE {
            self.notify(format!(
                "Maximum of {MAX_ATTACHMENTS_PER_MESSAGE} attachments per message reached."
            ));
            return;
        }
        let incoming_bytes = raw_bytes_hint.unwrap_or_else(|| {
            if is_image {
                (content.len() / 4) * 3
            } else {
                content.len()
            }
        });
        if current_total_bytes.saturating_add(incoming_bytes) > MAX_TOTAL_ATTACHMENT_BYTES {
            self.notify(format!(
                "Attachments exceed the {} total per-message limit.",
                format_limit_mb(MAX_TOTAL_ATTACHMENT_BYTES)
            ));
            return;
        }

        // 3. Allocate descriptive inline token (`[Image N · name]`, `[Image N]`, `[File N · name]`, `[Pasted ~N lines]`).
        let buffer_text = entry.map_or_else(String::new, |tv| {
            let buf = tv.buffer();
            buf.text(&buf.start_iter(), &buf.end_iter(), false)
                .to_string()
        });
        let kind = TokenKind::from_attachment(&label, is_image);
        let token = {
            let active = self.attachments.borrow();
            let stashed = self.stashed.borrow();
            allocate_next_token(kind, &label, &content, &active, &stashed, &buffer_text)
        };

        let id = uuid::Uuid::new_v4().to_string();
        if entry.is_some() {
            self.tracked_inline_ids.borrow_mut().insert(id.clone());
        }

        let att = Attachment {
            id,
            token: token.clone(),
            label,
            content,
            is_image,
        };

        self.attachments.borrow_mut().push(att);

        if let Some(tv) = entry {
            self.insert_inline_token(tv, &token);
        }
    }

    fn insert_inline_token(&self, entry: &gtk::TextView, token: &str) {
        let _guard = SyncGuard(self.in_sync.clone());
        self.in_sync.set(true);
        insert_tokens_at_cursor(entry, &[token.to_string()]);
    }

    /// Reconciles active/stashed attachments with `buffer_text` and returns the Unicode scalar
    /// (`char`) spans of all active inline tokens for `GtkTextTag` highlighting.
    #[must_use]
    pub fn sync_with_buffer_text(&self, buffer_text: &str) -> Vec<CharSpan> {
        if !self.in_sync.get() {
            let mut active = self.attachments.borrow_mut();
            let mut stashed = self.stashed.borrow_mut();
            let tracked = self.tracked_inline_ids.borrow();
            let _ = sync_attachments_with_buffer_text(
                &mut active,
                &mut stashed,
                &tracked,
                buffer_text,
            );
        }

        let active_tokens: Vec<String> = self
            .attachments
            .borrow()
            .iter()
            .filter(|a| !a.token.is_empty())
            .map(|a| a.token.clone())
            .collect();
        find_active_token_char_spans(buffer_text, &active_tokens)
    }

    /// Handles atomic single-keypress `Backspace`, `Delete`, `Left`, and `Right` actions
    /// over inline `[Image ...]`, `[File ...]`, and `[Pasted ~...]` tokens when no text is selected.
    #[must_use]
    pub fn handle_atomic_key(&self, entry: &gtk::TextView, op: AtomicKeyOp) -> bool {
        let buf = entry.buffer();
        if buf.has_selection() {
            return false;
        }
        let active_tokens: Vec<String> = self
            .attachments
            .borrow()
            .iter()
            .filter(|a| !a.token.is_empty())
            .map(|a| a.token.clone())
            .collect();
        if active_tokens.is_empty() {
            return false;
        }

        let text = buf
            .text(&buf.start_iter(), &buf.end_iter(), false)
            .to_string();
        let spans = find_active_token_char_spans(&text, &active_tokens);
        if spans.is_empty() {
            return false;
        }

        let cursor_char = usize::try_from(buf.cursor_position().max(0)).unwrap_or(0);
        let Some(action) = evaluate_atomic_token_key(&spans, cursor_char, op) else {
            return false;
        };

        match action {
            AtomicKeyAction::DeleteSpan(span) => {
                if let (Ok(s_off), Ok(e_off)) =
                    (i32::try_from(span.start), i32::try_from(span.end))
                {
                    let mut s_iter = buf.iter_at_offset(s_off);
                    let mut e_iter = buf.iter_at_offset(e_off);
                    buf.begin_user_action();
                    buf.delete(&mut s_iter, &mut e_iter);
                    buf.end_user_action();
                    return true;
                }
            }
            AtomicKeyAction::MoveCursor(target) => {
                if let Ok(off) = i32::try_from(target) {
                    let iter = buf.iter_at_offset(off);
                    buf.place_cursor(&iter);
                    return true;
                }
            }
        }
        false
    }

    /// Snaps the caret to the nearer edge of an inline attachment token if a non-selection
    /// mouse click landed strictly inside `[Image ...]`, `[File ...]`, or `[Pasted ~...]`.
    pub fn snap_click_cursor(&self, entry: &gtk::TextView) {
        let buf = entry.buffer();
        if buf.has_selection() {
            return;
        }
        let active_tokens: Vec<String> = self
            .attachments
            .borrow()
            .iter()
            .filter(|a| !a.token.is_empty())
            .map(|a| a.token.clone())
            .collect();
        if active_tokens.is_empty() {
            return;
        }
        let text = buf
            .text(&buf.start_iter(), &buf.end_iter(), false)
            .to_string();
        let spans = find_active_token_char_spans(&text, &active_tokens);
        let cursor_char = usize::try_from(buf.cursor_position().max(0)).unwrap_or(0);
        if let Some(target) = snap_cursor_out_of_token(&spans, cursor_char)
            && let Ok(off) = i32::try_from(target)
        {
            let iter = buf.iter_at_offset(off);
            buf.place_cursor(&iter);
        }
    }

    #[must_use]
    pub fn build_payload(&self, base_text: &str) -> AttachmentPayload {
        let active = self.attachments.borrow();
        build_payload_from_attachments(base_text, &active)
    }

    /// Opens a single-flight native multi-file chooser for attaching photos (PNG, JPEG, WebP, GIF)
    /// and UTF-8 text/code files as inline tags at the current cursor position.
    pub fn prompt_and_attach_files<W: IsA<gtk::Widget>>(
        &self,
        parent: &W,
        entry: &gtk::TextView,
        working_dir: Option<&str>,
    ) {
        if self.picker_in_flight.replace(true) {
            return;
        }
        let guard = InFlightGuard(self.picker_in_flight.clone());

        let window = parent.root().and_downcast::<gtk::Window>();
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Add Photos & Files");

        if let Some(cwd) = working_dir
            && !cwd.trim().is_empty()
        {
            let initial_folder = gio::File::for_path(cwd);
            dialog.set_initial_folder(Some(&initial_folder));
        }

        let filters = build_attachment_file_filters();
        if let Some(default_filter) = filters.item(0).and_downcast::<gtk::FileFilter>() {
            dialog.set_default_filter(Some(&default_filter));
        }
        dialog.set_filters(Some(&filters));

        let mgr = self.clone();
        let entry_clone = entry.clone();

        dialog.open_multiple(window.as_ref(), None::<&gio::Cancellable>, move |result| {
            let _keep_until_done = guard;
            let Ok(list_model) = result else {
                return;
            };

            let mut files = Vec::with_capacity(list_model.n_items() as usize);
            for idx in 0..list_model.n_items() {
                if let Some(file) = list_model.item(idx).and_downcast::<gio::File>() {
                    files.push(file);
                }
            }

            if !files.is_empty() {
                mgr.ingest_files(files, &entry_clone, None, false, FileIngestSource::Picker);
            }
        });
    }

    pub fn handle_paste(&self, entry: &gtk::TextView) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let clipboard = display.clipboard();
        let entry_clone = entry.clone();
        let clipboard_clone = clipboard.clone();
        let self_clone = self.clone();
        let expected_gen = self.generation.get();

        clipboard.read_text_async(None::<&gio::Cancellable>, move |res| {
            if self_clone.generation.get() != expected_gen {
                return;
            }
            let mut handled = false;
            if let Ok(Some(text)) = res {
                let text_str = text.to_string();
                if !text_str.is_empty() {
                    handled = true;
                    if let Some(candidate_paths) = parse_candidate_image_paths(&text_str) {
                        let mgr = self_clone.clone();
                        let entry_fallback = entry_clone.clone();
                        glib::spawn_future_local(async move {
                            let mut decoded_batch = Vec::with_capacity(candidate_paths.len());
                            for candidate_path in candidate_paths {
                                let label = file_display_label_from_path(&candidate_path, "Image");
                                match read_and_encode_local_image(candidate_path).await {
                                    Ok((b64, raw_len)) => {
                                        decoded_batch.push((label, b64, raw_len));
                                    }
                                    Err(Some(err_msg)) => {
                                        if mgr.generation.get() == expected_gen {
                                            mgr.notify(err_msg);
                                        }
                                        return;
                                    }
                                    Err(None) => {
                                        if mgr.generation.get() == expected_gen {
                                            insert_pasted_text(&mgr, &entry_fallback, &text_str);
                                        }
                                        return;
                                    }
                                }
                            }
                            if mgr.generation.get() != expected_gen {
                                return;
                            }
                            for (label, b64, raw_len) in decoded_batch {
                                mgr.add_attachment_at_cursor(
                                    label,
                                    b64,
                                    true,
                                    Some(raw_len),
                                    Some(&entry_fallback),
                                );
                            }
                        });
                    } else {
                        insert_pasted_text(&self_clone, &entry_clone, &text_str);
                    }
                }
            }

            if !handled {
                let self_clone_img = self_clone.clone();
                let entry_for_img = entry_clone.clone();
                clipboard_clone.read_texture_async(None::<&gio::Cancellable>, move |res| {
                    if self_clone_img.generation.get() != expected_gen {
                        return;
                    }
                    if let Ok(Some(texture)) = res {
                        self_clone_img.attach_texture_async(texture, entry_for_img, expected_gen);
                    }
                });
            }
        });
    }

    fn attach_texture_async(
        &self,
        texture: gdk::Texture,
        entry: gtk::TextView,
        expected_gen: u64,
    ) {
        let mgr = self.clone();
        glib::spawn_future_local(async move {
            let encoded = boxxy_sys_utils::runtime()
                .spawn_blocking(move || {
                    let bytes = texture.save_to_png_bytes();
                    let raw_len = bytes.len();
                    if raw_len > MAX_ATTACHMENT_FILE_BYTES {
                        return Err(format!(
                            "Pasted image exceeds the {} limit.",
                            format_limit_mb(MAX_ATTACHMENT_FILE_BYTES)
                        ));
                    }
                    Ok((glib::base64_encode(&bytes).to_string(), raw_len))
                })
                .await;

            if mgr.generation.get() != expected_gen {
                return;
            }
            match encoded {
                Ok(Ok((b64, raw_len))) => mgr.add_attachment_at_cursor(
                    PASTED_IMAGE_LABEL.to_string(),
                    b64,
                    true,
                    Some(raw_len),
                    Some(&entry),
                ),
                Ok(Err(msg)) => mgr.notify(msg),
                Err(e) => log::error!("Clipboard PNG encoding task panicked: {e}"),
            }
        });
    }

    #[must_use]
    pub fn handle_dropped_files(
        &self,
        files: Vec<gio::File>,
        entry: &gtk::TextView,
        working_dir: Option<&str>,
        is_incognito: bool,
    ) -> bool {
        if files.is_empty() {
            return false;
        }
        self.ingest_files(
            files,
            entry,
            working_dir,
            is_incognito,
            FileIngestSource::Drop,
        );
        true
    }

    fn ingest_files(
        &self,
        files: Vec<gio::File>,
        entry: &gtk::TextView,
        working_dir: Option<&str>,
        is_incognito: bool,
        source: FileIngestSource,
    ) {
        let mut text_tokens = Vec::new();
        let mut async_files = Vec::new();

        for file in files {
            if let Some(path) = file.path()
                && source == FileIngestSource::Drop
                && !is_supported_image_path(&path)
            {
                // For drag-and-drop inside a normal workspace session, keep `@rel/path`
                // tokens for folders or workspace paths without spaces so `file_refs.rs`
                // can list directories and register Turn-1 `replace_in_file` hashes.
                let is_dir = path.is_dir();
                let rel_opt = working_dir
                    .map(Path::new)
                    .and_then(|cwd| path.strip_prefix(cwd).ok())
                    .filter(|rel| !rel.as_os_str().is_empty());
                let chosen_path = rel_opt.unwrap_or(&path);
                let path_str = chosen_path.to_string_lossy();
                let has_spaces = path_str.chars().any(char::is_whitespace);

                if !is_incognito && !has_spaces && (is_dir || rel_opt.is_some()) {
                    text_tokens.push(format!("@{path_str}"));
                    continue;
                }
            }
            async_files.push(file);
        }

        entry.grab_focus();

        if !text_tokens.is_empty() {
            insert_tokens_at_cursor(entry, &text_tokens);
        }

        if !async_files.is_empty() {
            if async_files.len() > MAX_ATTACHMENTS_PER_MESSAGE {
                self.notify(format!(
                    "Only the first {MAX_ATTACHMENTS_PER_MESSAGE} files were attached (limit per message)."
                ));
                async_files.truncate(MAX_ATTACHMENTS_PER_MESSAGE);
            }
            let mgr = self.clone();
            let entry_for_async = entry.clone();
            let expected_gen = self.generation.get();
            glib::spawn_future_local(async move {
                for file in async_files {
                    if mgr.generation.get() != expected_gen {
                        break;
                    }
                    mgr.attach_single_gio_file(&file, &entry_for_async, expected_gen)
                        .await;
                }
            });
        }
    }

    // GIO async file stream futures run on the GTK thread-default main context (`!Send`).
    #[allow(clippy::future_not_send)]
    async fn attach_single_gio_file(
        &self,
        file: &gio::File,
        entry: &gtk::TextView,
        expected_gen: u64,
    ) {
        let label = resolve_gio_file_display_name(file).await;
        let bytes = match read_gio_file_bounded(file, MAX_ATTACHMENT_FILE_BYTES).await {
            Ok(b) => b,
            Err(FileReadReject::Directory) => {
                if self.generation.get() == expected_gen {
                    self.notify(format!(
                        "'{label}' is a folder — use @folder in the message box to reference directories."
                    ));
                }
                return;
            }
            Err(FileReadReject::Empty) => {
                if self.generation.get() == expected_gen {
                    self.notify(format!("'{label}' is empty and cannot be attached."));
                }
                return;
            }
            Err(FileReadReject::TooLarge { limit_bytes }) => {
                if self.generation.get() == expected_gen {
                    self.notify(format!(
                        "'{label}' exceeds the {} attachment size limit.",
                        format_limit_mb(limit_bytes)
                    ));
                }
                return;
            }
            Err(FileReadReject::IoError) => {
                if self.generation.get() == expected_gen {
                    self.notify(format!("Failed to read '{label}'."));
                }
                return;
            }
        };

        if self.generation.get() != expected_gen {
            return;
        }

        let raw_len = bytes.len();
        if has_supported_image_magic(&bytes) {
            let encoded = boxxy_sys_utils::runtime()
                .spawn_blocking(move || glib::base64_encode(&bytes).to_string())
                .await;
            if self.generation.get() != expected_gen {
                return;
            }
            if let Ok(b64) = encoded {
                self.add_attachment_at_cursor(label, b64, true, Some(raw_len), Some(entry));
            }
            return;
        }

        // If the filename had an image extension (including unsupported ones like `.bmp`, `.svg`,
        // `.avif`, `.heic`, `.ico`, `.tiff`) or looks like binary data, reject cleanly.
        if has_unsupported_image_or_media_extension(&label) || is_probably_binary(&bytes) {
            self.notify(format!(
                "Unsupported file '{label}' — attach PNG, JPEG, WebP, or GIF images, or UTF-8 text and code files."
            ));
            return;
        }

        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if text.len() > MAX_TEXT_SNIPPET_BYTES {
            let cut = text.floor_char_boundary(MAX_TEXT_SNIPPET_BYTES);
            text.truncate(cut);
            text.push_str("\n… [truncated to 512 KB]");
            self.notify(format!(
                "Attached '{label}' (large text file truncated to 512 KB)."
            ));
        }
        let text_len = text.len();
        self.add_attachment_at_cursor(label, text, false, Some(text_len), Some(entry));
    }
}

fn insert_tokens_at_cursor(entry: &gtk::TextView, text_tokens: &[String]) {
    let buf = entry.buffer();
    buf.begin_user_action();
    buf.delete_selection(true, true);
    let cursor_offset = buf.cursor_position();
    let needs_leading_space = if cursor_offset > 0 {
        let prev = buf.iter_at_offset(cursor_offset - 1);
        let cur = buf.iter_at_offset(cursor_offset);
        buf.text(&prev, &cur, false)
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_whitespace())
    } else {
        false
    };
    let needs_trailing_space = {
        let cur = buf.iter_at_offset(cursor_offset);
        if cur.is_end() {
            true
        } else {
            let next = buf.iter_at_offset(cursor_offset + 1);
            buf.text(&cur, &next, false)
                .chars()
                .next()
                .is_none_or(|c| !c.is_whitespace())
        }
    };
    let mut insertion = String::new();
    if needs_leading_space {
        insertion.push(' ');
    }
    insertion.push_str(&text_tokens.join(" "));
    if needs_trailing_space {
        insertion.push(' ');
    }
    buf.insert_at_cursor(&insertion);
    buf.end_user_action();
}

fn insert_pasted_text(mgr: &AttachmentManager, entry: &gtk::TextView, raw_text: &str) {
    let mut normalized = normalize_pasted_text(raw_text);
    if should_collapse_pasted_text(&normalized) {
        if normalized.len() > MAX_TEXT_SNIPPET_BYTES {
            let cut = normalized.floor_char_boundary(MAX_TEXT_SNIPPET_BYTES);
            normalized.truncate(cut);
            normalized.push_str("\n… [truncated]");
        }
        let snippet_len = normalized.len();
        mgr.add_attachment_at_cursor(
            TEXT_SNIPPET_LABEL.to_string(),
            normalized,
            false,
            Some(snippet_len),
            Some(entry),
        );
    } else {
        let buf = entry.buffer();
        buf.begin_user_action();
        buf.delete_selection(true, true);
        buf.insert_at_cursor(&normalized);
        buf.end_user_action();
    }
}
