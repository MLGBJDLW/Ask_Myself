use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use super::{ProviderConfig, ProviderType};
use crate::error::CoreError;

pub(crate) const H2_RESET_DOWNGRADE_THRESHOLD: u32 = 2;
const ADAPTIVE_MODE: u8 = 0;
const HTTP1_MODE: u8 = 1;
const TRANSPORT_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const TRANSPORT_TCP_KEEPALIVE: Duration = Duration::from_secs(30);
const H2_DOWNGRADE_COOLDOWN: Duration = Duration::from_secs(300);
const MAX_POOLED_TRANSPORTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HttpTransportMode {
    Adaptive,
    Http1,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TransportPoolKey {
    endpoint: String,
    transport_profile: HttpTransportMode,
    connect_timeout_ms: u64,
}

impl TransportPoolKey {
    fn from_config(config: &ProviderConfig) -> Self {
        let endpoint = normalized_endpoint(config);
        Self {
            transport_profile: initial_transport_mode(config.provider_type, &endpoint),
            endpoint,
            connect_timeout_ms: config
                .streaming
                .connect_timeout()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        }
    }
}

/// Registry/environment values are fingerprinted only; credentials are never
/// logged or used as a connection-pool identity. reqwest owns proxy parsing.
fn proxy_settings_fingerprint() -> u64 {
    let mut hash = DefaultHasher::new();
    for key in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
        "REQUEST_METHOD",
    ] {
        key.hash(&mut hash);
        std::env::var_os(key).hash(&mut hash);
    }
    #[cfg(windows)]
    {
        let settings = windows_registry::CURRENT_USER
            .open("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
            .ok();
        for key in ["ProxyEnable", "AutoDetect"] {
            settings
                .as_ref()
                .and_then(|settings| settings.get_u32(key).ok())
                .hash(&mut hash);
        }
        for key in ["ProxyServer", "ProxyOverride", "AutoConfigURL"] {
            settings
                .as_ref()
                .and_then(|settings| settings.get_string(key).ok())
                .hash(&mut hash);
        }
    }
    hash.finish()
}

/// One endpoint owner. Each request leases a generation; a network-settings
/// change only replaces future clients and cannot alter a running SSE stream.
pub(crate) struct HttpTransport {
    initial_mode: HttpTransportMode,
    connect_timeout: Duration,
    direct: bool,
    current: Mutex<Option<(u64, Arc<HttpRequestTransport>)>>,
}

impl HttpTransport {
    fn new(
        initial_mode: HttpTransportMode,
        connect_timeout: Duration,
        direct: bool,
    ) -> Result<Self, CoreError> {
        let transport = Self {
            initial_mode,
            connect_timeout,
            direct,
            current: Mutex::new(None),
        };
        transport.for_request()?;
        Ok(transport)
    }

    pub(crate) fn for_request(&self) -> Result<Arc<HttpRequestTransport>, CoreError> {
        self.for_proxy_settings(proxy_settings_fingerprint(), || {
            HttpRequestTransport::new(self.initial_mode, self.connect_timeout, self.direct)
        })
    }

    fn for_proxy_settings(
        &self,
        fingerprint: u64,
        build: impl FnOnce() -> Result<HttpRequestTransport, CoreError>,
    ) -> Result<Arc<HttpRequestTransport>, CoreError> {
        let mut current = self
            .current
            .lock()
            .map_err(|_| CoreError::Internal("HTTP client generation lock poisoned".into()))?;
        if let Some((existing, transport)) = &*current {
            if *existing == fingerprint {
                return Ok(Arc::clone(transport));
            }
        }
        let transport = Arc::new(build()?);
        *current = Some((fingerprint, Arc::clone(&transport)));
        Ok(transport)
    }
}

/// Shared connection transport for provider adapters.
///
/// `reqwest::Client` is intentionally cheap to clone and owns its connection
/// pool internally. Providers retain this shared wrapper so turns and delegated
/// workers reuse warm DNS/TCP/TLS state instead of constructing a client each
/// time. The adaptive lane allows ALPN negotiation; repeated HTTP/2 stream
/// resets switch future requests for this endpoint identity to HTTP/1.1.
pub(crate) struct HttpRequestTransport {
    adaptive_client: reqwest::Client,
    http1_client: reqwest::Client,
    mode: AtomicU8,
    h2_reset_failures: AtomicU32,
    downgraded_until: Mutex<Option<std::time::Instant>>,
}

impl HttpRequestTransport {
    fn new(
        initial_mode: HttpTransportMode,
        connect_timeout: Duration,
        direct: bool,
    ) -> Result<Self, CoreError> {
        let builder = || {
            let builder = base_client_builder(connect_timeout);
            if direct {
                builder.no_proxy()
            } else {
                builder
            }
        };
        let adaptive_client = builder().build().map_err(|error| {
            CoreError::Llm(format!("Failed to create adaptive HTTP client: {error}"))
        })?;
        let http1_client = builder().http1_only().build().map_err(|error| {
            CoreError::Llm(format!("Failed to create HTTP/1.1 client: {error}"))
        })?;
        Ok(Self::with_clients(
            initial_mode,
            adaptive_client,
            http1_client,
        ))
    }

    fn with_clients(
        initial_mode: HttpTransportMode,
        adaptive_client: reqwest::Client,
        http1_client: reqwest::Client,
    ) -> Self {
        Self {
            adaptive_client,
            http1_client,
            mode: AtomicU8::new(match initial_mode {
                HttpTransportMode::Adaptive => ADAPTIVE_MODE,
                HttpTransportMode::Http1 => HTTP1_MODE,
            }),
            h2_reset_failures: AtomicU32::new(0),
            downgraded_until: Mutex::new(None),
        }
    }

    pub(crate) fn client(&self) -> reqwest::Client {
        match self.mode() {
            HttpTransportMode::Adaptive => self.adaptive_client.clone(),
            HttpTransportMode::Http1 => self.http1_client.clone(),
        }
    }

    pub(crate) fn mode(&self) -> HttpTransportMode {
        if self.mode.load(Ordering::Acquire) == HTTP1_MODE {
            let cooldown_expired = self
                .downgraded_until
                .lock()
                .ok()
                .and_then(|deadline| *deadline)
                .is_some_and(|deadline| std::time::Instant::now() >= deadline);
            if cooldown_expired {
                self.h2_reset_failures.store(0, Ordering::Release);
                self.mode.store(ADAPTIVE_MODE, Ordering::Release);
                if let Ok(mut deadline) = self.downgraded_until.lock() {
                    *deadline = None;
                }
                return HttpTransportMode::Adaptive;
            }
            HttpTransportMode::Http1
        } else {
            HttpTransportMode::Adaptive
        }
    }

    pub(crate) fn record_transport_failure(&self, error: &(dyn std::error::Error + 'static)) {
        if self.mode() == HttpTransportMode::Http1 || !has_h2_reset(error) {
            return;
        }
        let failures = self.h2_reset_failures.fetch_add(1, Ordering::AcqRel) + 1;
        if failures >= H2_RESET_DOWNGRADE_THRESHOLD {
            self.mode.store(HTTP1_MODE, Ordering::Release);
            if let Ok(mut deadline) = self.downgraded_until.lock() {
                *deadline = Some(std::time::Instant::now() + H2_DOWNGRADE_COOLDOWN);
            }
        }
    }

    pub(crate) fn record_transport_success(&self) {
        self.h2_reset_failures.store(0, Ordering::Release);
    }
}

struct TransportPoolEntry {
    transport: Arc<HttpTransport>,
    last_used: u64,
}

#[derive(Default)]
struct TransportPool {
    entries: HashMap<TransportPoolKey, TransportPoolEntry>,
    clock: u64,
}

static HTTP_TRANSPORT_POOL: OnceLock<Mutex<TransportPool>> = OnceLock::new();

pub(crate) fn shared_http_transport(
    config: &ProviderConfig,
) -> Result<Arc<HttpTransport>, CoreError> {
    let key = TransportPoolKey::from_config(config);
    let pool = HTTP_TRANSPORT_POOL.get_or_init(|| Mutex::new(TransportPool::default()));
    let mut pool = pool
        .lock()
        .map_err(|_| CoreError::Internal("provider HTTP transport pool lock poisoned".into()))?;
    pool.clock = pool.clock.saturating_add(1);
    let last_used = pool.clock;
    if let Some(entry) = pool.entries.get_mut(&key) {
        entry.last_used = last_used;
        return Ok(Arc::clone(&entry.transport));
    }

    if pool.entries.len() >= MAX_POOLED_TRANSPORTS {
        let oldest = pool
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            pool.entries.remove(&oldest);
        }
    }
    let transport = Arc::new(HttpTransport::new(
        initial_transport_mode(config.provider_type, &key.endpoint),
        config.streaming.connect_timeout(),
        is_loopback_endpoint(&key.endpoint),
    )?);
    pool.entries.insert(
        key,
        TransportPoolEntry {
            transport: Arc::clone(&transport),
            last_used,
        },
    );
    Ok(transport)
}

fn base_client_builder(connect_timeout: Duration) -> reqwest::ClientBuilder {
    // Do not let another dependency's TLS feature unification choose our
    // backend. Windows keeps the system certificate store; other platforms
    // retain the core runtime's Rustls configuration.
    #[cfg(windows)]
    let builder = reqwest::Client::builder().use_native_tls();
    #[cfg(not(windows))]
    let builder = reqwest::Client::builder().use_rustls_tls();
    builder
        .connect_timeout(connect_timeout)
        .pool_idle_timeout(TRANSPORT_IDLE_TIMEOUT)
        .pool_max_idle_per_host(32)
        .tcp_keepalive(TRANSPORT_TCP_KEEPALIVE)
}

fn is_loopback_endpoint(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint).ok().is_some_and(|url| {
        url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        })
    })
}

fn normalized_endpoint(config: &ProviderConfig) -> String {
    config
        .base_url
        .as_deref()
        .unwrap_or_else(|| default_endpoint(config.provider_type))
        .trim()
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn initial_transport_mode(provider_type: ProviderType, endpoint: &str) -> HttpTransportMode {
    if matches!(
        provider_type,
        ProviderType::Custom | ProviderType::Ollama | ProviderType::LmStudio
    ) {
        return HttpTransportMode::Http1;
    }
    if is_official_endpoint(endpoint) {
        HttpTransportMode::Adaptive
    } else {
        HttpTransportMode::Http1
    }
}

fn is_official_endpoint(endpoint: &str) -> bool {
    [
        "api.openai.com",
        "openrouter.ai",
        "api.anthropic.com",
        "googleapis.com",
        "api.deepseek.com",
        "open.bigmodel.cn",
        "api.moonshot.cn",
        "dashscope.aliyuncs.com",
        "api.siliconflow.cn",
        "volces.com",
        "volcengineapi.com",
        "api.lingyiwanwu.com",
        "api.baichuan-ai.com",
        "openai.azure.com",
    ]
    .iter()
    .any(|domain| endpoint.contains(domain))
}

fn has_h2_reset(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    for _ in 0..8 {
        let Some(error) = current else {
            break;
        };
        if error
            .downcast_ref::<h2::Error>()
            .is_some_and(h2::Error::is_reset)
            || is_h2_stream_reset(&error.to_string())
        {
            return true;
        }
        current = error.source();
    }
    false
}

/// Preserve bounded diagnostic causes before reqwest's wrapper is flattened.
/// No request URL credentials or query values may reach logs or Run Events.
pub(crate) fn describe_http_error(error: &reqwest::Error) -> String {
    let kind = if has_h2_reset(error) {
        "HTTP/2 stream reset"
    } else if error.is_timeout() {
        "HTTP timeout"
    } else if error.is_connect() {
        "HTTP connection failure"
    } else if error.is_body() {
        "HTTP body failure"
    } else {
        "HTTP request failure"
    };
    let mut messages = vec![kind.to_string()];
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    for _ in 0..8 {
        let Some(error) = current else {
            break;
        };
        let message: String = error.to_string().chars().take(512).collect();
        if messages.last() != Some(&message) {
            messages.push(message);
        }
        current = error.source();
    }
    static URL_AUTH: OnceLock<regex::Regex> = OnceLock::new();
    let scrubbed = URL_AUTH
        .get_or_init(|| {
            regex::Regex::new(r"(?i)((?:https?|socks5h?)://)[^/@\s]+@")
                .expect("URL userinfo pattern")
        })
        .replace_all(&messages.join(": "), "${1}[REDACTED]@")
        .into_owned();
    crate::sensitive_data::sanitize_diagnostic(&scrubbed, None)
}

fn is_h2_stream_reset(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("rst_stream")
        || normalized.contains("http/2 stream") && normalized.contains("reset")
        || normalized.contains("h2") && normalized.contains("stream reset")
}

fn default_endpoint(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::OpenAi | ProviderType::AzureOpenAi => "https://api.openai.com/v1",
        ProviderType::OpenRouter => "https://openrouter.ai/api/v1",
        ProviderType::Anthropic => "https://api.anthropic.com/v1",
        ProviderType::Google => "https://generativelanguage.googleapis.com/v1beta",
        ProviderType::DeepSeek => "https://api.deepseek.com/v1",
        ProviderType::Ollama => "http://127.0.0.1:11434",
        ProviderType::LmStudio => "http://127.0.0.1:1234/v1",
        ProviderType::Zhipu => "https://open.bigmodel.cn/api/paas/v4",
        ProviderType::Moonshot => "https://api.moonshot.cn/v1",
        ProviderType::Qwen | ProviderType::AlibabaModelStudio => {
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        }
        ProviderType::SiliconFlow => "https://api.siliconflow.cn/v1",
        ProviderType::Doubao => "https://ark.cn-beijing.volces.com/api/v3",
        ProviderType::Yi => "https://api.lingyiwanwu.com/v1",
        ProviderType::Baichuan => "https://api.baichuan-ai.com/v1",
        ProviderType::Custom => "custom://endpoint",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{shared_http_transport, HttpTransportMode, H2_RESET_DOWNGRADE_THRESHOLD};
    use crate::llm::{ProviderConfig, ProviderType};

    fn config(
        provider_type: ProviderType,
        base_url: Option<&str>,
        api_key: &str,
    ) -> ProviderConfig {
        ProviderConfig {
            provider_type,
            base_url: base_url.map(str::to_string),
            api_key: Some(api_key.to_string()),
            org_id: None,
            timeout_secs: None,
            streaming: Default::default(),
        }
    }

    fn request_clients(proxy: Option<&str>) -> super::HttpRequestTransport {
        let make = || {
            let builder = super::base_client_builder(std::time::Duration::from_secs(3)).no_proxy();
            let builder = match proxy {
                Some(proxy) => builder.proxy(reqwest::Proxy::all(proxy).unwrap()),
                None => builder,
            };
            builder.build().unwrap()
        };
        super::HttpRequestTransport::with_clients(HttpTransportMode::Adaptive, make(), make())
    }

    #[tokio::test]
    async fn proxy_changes_replace_future_clients_without_interrupting_existing_streams() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let first_proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second_proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = format!("http://{}", first_proxy.local_addr().unwrap());
        let b = format!("http://{}", second_proxy.local_addr().unwrap());
        let (finish_old, finish) = tokio::sync::oneshot::channel();
        let serve_a = tokio::spawn(async move {
            let (mut socket, _) = first_proxy.accept().await.unwrap();
            let mut request = [0; 4096];
            let read = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..read])
                .starts_with("GET http://provider.test/events "));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 8\r\nConnection: close\r\n\r\nold-").await.unwrap();
            finish.await.unwrap();
            socket.write_all(b"tail").await.unwrap();
        });
        let serve_b = tokio::spawn(async move {
            let (mut socket, _) = second_proxy.accept().await.unwrap();
            let mut request = [0; 4096];
            let read = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..read])
                .starts_with("GET http://provider.test/events "));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nnew")
                .await
                .unwrap();
        });
        let owner = super::HttpTransport {
            initial_mode: HttpTransportMode::Adaptive,
            connect_timeout: std::time::Duration::from_secs(3),
            direct: false,
            current: Mutex::new(None),
        };
        let old = owner
            .for_proxy_settings(1, || Ok(request_clients(Some(&a))))
            .unwrap();
        let warm = owner
            .for_proxy_settings(1, || panic!("unchanged proxy must reuse clients"))
            .unwrap();
        assert!(Arc::ptr_eq(&old, &warm));
        let mut response = old
            .client()
            .get("http://provider.test/events")
            .send()
            .await
            .unwrap();
        assert_eq!(response.chunk().await.unwrap().unwrap().as_ref(), b"old-");
        let fresh = owner
            .for_proxy_settings(2, || Ok(request_clients(Some(&b))))
            .unwrap();
        assert!(!Arc::ptr_eq(&old, &fresh));
        assert_eq!(
            fresh
                .client()
                .get("http://provider.test/events")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "new"
        );
        finish_old.send(()).unwrap();
        assert_eq!(response.text().await.unwrap(), "tail");
        for _ in 0..H2_RESET_DOWNGRADE_THRESHOLD {
            old.record_transport_failure(&std::io::Error::other("HTTP/2 stream reset"));
        }
        assert_eq!(old.mode(), HttpTransportMode::Http1);
        assert_eq!(
            fresh.mode(),
            HttpTransportMode::Adaptive,
            "late failures cannot poison a new proxy generation"
        );
        let direct = owner
            .for_proxy_settings(3, || Ok(request_clients(None)))
            .unwrap();
        assert!(
            !Arc::ptr_eq(&fresh, &direct),
            "bypass changes also create a new generation"
        );
        serve_a.await.unwrap();
        serve_b.await.unwrap();
    }

    #[tokio::test]
    async fn wrapped_reqwest_h2_resets_keep_their_cause_for_transport_policy() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut connection = h2::server::handshake(socket).await.unwrap();
            while let Some(request) = connection.accept().await {
                let (_, mut response) = request.unwrap();
                response.send_reset(h2::Reason::INTERNAL_ERROR);
            }
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .http2_prior_knowledge()
            .build()
            .unwrap();
        let generation = request_clients(None);
        for _ in 0..H2_RESET_DOWNGRADE_THRESHOLD {
            let error = client
                .get(format!("http://{address}/?token=private-query-token"))
                .send()
                .await
                .unwrap_err();
            let diagnostic = super::describe_http_error(&error);
            assert!(diagnostic.contains("HTTP/2 stream reset"), "{diagnostic}");
            assert!(!diagnostic.contains("private-query-token"));
            generation.record_transport_failure(&error);
        }
        assert_eq!(generation.mode(), HttpTransportMode::Http1);
        server.abort();
    }

    #[tokio::test]
    #[ignore = "unauthenticated public DeepSeek connectivity probe using current system proxy"]
    async fn public_deepseek_probe_uses_the_actual_provider_transport() {
        let owner = shared_http_transport(&config(
            ProviderType::DeepSeek,
            Some("https://api.deepseek.com"),
            "",
        ))
        .unwrap();
        let response = owner
            .for_request()
            .unwrap()
            .client()
            .get("https://api.deepseek.com/models")
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .unwrap();
        eprintln!(
            "provider_probe status={} remote={:?} version={:?}",
            response.status(),
            response.remote_addr(),
            response.version()
        );
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn local_model_endpoints_always_bypass_remote_http_proxies() {
        for endpoint in [
            "http://localhost:11434",
            "http://127.0.0.1:1234",
            "http://[::1]:11434",
        ] {
            assert!(super::is_loopback_endpoint(endpoint));
        }
        for endpoint in [
            "https://localhost.example.com",
            "https://api.deepseek.com",
            "http://192.168.1.10:11434",
        ] {
            assert!(!super::is_loopback_endpoint(endpoint));
        }
    }

    #[test]
    fn pool_reuses_transport_for_the_same_provider_identity() {
        let config = config(
            ProviderType::Anthropic,
            Some("https://api.anthropic.com"),
            "shared-key",
        );

        let first = shared_http_transport(&config).expect("first transport");
        let second = shared_http_transport(&config).expect("second transport");

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn pool_keeps_transport_warm_after_provider_lifetimes_end() {
        let config = config(
            ProviderType::OpenAi,
            Some("https://api.openai.com/v1/long-lived-pool-test"),
            "rotating-provider-instance",
        );
        let first = shared_http_transport(&config).expect("first transport");
        let retained = Arc::downgrade(&first);
        drop(first);

        let still_warm = retained
            .upgrade()
            .expect("the bounded pool must retain warm transports between turns");
        let next_turn = shared_http_transport(&config).expect("next-turn transport");

        assert!(Arc::ptr_eq(&still_warm, &next_turn));
    }

    #[test]
    fn bearer_credentials_share_transport_when_headers_are_per_request() {
        let first = shared_http_transport(&config(
            ProviderType::OpenAi,
            Some("https://api.openai.com/v1"),
            "credential-a",
        ))
        .expect("first transport");
        let second = shared_http_transport(&config(
            ProviderType::OpenAi,
            Some("https://api.openai.com/v1"),
            "credential-b",
        ))
        .expect("second transport");

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn official_endpoints_start_adaptive_and_downgrade_after_repeated_h2_resets() {
        let transport = shared_http_transport(&config(
            ProviderType::Google,
            Some("https://generativelanguage.googleapis.com/v1beta"),
            "h2-reset-key",
        ))
        .expect("transport");

        assert_eq!(
            transport.for_request().unwrap().mode(),
            HttpTransportMode::Adaptive
        );
        for _ in 0..H2_RESET_DOWNGRADE_THRESHOLD {
            transport
                .for_request()
                .unwrap()
                .record_transport_failure(&std::io::Error::other(
                    "stream closed by HTTP/2 RST_STREAM",
                ));
        }
        assert_eq!(
            transport.for_request().unwrap().mode(),
            HttpTransportMode::Http1
        );
    }

    #[test]
    fn successful_completion_resets_the_h2_failure_streak() {
        let transport = shared_http_transport(&config(
            ProviderType::OpenAi,
            Some("https://api.openai.com/v1/completion-reset-test"),
            "completion-reset-key",
        ))
        .expect("transport");

        transport
            .for_request()
            .unwrap()
            .record_transport_failure(&std::io::Error::other("stream closed by HTTP/2 RST_STREAM"));
        transport.for_request().unwrap().record_transport_success();
        transport
            .for_request()
            .unwrap()
            .record_transport_failure(&std::io::Error::other("stream closed by HTTP/2 RST_STREAM"));

        assert_eq!(
            transport.for_request().unwrap().mode(),
            HttpTransportMode::Adaptive
        );
        transport
            .for_request()
            .unwrap()
            .record_transport_failure(&std::io::Error::other("stream closed by HTTP/2 RST_STREAM"));
        assert_eq!(
            transport.for_request().unwrap().mode(),
            HttpTransportMode::Http1
        );
    }

    #[test]
    fn compatibility_and_local_endpoints_prefer_http1() {
        let custom = shared_http_transport(&config(
            ProviderType::Custom,
            Some("https://proxy.example.test/v1"),
            "custom-key",
        ))
        .expect("custom transport");
        let local = shared_http_transport(&config(
            ProviderType::Ollama,
            Some("http://127.0.0.1:11434"),
            "local-key",
        ))
        .expect("local transport");

        assert_eq!(
            custom.for_request().unwrap().mode(),
            HttpTransportMode::Http1
        );
        assert_eq!(
            local.for_request().unwrap().mode(),
            HttpTransportMode::Http1
        );
    }
}
