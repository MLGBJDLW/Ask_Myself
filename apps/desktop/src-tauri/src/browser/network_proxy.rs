use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use url::Url;

use super::policy::private_or_special_ip;

const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// A loopback-only SOCKS5 proxy that enforces the agent's network boundary for
/// every WebView connection, including fetches, images, scripts and iframes.
pub struct BrowserNetworkProxy {
    url: Url,
    address: SocketAddr,
    running: Arc<AtomicBool>,
    agent_restricted: Arc<AtomicBool>,
    active_connections: Arc<Mutex<HashMap<u64, TcpStream>>>,
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
        let active_connections = Arc::new(Mutex::new(HashMap::new()));
        let next_connection_id = Arc::new(AtomicU64::new(1));

        let running_for_thread = Arc::clone(&running);
        let restriction_for_thread = Arc::clone(&agent_restricted);
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
                            let restriction = Arc::clone(&restriction_for_thread);
                            let connections = Arc::clone(&connections_for_thread);
                            let connection_id = next_connection_id.fetch_add(1, Ordering::Relaxed);
                            let _ = std::thread::Builder::new()
                                .name("nexa-browser-network-request".to_string())
                                .spawn(move || {
                                    let _ = handle_client(
                                        client,
                                        connection_id,
                                        restriction,
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
            active_connections,
        })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn set_agent_restricted(&self, restricted: bool) {
        let was_restricted = self.agent_restricted.swap(restricted, Ordering::AcqRel);
        if restricted && !was_restricted {
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
            for connection in connections.values() {
                let _ = connection.shutdown(Shutdown::Both);
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
    connections: Arc<Mutex<HashMap<u64, TcpStream>>>,
}

impl ActiveConnection {
    fn register(
        id: u64,
        stream: &TcpStream,
        connections: Arc<Mutex<HashMap<u64, TcpStream>>>,
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
                connections.insert(id, stream.try_clone()?);
                Ok(())
            })?;
        Ok(Self { id, connections })
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
    active_connections: Arc<Mutex<HashMap<u64, TcpStream>>>,
) -> io::Result<()> {
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
    if addresses.is_empty()
        || (agent_restricted.load(Ordering::Acquire)
            && addresses
                .iter()
                .any(|address| private_or_special_ip(address.ip())))
    {
        send_reply(&mut client, 2, None)?;
        finish_rejection(&mut client);
        return Ok(());
    }

    let mut upstream = None;
    for address in addresses {
        if agent_restricted.load(Ordering::Acquire) && private_or_special_ip(address.ip()) {
            continue;
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
    if agent_restricted.load(Ordering::Acquire)
        && upstream
            .peer_addr()
            .is_ok_and(|address| private_or_special_ip(address.ip()))
    {
        send_reply(&mut client, 2, None)?;
        finish_rejection(&mut client);
        return Ok(());
    }
    send_reply(&mut client, 0, upstream.local_addr().ok())?;
    client.flush()?;

    client.set_read_timeout(None)?;
    client.set_write_timeout(None)?;
    upstream.set_read_timeout(None)?;
    upstream.set_write_timeout(None)?;
    let mut client_reader = client.try_clone()?;
    let mut upstream_writer = upstream.try_clone()?;
    let reverse = std::thread::spawn(move || {
        let _ = io::copy(&mut client_reader, &mut upstream_writer);
        let _ = upstream_writer.shutdown(Shutdown::Write);
    });
    let _ = io::copy(&mut upstream, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    let _ = reverse.join();
    Ok(())
}

fn finish_rejection(client: &mut TcpStream) {
    let _ = client.flush();
    let _ = client.shutdown(Shutdown::Write);
}

enum Target {
    Ip(IpAddr, u16),
    Domain(String, u16),
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
    fn restricted_proxy_blocks_loopback_subresources() {
        let proxy = BrowserNetworkProxy::start(Arc::new(AtomicBool::new(true))).unwrap();
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut stream = connect_request(&proxy, target.local_addr().unwrap());
        let mut reply = [0_u8; 4];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(reply[1], 2);
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
