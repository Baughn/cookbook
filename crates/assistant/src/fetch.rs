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
    let html = match fetch.fetch(url).await {
        Ok(html) => html,
        Err(e) => return done(e, true),
    };
    let target = url.to_string();
    match run_with_deadline(EXTRACT_DEADLINE, move || extract::extract(&html, &target)).await {
        Ok(md) => done(md, false),
        Err(e) => done(e, true),
    }
}

/// Extraction's time budget. The fetch's own 20 s covers only the network;
/// Readability is superlinear in DOM depth, and the byte cap bounds bytes,
/// not work — a small, deeply nested page can grind for minutes.
const EXTRACT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// Run extraction work on a blocking thread under a deadline, so a
/// pathological page neither stalls the async runtime driving the
/// exchange nor holds the exchange open past its budget. On timeout the
/// worker is left to finish in the background (CPU work can't be
/// cancelled); the model gets an error result and the exchange moves on.
async fn run_with_deadline<F>(
    deadline: std::time::Duration,
    work: F,
) -> std::result::Result<String, String>
where
    F: FnOnce() -> std::result::Result<String, String> + Send + 'static,
{
    let work = tokio::task::spawn_blocking(work);
    match tokio::time::timeout(deadline, work).await {
        Ok(Ok(result)) => result,
        Ok(Err(join)) => Err(format!("extraction failed: {join}")),
        Err(_) => Err(format!(
            "page too complex to extract within {}s — try another source",
            deadline.as_secs(),
        )),
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
            // Canonicalize before matching: lowercase, and drop a single
            // trailing root dot. `localhost.` is the fully-qualified spelling
            // of `localhost` — the resolver strips the dot, so the textual
            // check has to as well or the suffix comparisons all miss.
            let d = d.to_ascii_lowercase();
            let d = d.strip_suffix('.').unwrap_or(&d);
            if d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || d.ends_with(".internal")
            {
                return Err(format!("refusing local host {d:?}"));
            }
            Ok(())
        }
        Some(url::Host::Ipv4(ip)) => reject_private_v4(ip),
        Some(url::Host::Ipv6(ip)) => {
            // An IPv4-mapped or IPv4-compatible literal reaches the v4 stack,
            // so it has to face the v4 predicate: ::ffff:127.0.0.1 is not
            // is_loopback() and clears both masks below, which made every
            // address the v4 arm refuses reachable by respelling it.
            if let Some(v4) = ip.to_ipv4_mapped().or_else(|| ip.to_ipv4()) {
                return reject_private_v4(v4);
            }
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

fn reject_private_v4(ip: std::net::Ipv4Addr) -> std::result::Result<(), String> {
    let shared_cgnat = ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]);
    if ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || shared_cgnat
        || ip.octets()[0] == 0
    {
        return Err(format!("refusing private address {ip}"));
    }
    Ok(())
}

/// Whether a redirect hop may be followed. Split out from the client's
/// closure so the policy is reachable from a test at all: the closure needs
/// a live `reqwest::redirect::Attempt`, which the suite has no way to build,
/// so neither the hop cap nor the re-validation had any coverage.
pub fn redirect_ok(next: &str, hops: usize) -> std::result::Result<(), String> {
    if hops > MAX_REDIRECTS {
        return Err("too many redirects".to_string());
    }
    validate_url(next)
}

/// Redirect hops allowed before we stop following.
pub const MAX_REDIRECTS: usize = 5;

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
            match redirect_ok(attempt.url().as_str(), attempt.previous().len()) {
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

    /// The motivating page is one paragraph in thousands of nested divs —
    /// Readability's work is superlinear in DOM depth, so the 2 MB byte
    /// cap bounds bytes but not work. The deadline mechanism is what's
    /// under test; the work is injected so the suite doesn't have to burn
    /// minutes of real Readability time to prove the cutoff.
    #[tokio::test]
    async fn overrunning_extraction_times_out_instead_of_stalling_the_exchange() {
        let err = run_with_deadline(std::time::Duration::from_millis(20), || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            Ok("never seen".into())
        })
        .await
        .unwrap_err();
        assert!(err.contains("too complex"), "{err}");

        // Work that fits its budget flows through untouched.
        let ok = run_with_deadline(std::time::Duration::from_secs(5), || Ok("article".into()))
            .await
            .unwrap();
        assert_eq!(ok, "article");
    }

    #[test]
    fn url_policy_rejects_the_obvious() {
        for bad in [
            "ftp://example.com/x",
            "file:///etc/passwd",
            "http://localhost/x",
            "http://foo.localhost/x",
            "http://printer.local/x",
            "http://db.internal/x",
            // A trailing root dot is a fully-qualified spelling the resolver
            // strips, so it must not walk past the suffix checks.
            "http://localhost./x",
            "http://foo.localhost./x",
            "http://printer.local./x",
            "http://db.internal./x",
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

    /// Every v4 literal the arm above refuses, respelled as IPv6. These all
    /// passed: `::ffff:127.0.0.1` has segment 0 == 0, so it is not
    /// `is_loopback()` and clears the unique-local and link-local masks, and
    /// an IP literal takes no DNS step on the way to the v4 stack.
    #[test]
    fn url_policy_rejects_v4_mapped_respellings() {
        for bad in [
            "http://[::ffff:127.0.0.1]/x",
            "http://[::ffff:10.0.0.1]/x",
            "http://[::ffff:192.168.1.1]/x",
            "http://[::ffff:169.254.169.254]/x", // cloud metadata
            "http://[::127.0.0.1]/x",            // deprecated v4-compatible
            "http://[::ffff:0.0.0.0]/x",
        ] {
            assert!(validate_url(bad).is_err(), "{bad} should be refused");
        }
    }

    /// Ranges that are not routable public destinations either.
    #[test]
    fn url_policy_rejects_the_less_obvious_v4() {
        for bad in [
            "http://0.0.0.0/x",
            "http://0.1.2.3/x",             // 0.0.0.0/8
            "http://100.64.0.1/x",          // CGNAT
            "http://255.255.255.255/x",     // broadcast
            "http://192.0.2.1/x",           // TEST-NET-1
        ] {
            assert!(validate_url(bad).is_err(), "{bad} should be refused");
        }
        assert!(validate_url("http://100.128.0.1/x").is_ok(), "just past CGNAT is public");
    }

    /// The hop cap and the per-hop re-validation are the two controls the
    /// spec names, and neither was reachable from a test before `redirect_ok`
    /// existed — the closure needs a live `Attempt`.
    #[test]
    fn redirects_are_capped_and_revalidated() {
        assert!(redirect_ok("https://example.com/x", 0).is_ok());
        assert!(redirect_ok("https://example.com/x", MAX_REDIRECTS).is_ok());
        assert!(redirect_ok("https://example.com/x", MAX_REDIRECTS + 1).is_err());

        // A public URL redirecting inward is the whole point of re-validating.
        assert!(redirect_ok("http://127.0.0.1/x", 1).is_err());
        assert!(redirect_ok("http://localhost./x", 1).is_err());
        assert!(redirect_ok("http://[::ffff:169.254.169.254]/x", 1).is_err());
        assert!(redirect_ok("file:///etc/passwd", 1).is_err());
    }
}
