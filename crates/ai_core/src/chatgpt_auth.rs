//! Native `ChatGPT` / Codex subscription OAuth management, status caching, and model spec resolution
//! built on [`rig::providers::chatgpt::auth`] and [`rig::client::signed_in::SignedInModel`].

use arc_swap::ArcSwap;
use base64::Engine as _;
use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use boxxy_model_selection::{
    catalog_get_spec, resolve_any_catalog_spec, set_chatgpt_account_models,
};
use parking_lot::Mutex;
use rig::catalog::ModelSpec;
use rig::completion::CacheRetention;
use rig::providers::auth::{AuthContext, DeviceCodeHandler, FileTokenStore, MemoryTokenStore};
use rig::providers::chatgpt::auth::{AuthSource, Authenticator, SignInMethod, SignInPrompt};
use rig::providers::registry::ProviderId;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Non-secret marker inserted into `Settings::get_effective_api_keys` under `"ChatGPT"`.
///
/// Signals that a `ChatGPT` OAuth session is active on disk so UI selectors treat the provider
/// as ready without exposing OAuth refresh or access tokens in `Settings`.
pub const CHATGPT_OAUTH_SENTINEL: &str = "__boxxy_chatgpt_oauth__";

const MAX_AUTH_FILE_BYTES: u64 = 64 * 1024;
const TOKEN_EXPIRY_SKEW_SECS: i64 = 60;
const MAX_DISPLAY_FIELD_CHARS: usize = 96;

/// Non-secret snapshot of the current `ChatGPT` / Codex authentication state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatGptAuthStatus {
    pub signed_in: bool,
    pub from_env_token: bool,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub account_id: Option<String>,
}

impl ChatGptAuthStatus {
    /// Formats a concise human-readable subtitle for the Preferences -> APIs Subscriptions row.
    #[must_use]
    pub fn format_subtitle(&self) -> String {
        if !self.signed_in {
            return "Use your ChatGPT Plus, Pro, or Team subscription (Codex) via OAuth."
                .to_owned();
        }
        if self.from_env_token {
            return "Connected via CHATGPT_ACCESS_TOKEN environment variable.".to_owned();
        }

        let plan_badge = self.plan_type.as_deref().map(format_plan_label);
        match (self.email.as_deref(), plan_badge.as_deref()) {
            (Some(email), Some(plan)) => format!("Connected · {email} ({plan})"),
            (Some(email), None) => format!("Connected · {email}"),
            (None, Some(plan)) => format!("Connected ({plan})"),
            (None, None) => "Connected".to_owned(),
        }
    }
}

/// Interactive sign-in method requested by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatGptSignInMode {
    /// Uses PKCE browser callback (`127.0.0.1:1455` / `1457`) when a local display is available,
    /// falling back automatically to device-code authorization if ports are busy or headless.
    BrowserWithDeviceFallback,
    /// Explicitly uses the device-code flow (`https://auth.openai.com/codex/device`).
    DeviceCodeOnly,
}

/// Real-time status events emitted while an interactive `ChatGPT` sign-in is in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatGptSignInPrompt {
    Browser {
        authorize_url: String,
        browser_launched: bool,
    },
    BrowserUnavailable {
        reason: String,
    },
    DeviceCode {
        verification_uri: String,
        user_code: String,
    },
}

#[derive(Debug, Deserialize, Default)]
struct StoredAuthRecord {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at: Option<i64>,
    account_id: Option<String>,
}

static CHATGPT_STATUS_CACHE: LazyLock<ArcSwap<Option<(PathBuf, ChatGptAuthStatus)>>> =
    LazyLock::new(|| ArcSwap::from_pointee(None));

static SHARED_AUTHENTICATOR: LazyLock<Mutex<Option<(PathBuf, Authenticator)>>> =
    LazyLock::new(|| Mutex::new(None));

fn format_plan_label(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "plus" => "Plus".to_owned(),
        "pro" => "Pro".to_owned(),
        "team" => "Team".to_owned(),
        "enterprise" => "Enterprise".to_owned(),
        "edu" => "Edu".to_owned(),
        "free" => "Free".to_owned(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(String::new, |first| {
                format!("{}{}", first.to_uppercase(), chars.as_str())
            })
        }
    }
}

fn sanitize_claim_field(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let cleaned: String = trimmed
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DISPLAY_FIELD_CHARS)
        .collect();
    let final_trimmed = cleaned.trim();
    (!final_trimmed.is_empty()).then(|| final_trimmed.to_owned())
}

fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
    let payload_b64 = token.trim().split('.').nth(1)?;
    let bytes = BASE64_URL_SAFE_NO_PAD.decode(payload_b64.as_bytes()).ok()?;
    serde_json::from_slice::<serde_json::Value>(&bytes).ok()
}

fn extract_jwt_identity(token: Option<&str>) -> (Option<String>, Option<String>, Option<String>) {
    let Some(claims) = token.and_then(decode_jwt_payload) else {
        return (None, None, None);
    };

    let email = claims
        .get("email")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            claims
                .get("https://api.openai.com/profile")
                .and_then(serde_json::Value::as_object)
                .and_then(|p| p.get("email"))
                .and_then(serde_json::Value::as_str)
        })
        .and_then(sanitize_claim_field);

    let auth_obj = claims
        .get("https://api.openai.com/auth")
        .and_then(serde_json::Value::as_object);

    let plan_type = auth_obj
        .and_then(|a| a.get("chatgpt_plan_type"))
        .and_then(serde_json::Value::as_str)
        .and_then(sanitize_claim_field);

    let account_id = auth_obj
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(serde_json::Value::as_str)
        .and_then(sanitize_claim_field);

    (email, plan_type, account_id)
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    let Some(exp) = expires_at else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    now.saturating_add(TOKEN_EXPIRY_SKEW_SECS) >= exp
}

/// Returns the directory holding OAuth token files (`<config_dir>/auth`).
#[must_use]
pub fn chatgpt_auth_dir_in(config_dir: &Path) -> PathBuf {
    config_dir.join("auth")
}

/// Inspects `ChatGPT` authentication status on disk for `config_dir` (and `CHATGPT_ACCESS_TOKEN`)
/// without making any network requests.
#[must_use]
pub fn inspect_chatgpt_auth_in(config_dir: &Path) -> ChatGptAuthStatus {
    if let Ok(env_token) = std::env::var("CHATGPT_ACCESS_TOKEN") {
        let trimmed = env_token.trim();
        if !trimmed.is_empty() {
            let (email, plan_type, jwt_acc) = extract_jwt_identity(Some(trimmed));
            let env_acc = std::env::var("CHATGPT_ACCOUNT_ID")
                .ok()
                .and_then(|v| sanitize_claim_field(&v));
            return ChatGptAuthStatus {
                signed_in: true,
                from_env_token: true,
                email,
                plan_type,
                account_id: env_acc.or(jwt_acc),
            };
        }
    }

    let path = chatgpt_auth_dir_in(config_dir).join("chatgpt.json");
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return ChatGptAuthStatus::default();
    };
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_AUTH_FILE_BYTES {
        return ChatGptAuthStatus::default();
    }
    let Ok(raw) = fs::read_to_string(&path) else {
        return ChatGptAuthStatus::default();
    };
    let Ok(record) = serde_json::from_str::<StoredAuthRecord>(&raw) else {
        return ChatGptAuthStatus::default();
    };

    let has_refresh = record
        .refresh_token
        .as_deref()
        .is_some_and(|t| !t.trim().is_empty());
    let has_live_access = record
        .access_token
        .as_deref()
        .is_some_and(|t| !t.trim().is_empty())
        && !is_token_expired(record.expires_at);

    if !has_refresh && !has_live_access {
        return ChatGptAuthStatus::default();
    }

    let (id_email, id_plan, id_acc) = extract_jwt_identity(record.id_token.as_deref());
    let (acc_email, acc_plan, acc_acc) = extract_jwt_identity(record.access_token.as_deref());

    ChatGptAuthStatus {
        signed_in: true,
        from_env_token: false,
        email: id_email.or(acc_email),
        plan_type: id_plan.or(acc_plan),
        account_id: record
            .account_id
            .as_deref()
            .and_then(sanitize_claim_field)
            .or(id_acc)
            .or(acc_acc),
    }
}

/// Returns the cached [`ChatGptAuthStatus`] for the active config directory (populating once if uninitialized).
#[must_use]
pub fn chatgpt_auth_status() -> ChatGptAuthStatus {
    let config_dir = boxxy_sys_utils::get_config_dir();
    if let Some((ref cached_dir, ref status)) = **CHATGPT_STATUS_CACHE.load()
        && cached_dir == &config_dir
    {
        return status.clone();
    }
    let fresh = inspect_chatgpt_auth_in(&config_dir);
    CHATGPT_STATUS_CACHE.store(Arc::new(Some((config_dir, fresh.clone()))));
    fresh
}

/// Returns `true` when `ChatGPT` is authenticated via stored OAuth credentials or `CHATGPT_ACCESS_TOKEN`.
#[must_use]
pub fn is_chatgpt_oauth_signed_in() -> bool {
    chatgpt_auth_status().signed_in
}

/// Re-evaluates `<config_dir>/auth/chatgpt.json` from disk and updates the lock-free status cache.
///
/// Drops any stale in-memory token on the shared [`Authenticator`] so cross-process
/// sign-in / sign-out transitions take effect immediately.
pub fn refresh_chatgpt_auth_status() -> ChatGptAuthStatus {
    let config_dir = boxxy_sys_utils::get_config_dir();
    if let Some((ref dir, ref auth)) = *SHARED_AUTHENTICATOR.lock()
        && dir == &config_dir
    {
        auth.forget();
    }
    let fresh = inspect_chatgpt_auth_in(&config_dir);
    if !fresh.signed_in {
        set_chatgpt_account_models(None);
    }
    CHATGPT_STATUS_CACHE.store(Arc::new(Some((config_dir, fresh.clone()))));
    fresh
}

/// Returns a process-wide [`Authenticator`] for the active `config_dir`.
///
/// Clones share the underlying [`FileTokenStore`], in-memory token cache, and refresh mutex,
/// with `allow_device_flow = false` so background agent turns fail cleanly instead of blocking
/// headless tasks if the stored refresh token was revoked.
#[must_use]
pub fn shared_chatgpt_authenticator() -> Authenticator {
    let config_dir = boxxy_sys_utils::get_config_dir();
    let mut guard = SHARED_AUTHENTICATOR.lock();
    if let Some((ref cached_dir, ref auth)) = *guard
        && cached_dir == &config_dir
    {
        return auth.clone();
    }

    let store = FileTokenStore::new(chatgpt_auth_dir_in(&config_dir));
    let auth = Authenticator::new(
        AuthSource::OAuth,
        store,
        DeviceCodeHandler::default(),
        false,
    )
    .with_app_name("Boxxy Terminal");
    *guard = Some((config_dir, auth.clone()));
    auth
}

/// Builds an [`Authenticator`] for an explicit static bearer access token (e.g. `CHATGPT_ACCESS_TOKEN`).
#[expect(
    clippy::redundant_pub_crate,
    reason = "workspace unreachable_pub = deny forbids pub items in private modules unless re-exported"
)]
#[must_use]
pub(crate) fn explicit_token_chatgpt_authenticator(access_token: String) -> Authenticator {
    let (_, _, jwt_account_id) = extract_jwt_identity(Some(&access_token));
    let account_id = std::env::var("CHATGPT_ACCOUNT_ID")
        .ok()
        .and_then(|v| sanitize_claim_field(&v))
        .or(jwt_account_id);
    Authenticator::new(
        AuthSource::AccessToken {
            access_token,
            account_id,
        },
        MemoryTokenStore::default(),
        DeviceCodeHandler::default(),
        false,
    )
}

/// Builds a [`ModelSpec`] targeted at the `"chatgpt"` subscription dialect for `model_id`.
///
/// Prefers the built-in `"chatgpt"` catalog entry, falling back to re-targeting a matching
/// canonical spec (such as `"openai"`), or synthesizing a tool-capable spec for newly rolled-out
/// account slugs returned by [`rig::client::chatgpt_models::account_models`].
///
/// # Panics
/// Panics only if Rig's built-in catalog were missing its own `"chatgpt"` vendor registration.
#[must_use]
pub fn build_chatgpt_model_spec(model_id: &str) -> ModelSpec {
    let trimmed = model_id.trim();
    let effective_id = if trimmed.is_empty() {
        rig::providers::chatgpt::DEFAULT_MODEL
    } else {
        trimmed
    };

    if let Some(spec) = catalog_get_spec("chatgpt", effective_id) {
        return spec.clone();
    }

    let chatgpt_pid = ProviderId::catalog(rig::providers::chatgpt::PROVIDER_NAME)
        .expect("rig builtin catalog registers 'chatgpt'");

    if let Some(base) = resolve_any_catalog_spec(effective_id) {
        let mut adapted = base.clone();
        effective_id.clone_into(&mut adapted.id);
        adapted.provider = chatgpt_pid;
        adapted.caching.retention = vec![CacheRetention::Long];
        adapted.pricing = None;
        return adapted;
    }

    ModelSpec::new(chatgpt_pid, effective_id).with_tools(true)
}

/// Queries the signed-in `ChatGPT` account's live `/backend-api/codex/models` list and updates
/// [`boxxy_model_selection::set_chatgpt_account_models`].
///
/// # Errors
/// Returns a human-readable error if no valid `ChatGPT` credential is available or the request fails.
pub async fn refresh_chatgpt_account_models() -> Result<Vec<String>, String> {
    let http = rig::rig_reqwest::shared();
    let context: AuthContext = if let Ok(env_tok) = std::env::var("CHATGPT_ACCESS_TOKEN")
        && !env_tok.trim().is_empty()
    {
        explicit_token_chatgpt_authenticator(env_tok.trim().to_owned())
            .auth_context(&http)
            .await
            .map_err(|e| e.to_string())?
    } else {
        shared_chatgpt_authenticator()
            .auth_context(&http)
            .await
            .map_err(|e| e.to_string())?
    };

    let slugs = rig::client::chatgpt_models::account_models(&context, &http)
        .await
        .map_err(|e| e.to_string())?;
    set_chatgpt_account_models(Some(slugs.clone()));
    Ok(slugs)
}

/// Runs an interactive `ChatGPT` OAuth sign-in flow (`Browser` PKCE or `DeviceCode`), persisting
/// the resulting credential to `<config_dir>/auth/chatgpt.json`.
///
/// Dropping the returned future cancels any active loopback listener on `127.0.0.1:1455`/`1457`.
///
/// # Errors
/// Returns a user-facing error message if authorization fails or times out.
pub async fn sign_in_chatgpt_oauth<FOpener, FPrompt>(
    mode: ChatGptSignInMode,
    opener: FOpener,
    on_prompt: FPrompt,
) -> Result<ChatGptAuthStatus, String>
where
    FOpener: Fn(&str) -> bool + Send + Sync + 'static,
    FPrompt: Fn(ChatGptSignInPrompt) + Send + Sync + 'static,
{
    let prompt_cb = Arc::new(on_prompt);
    let device_cb = Arc::clone(&prompt_cb);

    let auth = shared_chatgpt_authenticator()
        .with_app_name("Boxxy Terminal")
        .with_browser_opener(opener)
        .with_device_code_handler(DeviceCodeHandler::new(move |prompt| {
            device_cb(ChatGptSignInPrompt::DeviceCode {
                verification_uri: prompt.verification_uri,
                user_code: prompt.user_code,
            });
        }));

    let http = rig::rig_reqwest::shared();
    let sign_in_res = match mode {
        ChatGptSignInMode::DeviceCodeOnly => auth.sign_in_with_device_code(&http).await,
        ChatGptSignInMode::BrowserWithDeviceFallback => {
            let method = SignInMethod::detect(false);
            let browser_cb = Arc::clone(&prompt_cb);
            auth.sign_in(&http, method, move |prompt| match prompt {
                SignInPrompt::Browser(page) => {
                    browser_cb(ChatGptSignInPrompt::Browser {
                        authorize_url: page.authorize_url,
                        browser_launched: page.browser_launched,
                    });
                }
                SignInPrompt::BrowserUnavailable(reason) => {
                    browser_cb(ChatGptSignInPrompt::BrowserUnavailable { reason });
                }
            })
            .await
        }
    };

    let context = sign_in_res.map_err(|e| e.to_string())?;
    let status = refresh_chatgpt_auth_status();

    // Best-effort live model list warm-up using the freshly issued AuthContext.
    if let Ok(slugs) = rig::client::chatgpt_models::account_models(&context, &http).await {
        set_chatgpt_account_models(Some(slugs));
    }

    Ok(status)
}

/// Revokes stored `ChatGPT` OAuth tokens upstream (best-effort RFC 7009) and deletes
/// `<config_dir>/auth/chatgpt.json`.
///
/// # Errors
/// Returns an error if the local token store record could not be removed.
pub async fn sign_out_chatgpt_oauth() -> Result<ChatGptAuthStatus, String> {
    let http = rig::rig_reqwest::shared();
    shared_chatgpt_authenticator()
        .sign_out(&http)
        .await
        .map_err(|e| e.to_string())?;
    set_chatgpt_account_models(None);
    Ok(refresh_chatgpt_auth_status())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn make_synthetic_jwt(payload: &serde_json::Value) -> String {
        let header = BASE64_URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let body = BASE64_URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap());
        format!("{header}.{body}.sig")
    }

    #[test]
    #[serial(chatgpt_auth)]
    fn inspect_chatgpt_auth_parses_jwt_claims_and_handles_expiry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let auth_dir = tmp.path().join("auth");
        fs::create_dir_all(&auth_dir).expect("create auth dir");

        // 1. Missing file -> signed out
        let initial = inspect_chatgpt_auth_in(tmp.path());
        assert!(!initial.signed_in);
        assert_eq!(
            initial.format_subtitle(),
            "Use your ChatGPT Plus, Pro, or Team subscription (Codex) via OAuth."
        );

        // 2. Stored OAuth record with refresh token + OIDC id_token claims -> signed in
        let id_jwt = make_synthetic_jwt(&serde_json::json!({
            "email": "synthetic@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "plus",
                "chatgpt_account_id": "acct_synthetic_123"
            }
        }));
        fs::write(
            auth_dir.join("chatgpt.json"),
            serde_json::json!({
                "access_token": "expired-access",
                "refresh_token": "valid-refresh-token",
                "id_token": id_jwt,
                "expires_at": 1
            })
            .to_string(),
        )
        .expect("write chatgpt.json");

        let active = inspect_chatgpt_auth_in(tmp.path());
        assert!(active.signed_in);
        assert!(!active.from_env_token);
        assert_eq!(active.email.as_deref(), Some("synthetic@example.com"));
        assert_eq!(active.plan_type.as_deref(), Some("plus"));
        assert_eq!(active.account_id.as_deref(), Some("acct_synthetic_123"));
        assert_eq!(
            active.format_subtitle(),
            "Connected · synthetic@example.com (Plus)"
        );

        // 3. Expired access_token with NO refresh_token -> signed out
        fs::write(
            auth_dir.join("chatgpt.json"),
            serde_json::json!({
                "access_token": "expired-access",
                "expires_at": 1
            })
            .to_string(),
        )
        .expect("write expired chatgpt.json");

        let expired = inspect_chatgpt_auth_in(tmp.path());
        assert!(!expired.signed_in);
    }

    #[test]
    fn build_chatgpt_model_spec_handles_catalog_and_novel_slugs() {
        let builtin = build_chatgpt_model_spec("gpt-6.2-sol");
        assert_eq!(builtin.provider.vendor(), "chatgpt");
        assert_eq!(builtin.id, "gpt-6.2-sol");
        assert!(builtin.tools);

        let novel = build_chatgpt_model_spec("gpt-future-codex-preview");
        assert_eq!(novel.provider.vendor(), "chatgpt");
        assert_eq!(novel.id, "gpt-future-codex-preview");
        assert!(novel.tools);
    }
}
