use crate::config::Settings;
use crate::ui_utils::{SearchRow, open_uri, show_toast};
use adw::prelude::*;
use boxxy_ai_core::{
    ChatGptAuthStatus, ChatGptSignInMode, ChatGptSignInPrompt, chatgpt_auth_status,
    refresh_chatgpt_auth_status, sign_in_chatgpt_oauth, sign_out_chatgpt_oauth,
};
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Debug)]
enum CopyTarget {
    AuthorizeUrl(String),
    DeviceCode { code: String, uri: String },
}

enum UiSignInMsg {
    OpenBrowser(String),
    Prompt(ChatGptSignInPrompt),
    Finished(Result<ChatGptAuthStatus, String>),
}

#[derive(Clone)]
pub(super) struct ChatGptOAuthController {
    group: adw::PreferencesGroup,
    row: adw::ActionRow,
    spinner: adw::Spinner,
    copy_btn: gtk::Button,
    device_btn: gtk::Button,
    signin_btn: gtk::Button,
    cancel_btn: gtk::Button,
    signout_btn: gtk::Button,
    generation: Rc<Cell<u64>>,
    in_flight: Rc<Cell<bool>>,
    active_abort: Rc<RefCell<Option<tokio::task::AbortHandle>>>,
    copy_target: Rc<RefCell<Option<CopyTarget>>>,
}

impl ChatGptOAuthController {
    pub(super) fn setup(
        builder: &gtk::Builder,
        update_model_status: Rc<dyn Fn(&Settings)>,
    ) -> (Self, SearchRow) {
        let group: adw::PreferencesGroup = builder.object("group_subscriptions").unwrap();
        let row: adw::ActionRow = builder.object("chatgpt_oauth_row").unwrap();
        let spinner: adw::Spinner = builder.object("chatgpt_oauth_spinner").unwrap();
        let copy_btn: gtk::Button = builder.object("chatgpt_copy_btn").unwrap();
        let device_btn: gtk::Button = builder.object("chatgpt_device_btn").unwrap();
        let signin_btn: gtk::Button = builder.object("chatgpt_signin_btn").unwrap();
        let cancel_btn: gtk::Button = builder.object("chatgpt_cancel_btn").unwrap();
        let signout_btn: gtk::Button = builder.object("chatgpt_signout_btn").unwrap();

        row.set_use_markup(false);

        let search_row = SearchRow::new(
            &row,
            "chatgpt codex openai oauth subscription sign in login device code plus pro team",
        );

        let ctrl = Self {
            group,
            row,
            spinner,
            copy_btn,
            device_btn,
            signin_btn,
            cancel_btn,
            signout_btn,
            generation: Rc::new(Cell::new(0)),
            in_flight: Rc::new(Cell::new(false)),
            active_abort: Rc::new(RefCell::new(None)),
            copy_target: Rc::new(RefCell::new(None)),
        };

        ctrl.apply_idle_status(&chatgpt_auth_status());
        ctrl.wire_signals(update_model_status);
        (ctrl, search_row)
    }

    fn apply_idle_status(&self, status: &ChatGptAuthStatus) {
        self.in_flight.set(false);
        *self.copy_target.borrow_mut() = None;
        self.spinner.set_visible(false);
        self.copy_btn.set_visible(false);
        self.cancel_btn.set_visible(false);
        self.row.set_subtitle_selectable(false);
        self.row.set_subtitle(&status.format_subtitle());

        if status.signed_in {
            self.signin_btn.set_visible(false);
            self.device_btn.set_visible(false);
            // When connected exclusively via `CHATGPT_ACCESS_TOKEN`, sign-out is controlled by env.
            self.signout_btn.set_visible(!status.from_env_token);
            self.signout_btn.set_sensitive(true);
        } else {
            self.signin_btn.set_visible(true);
            self.signin_btn.set_sensitive(true);
            self.device_btn.set_visible(true);
            self.device_btn.set_sensitive(true);
            self.signout_btn.set_visible(false);
        }
    }

    fn handle_ui_sign_in_msg(
        &self,
        msg: UiSignInMsg,
        update_status: &Rc<dyn Fn(&Settings)>,
    ) -> bool {
        match msg {
            UiSignInMsg::OpenBrowser(url) => {
                open_uri(&self.row, &url);
                false
            }
            UiSignInMsg::Prompt(ChatGptSignInPrompt::Browser { authorize_url, .. }) => {
                *self.copy_target.borrow_mut() = Some(CopyTarget::AuthorizeUrl(authorize_url));
                self.copy_btn.set_label("Copy Link");
                self.copy_btn
                    .set_tooltip_text(Some("Copy the sign-in URL to your clipboard"));
                self.copy_btn.set_visible(true);
                self.row
                    .set_subtitle("Complete sign-in in your browser (listening on localhost)…");
                false
            }
            UiSignInMsg::Prompt(ChatGptSignInPrompt::BrowserUnavailable { .. }) => {
                self.device_btn.set_visible(false);
                self.copy_btn.set_visible(false);
                self.row
                    .set_subtitle("Local callback port is busy — switching to device code…");
                false
            }
            UiSignInMsg::Prompt(ChatGptSignInPrompt::DeviceCode {
                verification_uri,
                user_code,
            }) => {
                *self.copy_target.borrow_mut() = Some(CopyTarget::DeviceCode {
                    code: user_code.clone(),
                    uri: verification_uri.clone(),
                });
                self.device_btn.set_visible(false);
                self.copy_btn.set_label("Copy Code and Open");
                self.copy_btn
                    .set_tooltip_text(Some("Copy the device code and open the verification page"));
                self.copy_btn.set_visible(true);
                self.row.set_subtitle_selectable(true);
                self.row
                    .set_subtitle(&format!("Enter code {user_code} at {verification_uri}"));
                open_uri(&self.row, &verification_uri);
                false
            }
            UiSignInMsg::Finished(result) => {
                let _ = self.active_abort.borrow_mut().take();
                match result {
                    Ok(status) => {
                        self.apply_idle_status(&status);
                        let s = Settings::load();
                        update_status(&s);
                        show_toast(
                            &self.group,
                            "Connected to ChatGPT. Select a ChatGPT model in Models Selection.",
                        );
                    }
                    Err(err) => {
                        let status = chatgpt_auth_status();
                        self.apply_idle_status(&status);
                        show_toast(&self.group, &format!("ChatGPT sign-in failed: {err}"));
                    }
                }
                true
            }
        }
    }

    fn start_sign_in(&self, mode: ChatGptSignInMode, update_model_status: &Rc<dyn Fn(&Settings)>) {
        if let Some(prev) = self.active_abort.borrow_mut().take() {
            prev.abort();
        }
        let next_gen = self.generation.get().wrapping_add(1);
        self.generation.set(next_gen);
        self.in_flight.set(true);
        *self.copy_target.borrow_mut() = None;

        self.spinner.set_visible(true);
        self.signin_btn.set_visible(false);
        self.signout_btn.set_visible(false);
        self.copy_btn.set_visible(false);
        self.cancel_btn.set_visible(true);
        self.row.set_subtitle_selectable(false);

        match mode {
            ChatGptSignInMode::BrowserWithDeviceFallback => {
                self.device_btn.set_visible(true);
                self.device_btn.set_sensitive(true);
                self.row
                    .set_subtitle("Opening browser for ChatGPT sign-in…");
            }
            ChatGptSignInMode::DeviceCodeOnly => {
                self.device_btn.set_visible(false);
                self.row.set_subtitle("Requesting one-time device code…");
            }
        }

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiSignInMsg>();
        let tx_opener = tx.clone();
        let tx_prompt = tx.clone();

        let join_handle = boxxy_sys_utils::runtime().spawn(async move {
            let res = sign_in_chatgpt_oauth(
                mode,
                move |url| {
                    tx_opener
                        .send(UiSignInMsg::OpenBrowser(url.to_owned()))
                        .is_ok()
                },
                move |prompt| {
                    let _ = tx_prompt.send(UiSignInMsg::Prompt(prompt));
                },
            )
            .await;
            if res.is_ok() {
                Settings::notify_changed();
            }
            let _ = tx.send(UiSignInMsg::Finished(res));
        });

        *self.active_abort.borrow_mut() = Some(join_handle.abort_handle());

        let this = self.clone();
        let update_status = Rc::clone(update_model_status);
        glib::spawn_future_local(async move {
            while let Some(msg) = rx.recv().await {
                if this.generation.get() != next_gen
                    || this.handle_ui_sign_in_msg(msg, &update_status)
                {
                    break;
                }
            }
        });
    }

    fn wire_sign_out(&self, update_model_status: &Rc<dyn Fn(&Settings)>) {
        let this = self.clone();
        let update_status = Rc::clone(update_model_status);
        self.signout_btn.connect_clicked(move |btn| {
            btn.set_sensitive(false);
            let this_async = this.clone();
            let update_async = Rc::clone(&update_status);
            glib::spawn_future_local(async move {
                let was_default_broken =
                    Settings::load().default_model_broken_reason().is_some();
                let res = boxxy_sys_utils::runtime()
                    .spawn(async { sign_out_chatgpt_oauth().await })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));

                match res {
                    Ok(status) => {
                        this_async.apply_idle_status(&status);
                        let mut reverted_count = 0usize;
                        let updated = Settings::update(|s| {
                            s.api_keys.remove("ChatGPT");
                            reverted_count = s.revert_non_default_invalid_models();
                        });
                        Settings::notify_changed();
                        update_async(&updated);

                        if !was_default_broken
                            && let Some(reason) = updated.default_model_broken_reason()
                        {
                            show_toast(
                                &this_async.group,
                                &format!("Default model is broken: {reason}"),
                            );
                        } else if reverted_count > 0 {
                            show_toast(
                                &this_async.group,
                                &format!(
                                    "Signed out of ChatGPT — {reverted_count} character override(s) reverted to default model."
                                ),
                            );
                        } else {
                            show_toast(&this_async.group, "Signed out of ChatGPT.");
                        }
                    }
                    Err(err) => {
                        this_async.apply_idle_status(&chatgpt_auth_status());
                        show_toast(
                            &this_async.group,
                            &format!("Failed to sign out of ChatGPT: {err}"),
                        );
                    }
                }
            });
        });
    }

    fn wire_signals(&self, update_model_status: Rc<dyn Fn(&Settings)>) {
        {
            let this = self.clone();
            let update_status = Rc::clone(&update_model_status);
            self.signin_btn.connect_clicked(move |_| {
                this.start_sign_in(ChatGptSignInMode::BrowserWithDeviceFallback, &update_status);
            });
        }

        {
            let this = self.clone();
            let update_status = Rc::clone(&update_model_status);
            self.device_btn.connect_clicked(move |_| {
                this.start_sign_in(ChatGptSignInMode::DeviceCodeOnly, &update_status);
            });
        }

        {
            let this = self.clone();
            self.cancel_btn.connect_clicked(move |_| {
                let next_gen = this.generation.get().wrapping_add(1);
                this.generation.set(next_gen);
                if let Some(handle) = this.active_abort.borrow_mut().take() {
                    handle.abort();
                }
                this.apply_idle_status(&chatgpt_auth_status());
            });
        }

        {
            let this = self.clone();
            self.copy_btn.connect_clicked(move |_| {
                let target = this.copy_target.borrow().clone();
                match target {
                    Some(CopyTarget::AuthorizeUrl(url)) => {
                        this.row.clipboard().set_text(&url);
                        show_toast(&this.group, "Sign-in URL copied to clipboard.");
                    }
                    Some(CopyTarget::DeviceCode { code, uri }) => {
                        this.row.clipboard().set_text(&code);
                        open_uri(&this.row, &uri);
                        show_toast(
                            &this.group,
                            &format!("Device code {code} copied to clipboard."),
                        );
                    }
                    None => {}
                }
            });
        }

        self.wire_sign_out(&update_model_status);

        // Refresh disk status on map when no sign-in is in flight so external sign-ins/outs reflect immediately.
        {
            let this = self.clone();
            let update_status = update_model_status;
            self.row.connect_map(move |_| {
                if this.in_flight.get() {
                    return;
                }
                let this_map = this.clone();
                let update_map = Rc::clone(&update_status);
                glib::spawn_future_local(async move {
                    let status = tokio::task::spawn_blocking(refresh_chatgpt_auth_status)
                        .await
                        .unwrap_or_default();
                    if !this_map.in_flight.get() {
                        this_map.apply_idle_status(&status);
                        update_map(&Settings::load());
                    }
                });
            });
        }
    }

    pub(super) fn sync(&self) {
        if !self.in_flight.get() {
            self.apply_idle_status(&chatgpt_auth_status());
        }
    }

    pub(super) fn apply_filter(&self, search_row: &SearchRow, query: &str) -> bool {
        let visible = search_row.apply(query);
        self.group.set_visible(visible);
        visible
    }
}
