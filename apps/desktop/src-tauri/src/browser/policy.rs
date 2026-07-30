use std::collections::HashSet;
use std::net::IpAddr;

pub use nexa_core::browser_runtime::{
    classify_action_risk as classify_agent_action, BrowserActionRisk,
};
use url::{Host, Url};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationActor {
    User,
    Agent,
}

fn private_or_special_ip(ip: IpAddr) -> bool {
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
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter a URL or search query".to_string());
    }

    let candidate = if input.split_whitespace().count() > 1 {
        let encoded: String = url::form_urlencoded::byte_serialize(input.as_bytes()).collect();
        format!("https://www.google.com/search?q={encoded}")
    } else if input.contains("://")
        || input.starts_with("javascript:")
        || input.starts_with("data:")
        || input.starts_with("file:")
    {
        input.to_string()
    } else {
        format!("https://{input}")
    };

    let url = Url::parse(&candidate).map_err(|_| "Invalid browser address".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only HTTP and HTTPS pages can open in Nexa Browser".to_string());
    }
    if actor == NavigationActor::Agent && !agent_host_allowed(&url) {
        return Err(
            "Agent browser navigation to local or private networks requires explicit user control"
                .to_string(),
        );
    }
    Ok(url)
}

pub async fn validate_agent_network_url(url: &Url) -> Result<(), String> {
    if !agent_host_allowed(url) {
        return Err(
            "Agent browser navigation to local or private networks requires explicit user control"
                .to_string(),
        );
    }
    let host = url
        .host_str()
        .ok_or_else(|| "Browser address has no host".to_string())?;
    let port = url.port_or_known_default().unwrap_or(443);
    let resolved = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| "Could not resolve the browser address".to_string())?;
    let mut resolved_any = false;
    for address in resolved {
        resolved_any = true;
        if private_or_special_ip(address.ip()) {
            return Err(
                "Agent browser navigation resolved to a local or private network".to_string(),
            );
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
    navigation_allowed(url, agent_restricted)
        && (!agent_restricted
            || approved_agent_urls.remove(url.as_str())
            || approved_agent_urls.remove(&form_navigation_approval_key(url)))
}
