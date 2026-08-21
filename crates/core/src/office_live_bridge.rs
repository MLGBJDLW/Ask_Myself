//! User-authorized loopback bridge for a separately sideloaded Office.js add-in.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::CoreError;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const SESSION_TTL_SECONDS: u64 = 30 * 60;
const CLIENT_TTL_SECONDS: u64 = 35 * 60;
const MAX_PAIRING_FAILURES: u8 = 5;
const PAIRING_BACKOFF_SECONDS: u64 = 30;
const OPERATION_TTL_SECONDS: u64 = 5 * 60;

static LIVE_BRIDGE: OnceLock<Arc<OfficeLiveBridge>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeHostSession {
    pub session_id: String,
    pub host: String,
    pub document_id: String,
    pub requirement_sets: Vec<String>,
    pub capabilities: Vec<String>,
    pub connected_at: u64,
    pub last_seen_at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeLiveBridgeStatus {
    pub kind: &'static str,
    pub endpoint: String,
    pub pairing_code: Option<String>,
    pub add_in_manifest_path: Option<String>,
    pub sessions: Vec<OfficeHostSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeLiveOperationResult {
    pub operation_id: String,
    pub session_id: String,
    pub status: String,
    #[serde(default)]
    pub result: Value,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueuedOperation {
    operation_id: String,
    request_version: u8,
    operation: Value,
    created_at: u64,
    deadline_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationStatus {
    Queued,
    Leased,
    Cancelled,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeLiveCancelOutcome {
    CancelledBeforeLease,
    Completed,
    IndeterminateLeased,
    Missing,
}

#[derive(Debug)]
struct ClientGrant {
    expires_at: u64,
}

#[derive(Debug)]
struct BridgeState {
    pairing_code: String,
    failed_pairing_attempts: u8,
    pairing_blocked_until: u64,
    clients: HashMap<String, ClientGrant>,
    sessions: HashMap<String, OfficeHostSession>,
    session_owners: HashMap<String, String>,
    queues: HashMap<String, VecDeque<QueuedOperation>>,
    operation_owners: HashMap<String, String>,
    operation_deadlines: HashMap<String, u64>,
    operation_statuses: HashMap<String, OperationStatus>,
    results: HashMap<String, OfficeLiveOperationResult>,
}

#[derive(Debug)]
pub struct OfficeLiveBridge {
    endpoint: String,
    state: Mutex<BridgeState>,
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_pairing_code() -> String {
    let bytes = Uuid::new_v4().into_bytes();
    let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
    format!("{value:06}")
}

fn random_client_token() -> String {
    Uuid::new_v4().simple().to_string() + &Uuid::new_v4().simple().to_string()
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

impl OfficeLiveBridge {
    fn start() -> Result<Arc<Self>, CoreError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(CoreError::Io)?;
        listener.set_nonblocking(false).map_err(CoreError::Io)?;
        let endpoint = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().map_err(CoreError::Io)?.port()
        );
        let bridge = Arc::new(Self {
            endpoint,
            state: Mutex::new(BridgeState {
                pairing_code: random_pairing_code(),
                failed_pairing_attempts: 0,
                pairing_blocked_until: 0,
                clients: HashMap::new(),
                sessions: HashMap::new(),
                session_owners: HashMap::new(),
                queues: HashMap::new(),
                operation_owners: HashMap::new(),
                operation_deadlines: HashMap::new(),
                operation_statuses: HashMap::new(),
                results: HashMap::new(),
            }),
        });
        let server = Arc::clone(&bridge);
        thread::Builder::new()
            .name("nexa-office-live-bridge".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            let bridge = Arc::clone(&server);
                            let _ = thread::Builder::new()
                                .name("nexa-office-live-request".to_string())
                                .spawn(move || {
                                    let _ = bridge.handle(stream);
                                });
                        }
                        Err(error) => {
                            tracing::warn!("Office live bridge accept failed: {error}");
                            break;
                        }
                    }
                }
            })
            .map_err(CoreError::Io)?;
        Ok(bridge)
    }

    pub fn status(&self, include_pairing_code: bool) -> OfficeLiveBridgeStatus {
        let now = now_seconds();
        let mut state = self
            .state
            .lock()
            .expect("Office live bridge state poisoned");
        prune_expired(&mut state, now);
        let mut sessions = state.sessions.values().cloned().collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        OfficeLiveBridgeStatus {
            kind: "officeLiveBridgeStatus",
            endpoint: self.endpoint.clone(),
            pairing_code: include_pairing_code.then(|| state.pairing_code.clone()),
            add_in_manifest_path: std::env::var(crate::office_runtime::OFFICE_ADDIN_MANIFEST_ENV)
                .ok(),
            sessions,
        }
    }

    pub fn enqueue(&self, session_id: &str, operation: Value) -> Result<String, CoreError> {
        let mut state = self
            .state
            .lock()
            .expect("Office live bridge state poisoned");
        let now = now_seconds();
        prune_expired(&mut state, now);
        if !state.sessions.contains_key(session_id) {
            return Err(CoreError::InvalidInput(format!(
                "Office.js host session is not connected: {session_id}"
            )));
        }
        let operation_id = Uuid::new_v4().simple().to_string();
        state
            .operation_owners
            .insert(operation_id.clone(), session_id.to_string());
        state
            .operation_deadlines
            .insert(operation_id.clone(), now + OPERATION_TTL_SECONDS);
        state
            .operation_statuses
            .insert(operation_id.clone(), OperationStatus::Queued);
        state
            .queues
            .entry(session_id.to_string())
            .or_default()
            .push_back(QueuedOperation {
                operation_id: operation_id.clone(),
                request_version: 1,
                operation,
                created_at: now,
                deadline_at: now + OPERATION_TTL_SECONDS,
            });
        Ok(operation_id)
    }

    pub fn cancel(&self, operation_id: &str) -> OfficeLiveCancelOutcome {
        let mut state = self
            .state
            .lock()
            .expect("Office live bridge state poisoned");
        prune_expired(&mut state, now_seconds());
        if state.results.contains_key(operation_id)
            || state.operation_statuses.get(operation_id) == Some(&OperationStatus::Completed)
        {
            return OfficeLiveCancelOutcome::Completed;
        }
        let Some(status) = state.operation_statuses.get_mut(operation_id) else {
            return OfficeLiveCancelOutcome::Missing;
        };
        if *status == OperationStatus::Leased {
            return OfficeLiveCancelOutcome::IndeterminateLeased;
        }
        if *status == OperationStatus::Cancelled {
            return OfficeLiveCancelOutcome::CancelledBeforeLease;
        }
        *status = OperationStatus::Cancelled;
        for queue in state.queues.values_mut() {
            queue.retain(|operation| operation.operation_id != operation_id);
        }
        OfficeLiveCancelOutcome::CancelledBeforeLease
    }

    pub fn take_result(&self, operation_id: &str) -> Option<OfficeLiveOperationResult> {
        let mut state = self
            .state
            .lock()
            .expect("Office live bridge state poisoned");
        let result = state.results.remove(operation_id);
        if result.is_some() {
            state.operation_owners.remove(operation_id);
            state.operation_deadlines.remove(operation_id);
            state.operation_statuses.remove(operation_id);
        }
        result
    }

    fn handle(&self, mut stream: TcpStream) -> std::io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let request = read_request(&mut stream)?;
        let origin = request.headers.get("origin").map(String::as_str);
        if !is_allowed_origin(origin) {
            return write_response(
                &mut stream,
                403,
                &json!({"error": "origin_forbidden"}),
                None,
            );
        }
        if request.method == "OPTIONS" {
            return write_response(&mut stream, 204, &json!({}), origin);
        }
        let response = self.route(&request);
        write_response(&mut stream, response.0, &response.1, origin)
    }

    fn route(&self, request: &HttpRequest) -> (u16, Value) {
        match (request.method.as_str(), request.path.as_str()) {
            ("POST", "/v1/pair") => self.pair(&request.body),
            ("POST", "/v1/register") => {
                let Some(token) = self.authorized_token(request) else {
                    return (401, json!({"error": "unauthorized"}));
                };
                self.register(&token, &request.body)
            }
            ("POST", "/v1/poll") => {
                let Some(token) = self.authorized_token(request) else {
                    return (401, json!({"error": "unauthorized"}));
                };
                self.poll(&token, &request.body)
            }
            ("POST", "/v1/result") => {
                let Some(token) = self.authorized_token(request) else {
                    return (401, json!({"error": "unauthorized"}));
                };
                self.result(&token, &request.body)
            }
            _ => (404, json!({"error": "not_found"})),
        }
    }

    fn authorized_token(&self, request: &HttpRequest) -> Option<String> {
        let supplied = request
            .headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default();
        if supplied.len() != 64 {
            return None;
        }
        let now = now_seconds();
        let mut state = self
            .state
            .lock()
            .expect("Office live bridge state poisoned");
        prune_expired(&mut state, now);
        let matched = state
            .clients
            .keys()
            .find(|token| constant_time_equal(supplied, token))
            .cloned()?;
        if let Some(grant) = state.clients.get_mut(&matched) {
            grant.expires_at = now + CLIENT_TTL_SECONDS;
        }
        Some(matched)
    }

    fn pair(&self, body: &Value) -> (u16, Value) {
        let supplied = body
            .get("pairingCode")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut state = self
            .state
            .lock()
            .expect("Office live bridge state poisoned");
        let now = now_seconds();
        if now < state.pairing_blocked_until {
            return (429, json!({"error": "pairing_rate_limited"}));
        }
        if !constant_time_equal(supplied, &state.pairing_code) {
            state.failed_pairing_attempts = state.failed_pairing_attempts.saturating_add(1);
            if state.failed_pairing_attempts >= MAX_PAIRING_FAILURES {
                state.pairing_code = random_pairing_code();
                state.failed_pairing_attempts = 0;
                state.pairing_blocked_until = now + PAIRING_BACKOFF_SECONDS;
                return (429, json!({"error": "pairing_rate_limited"}));
            }
            return (403, json!({"error": "pairing_failed"}));
        }
        let token = random_client_token();
        state.clients.insert(
            token.clone(),
            ClientGrant {
                expires_at: now + CLIENT_TTL_SECONDS,
            },
        );
        state.pairing_code = random_pairing_code();
        state.failed_pairing_attempts = 0;
        state.pairing_blocked_until = 0;
        (
            200,
            json!({"kind": "officeLivePairing", "bridgeToken": token}),
        )
    }

    fn register(&self, client_token: &str, body: &Value) -> (u16, Value) {
        let host = body.get("host").and_then(Value::as_str).unwrap_or_default();
        if !matches!(host, "Word" | "Excel" | "PowerPoint") {
            return (400, json!({"error": "unsupported_host"}));
        }
        let document_id = body
            .get("documentId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if document_id.is_empty() || document_id.len() > 256 {
            return (400, json!({"error": "invalid_document_id"}));
        }
        let strings = |key: &str| {
            body.get(key)
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .take(128)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let session_id = Uuid::new_v4().simple().to_string();
        let now = now_seconds();
        let session = OfficeHostSession {
            session_id: session_id.clone(),
            host: host.to_string(),
            document_id: document_id.to_string(),
            requirement_sets: strings("requirementSets"),
            capabilities: strings("capabilities"),
            connected_at: now,
            last_seen_at: now,
        };
        let mut state = self
            .state
            .lock()
            .expect("Office live bridge state poisoned");
        state.sessions.insert(session_id.clone(), session.clone());
        state
            .session_owners
            .insert(session_id.clone(), client_token.to_string());
        (
            200,
            json!({"kind": "officeLiveSession", "session": session}),
        )
    }

    fn poll(&self, client_token: &str, body: &Value) -> (u16, Value) {
        let session_id = body
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut state = self
            .state
            .lock()
            .expect("Office live bridge state poisoned");
        if !session_owned_by(&state, session_id, client_token) {
            return (403, json!({"error": "session_forbidden"}));
        }
        let Some(session) = state.sessions.get_mut(session_id) else {
            return (404, json!({"error": "session_not_found"}));
        };
        session.last_seen_at = now_seconds();
        let now = now_seconds();
        prune_expired(&mut state, now);
        let mut operation = None;
        loop {
            let candidate = state
                .queues
                .entry(session_id.to_string())
                .or_default()
                .pop_front();
            let Some(candidate) = candidate else {
                break;
            };
            if candidate.deadline_at <= now
                || state.operation_statuses.get(&candidate.operation_id)
                    != Some(&OperationStatus::Queued)
            {
                continue;
            }
            state
                .operation_statuses
                .insert(candidate.operation_id.clone(), OperationStatus::Leased);
            operation = Some(candidate);
            break;
        }
        (
            200,
            json!({"kind": "officeLivePoll", "operation": operation}),
        )
    }

    fn result(&self, client_token: &str, body: &Value) -> (u16, Value) {
        let parsed = serde_json::from_value::<OfficeLiveOperationResult>(body.clone());
        let Ok(result) = parsed else {
            return (400, json!({"error": "invalid_result"}));
        };
        let mut state = self
            .state
            .lock()
            .expect("Office live bridge state poisoned");
        let now = now_seconds();
        prune_expired(&mut state, now);
        if !state.sessions.contains_key(&result.session_id) {
            return (404, json!({"error": "session_not_found"}));
        }
        if !session_owned_by(&state, &result.session_id, client_token) {
            return (403, json!({"error": "session_forbidden"}));
        }
        if state.operation_owners.get(&result.operation_id) != Some(&result.session_id) {
            return (409, json!({"error": "operation_session_mismatch"}));
        }
        match state.operation_statuses.get(&result.operation_id) {
            Some(OperationStatus::Cancelled) => {
                return (409, json!({"error": "operation_expired_or_cancelled"}));
            }
            Some(OperationStatus::Leased) => {}
            Some(OperationStatus::Completed) => {
                return (409, json!({"error": "duplicate_result"}));
            }
            _ => return (409, json!({"error": "operation_not_leased"})),
        }
        if state.results.contains_key(&result.operation_id) {
            return (409, json!({"error": "duplicate_result"}));
        }
        state
            .operation_statuses
            .insert(result.operation_id.clone(), OperationStatus::Completed);
        state.results.insert(result.operation_id.clone(), result);
        (202, json!({"kind": "officeLiveResultAccepted"}))
    }
}

fn session_owned_by(state: &BridgeState, session_id: &str, client_token: &str) -> bool {
    state
        .session_owners
        .get(session_id)
        .is_some_and(|owner| constant_time_equal(owner, client_token))
}

fn prune_expired(state: &mut BridgeState, now: u64) {
    state.clients.retain(|_, grant| now <= grant.expires_at);
    let expired_sessions = state
        .sessions
        .iter()
        .filter_map(|(session_id, session)| {
            let session_expired = now.saturating_sub(session.last_seen_at) > SESSION_TTL_SECONDS;
            let client_expired = state
                .session_owners
                .get(session_id)
                .is_none_or(|owner| !state.clients.contains_key(owner));
            (session_expired || client_expired).then(|| session_id.clone())
        })
        .collect::<Vec<_>>();
    for session_id in expired_sessions {
        state.sessions.remove(&session_id);
        state.session_owners.remove(&session_id);
        state.queues.remove(&session_id);
        let operation_ids = state
            .operation_owners
            .iter()
            .filter(|(_, owner)| *owner == &session_id)
            .map(|(operation_id, _)| operation_id.clone())
            .collect::<Vec<_>>();
        for operation_id in operation_ids {
            state.operation_owners.remove(&operation_id);
            state.operation_deadlines.remove(&operation_id);
            state.operation_statuses.remove(&operation_id);
            state.results.remove(&operation_id);
        }
    }
    let expired_operations = state
        .operation_deadlines
        .iter()
        .filter_map(|(operation_id, deadline)| {
            (*deadline <= now
                && state.operation_statuses.get(operation_id) == Some(&OperationStatus::Queued))
            .then(|| operation_id.clone())
        })
        .collect::<Vec<_>>();
    for operation_id in &expired_operations {
        state
            .operation_statuses
            .insert(operation_id.clone(), OperationStatus::Cancelled);
    }
    if !expired_operations.is_empty() {
        for queue in state.queues.values_mut() {
            queue.retain(|operation| !expired_operations.contains(&operation.operation_id));
        }
    }
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<HttpRequest> {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 8192];
    let header_end;
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "HTTP headers incomplete",
            ));
        }
        data.extend_from_slice(&buffer[..read]);
        if data.len() > 64 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP headers too large",
            ));
        }
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = index + 4;
            break;
        }
    }
    let header_text = std::str::from_utf8(&data[..header_end]).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP headers are not UTF-8",
        )
    })?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let path = request_parts
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default()
        .to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<HashMap<_, _>>();
    if headers.contains_key("transfer-encoding") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "chunked or transformed request bodies are unsupported",
        ));
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > MAX_REQUEST_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP body too large",
        ));
    }
    while data.len() < header_end + content_length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "HTTP body incomplete",
            ));
        }
        data.extend_from_slice(&buffer[..read]);
    }
    let body = if content_length == 0 {
        json!({})
    } else {
        serde_json::from_slice(&data[header_end..header_end + content_length])
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?
    };
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn is_allowed_origin(origin: Option<&str>) -> bool {
    origin.is_some_and(|value| {
        value
            .strip_prefix("https://localhost:")
            .or_else(|| value.strip_prefix("https://127.0.0.1:"))
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
    })
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    body: &Value,
    origin: Option<&str>,
) -> std::io::Result<()> {
    let encoded = if status == 204 {
        Vec::new()
    } else {
        serde_json::to_vec(body).unwrap_or_default()
    };
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: {}\r\nAccess-Control-Allow-Headers: authorization, content-type\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nVary: Origin\r\n\r\n",
        encoded.len(),
        origin.unwrap_or("null"),
    );
    stream.write_all(response.as_bytes())?;
    stream.write_all(&encoded)
}

pub fn ensure_office_live_bridge() -> Result<Arc<OfficeLiveBridge>, CoreError> {
    if let Some(bridge) = LIVE_BRIDGE.get() {
        return Ok(Arc::clone(bridge));
    }
    let bridge = OfficeLiveBridge::start()?;
    let _ = LIVE_BRIDGE.set(Arc::clone(&bridge));
    Ok(LIVE_BRIDGE.get().map(Arc::clone).unwrap_or(bridge))
}

pub fn office_live_bridge() -> Option<Arc<OfficeLiveBridge>> {
    LIVE_BRIDGE.get().map(Arc::clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http_post(
        bridge: &OfficeLiveBridge,
        path: &str,
        origin: Option<&str>,
        token: Option<&str>,
        body: &Value,
    ) -> (u16, Value) {
        let address = bridge.endpoint.strip_prefix("http://").unwrap();
        let encoded = serde_json::to_vec(body).unwrap();
        let mut headers = format!(
            "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            encoded.len()
        );
        if let Some(origin) = origin {
            headers.push_str(&format!("Origin: {origin}\r\n"));
        }
        if let Some(token) = token {
            headers.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        headers.push_str("\r\n");
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(headers.as_bytes()).unwrap();
        stream.write_all(&encoded).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();
        (status, serde_json::from_str(body).unwrap())
    }

    #[test]
    fn bridge_binds_loopback_rotates_pairing_code_and_queues_typed_work() {
        let bridge = OfficeLiveBridge::start().unwrap();
        let initial = bridge.status(true);
        assert!(initial.endpoint.starts_with("http://127.0.0.1:"));
        let pairing = initial.pairing_code.unwrap();
        let (status, paired) = bridge.pair(&json!({"pairingCode": pairing}));
        assert_eq!(status, 200);
        assert_ne!(bridge.status(true).pairing_code.unwrap(), pairing);
        let token = paired["bridgeToken"].as_str().unwrap();
        assert_eq!(token.len(), 64);

        let (status, registered) = bridge.register(
            token,
            &json!({
                "host": "Excel",
                "documentId": "Book1",
                "requirementSets": ["ExcelApi:1.13"],
                "capabilities": ["set-range"]
            }),
        );
        assert_eq!(status, 200);
        let session_id = registered["session"]["sessionId"].as_str().unwrap();
        let operation_id = bridge
            .enqueue(
                session_id,
                json!({"op": "excel_set_range", "address": "A1:B1", "values": [[1, 2]]}),
            )
            .unwrap();
        let (status, poll) = bridge.poll(token, &json!({"sessionId": session_id}));
        assert_eq!(status, 200);
        assert_eq!(poll["operation"]["operationId"], operation_id);
        let (status, _) = bridge.result(
            token,
            &json!({
                "operationId": operation_id,
                "sessionId": session_id,
                "status": "ok",
                "result": {"updated": true}
            }),
        );
        assert_eq!(status, 202);
        assert_eq!(
            bridge.take_result(&operation_id).unwrap().result["updated"],
            true
        );
    }

    #[test]
    fn client_tokens_cannot_cross_sessions_or_forge_results() {
        let bridge = OfficeLiveBridge::start().unwrap();
        let first_pairing = bridge.status(true).pairing_code.unwrap();
        let (_, first) = bridge.pair(&json!({"pairingCode": first_pairing}));
        let first_token = first["bridgeToken"].as_str().unwrap();
        let second_pairing = bridge.status(true).pairing_code.unwrap();
        let (_, second) = bridge.pair(&json!({"pairingCode": second_pairing}));
        let second_token = second["bridgeToken"].as_str().unwrap();
        assert_ne!(first_token, second_token);

        let (_, registered) = bridge.register(
            first_token,
            &json!({
                "host": "Word",
                "documentId": "Doc1",
                "capabilities": ["word.replace-text"]
            }),
        );
        let session_id = registered["session"]["sessionId"].as_str().unwrap();
        assert_eq!(
            bridge
                .poll(second_token, &json!({"sessionId": session_id}))
                .0,
            403
        );
        let operation_id = bridge
            .enqueue(session_id, json!({"op": "word_replace_text"}))
            .unwrap();
        assert_eq!(
            bridge
                .result(
                    second_token,
                    &json!({
                        "operationId": operation_id,
                        "sessionId": session_id,
                        "status": "ok"
                    })
                )
                .0,
            403
        );
        assert_eq!(
            bridge
                .result(
                    first_token,
                    &json!({
                        "operationId": "not-queued",
                        "sessionId": session_id,
                        "status": "ok"
                    })
                )
                .0,
            409
        );
    }

    #[test]
    fn cancelled_or_expired_operations_cannot_be_polled_or_completed() {
        let bridge = OfficeLiveBridge::start().unwrap();
        let pairing = bridge.status(true).pairing_code.unwrap();
        let (_, paired) = bridge.pair(&json!({"pairingCode": pairing}));
        let token = paired["bridgeToken"].as_str().unwrap();
        let (_, registered) = bridge.register(
            token,
            &json!({
                "host": "Excel",
                "documentId": "Book1",
                "capabilities": ["set-range"]
            }),
        );
        let session_id = registered["session"]["sessionId"].as_str().unwrap();

        let queued = bridge
            .enqueue(session_id, json!({"op": "excel_set_range"}))
            .unwrap();
        assert_eq!(
            bridge.cancel(&queued),
            OfficeLiveCancelOutcome::CancelledBeforeLease
        );
        let (_, poll) = bridge.poll(token, &json!({"sessionId": session_id}));
        assert!(poll["operation"].is_null());
        let (status, rejected) = bridge.result(
            token,
            &json!({
                "operationId": queued,
                "sessionId": session_id,
                "status": "ok"
            }),
        );
        assert_eq!(status, 409);
        assert_eq!(rejected["error"], "operation_expired_or_cancelled");

        let leased = bridge
            .enqueue(session_id, json!({"op": "excel_set_range"}))
            .unwrap();
        let (_, poll) = bridge.poll(token, &json!({"sessionId": session_id}));
        assert_eq!(poll["operation"]["operationId"], leased);
        assert!(poll["operation"]["deadlineAt"].as_u64().is_some());
        assert_eq!(
            bridge.cancel(&leased),
            OfficeLiveCancelOutcome::IndeterminateLeased
        );
        assert_eq!(
            bridge
                .result(
                    token,
                    &json!({
                        "operationId": leased,
                        "sessionId": session_id,
                        "status": "ok"
                    })
                )
                .0,
            202
        );

        let expired = bridge
            .enqueue(session_id, json!({"op": "excel_set_range"}))
            .unwrap();
        {
            let mut state = bridge.state.lock().unwrap();
            state
                .operation_deadlines
                .insert(expired.clone(), now_seconds());
            if let Some(operation) = state
                .queues
                .get_mut(session_id)
                .and_then(|queue| queue.iter_mut().find(|item| item.operation_id == expired))
            {
                operation.deadline_at = now_seconds();
            }
        }
        let (_, poll) = bridge.poll(token, &json!({"sessionId": session_id}));
        assert!(poll["operation"].is_null());
    }

    #[test]
    fn pairing_is_rate_limited_and_untrusted_origins_are_rejected() {
        let bridge = OfficeLiveBridge::start().unwrap();
        for attempt in 1..=MAX_PAIRING_FAILURES {
            let (status, _) = bridge.pair(&json!({"pairingCode": "wrong"}));
            assert_eq!(
                status,
                if attempt == MAX_PAIRING_FAILURES {
                    429
                } else {
                    403
                }
            );
        }
        assert_eq!(
            bridge
                .pair(&json!({"pairingCode": bridge.status(true).pairing_code.unwrap()}))
                .0,
            429
        );
        assert!(is_allowed_origin(Some("https://localhost:3000")));
        assert!(!is_allowed_origin(Some("https://evil.example")));
        assert!(!is_allowed_origin(None));
    }

    #[test]
    fn wire_protocol_rejects_cross_origin_and_completes_pair_register_roundtrip() {
        let bridge = OfficeLiveBridge::start().unwrap();
        let pairing = bridge.status(true).pairing_code.unwrap();
        let (status, _) = http_post(
            &bridge,
            "/v1/pair",
            Some("https://evil.example"),
            None,
            &json!({"pairingCode": pairing}),
        );
        assert_eq!(status, 403);
        let (status, paired) = http_post(
            &bridge,
            "/v1/pair",
            Some("https://localhost:3000"),
            None,
            &json!({"pairingCode": pairing}),
        );
        assert_eq!(status, 200);
        let token = paired["bridgeToken"].as_str().unwrap();
        let (status, registered) = http_post(
            &bridge,
            "/v1/register",
            Some("https://localhost:3000"),
            Some(token),
            &json!({
                "host": "PowerPoint",
                "documentId": "Presentation1",
                "requirementSets": ["PowerPointApi:1.4"],
                "capabilities": ["powerpoint.set-text"]
            }),
        );
        assert_eq!(status, 200);
        assert_eq!(registered["session"]["host"], "PowerPoint");
    }

    #[test]
    fn constant_time_token_check_rejects_prefixes() {
        assert!(constant_time_equal("abcdef", "abcdef"));
        assert!(!constant_time_equal("abc", "abcdef"));
        assert!(!constant_time_equal("abcdeg", "abcdef"));
    }
}
