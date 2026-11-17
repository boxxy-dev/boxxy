//! Bounded read-only HTTP fetching tool (`http_fetch`) with SSRF redirect protection
//! and structure-preserving HTML text extraction.

mod html;

use crate::ApprovalHandler;
use crate::approval::{begin_tool, finish_tool};
use html::strip_html_tags;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

const MAX_HTML_RAW_BYTES: usize = 200 * 1024;
const MAX_OTHER_RAW_BYTES: usize = 1024 * 1024;
const MAX_EXTRACTED_HTML_CHARS: usize = 50_000;
const MAX_REDIRECT_HOPS: usize = 10;

/// Safe, high-signal response headers forwarded to the model (excluding `set-cookie` and noisy CDN/CSP headers).
const ALLOWED_RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "location",
    "last-modified",
    "etag",
    "cache-control",
    "retry-after",
    "www-authenticate",
];

const fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 64)
}

fn has_domain_tld_ignore_case(host: &str, tld: &str) -> bool {
    host.rsplit_once('.')
        .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case(tld))
}

pub(crate) fn is_local_or_private_url(url: &reqwest::Url) -> bool {
    let Some(host_str) = url.host_str() else {
        return false;
    };
    let trimmed_host = host_str.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = trimmed_host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => is_private_ipv4(v4),
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_unique_local()
                    || v6.is_unicast_link_local()
                    || v6.to_ipv4_mapped().is_some_and(is_private_ipv4)
            }
        };
    }

    trimmed_host.eq_ignore_ascii_case("localhost")
        || has_domain_tld_ignore_case(trimmed_host, "localhost")
        || has_domain_tld_ignore_case(trimmed_host, "local")
        || has_domain_tld_ignore_case(trimmed_host, "internal")
}

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECT_HOPS {
            return attempt.error("Too many HTTP redirects");
        }
        let target = attempt.url();
        if !matches!(target.scheme(), "http" | "https") {
            return attempt.error("Blocked redirect to non-HTTP scheme");
        }
        if let Some(origin) = attempt.previous().first()
            && !is_local_or_private_url(origin)
            && is_local_or_private_url(target)
        {
            return attempt
                .error("Blocked redirect from external URL to loopback or private network");
        }
        attempt.follow()
    });

    reqwest::Client::builder()
        .user_agent("Boxxy-Claw/0.1.0")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(redirect_policy)
        .build()
        .expect("Failed to initialize shared HTTP client")
});

#[derive(Deserialize)]
pub struct HttpFetchArgs {
    pub url: String,
    pub method: Option<String>,
}

#[derive(Serialize)]
pub struct HttpFetchOutput {
    pub status: u16,
    pub body: String,
    pub headers: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl HttpFetchOutput {
    fn err(msg: impl Into<String>) -> Self {
        Self {
            status: 0,
            body: String::new(),
            headers: HashMap::new(),
            error: Some(msg.into()),
        }
    }
}

fn validate_fetch_request(
    raw_url: &str,
    raw_method: Option<&str>,
) -> Result<(reqwest::Url, reqwest::Method), String> {
    let parsed_url = match reqwest::Url::parse(raw_url.trim()) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.host_str().is_some() => u,
        Ok(u) => {
            return Err(format!(
                "Unsupported URL scheme '{}': `http_fetch` only supports `http://` and `https://` URLs.",
                u.scheme()
            ));
        }
        Err(e) => return Err(format!("Invalid URL '{raw_url}': {e}")),
    };

    let method = match raw_method.map(str::trim) {
        None | Some("") => reqwest::Method::GET,
        Some(m) if m.eq_ignore_ascii_case("GET") => reqwest::Method::GET,
        Some(m) if m.eq_ignore_ascii_case("HEAD") => reqwest::Method::HEAD,
        Some(other) => {
            return Err(format!(
                "`http_fetch` is read-only and only allows `GET` or `HEAD` (got `{other}`). Use `terminal_exec` with `curl` for state-mutating requests so the user can approve them."
            ));
        }
    };

    Ok((parsed_url, method))
}

async fn read_bounded_response(mut response: reqwest::Response) -> HttpFetchOutput {
    let status = response.status().as_u16();
    let mut headers = HashMap::new();
    let mut content_type_lower = String::new();
    for (name, value) in response.headers() {
        let name_lower = name.as_str().to_ascii_lowercase();
        if ALLOWED_RESPONSE_HEADERS.contains(&name_lower.as_str())
            && let Ok(val_str) = value.to_str()
        {
            if name_lower == "content-type" {
                content_type_lower = val_str.to_ascii_lowercase();
            }
            headers.insert(name_lower, val_str.to_string());
        }
    }

    let is_html = content_type_lower.contains("text/html")
        || content_type_lower.contains("application/xhtml+xml");
    let max_bytes = if is_html {
        MAX_HTML_RAW_BYTES
    } else {
        MAX_OTHER_RAW_BYTES
    };

    let mut was_truncated = false;
    let mut body_bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body_bytes.len() + chunk.len() > max_bytes {
                    let remaining = max_bytes - body_bytes.len();
                    body_bytes.extend_from_slice(&chunk[..remaining]);
                    was_truncated = true;
                    break;
                }
                body_bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => {
                return HttpFetchOutput {
                    status,
                    body: String::from_utf8_lossy(&body_bytes).into_owned(),
                    headers,
                    error: Some(format!("Error reading HTTP response body: {e}")),
                };
            }
        }
    }

    let body = if is_html {
        let raw = String::from_utf8_lossy(&body_bytes);
        let text = strip_html_tags(&raw);
        if text.len() > MAX_EXTRACTED_HTML_CHARS {
            let cut = text.floor_char_boundary(MAX_EXTRACTED_HTML_CHARS);
            format!("{}\n\n[NOTE: Page text truncated to 50KB]", &text[..cut])
        } else if was_truncated {
            format!("{text}\n\n[NOTE: Source HTML was truncated before stripping]")
        } else {
            text
        }
    } else {
        let mut s = String::from_utf8_lossy(&body_bytes).into_owned();
        if was_truncated {
            s.push_str("\n\n[WARNING: Response body truncated at 1MB]");
        }
        s
    };

    HttpFetchOutput {
        status,
        body,
        headers,
        error: None,
    }
}

pub struct HttpFetchTool {
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for HttpFetchTool {
    const NAME: &'static str = "http_fetch";

    type Error = std::io::Error;
    type Args = HttpFetchArgs;
    type Output = HttpFetchOutput;

    fn description(&self) -> String {
        "Fetch the full text content of a URL (read-only GET or HEAD). Use this ONLY when: (a) the user provides a specific URL to read, or (b) `web_search` returned no useful answer and you must read a specific page in full. Do NOT use this to re-fetch or verify URLs already returned by `web_search` — those results are already summarised. Returns the status code, response body, and selected metadata headers.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The absolute URL to fetch (must start with http:// or https://)."
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "HEAD"],
                    "description": "The read-only HTTP method to use ('GET' or 'HEAD'). Defaults to 'GET'."
                }
            },
            "required": ["url"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let (parsed_url, method) = match validate_fetch_request(&args.url, args.method.as_deref()) {
            Ok(pair) => pair,
            Err(e) => {
                let out = HttpFetchOutput::err(e);
                finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
                return Ok(out);
            }
        };

        let out = match HTTP_CLIENT.request(method, parsed_url).send().await {
            Ok(resp) => read_bounded_response(resp).await,
            Err(e) => HttpFetchOutput::err(format!("HTTP Request failed: {e}")),
        };

        finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockApproval;

    #[test]
    fn private_url_classifier_detects_loopback_link_local_and_rfc1918() {
        for priv_url in [
            "http://localhost:3000/api",
            "http://127.0.0.1:8080/health",
            "http://[::1]:8080/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/admin",
            "http://192.168.1.1/",
        ] {
            let parsed = reqwest::Url::parse(priv_url).unwrap();
            assert!(is_local_or_private_url(&parsed), "expected private: {priv_url}");
        }

        let public = reqwest::Url::parse("https://example.com/docs").unwrap();
        assert!(!is_local_or_private_url(&public));
    }

    #[tokio::test]
    async fn http_fetch_rejects_mutating_methods_and_non_http_schemes() {
        let approval = Arc::new(MockApproval::approved(None));
        let tool = HttpFetchTool {
            approval: approval.clone(),
        };
        let mut ctx = ToolContext::default();

        let post_out = tool
            .call(
                &mut ctx,
                HttpFetchArgs {
                    url: "https://example.com/api".to_string(),
                    method: Some("DELETE".to_string()),
                },
            )
            .await
            .unwrap();
        assert!(post_out.error.unwrap().contains("read-only"));

        let file_scheme = tool
            .call(
                &mut ctx,
                HttpFetchArgs {
                    url: "file:///etc/hosts".to_string(),
                    method: None,
                },
            )
            .await
            .unwrap();
        assert!(file_scheme.error.unwrap().contains("Unsupported URL scheme"));
        assert_eq!(approval.results.lock().len(), 2);
    }
}
