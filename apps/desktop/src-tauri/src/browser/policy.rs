use std::collections::HashSet;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub use nexa_core::browser_runtime::{
    classify_action_risk as classify_agent_action, BrowserActionRisk,
};
use nexa_core::tools::run_shell_tool::ManagedLoopbackPermit;
use url::{Host, Url};

const AGENT_DNS_RESOLUTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const SYNTHETIC_DNS_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const SYNTHETIC_DNS_POSITIVE_CACHE_TTL: Duration = Duration::from_secs(5);
const SYNTHETIC_DNS_NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(30);
const SYNTHETIC_DNS_CANARIES: [&str; 2] = ["www.google.com", "github.com"];

#[derive(Debug, Clone, Copy)]
struct SyntheticDnsProbeCache {
    checked_at: Instant,
    verified: bool,
}

#[derive(Debug, Default)]
struct SyntheticDnsProbeState {
    cached: Option<SyntheticDnsProbeCache>,
    in_flight: bool,
}

#[derive(Debug, Default)]
struct SyntheticDnsProbeCoordinator {
    state: Mutex<SyntheticDnsProbeState>,
    completed: Condvar,
}

fn fresh_synthetic_dns_result(cached: Option<SyntheticDnsProbeCache>) -> Option<bool> {
    let cached = cached?;
    let ttl = if cached.verified {
        SYNTHETIC_DNS_POSITIVE_CACHE_TTL
    } else {
        SYNTHETIC_DNS_NEGATIVE_CACHE_TTL
    };
    (cached.checked_at.elapsed() < ttl).then_some(cached.verified)
}

impl SyntheticDnsProbeCoordinator {
    fn complete(&self, verified: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.cached = Some(SyntheticDnsProbeCache {
                checked_at: Instant::now(),
                verified,
            });
            state.in_flight = false;
            self.completed.notify_all();
        }
    }

    fn verify_with<F>(self: &Arc<Self>, timeout: Duration, probe: F) -> bool
    where
        F: FnOnce() -> bool + Send + 'static,
    {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return false,
        };
        if let Some(verified) = fresh_synthetic_dns_result(state.cached) {
            return verified;
        }

        if !state.in_flight {
            state.in_flight = true;
            drop(state);
            let coordinator = Arc::clone(self);
            let spawned = std::thread::Builder::new()
                .name("nexa-synthetic-dns-probe".to_string())
                .spawn(move || coordinator.complete(probe()));
            if spawned.is_err() {
                self.complete(false);
            }
            state = match self.state.lock() {
                Ok(state) => state,
                Err(_) => return false,
            };
        }

        match self
            .completed
            .wait_timeout_while(state, timeout, |state| state.in_flight)
        {
            Ok((state, _)) => fresh_synthetic_dns_result(state.cached).unwrap_or(false),
            Err(_) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationActor {
    User,
    Agent,
}

pub(super) fn private_or_special_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [first, second, third, _] = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_documentation()
                || first == 0
                || (first == 100 && second & 0xc0 == 0x40)
                || (first == 192 && second == 0 && third == 0)
                || (first == 192 && second == 88 && third == 99)
                || (first == 198 && second & 0xfe == 18)
                || first >= 240
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return private_or_special_ip(IpAddr::V4(mapped));
            }
            let segments = ip.segments();
            let globally_routable_unicast = (segments[0] & 0xe000) == 0x2000;
            !globally_routable_unicast
                || (segments[0] == 0x2001 && segments[1] < 0x0200)
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || segments[0] == 0x2002
                || segments[0] == 0x3fff
        }
    }
}

/// RFC 2544's benchmarking range is commonly used by TUN proxies (for
/// example Clash fake-IP mode) as a synthetic DNS transport. It remains a
/// special address when entered literally; only a public domain that resolves
/// to this range may use it as a proxy hop.
pub(super) fn synthetic_dns_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [first, second, _, _] = ip.octets();
            first == 198 && second & 0xfe == 18
        }
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .is_some_and(|mapped| synthetic_dns_ip(IpAddr::V4(mapped))),
    }
}

fn canary_addresses_verify_synthetic_dns(
    canary_addresses: impl IntoIterator<Item = Vec<IpAddr>>,
) -> bool {
    let mut observed = 0_usize;
    for addresses in canary_addresses {
        if addresses.is_empty() || !addresses.iter().all(|address| synthetic_dns_ip(*address)) {
            return false;
        }
        observed += 1;
    }
    observed == SYNTHETIC_DNS_CANARIES.len()
}

fn probe_synthetic_dns_mode() -> bool {
    let canary_addresses = SYNTHETIC_DNS_CANARIES
        .iter()
        .map(|host| {
            (*host, 443)
                .to_socket_addrs()
                .map(|addresses| addresses.map(|address| address.ip()).collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    canary_addresses_verify_synthetic_dns(canary_addresses)
}

/// Positively identify a system fake-IP DNS mode before treating RFC 2544
/// addresses as synthetic transport hops. A target hostname alone is not
/// evidence: two fixed, unrelated public canaries must independently resolve
/// into the synthetic range. Results are briefly cached so the SOCKS boundary
/// can enforce this on every connection without repeatedly blocking on DNS.
pub(super) fn verified_synthetic_dns_mode() -> bool {
    static COORDINATOR: OnceLock<Arc<SyntheticDnsProbeCoordinator>> = OnceLock::new();
    COORDINATOR
        .get_or_init(|| Arc::new(SyntheticDnsProbeCoordinator::default()))
        .verify_with(SYNTHETIC_DNS_PROBE_TIMEOUT, probe_synthetic_dns_mode)
}

pub fn form_navigation_approval_key(url: &Url) -> String {
    let mut target = url.clone();
    target.set_query(None);
    target.set_fragment(None);
    format!("form:{}", target.as_str())
}

pub(super) fn agent_domain_host_allowed(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    !host.is_empty()
        && host.parse::<IpAddr>().is_err()
        && host != "localhost"
        && !host.ends_with(".localhost")
        && !host.ends_with(".local")
        && host != "metadata.google.internal"
}

fn agent_host_allowed(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => !private_or_special_ip(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => !private_or_special_ip(IpAddr::V6(ip)),
        Some(Host::Domain(host)) => agent_domain_host_allowed(host),
        None => false,
    }
}

pub(super) fn agent_resolved_address_allowed(
    url: &Url,
    ip: IpAddr,
    synthetic_dns_verified: bool,
) -> bool {
    !private_or_special_ip(ip)
        || (synthetic_dns_verified
            && matches!(url.host(), Some(Host::Domain(_)))
            && synthetic_dns_ip(ip))
}

pub fn normalize_browser_url(input: &str, actor: NavigationActor) -> Result<Url, String> {
    let url = normalize_browser_url_candidate(input)?;
    if actor == NavigationActor::Agent && !agent_host_allowed(&url) {
        return Err(
            "Agent browser navigation to a local or private network requires a live service started in this conversation with run_shell background:true and ready_url, or explicit user control."
                .to_string(),
        );
    }
    Ok(url)
}

pub(super) fn normalize_browser_url_candidate(input: &str) -> Result<Url, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter a URL or search query".to_string());
    }

    if input.eq_ignore_ascii_case("about:blank") {
        return Url::parse("about:blank").map_err(|_| "Invalid browser address".to_string());
    }

    let lowercase_input = input.to_ascii_lowercase();
    let explicit_scheme = input.contains("://")
        || lowercase_input.starts_with("javascript:")
        || lowercase_input.starts_with("data:")
        || lowercase_input.starts_with("file:");
    let candidate = if !explicit_scheme && !looks_like_browser_host(input) {
        let encoded: String = url::form_urlencoded::byte_serialize(input.as_bytes()).collect();
        format!("https://www.google.com/search?q={encoded}")
    } else if explicit_scheme {
        input.to_string()
    } else {
        format!("https://{input}")
    };

    let url = Url::parse(&candidate).map_err(|_| "Invalid browser address".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only HTTP and HTTPS pages can open in Nexa Browser".to_string());
    }
    Ok(url)
}

fn looks_like_browser_host(input: &str) -> bool {
    if input.chars().any(char::is_whitespace) || input.contains('@') {
        return false;
    }
    let authority = input
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('.');
    authority.eq_ignore_ascii_case("localhost")
        || authority.to_ascii_lowercase().ends_with(".localhost")
        || authority.contains('.')
        || (authority.starts_with('[') && authority.contains(']'))
        || authority.parse::<IpAddr>().is_ok()
        || authority
            .rsplit_once(':')
            .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
}

pub(super) fn managed_permit_matches_url(permit: &ManagedLoopbackPermit, url: &Url) -> bool {
    if !permit.is_live() {
        return false;
    }
    let Some(origin) = Url::parse(&permit.origin).ok() else {
        return false;
    };
    matches!(origin.scheme(), "http" | "https")
        && origin.origin().ascii_serialization() == permit.origin
        && url.origin() == origin.origin()
        && url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(&permit.host))
        && url.port_or_known_default() == Some(permit.port)
        && managed_loopback_host(&permit.host)
}

fn managed_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn resolved_is_loopback(address: IpAddr) -> bool {
    address.is_loopback()
        || matches!(address, IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some_and(|mapped| mapped.is_loopback()))
}

pub(super) async fn validate_agent_network_url_with_permit(
    url: &Url,
    permit: Option<&ManagedLoopbackPermit>,
) -> Result<(), String> {
    let managed_loopback = permit.is_some_and(|permit| managed_permit_matches_url(permit, url));
    if !agent_host_allowed(url) {
        if !managed_loopback {
            return Err(
                "Agent browser navigation to a local or private network requires a live service started in this conversation with run_shell background:true and ready_url, or explicit user control."
                    .to_string(),
            );
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| "Browser address has no host".to_string())?;
    let port = url.port_or_known_default().unwrap_or(443);
    let resolved = tokio::time::timeout(
        AGENT_DNS_RESOLUTION_TIMEOUT,
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| "Browser address resolution timed out".to_string())?
    .map_err(|_| "Could not resolve the browser address".to_string())?;
    let resolved = resolved.collect::<Vec<_>>();
    let resolved_any = !resolved.is_empty();
    let needs_synthetic_dns_evidence = !managed_loopback
        && resolved
            .iter()
            .any(|address| synthetic_dns_ip(address.ip()));
    let synthetic_dns_verified = if needs_synthetic_dns_evidence {
        tokio::task::spawn_blocking(verified_synthetic_dns_mode)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    for address in resolved {
        if (managed_loopback && !resolved_is_loopback(address.ip()))
            || (!managed_loopback
                && !agent_resolved_address_allowed(url, address.ip(), synthetic_dns_verified))
        {
            return Err(if managed_loopback {
                "Managed browser service resolved outside loopback".to_string()
            } else if matches!(url.host(), Some(Host::Domain(_))) {
                format!(
                    "Agent browser domain resolved to protected network address {}. RFC 2544 fake-IP transport is allowed only after Nexa verifies the active VPN/TUN DNS mode; other private ranges require explicit user-controlled network authority.",
                    address.ip()
                )
            } else {
                "Agent browser navigation resolved to a local or private network".to_string()
            });
        }
    }
    if !resolved_any {
        return Err("Browser address did not resolve to a public network".to_string());
    }
    Ok(())
}

pub fn navigation_allowed(url: &Url, agent_restricted: bool) -> bool {
    if url.scheme() == "about" {
        return url.as_str() == "about:blank";
    }
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    !agent_restricted || agent_host_allowed(url)
}

pub fn navigation_preapproved(
    url: &Url,
    agent_restricted: bool,
    approved_agent_urls: &mut HashSet<String>,
) -> bool {
    if !agent_restricted {
        return navigation_allowed(url, false);
    }
    if url.scheme() == "about" {
        return url.as_str() == "about:blank";
    }
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    approved_agent_urls.remove(url.as_str())
        || approved_agent_urls.remove(&form_navigation_approval_key(url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;

    #[test]
    fn fake_ip_mode_requires_independent_canary_evidence() {
        let google = vec!["198.18.0.8".parse().unwrap()];
        let github = vec!["198.18.0.25".parse().unwrap()];
        assert!(canary_addresses_verify_synthetic_dns([
            google.clone(),
            github.clone(),
        ]));
        assert!(!canary_addresses_verify_synthetic_dns([
            google.clone(),
            vec!["140.82.113.4".parse().unwrap()],
        ]));
        assert!(!canary_addresses_verify_synthetic_dns([
            google.clone(),
            vec![
                "198.18.0.25".parse().unwrap(),
                "140.82.113.4".parse().unwrap(),
            ],
        ]));
        assert!(!canary_addresses_verify_synthetic_dns([
            google,
            vec!["127.0.0.1".parse().unwrap(), github[0]],
        ]));
    }

    #[test]
    fn synthetic_dns_probe_is_single_flight_across_concurrent_connections() {
        const CALLERS: usize = 16;
        let coordinator = Arc::new(SyntheticDnsProbeCoordinator::default());
        let callers_ready = Arc::new(Barrier::new(CALLERS + 1));
        let release_probe = Arc::new(Barrier::new(2));
        let probe_calls = Arc::new(AtomicUsize::new(0));
        let (probe_started, wait_for_probe) = std::sync::mpsc::sync_channel(1);
        let mut callers = Vec::new();

        for _ in 0..CALLERS {
            let coordinator = Arc::clone(&coordinator);
            let callers_ready = Arc::clone(&callers_ready);
            let release_probe = Arc::clone(&release_probe);
            let probe_calls = Arc::clone(&probe_calls);
            let probe_started = probe_started.clone();
            callers.push(std::thread::spawn(move || {
                callers_ready.wait();
                coordinator.verify_with(Duration::from_secs(1), move || {
                    probe_calls.fetch_add(1, Ordering::SeqCst);
                    let _ = probe_started.send(());
                    release_probe.wait();
                    true
                })
            }));
        }

        callers_ready.wait();
        wait_for_probe
            .recv_timeout(Duration::from_secs(1))
            .expect("one shared probe should start");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(probe_calls.load(Ordering::SeqCst), 1);
        release_probe.wait();
        assert!(callers.into_iter().all(|caller| caller.join().unwrap()));
        assert_eq!(probe_calls.load(Ordering::SeqCst), 1);

        assert!(coordinator.verify_with(Duration::from_millis(10), || false));
        assert_eq!(probe_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn timed_out_synthetic_dns_probe_does_not_spawn_another_worker() {
        let coordinator = Arc::new(SyntheticDnsProbeCoordinator::default());
        let release_probe = Arc::new((Mutex::new(false), Condvar::new()));
        let probe_calls = Arc::new(AtomicUsize::new(0));
        let (probe_started, wait_for_probe) = std::sync::mpsc::sync_channel(1);
        let worker_release = Arc::clone(&release_probe);
        let worker_calls = Arc::clone(&probe_calls);

        assert!(
            !coordinator.verify_with(Duration::from_millis(100), move || {
                worker_calls.fetch_add(1, Ordering::SeqCst);
                let _ = probe_started.send(());
                let (released, completed) = &*worker_release;
                let released = released.lock().unwrap();
                drop(
                    completed
                        .wait_while(released, |released| !*released)
                        .unwrap(),
                );
                true
            })
        );
        wait_for_probe
            .recv_timeout(Duration::from_secs(1))
            .expect("the shared probe should have started before timing out");
        assert!(!coordinator.verify_with(Duration::from_millis(25), || false));
        assert_eq!(probe_calls.load(Ordering::SeqCst), 1);

        let (released, completed) = &*release_probe;
        *released.lock().unwrap() = true;
        completed.notify_all();
        assert!(coordinator.verify_with(Duration::from_secs(1), || false));
        assert_eq!(probe_calls.load(Ordering::SeqCst), 1);
    }
}
