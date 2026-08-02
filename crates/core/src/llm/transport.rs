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
const TRANSPORT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSPORT_TCP_KEEPALIVE: Duration = Duration::from_secs(30);
const H2_DOWNGRADE_COOLDOWN: Duration = Duration::from_secs(300);
const MAX_POOLED_TRANSPORTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpTransportMode {
    Adaptive,
    Http1,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ProviderPoolKey {
    provider: &'static str,
    endpoint: String,
    credential_fingerprint: u64,
    proxy_profile: &'static str,
    transport_profile: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TransportPoolKey {
    endpoint: String,
    proxy_profile: &'static str,
    transport_profile: &'static str,
}

impl TransportPoolKey {
    fn from_config(config: &ProviderConfig) -> Self {
        let provider_key = ProviderPoolKey::from_config(config);
        Self {
            endpoint: provider_key.endpoint,
            proxy_profile: provider_key.proxy_profile,
            transport_profile: provider_key.transport_profile,
        }
    }
}

impl ProviderPoolKey {
    pub(crate) fn from_config(config: &ProviderConfig) -> Self {
        let endpoint = normalized_endpoint(config);
        let initial_mode = initial_transport_mode(config.provider_type, &endpoint);
        Self {
            provider: provider_key(config.provider_type),
            endpoint,
            credential_fingerprint: credential_fingerprint(config.api_key.as_deref()),
            // ProviderConfig does not yet expose a proxy object. Keeping the
            // dimension explicit prevents an unsafe cache merge when it does.
            proxy_profile: "direct",
            transport_profile: match initial_mode {
                HttpTransportMode::Adaptive => "adaptive",
                HttpTransportMode::Http1 => "http1",
            },
        }
    }
}

/// Shared connection transport for provider adapters.
///
/// `reqwest::Client` is intentionally cheap to clone and owns its connection
/// pool internally. Providers retain this shared wrapper so turns and delegated
/// workers reuse warm DNS/TCP/TLS state instead of constructing a client each
/// time. The adaptive lane allows ALPN negotiation; repeated HTTP/2 stream
/// resets switch future requests for this endpoint identity to HTTP/1.1.
pub(crate) struct HttpTransport {
    adaptive_client: reqwest::Client,
    http1_client: reqwest::Client,
    mode: AtomicU8,
    h2_reset_failures: AtomicU32,
    downgraded_until: Mutex<Option<std::time::Instant>>,
}

impl HttpTransport {
    fn new(initial_mode: HttpTransportMode) -> Result<Self, CoreError> {
        let adaptive_client = base_client_builder().build().map_err(|error| {
            CoreError::Llm(format!("Failed to create adaptive HTTP client: {error}"))
        })?;
        let http1_client = base_client_builder()
            .http1_only()
            .build()
            .map_err(|error| {
                CoreError::Llm(format!("Failed to create HTTP/1.1 client: {error}"))
            })?;
        Ok(Self {
            adaptive_client,
            http1_client,
            mode: AtomicU8::new(match initial_mode {
                HttpTransportMode::Adaptive => ADAPTIVE_MODE,
                HttpTransportMode::Http1 => HTTP1_MODE,
            }),
            h2_reset_failures: AtomicU32::new(0),
            downgraded_until: Mutex::new(None),
        })
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

    pub(crate) fn record_transport_failure(&self, message: &str) {
        if self.mode() == HttpTransportMode::Http1 || !is_h2_stream_reset(message) {
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
    let transport = Arc::new(HttpTransport::new(initial_transport_mode(
        config.provider_type,
        &key.endpoint,
    ))?);
    pool.entries.insert(
        key,
        TransportPoolEntry {
            transport: Arc::clone(&transport),
            last_used,
        },
    );
    Ok(transport)
}

fn base_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(TRANSPORT_CONNECT_TIMEOUT)
        .pool_idle_timeout(TRANSPORT_IDLE_TIMEOUT)
        .pool_max_idle_per_host(32)
        .tcp_keepalive(TRANSPORT_TCP_KEEPALIVE)
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

fn credential_fingerprint(api_key: Option<&str>) -> u64 {
    let mut hasher = DefaultHasher::new();
    api_key.unwrap_or_default().hash(&mut hasher);
    hasher.finish()
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

fn is_h2_stream_reset(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("rst_stream")
        || normalized.contains("http/2 stream") && normalized.contains("reset")
        || normalized.contains("h2") && normalized.contains("stream reset")
}

fn provider_key(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::OpenAi => "openai",
        ProviderType::OpenRouter => "openrouter",
        ProviderType::Anthropic => "anthropic",
        ProviderType::Google => "google",
        ProviderType::DeepSeek => "deepseek",
        ProviderType::Ollama => "ollama",
        ProviderType::LmStudio => "lmstudio",
        ProviderType::AzureOpenAi => "azureOpenAi",
        ProviderType::Zhipu => "zhipu",
        ProviderType::Moonshot => "moonshot",
        ProviderType::Qwen => "qwen",
        ProviderType::AlibabaModelStudio => "alibabaModelStudio",
        ProviderType::SiliconFlow => "siliconFlow",
        ProviderType::Doubao => "doubao",
        ProviderType::Yi => "yi",
        ProviderType::Baichuan => "baichuan",
        ProviderType::Custom => "custom",
    }
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
    use std::sync::Arc;

    use super::{
        shared_http_transport, HttpTransportMode, ProviderPoolKey, H2_RESET_DOWNGRADE_THRESHOLD,
    };
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
        }
    }

    #[test]
    fn pool_key_isolates_endpoint_and_credential_without_exposing_secret() {
        let first = ProviderPoolKey::from_config(&config(
            ProviderType::OpenAi,
            Some("https://api.openai.com/v1/"),
            "secret-one",
        ));
        let same = ProviderPoolKey::from_config(&config(
            ProviderType::OpenAi,
            Some("https://api.openai.com/v1"),
            "secret-one",
        ));
        let other_credential = ProviderPoolKey::from_config(&config(
            ProviderType::OpenAi,
            Some("https://api.openai.com/v1"),
            "secret-two",
        ));

        assert_eq!(first, same);
        assert_ne!(first, other_credential);
        assert!(!format!("{first:?}").contains("secret-one"));
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

        assert_eq!(transport.mode(), HttpTransportMode::Adaptive);
        for _ in 0..H2_RESET_DOWNGRADE_THRESHOLD {
            transport.record_transport_failure("stream closed by HTTP/2 RST_STREAM");
        }
        assert_eq!(transport.mode(), HttpTransportMode::Http1);
    }

    #[test]
    fn successful_completion_resets_the_h2_failure_streak() {
        let transport = shared_http_transport(&config(
            ProviderType::OpenAi,
            Some("https://api.openai.com/v1/completion-reset-test"),
            "completion-reset-key",
        ))
        .expect("transport");

        transport.record_transport_failure("stream closed by HTTP/2 RST_STREAM");
        transport.record_transport_success();
        transport.record_transport_failure("stream closed by HTTP/2 RST_STREAM");

        assert_eq!(transport.mode(), HttpTransportMode::Adaptive);
        transport.record_transport_failure("stream closed by HTTP/2 RST_STREAM");
        assert_eq!(transport.mode(), HttpTransportMode::Http1);
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

        assert_eq!(custom.mode(), HttpTransportMode::Http1);
        assert_eq!(local.mode(), HttpTransportMode::Http1);
    }
}
