//! Consumer tests: removing a gate must produce observable TCP payload at the capture server.
//! These tests qualify the broker only, not a model route or physical worker isolation.
use ironbark_core::{contracts::*, error::Result, net::{BrokerLimits, CredentialScope, Destination, NetworkAuthority, NetworkBroker}};
use std::{fs::Permissions, os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
use parking_lot::RwLock;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, UnixStream}, sync::mpsc};

struct Fixture {
    _directory: tempfile::TempDir,
    broker: NetworkBroker,
    authority: Arc<RwLock<NetworkAuthority>>,
    capture: mpsc::Receiver<String>,
    origin: String,
}
impl Fixture {
    async fn new(max_requests: u32, response: impl Into<String>) -> Self {
        let response = response.into();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (send, capture) = mpsc::channel(16);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let send = send.clone();
                let response = response.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    loop {
                        let mut chunk = [0; 4096];
                        let count = stream.read(&mut chunk).await.unwrap();
                        if count == 0 { break; }
                        bytes.extend_from_slice(&chunk[..count]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&bytes[..end]);
                            let length = head.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|v| v.parse::<usize>().ok())).unwrap_or(0);
                            if bytes.len() >= end + 4 + length { break; }
                        }
                    }
                    send.send(String::from_utf8(bytes).unwrap()).await.unwrap();
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        let project_id = ProjectId::new();
        let route = RoutePin { connection_id: "local".into(), model_id: "test".into(), harness: None };
        let authority = Arc::new(RwLock::new(NetworkAuthority {
            run_id: RunId::new(), project_id, route,
            policy: ProjectPolicy { version: 1, project_id, allowed_connection_ids: vec!["local".into()], allowed_tools: vec!["http:get".into()], read_roots: vec![], write_roots: vec![], network_destinations: vec![format!("{origin}/v1")], local_only: true, max_workers: 1, max_requests, max_cost_microusd: None, spend_policy: SpendPolicy::FreeOnly, allow_unknown_cost: true },
            connection: ConnectionConfig { id: "local".into(), protocol: ProviderProtocol::OpenaiCompletions, endpoint: format!("{origin}/v1"), models: vec![ModelDescriptor { id: "test".into(), context_limit: 1024, output_limit: 32, capabilities: vec![] }], auth_ref: Some(AuthRef::SecretService { service: "test".into(), account: "native-account-A".into() }), allowed_projects: vec![project_id] },
        }));
        let read = authority.clone();
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), Permissions::from_mode(0o700)).unwrap();
        let broker = NetworkBroker::new(directory.path().to_owned(), Arc::new(move || -> Result<NetworkAuthority> { Ok(read.read().clone()) }), BrokerLimits { max_requests, max_request_bytes: 64, max_response_bytes: 1024, lifetime: Duration::from_secs(5), request_timeout: Duration::from_secs(2) }).unwrap();
        Self { _directory: directory, broker, authority, capture, origin }
    }
    async fn lease(&self) -> ironbark_core::net::NetworkLease {
        let state = self.authority.read().clone();
        self.broker.lease(state.run_id, state.route, vec![Destination::provider(state.connection.endpoint)]).await.unwrap()
    }
    async fn no_payload(&mut self) {
        assert!(tokio::time::timeout(Duration::from_millis(100), self.capture.recv()).await.is_err(), "forbidden request reached upstream");
    }
    async fn captured(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(2), self.capture.recv()).await.expect("expected actual upstream payload").expect("capture server closed")
    }
}
async fn request(lease: &ironbark_core::net::NetworkLease, origin: &str, method: &str, path: &str, headers: &str, body: &str) -> String {
    let mut stream = UnixStream::connect(lease.socket_path()).await.unwrap();
    stream.write_all(format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", origin.trim_start_matches("http://"), body.len()).as_bytes()).await.unwrap();
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut output)).await.expect("broker response stalled").unwrap();
    String::from_utf8(output).unwrap()
}
const OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";

#[tokio::test]
async fn wrong_method_path_origin_and_connect_produce_zero_outbound_payload() {
    let mut f = Fixture::new(10, OK).await;
    let lease = f.lease().await;
    for (method, path, origin) in [("GET", "/v1/chat/completions", f.origin.as_str()), ("POST", "/v1/models", f.origin.as_str()), ("POST", "/v1/chat/completions", "http://127.0.0.2:9"), ("CONNECT", "example.com:443", f.origin.as_str()), ("POST", "/v1/chat/completions?redirect=evil", f.origin.as_str())] {
        assert!(request(&lease, origin, method, path, "", r#"{"model":"test"}"#).await.contains("403"));
    }
    f.no_payload().await;
}
#[tokio::test]
async fn revoked_and_changed_policy_never_forward() {
    let mut f = Fixture::new(10, OK).await;
    let lease = f.lease().await;
    f.authority.write().policy.version = 2;
    assert!(request(&lease, &f.origin, "POST", "/v1/chat/completions", "", r#"{"model":"test"}"#).await.contains("403"));
    f.no_payload().await;
    lease.revoke();
    assert!(!lease.active());
    assert!(lease.assert_live().is_err());
}
#[tokio::test]
async fn body_and_request_caps_deny_without_forwarding() {
    let mut f = Fixture::new(1, OK).await;
    let lease = f.lease().await;
    assert!(request(&lease, &f.origin, "POST", "/v1/chat/completions", "", &"x".repeat(65)).await.contains("413"));
    f.no_payload().await;
    assert!(request(&lease, &f.origin, "POST", "/v1/chat/completions", "", r#"{"model":"test"}"#).await.contains("200"));
    assert!(f.captured().await.ends_with(r#"{"model":"test"}"#));
    assert!(request(&lease, &f.origin, "POST", "/v1/chat/completions", "", r#"{"model":"test"}"#).await.contains("429"));
    f.no_payload().await;
}
#[tokio::test]
async fn redirects_are_errors_not_second_destination_requests() {
    let mut forbidden = Fixture::new(3, OK).await;
    let mut f = Fixture::new(3, format!("HTTP/1.1 302 Found\r\nLocation: {}/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", forbidden.origin)).await;
    let lease = f.lease().await;
    assert!(request(&lease, &f.origin, "POST", "/v1/chat/completions", "", r#"{"model":"test"}"#).await.contains("502"));
    f.captured().await;
    f.no_payload().await;
    forbidden.no_payload().await;
}
#[tokio::test]
async fn credential_headers_are_never_automatically_injected_or_borrowed() {
    let mut f = Fixture::new(3, OK).await;
    let mut forbidden = Fixture::new(3, OK).await;
    let state = f.authority.read().clone();
    let scope = CredentialScope::new(&state.connection, vec![("authorization".into(), "Bearer native-account-A".into())]).unwrap();
    let lease = f.broker.lease_with_credentials(state.run_id, state.route, vec![Destination::provider(state.connection.endpoint)], scope).await.unwrap();
    request(&lease, &f.origin, "POST", "/v1/chat/completions", "Authorization: Bearer other-account\r\nX-Api-Key: stolen\r\nCookie: private\r\n", r#"{"model":"test"}"#).await;
    let capture = f.captured().await.to_ascii_lowercase();
    assert!(!capture.contains("authorization:") && !capture.contains("x-api-key:") && !capture.contains("cookie:"));
    request(&lease, &f.origin, "POST", "/v1/chat/completions", "Authorization: Bearer native-account-A\r\n", r#"{"model":"test"}"#).await;
    assert!(f.captured().await.contains("Bearer native-account-A"));
    request(&lease, &forbidden.origin, "POST", "/v1/chat/completions", "Authorization: Bearer native-account-A\r\n", r#"{"model":"test"}"#).await;
    f.no_payload().await;
    forbidden.no_payload().await;
}
#[tokio::test]
async fn original_cloud_endpoint_cannot_be_disguised_as_local_transport() {
    let f = Fixture::new(1, OK).await;
    let (run, route, endpoint) = {
        let mut state = f.authority.write();
        state.connection.endpoint = "https://example.com/v1".into();
        state.policy.network_destinations = vec![state.connection.endpoint.clone()];
        (state.run_id, state.route.clone(), state.connection.endpoint.clone())
    };
    assert!(f.broker.lease(run, route, vec![Destination::provider(endpoint)]).await.is_err());
}

#[tokio::test]
async fn body_model_cannot_change_pinned_route() {
    let mut f = Fixture::new(2, OK).await;
    let lease = f.lease().await;
    for body in [r#"{"model":"other-account-model"}"#, "{}", r#"{"model":"other","model":"test"}"#] {
        assert!(request(&lease, &f.origin, "POST", "/v1/chat/completions", "", body).await.contains("403"));
    }
    f.no_payload().await;
}

#[tokio::test]
async fn local_endpoint_aliases_and_wrong_run_are_denied_before_listening() {
    let mut f = Fixture::new(2, OK).await;
    let state = f.authority.read().clone();
    assert!(f.broker.lease(RunId::new(), state.route.clone(), vec![Destination::provider(state.connection.endpoint.clone())]).await.is_err());
    let endpoint = {
        let mut authority = f.authority.write();
        authority.connection.endpoint = authority.connection.endpoint.replace("127.0.0.1", "localhost");
        authority.policy.network_destinations = vec![authority.connection.endpoint.clone()];
        authority.connection.endpoint.clone()
    };
    assert!(f.broker.lease(state.run_id, state.route, vec![Destination::provider(endpoint)]).await.is_err());
    f.no_payload().await;
}

#[tokio::test]
async fn sse_body_is_forwarded_without_buffering_or_silent_truncation() {
    let mut f = Fixture::new(2, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n9\r\ndata: A\n\n\r\n9\r\ndata: B\n\n\r\n0\r\n\r\n").await;
    let lease = f.lease().await;
    let output = request(&lease, &f.origin, "POST", "/v1/chat/completions", "", r#"{"model":"test"}"#).await;
    assert!(output.contains("data: A\n\n") && output.contains("data: B\n\n"));
    assert!(output.ends_with("0\r\n\r\n"), "SSE must have an honest completed transfer");
    assert!(f.captured().await.ends_with(r#"{"model":"test"}"#));
}

#[tokio::test]
async fn public_get_has_independent_destination_and_cannot_use_provider_credentials() {
    let mut f = Fixture::new(3, OK).await;
    let mut public = Fixture::new(3, OK).await;
    let url = format!("{}/public", public.origin);
    f.authority.write().policy.network_destinations.push(url.clone());
    let state = f.authority.read().clone();
    let credentials = CredentialScope::new(&state.connection, vec![("authorization".into(), "Bearer native-account-A".into())]).unwrap();
    assert!(f.broker.lease_with_credentials(state.run_id, state.route.clone(), vec![Destination::public_get(url.clone())], credentials).await.is_err());
    let lease = f.broker.lease(state.run_id, state.route, vec![Destination::public_get(url)]).await.unwrap();
    assert!(request(&lease, &public.origin, "GET", "/public", "Authorization: Bearer native-account-A\r\nCookie: private\r\n", "").await.contains("200"));
    let capture = public.captured().await.to_ascii_lowercase();
    assert!(!capture.contains("authorization:") && !capture.contains("cookie:"));
    f.no_payload().await;
}

#[tokio::test]
async fn expired_lease_closes_its_listener_and_never_forwards_payload() {
    let mut f = Fixture::new(3, OK).await;
    let read = f.authority.clone();
    let broker = NetworkBroker::new(f._directory.path().to_owned(), Arc::new(move || Ok(read.read().clone())), BrokerLimits { lifetime: Duration::from_millis(50), ..BrokerLimits::default() }).unwrap();
    let state = f.authority.read().clone();
    let lease = broker.lease(state.run_id, state.route, vec![Destination::provider(state.connection.endpoint)]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!lease.active());
    assert!(lease.assert_live().is_err());
    assert!(UnixStream::connect(lease.socket_path()).await.is_err());
    f.no_payload().await;
}

#[tokio::test]
async fn oversized_unknown_length_response_cannot_look_successfully_complete() {
    let mut f = Fixture::new(2, format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n401\r\n{}\r\n0\r\n\r\n", "x".repeat(1025))).await;
    let lease = f.lease().await;
    let mut stream = UnixStream::connect(lease.socket_path()).await.unwrap();
    stream.write_all(format!("POST /v1/chat/completions HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: 16\r\n\r\n{{\"model\":\"test\"}}", f.origin.trim_start_matches("http://")).as_bytes()).await.unwrap();
    // Body is 16 bytes. If hyper reports a reset, that is an honest failure, not a truncated success.
    let mut output = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut output)).await.expect("stream budget was not enforced");
    let output = String::from_utf8(output).unwrap();
    assert!(!output.ends_with("0\r\n\r\n"), "oversized data must not complete a successful chunked transfer");
    assert!(f.captured().await.contains("\"model\":\"test\""));
}

#[tokio::test]
async fn revocation_interrupts_an_active_upstream_stream() {
    let f = Fixture::new(3, OK).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    {
        let mut authority = f.authority.write();
        authority.connection.endpoint = format!("{origin}/v1");
        authority.policy.network_destinations = vec![authority.connection.endpoint.clone()];
    }
    let (ready_send, ready) = tokio::sync::oneshot::channel();
    let (closed_send, closed) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut payload = Vec::new();
        loop {
            let mut bytes = [0; 4096];
            let count = socket.read(&mut bytes).await.unwrap();
            assert!(count > 0);
            payload.extend_from_slice(&bytes[..count]);
            if payload.ends_with(br#"{"model":"test"}"#) { break; }
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/event-stream\r\n\r\n9\r\ndata: A\n\n\r\n").await.unwrap();
        ready_send.send(()).unwrap();
        let mut byte = [0; 1];
        let result = socket.read(&mut byte).await;
        closed_send.send(matches!(result, Ok(0)) || result.is_err()).unwrap();
    });
    let lease = f.lease().await;
    let mut worker = UnixStream::connect(lease.socket_path()).await.unwrap();
    worker.write_all(format!("POST /v1/chat/completions HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: 16\r\n\r\n{{\"model\":\"test\"}}", origin.trim_start_matches("http://")).as_bytes()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), ready).await.expect("upstream request did not begin").unwrap();
    let mut received = Vec::new();
    while !received.windows(9).any(|window| window == b"data: A\n\n") {
        let mut bytes = [0; 4096];
        let count = tokio::time::timeout(Duration::from_secs(2), worker.read(&mut bytes)).await.unwrap().unwrap();
        assert!(count > 0);
        received.extend_from_slice(&bytes[..count]);
    }
    lease.revoke();
    assert!(tokio::time::timeout(Duration::from_secs(2), closed).await.expect("broker kept active upstream alive").unwrap());
    let mut remainder = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), worker.read_to_end(&mut remainder)).await.expect("worker stream did not terminate");
    assert!(!remainder.ends_with(b"0\r\n\r\n"), "revocation cannot silently finish successful SSE");
    assert!(UnixStream::connect(lease.socket_path()).await.is_err());
}

#[tokio::test]
async fn credential_scope_for_another_registered_account_is_denied() {
    let mut f = Fixture::new(3, OK).await;
    let state = f.authority.read().clone();
    let mut other = state.connection.clone();
    other.auth_ref = Some(AuthRef::SecretService { service: "test".into(), account: "native-account-B".into() });
    let credentials = CredentialScope::new(&other, vec![("authorization".into(), "Bearer other-account".into())]).unwrap();
    assert!(f.broker.lease_with_credentials(state.run_id, state.route, vec![Destination::provider(state.connection.endpoint)], credentials).await.is_err());
    f.no_payload().await;
    other.auth_ref = Some(AuthRef::NativeSession { harness: HarnessKind::Alphacode, profile: "shared".into() });
    assert!(CredentialScope::new(&other, vec![("authorization".into(), "Bearer native-session".into())]).is_err());
}

#[tokio::test]
async fn private_bootstrap_is_sealed_and_contains_no_provider_secret() {
    use std::{io::Read, os::fd::AsRawFd};
    let f = Fixture::new(3, OK).await;
    let state = f.authority.read().clone();
    let credentials = CredentialScope::new(&state.connection, vec![("authorization".into(), "Bearer native-account-A".into())]).unwrap();
    let lease = f.broker.lease_with_credentials(state.run_id, state.route, vec![Destination::provider(state.connection.endpoint)], credentials).await.unwrap();
    let mut bootstrap = lease.bridge_bootstrap().unwrap();
    let seals = unsafe { libc::fcntl(bootstrap.as_raw_fd(), libc::F_GET_SEALS) };
    assert_eq!(seals & libc::F_SEAL_WRITE, libc::F_SEAL_WRITE);
    let mut bytes = Vec::new();
    bootstrap.read_to_end(&mut bytes).unwrap();
    assert!(bytes.starts_with(b"IBNET001"));
    assert!(!bytes.windows(b"native-account-A".len()).any(|window| window == b"native-account-A"));
}

#[tokio::test]
async fn native_session_connections_remain_unavailable_without_tool_isolation_proof() {
    let mut f = Fixture::new(3, OK).await;
    f.authority.write().connection.auth_ref = Some(AuthRef::NativeSession { harness: HarnessKind::Alphacode, profile: "shared".into() });
    let state = f.authority.read().clone();
    assert!(f.broker.lease(state.run_id, state.route, vec![Destination::provider(state.connection.endpoint)]).await.is_err());
    f.no_payload().await;
}
