use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use nexa_core::tools::run_shell_tool::ManagedLoopbackPermit;
use url::Url;

use super::policy::{
    agent_domain_host_allowed, private_or_special_ip, synthetic_dns_ip, verified_synthetic_dns_mode,
};

const IO_TIMEOUT: Duration = Duration::from_secs(10);
const COPY_INTERRUPT_POLL: Duration = Duration::from_millis(100);
const MAX_GLOBAL_PROXY_CONNECTIONS: usize = 128;

#[derive(Clone)]
struct ConnectionBudget {
    active: Arc<AtomicUsize>,
    maximum: usize,
}

impl ConnectionBudget {
    fn new(maximum: usize) -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            maximum,
        }
    }

    fn try_acquire(&self) -> Option<ConnectionBudgetPermit> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.maximum).then_some(active + 1)
            })
            .ok()
            .map(|_| ConnectionBudgetPermit {
                active: Arc::clone(&self.active),
            })
    }
}

struct ConnectionBudgetPermit {
    active: Arc<AtomicUsize>,
}

impl Drop for ConnectionBudgetPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn global_proxy_connection_budget() -> &'static ConnectionBudget {
    static BUDGET: OnceLock<ConnectionBudget> = OnceLock::new();
    BUDGET.get_or_init(|| ConnectionBudget::new(MAX_GLOBAL_PROXY_CONNECTIONS))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AgentLoopbackEndpoint {
    permit: ManagedLoopbackPermit,
    origin: String,
    host: String,
    port: u16,
}

impl AgentLoopbackEndpoint {
    fn from_managed_permit(permit: ManagedLoopbackPermit) -> Option<Self> {
        if permit.service_id.is_empty() || !permit.is_live() {
            return None;
        }
        let origin = Url::parse(&permit.origin).ok()?;
        if !matches!(origin.scheme(), "http" | "https")
            || origin.origin().ascii_serialization() != permit.origin
            || origin.port_or_known_default() != Some(permit.port)
        {
            return None;
        }
        let origin_host = normalize_target_host(origin.host_str()?);
        let permit_host = normalize_target_host(&permit.host);
        if origin_host != permit_host || !is_loopback_host(&permit_host) {
            return None;
        }
        let permit_origin = permit.origin.clone();
        let permit_port = permit.port;
        Some(Self {
            permit,
            origin: permit_origin,
            host: permit_host,
            port: permit_port,
        })
    }

    fn matches_target(&self, target: &Target) -> bool {
        if !self.permit.is_live() {
            return false;
        }
        match target {
            Target::Ip(ip, port) => self.port == *port && self.host == ip.to_string(),
            Target::Domain(host, port) => {
                self.port == *port && self.host == normalize_target_host(host)
            }
        }
    }

    fn is_live(&self) -> bool {
        self.permit.is_live()
    }
}

fn normalize_target_host(host: &str) -> String {
    host.to_ascii_lowercase()
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// A loopback-only SOCKS5 proxy that enforces the agent's network boundary for
/// every WebView connection, including fetches, images, scripts and iframes.
pub struct BrowserNetworkProxy {
    url: Url,
    address: SocketAddr,
    running: Arc<AtomicBool>,
    agent_restricted: Arc<AtomicBool>,
    agent_loopback_permits: Arc<Mutex<HashSet<AgentLoopbackEndpoint>>>,
    restriction_generation: Arc<AtomicU64>,
    active_connections: Arc<Mutex<HashMap<u64, Vec<TcpStream>>>>,
}

impl BrowserNetworkProxy {
    pub fn start(agent_restricted: Arc<AtomicBool>) -> Result<Self, String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|error| format!("Could not start browser network policy proxy: {error}"))?;
        listener.set_nonblocking(true).map_err(|error| {
            format!("Could not configure browser network policy proxy: {error}")
        })?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("Could not read browser network policy address: {error}"))?;
        let url = Url::parse(&format!("socks5://{address}"))
            .map_err(|error| format!("Could not create browser network policy URL: {error}"))?;
        let running = Arc::new(AtomicBool::new(true));
        let agent_loopback_permits = Arc::new(Mutex::new(HashSet::new()));
        let restriction_generation = Arc::new(AtomicU64::new(0));
        let active_connections = Arc::new(Mutex::new(HashMap::new()));
        let next_connection_id = Arc::new(AtomicU64::new(1));

        let running_for_thread = Arc::clone(&running);
        let restriction_for_thread = Arc::clone(&agent_restricted);
        let loopback_permits_for_thread = Arc::clone(&agent_loopback_permits);
        let restriction_generation_for_thread = Arc::clone(&restriction_generation);
        let connections_for_thread = Arc::clone(&active_connections);
        std::thread::Builder::new()
            .name("nexa-browser-network-policy".to_string())
            .spawn(move || {
                while running_for_thread.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((client, _)) => {
                            if !running_for_thread.load(Ordering::Acquire) {
                                break;
                            }
                            let Some(connection_budget_permit) =
                                global_proxy_connection_budget().try_acquire()
                            else {
                                let _ = client.shutdown(Shutdown::Both);
                                continue;
                            };
                            let restriction = Arc::clone(&restriction_for_thread);
                            let loopback_permits = Arc::clone(&loopback_permits_for_thread);
                            let restriction_generation =
                                Arc::clone(&restriction_generation_for_thread);
                            let connections = Arc::clone(&connections_for_thread);
                            let connection_id = next_connection_id.fetch_add(1, Ordering::Relaxed);
                            let _ = std::thread::Builder::new()
                                .name("nexa-browser-network-request".to_string())
                                .spawn(move || {
                                    let _connection_budget_permit = connection_budget_permit;
                                    let _ = handle_client(
                                        client,
                                        connection_id,
                                        restriction,
                                        loopback_permits,
                                        restriction_generation,
                                        connections,
                                    );
                                });
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| format!("Could not run browser network policy proxy: {error}"))?;

        Ok(Self {
            url,
            address,
            running,
            agent_restricted,
            agent_loopback_permits,
            restriction_generation,
            active_connections,
        })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn set_agent_restricted(&self, restricted: bool) {
        let was_restricted = self.agent_restricted.swap(restricted, Ordering::AcqRel);
        if restricted != was_restricted {
            self.restriction_generation.fetch_add(1, Ordering::AcqRel);
            self.close_active_connections();
        }
    }

    /// Atomically replace the exact managed-service endpoints available while
    /// the browser is agent restricted. Invalid or non-loopback permissions are
    /// discarded. Any change revokes existing connections immediately.
    pub fn replace_agent_loopback_permits(&self, permits: Vec<ManagedLoopbackPermit>) {
        let replacement = permits
            .into_iter()
            .filter_map(AgentLoopbackEndpoint::from_managed_permit)
            .collect::<HashSet<_>>();
        let changed = match self.agent_loopback_permits.lock() {
            Ok(mut current) => {
                if *current == replacement {
                    false
                } else {
                    *current = replacement;
                    true
                }
            }
            Err(poisoned) => {
                *poisoned.into_inner() = HashSet::new();
                true
            }
        };
        if changed {
            self.restriction_generation.fetch_add(1, Ordering::AcqRel);
            self.close_active_connections();
        }
    }

    /// Revoke all Agent-only network authority and interrupt every connection
    /// created under the previous browser lifecycle state. This is deliberately
    /// stronger than replacing an already-empty permit set: public connections
    /// must also stop when the workspace is hidden or control changes hands.
    pub fn revoke_agent_network_access(&self) {
        match self.agent_loopback_permits.lock() {
            Ok(mut current) => current.clear(),
            Err(poisoned) => poisoned.into_inner().clear(),
        }
        self.restriction_generation.fetch_add(1, Ordering::AcqRel);
        self.close_active_connections();
    }

    /// Keep a permit only when the in-flight load still targets its exact
    /// origin. Redirects and unrelated loads revoke it and interrupt existing
    /// connections before the new page can issue subresource requests.
    pub fn retain_agent_loopback_permit_for_url(&self, url: &Url) {
        let origin = url.origin().ascii_serialization();
        let changed = match self.agent_loopback_permits.lock() {
            Ok(mut current) => {
                let before = current.len();
                current.retain(|permit| permit.origin == origin && permit.is_live());
                current.len() != before
            }
            Err(poisoned) => {
                poisoned.into_inner().clear();
                true
            }
        };
        if changed {
            self.restriction_generation.fetch_add(1, Ordering::AcqRel);
            self.close_active_connections();
        }
    }

    pub fn shutdown(&self) {
        if self.running.swap(false, Ordering::AcqRel) {
            self.close_active_connections();
            let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
        }
    }

    fn close_active_connections(&self) {
        if let Ok(connections) = self.active_connections.lock() {
            for connection_group in connections.values() {
                for connection in connection_group {
                    let _ = connection.shutdown(Shutdown::Both);
                }
            }
        }
    }
}

impl Drop for BrowserNetworkProxy {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ActiveConnection {
    id: u64,
    connections: Arc<Mutex<HashMap<u64, Vec<TcpStream>>>>,
}

impl ActiveConnection {
    fn register(
        id: u64,
        stream: &TcpStream,
        connections: Arc<Mutex<HashMap<u64, Vec<TcpStream>>>>,
    ) -> io::Result<Self> {
        connections
            .lock()
            .map_err(|_| io::Error::other("browser proxy connection registry is unavailable"))
            .and_then(|mut connections| {
                if connections.len() >= 256 {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        "browser proxy connection limit reached",
                    ));
                }
                connections.insert(id, vec![stream.try_clone()?]);
                Ok(())
            })?;
        Ok(Self { id, connections })
    }

    fn register_peer(&self, stream: &TcpStream) -> io::Result<()> {
        let peer = stream.try_clone()?;
        let mut connections = self
            .connections
            .lock()
            .map_err(|_| io::Error::other("browser proxy connection registry is unavailable"))?;
        let group = connections.get_mut(&self.id).ok_or_else(|| {
            io::Error::other("browser proxy connection closed before peer registration")
        })?;
        group.push(peer);
        Ok(())
    }
}

impl Drop for ActiveConnection {
    fn drop(&mut self) {
        if let Ok(mut connections) = self.connections.lock() {
            connections.remove(&self.id);
        }
    }
}

fn handle_client(
    mut client: TcpStream,
    connection_id: u64,
    agent_restricted: Arc<AtomicBool>,
    agent_loopback_permits: Arc<Mutex<HashSet<AgentLoopbackEndpoint>>>,
    restriction_generation: Arc<AtomicU64>,
    active_connections: Arc<Mutex<HashMap<u64, Vec<TcpStream>>>>,
) -> io::Result<()> {
    let connection_generation = restriction_generation.load(Ordering::Acquire);
    client.set_nonblocking(false)?;
    client.set_read_timeout(Some(IO_TIMEOUT))?;
    client.set_write_timeout(Some(IO_TIMEOUT))?;
    let _active = ActiveConnection::register(connection_id, &client, active_connections)?;

    let mut greeting = [0_u8; 2];
    client.read_exact(&mut greeting)?;
    if greeting[0] != 5 || greeting[1] == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid SOCKS greeting",
        ));
    }
    let mut methods = vec![0_u8; usize::from(greeting[1])];
    client.read_exact(&mut methods)?;
    if !methods.contains(&0) {
        client.write_all(&[5, 0xff])?;
        return Ok(());
    }
    client.write_all(&[5, 0])?;

    let mut request = [0_u8; 4];
    client.read_exact(&mut request)?;
    if request[0] != 5 || request[1] != 1 || request[2] != 0 {
        send_reply(&mut client, 7, None)?;
        finish_rejection(&mut client);
        return Ok(());
    }
    let target = read_target(&mut client, request[3])?;
    let addresses = resolve_target(&target)?;
    let restricted = agent_restricted.load(Ordering::Acquire);
    let synthetic_dns_verified = restricted
        && addresses
            .iter()
            .any(|address| synthetic_dns_ip(address.ip()))
        && verified_synthetic_dns_mode();
    let loopback_permit = restricted
        .then(|| {
            agent_loopback_permits.lock().ok().and_then(|permits| {
                permits
                    .iter()
                    .find(|permit| permit.matches_target(&target))
                    .cloned()
            })
        })
        .flatten();
    let loopback_permitted = loopback_permit.is_some();
    if addresses.is_empty()
        || (restricted
            && !loopback_permitted
            && addresses
                .iter()
                .any(|address| !target.agent_address_allowed(address.ip(), synthetic_dns_verified)))
        || !connection_authorization_is_current(
            restriction_generation.as_ref(),
            connection_generation,
            agent_loopback_permits.as_ref(),
            loopback_permit.as_ref(),
        )
    {
        send_reply(&mut client, 2, None)?;
        finish_rejection(&mut client);
        return Ok(());
    }

    let mut upstream = None;
    for address in addresses {
        if restricted {
            if loopback_permitted {
                if !address.ip().is_loopback() {
                    continue;
                }
            } else if !target.agent_address_allowed(address.ip(), synthetic_dns_verified) {
                continue;
            }
        }
        if let Ok(stream) = TcpStream::connect_timeout(&address, IO_TIMEOUT) {
            upstream = Some(stream);
            break;
        }
    }
    let Some(mut upstream) = upstream else {
        send_reply(&mut client, 5, None)?;
        finish_rejection(&mut client);
        return Ok(());
    };
    _active.register_peer(&upstream)?;
    let peer_is_allowed = !restricted
        || upstream.peer_addr().is_ok_and(|address| {
            if loopback_permitted {
                address.ip().is_loopback() && address.port() == target.port()
            } else {
                target.agent_address_allowed(address.ip(), synthetic_dns_verified)
            }
        });
    if !peer_is_allowed
        || !connection_authorization_is_current(
            restriction_generation.as_ref(),
            connection_generation,
            agent_loopback_permits.as_ref(),
            loopback_permit.as_ref(),
        )
    {
        send_reply(&mut client, 2, None)?;
        finish_rejection(&mut client);
        return Ok(());
    }
    send_reply(&mut client, 0, upstream.local_addr().ok())?;
    client.flush()?;

    client.set_read_timeout(Some(COPY_INTERRUPT_POLL))?;
    client.set_write_timeout(Some(IO_TIMEOUT))?;
    upstream.set_read_timeout(Some(COPY_INTERRUPT_POLL))?;
    upstream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut client_reader = client.try_clone()?;
    let mut upstream_writer = upstream.try_clone()?;
    let reverse_generation = Arc::clone(&restriction_generation);
    let reverse_permits = Arc::clone(&agent_loopback_permits);
    let reverse_loopback_permit = loopback_permit.clone();
    let reverse = std::thread::spawn(move || {
        let _ = copy_until_authorization_changes(
            &mut client_reader,
            &mut upstream_writer,
            reverse_generation.as_ref(),
            connection_generation,
            reverse_permits.as_ref(),
            reverse_loopback_permit.as_ref(),
        );
        let _ = upstream_writer.shutdown(Shutdown::Write);
    });
    let _ = copy_until_authorization_changes(
        &mut upstream,
        &mut client,
        restriction_generation.as_ref(),
        connection_generation,
        agent_loopback_permits.as_ref(),
        loopback_permit.as_ref(),
    );
    let _ = client.shutdown(Shutdown::Write);
    let _ = reverse.join();
    Ok(())
}

fn copy_until_authorization_changes(
    reader: &mut TcpStream,
    writer: &mut TcpStream,
    restriction_generation: &AtomicU64,
    connection_generation: u64,
    permits: &Mutex<HashSet<AgentLoopbackEndpoint>>,
    loopback_permit: Option<&AgentLoopbackEndpoint>,
) -> io::Result<()> {
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        if !connection_authorization_is_current(
            restriction_generation,
            connection_generation,
            permits,
            loopback_permit,
        ) {
            return Ok(());
        }
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => writer.write_all(&buffer[..read])?,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
    }
}

fn connection_authorization_is_current(
    restriction_generation: &AtomicU64,
    connection_generation: u64,
    permits: &Mutex<HashSet<AgentLoopbackEndpoint>>,
    loopback_permit: Option<&AgentLoopbackEndpoint>,
) -> bool {
    if restriction_generation.load(Ordering::Acquire) != connection_generation {
        return false;
    }
    let Some(loopback_permit) = loopback_permit else {
        return true;
    };
    loopback_permit.is_live()
        && permits
            .lock()
            .map(|current| current.contains(loopback_permit))
            .unwrap_or(false)
}

fn finish_rejection(client: &mut TcpStream) {
    let _ = client.flush();
    let _ = client.shutdown(Shutdown::Write);
}

enum Target {
    Ip(IpAddr, u16),
    Domain(String, u16),
}

impl Target {
    fn port(&self) -> u16 {
        match self {
            Self::Ip(_, port) | Self::Domain(_, port) => *port,
        }
    }

    fn agent_address_allowed(&self, ip: IpAddr, synthetic_dns_verified: bool) -> bool {
        if !private_or_special_ip(ip) {
            return match self {
                Self::Ip(_, _) => true,
                Self::Domain(host, _) => agent_domain_host_allowed(host),
            };
        }
        synthetic_dns_verified
            && matches!(self, Self::Domain(host, _) if agent_domain_host_allowed(host) && synthetic_dns_ip(ip))
    }
}

fn read_target(client: &mut TcpStream, address_type: u8) -> io::Result<Target> {
    let host = match address_type {
        1 => {
            let mut octets = [0_u8; 4];
            client.read_exact(&mut octets)?;
            TargetHost::Ip(IpAddr::V4(Ipv4Addr::from(octets)))
        }
        3 => {
            let mut length = [0_u8; 1];
            client.read_exact(&mut length)?;
            if length[0] == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "empty SOCKS host",
                ));
            }
            let mut bytes = vec![0_u8; usize::from(length[0])];
            client.read_exact(&mut bytes)?;
            let host = String::from_utf8(bytes)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid SOCKS host"))?;
            TargetHost::Domain(host)
        }
        4 => {
            let mut octets = [0_u8; 16];
            client.read_exact(&mut octets)?;
            TargetHost::Ip(IpAddr::V6(Ipv6Addr::from(octets)))
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid SOCKS address",
            ))
        }
    };
    let mut port = [0_u8; 2];
    client.read_exact(&mut port)?;
    let port = u16::from_be_bytes(port);
    Ok(match host {
        TargetHost::Ip(ip) => Target::Ip(ip, port),
        TargetHost::Domain(domain) => Target::Domain(domain, port),
    })
}

enum TargetHost {
    Ip(IpAddr),
    Domain(String),
}

fn resolve_target(target: &Target) -> io::Result<Vec<SocketAddr>> {
    match target {
        Target::Ip(ip, port) => Ok(vec![SocketAddr::new(*ip, *port)]),
        Target::Domain(host, port) => (host.as_str(), *port)
            .to_socket_addrs()
            .map(|addresses| addresses.collect()),
    }
}

fn send_reply(client: &mut TcpStream, status: u8, address: Option<SocketAddr>) -> io::Result<()> {
    match address.unwrap_or_else(|| SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))) {
        SocketAddr::V4(address) => {
            let mut reply = vec![5, status, 0, 1];
            reply.extend_from_slice(&address.ip().octets());
            reply.extend_from_slice(&address.port().to_be_bytes());
            client.write_all(&reply)
        }
        SocketAddr::V6(address) => {
            let mut reply = vec![5, status, 0, 4];
            reply.extend_from_slice(&address.ip().octets());
            reply.extend_from_slice(&address.port().to_be_bytes());
            client.write_all(&reply)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexa_core::tools::run_shell_tool::{ManagedLoopbackPermit, ManagedLoopbackPermitIssuer};

    fn loopback_permit(service_id: &str, host: &str, port: u16) -> ManagedLoopbackPermit {
        ManagedLoopbackPermitIssuer::new(service_id, None).issue(
            format!("http://{host}:{port}"),
            host,
            port,
        )
    }

    fn connect_request(proxy: &BrowserNetworkProxy, address: SocketAddr) -> TcpStream {
        let mut stream = TcpStream::connect(proxy.address).expect("connect to proxy");
        stream.write_all(&[5, 1, 0]).expect("write greeting");
        let mut greeting = [0_u8; 2];
        stream.read_exact(&mut greeting).expect("read greeting");
        assert_eq!(greeting, [5, 0]);
        match address {
            SocketAddr::V4(address) => {
                let mut request = vec![5, 1, 0, 1];
                request.extend_from_slice(&address.ip().octets());
                request.extend_from_slice(&address.port().to_be_bytes());
                stream.write_all(&request).expect("write request");
            }
            SocketAddr::V6(address) => {
                let mut request = vec![5, 1, 0, 4];
                request.extend_from_slice(&address.ip().octets());
                request.extend_from_slice(&address.port().to_be_bytes());
                stream.write_all(&request).expect("write request");
            }
        }
        stream
    }

    #[test]
    fn connection_budget_is_shared_and_released_before_spawning_workers() {
        let budget = ConnectionBudget::new(2);
        let first = budget.try_acquire().expect("first permit");
        let second = budget.try_acquire().expect("second permit");
        assert!(
            budget.try_acquire().is_none(),
            "budget must be hard bounded"
        );

        drop(first);
        let replacement = budget.try_acquire().expect("released permit is reusable");
        assert!(budget.try_acquire().is_none());
        drop(second);
        drop(replacement);
        assert!(budget.try_acquire().is_some());
    }

    #[test]
    fn restricted_proxy_allows_fake_ip_only_for_public_domain_targets() {
        let fake_ip = IpAddr::V4(Ipv4Addr::new(198, 18, 0, 8));
        let public_domain = Target::Domain("example.com".to_string(), 443);
        assert!(!public_domain.agent_address_allowed(fake_ip, false));
        assert!(public_domain.agent_address_allowed(fake_ip, true));
        assert!(!Target::Ip(fake_ip, 443).agent_address_allowed(fake_ip, true));
        assert!(
            !Target::Domain("printer.local".to_string(), 443).agent_address_allowed(fake_ip, true)
        );
        assert!(!Target::Domain("localhost".to_string(), 443).agent_address_allowed(fake_ip, true));
        assert!(!Target::Domain("10.0.0.1".to_string(), 443).agent_address_allowed(fake_ip, true));
    }

    #[test]
    fn restricted_proxy_blocks_loopback_subresources() {
        let proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(true))).unwrap();
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut stream = connect_request(&proxy, target.local_addr().unwrap());
        let mut reply = [0_u8; 4];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(reply[1], 2);
    }

    #[test]
    fn restricted_proxy_allows_exact_managed_loopback_endpoint_for_subresources() {
        let proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(true))).unwrap();
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = target.local_addr().unwrap();
        proxy.replace_agent_loopback_permits(vec![loopback_permit(
            "managed-service",
            &address.ip().to_string(),
            address.port(),
        )]);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = target.accept().unwrap();
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).unwrap();
            stream.write_all(&byte).unwrap();
        });

        let mut stream = connect_request(&proxy, address);
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(header[1], 0);
        let address_length = if header[3] == 1 { 6 } else { 18 };
        let mut bound_address = vec![0_u8; address_length];
        stream.read_exact(&mut bound_address).unwrap();
        stream.write_all(&[42]).unwrap();
        let mut response = [0_u8; 1];
        stream.read_exact(&mut response).unwrap();
        assert_eq!(response, [42]);
        server.join().unwrap();
    }

    #[test]
    fn managed_loopback_permit_isolated_to_one_tab_proxy() {
        let permitted_tab_proxy =
            BrowserNetworkProxy::start(Arc::new(AtomicBool::new(true))).unwrap();
        let other_tab_proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(true))).unwrap();
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = target.local_addr().unwrap();
        permitted_tab_proxy.replace_agent_loopback_permits(vec![loopback_permit(
            "managed-service",
            &address.ip().to_string(),
            address.port(),
        )]);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = target.accept().unwrap();
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).unwrap();
            stream.write_all(&byte).unwrap();
        });

        let mut permitted = connect_request(&permitted_tab_proxy, address);
        let mut header = [0_u8; 4];
        permitted.read_exact(&mut header).unwrap();
        assert_eq!(header[1], 0);
        let address_length = if header[3] == 1 { 6 } else { 18 };
        let mut bound_address = vec![0_u8; address_length];
        permitted.read_exact(&mut bound_address).unwrap();
        permitted.write_all(&[42]).unwrap();
        let mut response = [0_u8; 1];
        permitted.read_exact(&mut response).unwrap();
        assert_eq!(response, [42]);

        let mut rejected = connect_request(&other_tab_proxy, address);
        let mut reply = [0_u8; 4];
        rejected.read_exact(&mut reply).unwrap();
        assert_eq!(reply[1], 2);
        server.join().unwrap();
    }

    #[test]
    fn restricted_proxy_rejects_unpermitted_port_on_permitted_loopback_host() {
        let proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(true))).unwrap();
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = target.local_addr().unwrap();
        let other_port = if address.port() == u16::MAX {
            address.port() - 1
        } else {
            address.port() + 1
        };
        proxy.replace_agent_loopback_permits(vec![loopback_permit(
            "managed-service",
            &address.ip().to_string(),
            other_port,
        )]);

        let mut stream = connect_request(&proxy, address);
        let mut reply = [0_u8; 4];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(reply[1], 2);
    }

    #[test]
    fn restricted_proxy_rejects_loopback_alias_not_named_by_permit() {
        let proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(true))).unwrap();
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = target.local_addr().unwrap();
        proxy.replace_agent_loopback_permits(vec![loopback_permit(
            "managed-service",
            "localhost",
            address.port(),
        )]);

        let mut stream = connect_request(&proxy, address);
        let mut reply = [0_u8; 4];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(reply[1], 2);
    }

    #[test]
    fn replacing_agent_loopback_permits_closes_existing_connections() {
        let proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(true))).unwrap();
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = target.local_addr().unwrap();
        proxy.replace_agent_loopback_permits(vec![loopback_permit(
            "managed-service",
            &address.ip().to_string(),
            address.port(),
        )]);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = target.accept().unwrap();
            let mut byte = [0_u8; 1];
            let _ = stream.read_exact(&mut byte);
        });
        let mut stream = connect_request(&proxy, address);
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(header[1], 0);
        let address_length = if header[3] == 1 { 6 } else { 18 };
        let mut bound_address = vec![0_u8; address_length];
        stream.read_exact(&mut bound_address).unwrap();

        proxy.replace_agent_loopback_permits(Vec::new());
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut byte = [0_u8; 1];
        assert_ne!(stream.read(&mut byte).unwrap_or(0), 1);
        server.join().unwrap();
    }

    #[test]
    fn workspace_revocation_closes_connections_even_without_a_loopback_permit() {
        let proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(false))).unwrap();
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = target.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = target.accept().unwrap();
            let mut byte = [0_u8; 1];
            let _ = stream.read_exact(&mut byte);
        });
        let mut stream = connect_request(&proxy, address);
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(header[1], 0);
        let address_length = if header[3] == 1 { 6 } else { 18 };
        let mut bound_address = vec![0_u8; address_length];
        stream.read_exact(&mut bound_address).unwrap();

        proxy.revoke_agent_network_access();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut byte = [0_u8; 1];
        assert_ne!(stream.read(&mut byte).unwrap_or(0), 1);
        server.join().unwrap();
    }

    #[test]
    fn dead_managed_service_identity_rejects_new_requests_and_closes_existing_connections() {
        let proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(true))).unwrap();
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = target.local_addr().unwrap();
        let issuer = ManagedLoopbackPermitIssuer::new("managed-service", Some(42));
        proxy.replace_agent_loopback_permits(vec![issuer.issue(
            format!("http://{}:{}", address.ip(), address.port()),
            address.ip().to_string(),
            address.port(),
        )]);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = target.accept().unwrap();
            let mut byte = [0_u8; 1];
            let _ = stream.read_exact(&mut byte);
        });
        let mut stream = connect_request(&proxy, address);
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(header[1], 0);
        let address_length = if header[3] == 1 { 6 } else { 18 };
        let mut bound_address = vec![0_u8; address_length];
        stream.read_exact(&mut bound_address).unwrap();

        issuer.revoke();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut byte = [0_u8; 1];
        assert_ne!(stream.read(&mut byte).unwrap_or(0), 1);

        let mut rejected = connect_request(&proxy, address);
        let mut reply = [0_u8; 4];
        rejected.read_exact(&mut reply).unwrap();
        assert_eq!(reply[1], 2);
        server.join().unwrap();
    }

    #[test]
    fn user_controlled_proxy_allows_loopback_subresources() {
        let proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(false))).unwrap();
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = target.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = target.accept().unwrap();
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).unwrap();
            stream.write_all(&byte).unwrap();
        });
        let mut stream = connect_request(&proxy, address);
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(header[1], 0);
        let address_length = if header[3] == 1 { 6 } else { 18 };
        let mut address = vec![0_u8; address_length];
        stream.read_exact(&mut address).unwrap();
        stream.write_all(&[42]).unwrap();
        let mut response = [0_u8; 1];
        stream.read_exact(&mut response).unwrap();
        assert_eq!(response, [42]);
        server.join().unwrap();
    }

    #[test]
    fn taking_agent_control_closes_existing_private_connections() {
        for _ in 0..16 {
            let proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(false))).unwrap();
            let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let address = target.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = target.accept().unwrap();
                let mut byte = [0_u8; 1];
                let _ = stream.read_exact(&mut byte);
            });
            let mut stream = connect_request(&proxy, address);
            let mut header = [0_u8; 4];
            stream.read_exact(&mut header).unwrap();
            let address_length = if header[3] == 1 { 6 } else { 18 };
            let mut address = vec![0_u8; address_length];
            stream.read_exact(&mut address).unwrap();

            proxy.set_agent_restricted(true);
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut byte = [0_u8; 1];
            assert_ne!(stream.read(&mut byte).unwrap_or(0), 1);
            server.join().unwrap();
        }
    }
}
