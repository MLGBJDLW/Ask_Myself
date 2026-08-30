use std::collections::HashSet;
use std::net::IpAddr;

pub use nexa_core::browser_runtime::{
    classify_action_risk as classify_agent_action, BrowserActionRisk,
};
use nexa_core::tools::run_shell_tool::ManagedLoopbackPermit;
use url::{Host, Url};

const AGENT_DNS_RESOLUTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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

pub fn form_navigation_approval_key(url: &Url) -> String {
    let mut target = url.clone();
    target.set_query(None);
    target.set_fragment(None);
    format!("form:{}", target.as_str())
}

fn agent_host_allowed(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => !private_or_special_ip(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => !private_or_special_ip(IpAddr::V6(ip)),
        Some(Host::Domain(host)) => {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            host != "localhost"
                && !host.ends_with(".localhost")
                && !host.ends_with(".local")
                && host != "metadata.google.internal"
        }
        None => false,
    }
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
    let mut resolved_any = false;
    for address in resolved {
        resolved_any = true;
        if (managed_loopback && !resolved_is_loopback(address.ip()))
            || (!managed_loopback && private_or_special_ip(address.ip()))
        {
            return Err(if managed_loopback {
                "Managed browser service resolved outside loopback"
            } else {
                "Agent browser navigation resolved to a local or private network"
            }
            .to_string());
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
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    approved_agent_urls.remove(url.as_str())
        || approved_agent_urls.remove(&form_navigation_approval_key(url))
}
