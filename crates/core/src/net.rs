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

pub struct HttpClient {
    client: reqwest::Client,
    #[cfg(test)]
    base_override: Option<String>,
}

impl HttpClient {
    pub fn new() -> Result<Self, FetchError> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("QuotaGlass/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| FetchError::Network(NetErrorKind::Connect))?;
        Ok(Self {
            client,
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
        let mut resp = self
            .client
            .get(self.url(host, path))
            .headers(map)
            .send()
            .await
            .map_err(map_err)?;
        let status = resp.status();
        if status.is_redirection() {
            return Err(FetchError::Network(NetErrorKind::Blocked));
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
