use crate::model::{FetchError, NetErrorKind};
use crate::secret::Secret;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::time::Duration;

/// The only hosts QuotaGlass ever contacts. There is no API that takes a free-form URL.
/// `claude.ai` joins this list only if the gated session-key source ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Anthropic,
    ChatGpt,
}

impl Host {
    pub fn base(self) -> &'static str {
        match self {
            Host::Anthropic => "https://api.anthropic.com",
            Host::ChatGpt => "https://chatgpt.com",
        }
    }
}

pub enum HeaderValueSource<'a> {
    Static(&'static str),
    Bearer(&'a Secret),
    Plain(&'a str),
}

const MAX_BODY: usize = 1024 * 1024;

/// Builds `http://127.0.0.1:{port}{path}`. The host is fixed; `path` must be a plain absolute path.
/// Rejects a path without a leading `/`, and any `@`, `://`, backslash, whitespace or control character, so the
/// path can never name another host or user.
fn loopback_url(port: u16, path: &str) -> Result<String, FetchError> {
    let plain = path.starts_with('/')
        && !path.contains('@')
        && !path.contains("://")
        && !path
            .chars()
            .any(|c| c == '\\' || c.is_whitespace() || c.is_control());
    if port == 0 || !plain {
        return Err(FetchError::Network(NetErrorKind::Blocked));
    }
    Ok(format!("http://127.0.0.1:{port}{path}"))
}

/// The only plain-HTTP client: loopback only, no proxy, no redirects, 5 s timeout.
fn loopback_client() -> Result<reqwest::Client, FetchError> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .user_agent(concat!("QuotaGlass/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| FetchError::Network(NetErrorKind::Connect))
}

pub struct HttpClient {
    client: reqwest::Client,
    /// Used only through `loopback_url`, so it can reach nothing but `127.0.0.1`.
    loopback: reqwest::Client,
    #[cfg(test)]
    base_override: Option<String>,
}

impl HttpClient {
    pub fn new() -> Result<Self, FetchError> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("QuotaGlass/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| FetchError::Network(NetErrorKind::Connect))?;
        Ok(Self {
            client,
            loopback: loopback_client()?,
            #[cfg(test)]
            base_override: None,
        })
    }

    /// Test-only client that sends every request to `base` over plain HTTP.
    #[cfg(test)]
    pub(crate) fn for_test(base: &str) -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .expect("test client");
        Self {
            client,
            loopback: loopback_client().expect("loopback client"),
            base_override: Some(base.to_string()),
        }
    }

    fn url(&self, host: Host, path: &'static str) -> String {
        #[cfg(test)]
        if let Some(base) = &self.base_override {
            return format!("{base}{path}");
        }
        format!("{}{}", host.base(), path)
    }

    /// GET `http://127.0.0.1:{port}{path}`. Same redirect and size rules as `get_json`.
    pub async fn get_loopback(&self, port: u16, path: &str) -> Result<Vec<u8>, FetchError> {
        let url = loopback_url(port, path)?;
        let resp = self.loopback.get(url).send().await.map_err(map_err)?;
        read_body(resp).await
    }

    /// POST a JSON body to `http://127.0.0.1:{port}{path}`, with `x-codeium-csrf-token` when `csrf` is set.
    pub async fn post_loopback(
        &self,
        port: u16,
        path: &str,
        csrf: Option<&str>,
        body: &[u8],
    ) -> Result<Vec<u8>, FetchError> {
        let url = loopback_url(port, path)?;
        let mut req = self
            .loopback
            .post(url)
            .header("content-type", "application/json")
            .body(body.to_vec());
        if let Some(token) = csrf {
            let mut v = HeaderValue::from_str(token).map_err(|_| FetchError::Format("csrf"))?;
            v.set_sensitive(true);
            req = req.header("x-codeium-csrf-token", v);
        }
        let resp = req.send().await.map_err(map_err)?;
        read_body(resp).await
    }

    /// GET an allowlisted host. Redirects are errors. Bodies over 1 MiB are rejected.
    pub async fn get_json(
        &self,
        host: Host,
        path: &'static str,
        headers: &[(&'static str, HeaderValueSource<'_>)],
    ) -> Result<Vec<u8>, FetchError> {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            let value = match value {
                HeaderValueSource::Static(v) => HeaderValue::from_static(v),
                HeaderValueSource::Bearer(s) => {
                    // The formatted copy is wiped on drop; the header value itself is marked sensitive.
                    let bearer = zeroize::Zeroizing::new(format!("Bearer {}", s.expose()));
                    let mut v =
                        HeaderValue::from_str(&bearer).map_err(|_| FetchError::Format("token"))?;
                    v.set_sensitive(true);
                    v
                }
                HeaderValueSource::Plain(v) => {
                    HeaderValue::from_str(v).map_err(|_| FetchError::Format("header"))?
                }
            };
            map.insert(HeaderName::from_static(name), value);
        }
        let resp = self
            .client
            .get(self.url(host, path))
            .headers(map)
            .send()
            .await
            .map_err(map_err)?;
        read_body(resp).await
    }
}

/// Redirects are errors. Bodies over 1 MiB are rejected.
async fn read_body(mut resp: reqwest::Response) -> Result<Vec<u8>, FetchError> {
    let status = resp.status();
    if status.is_redirection() {
        return Err(FetchError::Network(NetErrorKind::Blocked));
    }
    if status.as_u16() == 429 {
        let retry_after_secs = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok()); // an HTTP-date gives None
        return Err(FetchError::RateLimited { retry_after_secs });
    }
    if !status.is_success() {
        return Err(FetchError::Network(NetErrorKind::Status(status.as_u16())));
    }
    if resp.content_length().is_some_and(|n| n as usize > MAX_BODY) {
        return Err(FetchError::Format("response too large"));
    }
    // Count while reading: a chunked body has no length up front.
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(map_err)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(FetchError::Format("response too large"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn map_err(e: reqwest::Error) -> FetchError {
    if e.is_timeout() {
        FetchError::Timeout
    } else if e.is_redirect() {
        FetchError::Network(NetErrorKind::Blocked)
    } else {
        FetchError::Network(NetErrorKind::Connect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockServer;

    #[test]
    fn hosts_are_https_allowlist() {
        for h in [Host::Anthropic, Host::ChatGpt] {
            assert!(h.base().starts_with("https://"));
        }
    }

    #[tokio::test]
    async fn http_redirect_is_blocked() {
        let server = MockServer::start(vec![(
            302,
            "Location: https://evil.example/\r\n",
            String::new(),
        )]);
        let http = HttpClient::for_test(&server.base);
        let err = http.get_json(Host::Anthropic, "/x", &[]).await.unwrap_err();
        assert_eq!(err, FetchError::Network(NetErrorKind::Blocked));
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn non_success_maps_status() {
        let server = MockServer::start(vec![(401, "", String::new())]);
        let http = HttpClient::for_test(&server.base);
        let err = http.get_json(Host::ChatGpt, "/x", &[]).await.unwrap_err();
        assert_eq!(err, FetchError::Network(NetErrorKind::Status(401)));
    }

    async fn status_of(status: u16, extra: &'static str) -> FetchError {
        let server = MockServer::start(vec![(status, extra, String::new())]);
        let http = HttpClient::for_test(&server.base);
        http.get_json(Host::Anthropic, "/x", &[]).await.unwrap_err()
    }

    #[tokio::test]
    async fn status_429_with_retry_after_maps_to_rate_limited() {
        assert_eq!(
            status_of(429, "Retry-After: 120\r\n").await,
            FetchError::RateLimited {
                retry_after_secs: Some(120)
            }
        );
    }

    #[tokio::test]
    async fn status_429_without_header_is_none() {
        assert_eq!(
            status_of(429, "").await,
            FetchError::RateLimited {
                retry_after_secs: None
            }
        );
    }

    #[tokio::test]
    async fn status_429_with_http_date_is_none() {
        assert_eq!(
            status_of(429, "Retry-After: Wed, 21 Oct 2015 07:28:00 GMT\r\n").await,
            FetchError::RateLimited {
                retry_after_secs: None
            }
        );
    }

    #[tokio::test]
    async fn other_status_unchanged() {
        assert_eq!(
            status_of(500, "Retry-After: 120\r\n").await,
            FetchError::Network(NetErrorKind::Status(500))
        );
    }

    #[test]
    fn loopback_url_is_fixed_host() {
        assert_eq!(loopback_url(51234, "/").unwrap(), "http://127.0.0.1:51234/");
        assert_eq!(
            loopback_url(80, "/exa.x/Get").unwrap(),
            "http://127.0.0.1:80/exa.x/Get"
        );
        let blocked = Err(FetchError::Network(NetErrorKind::Blocked));
        for path in [
            "",
            "x",
            "evil.example/",
            "/@evil.example",
            "/a@b",
            "/http://evil.example",
            "/a b",
            "/a\\b",
            "/a\r\nHost: x",
        ] {
            assert_eq!(loopback_url(1234, path), blocked, "{path:?}");
        }
        assert_eq!(loopback_url(0, "/"), blocked);
    }

    #[tokio::test]
    async fn loopback_get_and_post() {
        let server = MockServer::start(vec![
            (200, "", "page".to_string()),
            (200, "", "{}".to_string()),
        ]);
        let port: u16 = server.base.rsplit(':').next().unwrap().parse().unwrap();
        let http = HttpClient::new().unwrap();
        assert_eq!(http.get_loopback(port, "/").await.unwrap(), b"page");
        let body = http
            .post_loopback(port, "/svc/Call", Some("tok-1"), b"{}")
            .await
            .unwrap();
        assert_eq!(body, b"{}");
        let req = server.last_request.lock().unwrap().to_ascii_lowercase();
        assert!(req.starts_with("post /svc/call http/1.1"), "{req}");
        assert!(req.contains("x-codeium-csrf-token: tok-1"));
        assert!(req.contains("content-type: application/json"));
        assert!(req.ends_with("{}"));
    }

    #[tokio::test]
    async fn loopback_without_csrf_and_errors() {
        let server = MockServer::start(vec![
            (200, "", "{}".to_string()),
            (302, "Location: http://evil.example/\r\n", String::new()),
            (404, "", String::new()),
        ]);
        let port: u16 = server.base.rsplit(':').next().unwrap().parse().unwrap();
        let http = HttpClient::new().unwrap();
        http.post_loopback(port, "/x", None, b"{}").await.unwrap();
        let req = server.last_request.lock().unwrap().to_ascii_lowercase();
        assert!(!req.contains("x-codeium-csrf-token"));
        assert_eq!(
            http.get_loopback(port, "/").await,
            Err(FetchError::Network(NetErrorKind::Blocked))
        );
        assert_eq!(
            http.get_loopback(port, "/").await,
            Err(FetchError::Network(NetErrorKind::Status(404)))
        );
        assert_eq!(
            http.get_loopback(port, "nope").await,
            Err(FetchError::Network(NetErrorKind::Blocked))
        );
    }

    #[tokio::test]
    async fn loopback_closed_port_is_connect_error() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let http = HttpClient::new().unwrap();
        assert_eq!(
            http.get_loopback(port, "/").await,
            Err(FetchError::Network(NetErrorKind::Connect))
        );
    }

    #[test]
    fn release_client_refuses_plain_http() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = HttpClient::new().unwrap();
        let err = rt
            .block_on(client.client.get("http://127.0.0.1:9/").send())
            .unwrap_err();
        assert!(!err.is_status());
    }
}
