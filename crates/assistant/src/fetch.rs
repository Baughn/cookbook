//! The web-fetch seam, sibling to [`crate::seam::Model`]: IO only, no
//! parsing. Drivers execute `fetch_url` calls through this trait — the
//! real client below in production, scripted closures in tests — and the
//! pure [`crate::extract`] pipeline turns what comes back into markdown.
//! Errors are model-facing strings: they land in error tool results.

use crate::extract;
use crate::turn::{ToolCall, ToolOutcome};

/// The tool drivers must intercept and route here instead of the store.
pub const FETCH_URL: &str = "fetch_url";

pub trait Fetch {
    fn fetch(
        &mut self,
        url: &str,
    ) -> impl Future<Output = std::result::Result<String, String>> + Send;
}

/// Run one `fetch_url` call: fetch, extract, wrap as a tool outcome.
pub async fn execute_fetch<F: Fetch>(fetch: &mut F, call: &ToolCall) -> ToolOutcome {
    let done = |content: String, is_error| ToolOutcome {
        tool_use_id: call.id.clone(),
        content,
        is_error,
    };
    let Some(url) = call.input.get("url").and_then(|v| v.as_str()) else {
        return done("fetch_url needs a url".into(), true);
    };
    match validate_url(url) {
        Ok(()) => {}
        Err(e) => return done(e, true),
    }
    match fetch.fetch(url).await.and_then(|html| extract::extract(&html, url)) {
        Ok(md) => done(md, false),
        Err(e) => done(e, true),
    }
}

/// Only http(s), only public-looking hosts. This is a personal server, not
/// a proxy: the point is to stop obvious mistakes (and obvious mischief),
/// not to be a bulletproof SSRF boundary.
pub fn validate_url(url: &str) -> std::result::Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("bad url {url:?}: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("refusing non-http(s) url {url:?}"));
    }
    match parsed.host() {
        None => Err(format!("bad url {url:?}: no host")),
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            if d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || d.ends_with(".internal")
            {
                return Err(format!("refusing local host {d:?}"));
            }
            Ok(())
        }
        Some(url::Host::Ipv4(ip)) => {
            if ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() {
                return Err(format!("refusing private address {ip}"));
            }
            Ok(())
        }
        Some(url::Host::Ipv6(ip)) => {
            let seg0 = ip.segments()[0];
            let local = ip.is_loopback()
                || ip.is_unspecified()
                || (seg0 & 0xfe00) == 0xfc00 // unique local
                || (seg0 & 0xffc0) == 0xfe80; // link local
            if local {
                return Err(format!("refusing private address {ip}"));
            }
            Ok(())
        }
    }
}

// ------------------------------------------------------------ HTTP impl --

/// Raw HTML larger than this is cut off; the extractor is best-effort on
/// a truncated document.
const MAX_HTML: usize = 2 * 1024 * 1024;

/// The production fetcher: rustls, 20 s budget, every redirect hop
/// re-validated against the same URL policy.
pub struct HttpFetch {
    http: reqwest::Client,
}

impl HttpFetch {
    #[allow(clippy::new_without_default)]
    pub fn new() -> HttpFetch {
        let redirects = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                return attempt.error("too many redirects");
            }
            match validate_url(attempt.url().as_str()) {
                Ok(()) => attempt.follow(),
                Err(e) => attempt.error(e),
            }
        });
        HttpFetch {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .redirect(redirects)
                .build()
                .expect("client construction is infallible with these options"),
        }
    }
}

impl Fetch for HttpFetch {
    async fn fetch(&mut self, url: &str) -> std::result::Result<String, String> {
        let mut resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| format!("fetch failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("fetch failed: HTTP {}", resp.status()));
        }
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| format!("fetch failed: {e}"))? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_HTML {
                break;
            }
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_policy_rejects_the_obvious() {
        for bad in [
            "ftp://example.com/x",
            "file:///etc/passwd",
            "http://localhost/x",
            "http://foo.localhost/x",
            "http://printer.local/x",
            "http://db.internal/x",
            "http://127.0.0.1/x",
            "http://10.1.2.3/x",
            "http://192.168.1.1/x",
            "http://169.254.1.1/x",
            "http://[::1]/x",
            "http://[fd00::1]/x",
            "http://[fe80::1]/x",
            "not a url",
        ] {
            assert!(validate_url(bad).is_err(), "{bad} should be refused");
        }
        for good in ["https://example.com/recipe", "http://93.184.216.34/x"] {
            assert!(validate_url(good).is_ok(), "{good} should pass");
        }
    }
}
