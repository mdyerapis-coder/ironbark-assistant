use crate::{contracts::{AuthRef, ConnectionConfig, ProjectId, ProjectPolicy, ProviderProtocol, RoutePin, RunId}, error::{CoreError, Result}};
use bytes::Bytes;
use futures_util::{stream, StreamExt};
use http_body_util::{BodyExt, Full, Limited, StreamBody, combinators::UnsyncBoxBody};
use hyper::{body::{Frame, Incoming}, header::{HeaderMap, HeaderName, HeaderValue, HOST}, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::{collections::BTreeSet, fs::{DirBuilder, File, OpenOptions}, net::{IpAddr, SocketAddr}, os::{fd::OwnedFd, unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt}}, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicU32, AtomicUsize, Ordering}}, time::{Duration, Instant}};
use tokio::{net::{lookup_host, UnixListener}, task::JoinSet};
use tokio_util::sync::CancellationToken;
use url::Url;

type BodyError = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type HttpBody = UnsyncBoxBody<Bytes, BodyError>;
pub(crate) fn full(data: impl Into<Bytes>) -> HttpBody { Full::new(data.into()).map_err(|never| match never {}).boxed_unsync() }
fn deny(code: &'static str) -> CoreError { CoreError::new(code, "native network boundary denied request") }

#[derive(Clone, PartialEq)]
pub struct NetworkAuthority {
    pub run_id: RunId,
    pub project_id: ProjectId,
    pub route: RoutePin,
    pub policy: ProjectPolicy,
    pub connection: ConnectionConfig,
}
#[derive(Clone)]
pub struct BrokerLimits {
    pub max_requests: u32,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub lifetime: Duration,
    pub request_timeout: Duration,
}
impl Default for BrokerLimits {
    fn default() -> Self { Self { max_requests: 32, max_request_bytes: 4 * 1024 * 1024, max_response_bytes: 32 * 1024 * 1024, lifetime: Duration::from_secs(900), request_timeout: Duration::from_secs(300) } }
}
#[derive(Clone)]
pub struct Destination { endpoint: String, provider: bool }
impl Destination {
    pub fn provider(endpoint: String) -> Self { Self { endpoint, provider: true } }
    /// Exact URL, GET only, requiring independent policy `http:get` and destination approval.
    pub fn public_get(endpoint: String) -> Self { Self { endpoint, provider: false } }
}
/// Native secret scope, never serializable or Debug. This is an allowlist, NOT credential injection.
pub struct CredentialScope { headers: HeaderMap, binding: Option<(String, AuthRef)> }
impl CredentialScope {
    pub fn new(connection: &ConnectionConfig, values: Vec<(String, String)>) -> Result<Self> {
        let auth_ref = connection.auth_ref.clone().ok_or_else(|| deny("credential_reference_missing"))?;
        if matches!(auth_ref, AuthRef::NativeSession { .. }) || values.is_empty() { return Err(deny("native_auth_unqualified")); }
        let mut headers = HeaderMap::new();
        for (name, value) in values {
            if !matches!(name.as_str(), "authorization" | "x-api-key" | "x-goog-api-key") || headers.contains_key(name.as_str()) { return Err(deny("invalid_credential_scope")); }
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| deny("invalid_credential_scope"))?;
            let mut value = HeaderValue::from_str(&value).map_err(|_| deny("invalid_credential_scope"))?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        Ok(Self { headers, binding: Some((connection.id.clone(), auth_ref)) })
    }
}
type AuthoritySource = Arc<dyn Fn() -> Result<NetworkAuthority> + Send + Sync>;
pub struct NetworkBroker { directory: PathBuf, authority: AuthoritySource, limits: BrokerLimits }
struct LeaseState {
    authority: AuthoritySource, pinned: NetworkAuthority, endpoint: Url, original: String,
    authority_header: String, paths: Vec<String>, provider: bool, addresses: Vec<SocketAddr>,
    credentials: CredentialScope, client: reqwest::Client, limits: BrokerLimits, requests: AtomicU32,
    handles: AtomicUsize, expires: Instant, token: CancellationToken, socket: PathBuf,
    directory: PathBuf, socket_device: u64, socket_inode: u64,
}
/// Owns only native authority and broker lifetime; no serde, no secret-bearing Debug.
pub struct NetworkLease { state: Arc<LeaseState> }
impl Clone for NetworkLease {
    fn clone(&self) -> Self { self.state.handles.fetch_add(1, Ordering::Relaxed); Self { state: self.state.clone() } }
}
impl Drop for NetworkLease {
    fn drop(&mut self) { if self.state.handles.fetch_sub(1, Ordering::AcqRel) == 1 { self.revoke(); } }
}
impl NetworkLease {
    pub fn run_id(&self) -> RunId { self.state.pinned.run_id }
    pub fn active(&self) -> bool { !self.state.token.is_cancelled() && Instant::now() < self.state.expires }
    pub fn assert_live(&self) -> Result<()> {
        if !self.active() { return Err(deny("network_lease_revoked")); }
        if (self.state.authority)()? != self.state.pinned { return Err(deny("stale_network_authority")); }
        Ok(())
    }
    pub fn revoke(&self) {
        self.state.token.cancel();
        let _ = std::fs::remove_file(&self.state.socket);
        let _ = std::fs::remove_dir(&self.state.directory);
    }
    pub fn socket_path(&self) -> &Path { &self.state.socket }
    pub fn namespace_socket_path(&self) -> &Path { Path::new("/run/ironbark-net/broker.sock") }
    pub fn socket_mount_fd(&self) -> Result<OwnedFd> {
        self.assert_live()?;
        let file = OpenOptions::new().read(true).custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&self.state.socket)?;
        use std::os::unix::fs::FileTypeExt;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_socket() || metadata.dev() != self.state.socket_device || metadata.ino() != self.state.socket_inode { return Err(deny("invalid_broker_socket")); }
        Ok(file.into())
    }
    pub fn original_endpoint(&self) -> &str { &self.state.original }
    pub fn transport_endpoint(&self) -> String {
        format!("http://127.0.0.1:43187{}", self.state.endpoint.path().trim_end_matches('/'))
    }
    /// Nonsecret, sealed inherited-fd bootstrap. Do not pass via argv, env, DTO or model text.
    pub fn bridge_bootstrap(&self) -> Result<File> {
        self.assert_live()?;
        super::namespace_bridge::bootstrap(self.namespace_socket_path(), &self.state.authority_header)
    }
}
impl NetworkBroker {
    pub fn new(directory: PathBuf, authority: AuthoritySource, limits: BrokerLimits) -> Result<Self> {
        if limits.max_requests == 0 || limits.max_request_bytes == 0 || limits.max_response_bytes == 0 || limits.lifetime.is_zero() || limits.request_timeout.is_zero() { return Err(deny("invalid_network_limits")); }
        if Instant::now().checked_add(limits.lifetime).is_none() || Instant::now().checked_add(limits.request_timeout).is_none() { return Err(deny("invalid_network_limits")); }
        // Caller supplies a native-private directory, not a worker-writable workspace.
        if !directory.exists() { DirBuilder::new().mode(0o700).create(&directory)?; }
        let metadata = std::fs::symlink_metadata(&directory)?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } || metadata.permissions().mode() & 0o077 != 0 { return Err(deny("broker_directory_not_private")); }
        Ok(Self { directory, authority, limits })
    }
    pub async fn lease(&self, run_id: RunId, route: RoutePin, destinations: Vec<Destination>) -> Result<NetworkLease> {
        self.lease_with_credentials(run_id, route, destinations, CredentialScope { headers: HeaderMap::new(), binding: None }).await
    }
    pub async fn lease_with_credentials(&self, run_id: RunId, route: RoutePin, destinations: Vec<Destination>, credentials: CredentialScope) -> Result<NetworkLease> {
        let pinned = (self.authority)()?;
        validate_authority(&pinned, run_id, &route)?;
        if credentials.binding.as_ref().is_some_and(|(id, auth_ref)| id != &pinned.connection.id || pinned.connection.auth_ref.as_ref() != Some(auth_ref)) { return Err(deny("credential_account_mismatch")); }
        // One origin per lease: public tools never share the provider credential scope/socket.
        if destinations.len() != 1 { return Err(deny("one_destination_per_lease")); }
        let destination = destinations.into_iter().next().ok_or_else(|| deny("missing_destination"))?;
        if destination.provider && destination.endpoint != pinned.connection.endpoint { return Err(deny("route_endpoint_mismatch")); }
        if !destination.provider && (!credentials.headers.is_empty() || !pinned.policy.allowed_tools.iter().any(|tool| tool == "http:get")) { return Err(deny("tool_credential_or_policy_denied")); }
        if !crate::policy::routes::destination_allowed(&destination.endpoint, &pinned.policy.network_destinations) { return Err(deny("destination_not_approved")); }
        let endpoint = parse_endpoint(&destination.endpoint)?;
        let addresses = tokio::time::timeout(self.limits.request_timeout, resolve(&endpoint, pinned.policy.local_only)).await.map_err(|_| deny("dns_timeout"))??;
        let paths = if destination.provider { provider_paths(&endpoint, pinned.connection.protocol, &route.model_id)? } else { vec![path_query(&endpoint)] };
        if (self.authority)()? != pinned { return Err(deny("stale_network_authority")); }
        let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).retry(reqwest::retry::never()).timeout(self.limits.request_timeout)
            .resolve_to_addrs(endpoint.host_str().ok_or_else(|| deny("invalid_host"))?, &addresses)
            .build().map_err(|_| deny("transport_unavailable"))?;
        let directory = self.directory.join(uuid::Uuid::new_v4().simple().to_string());
        DirBuilder::new().mode(0o700).create(&directory)?;
        let socket = directory.join("broker.sock");
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let socket_metadata = std::fs::symlink_metadata(&socket)?;
        let mut limits = self.limits.clone();
        limits.max_requests = limits.max_requests.min(pinned.policy.max_requests);
        let expires = Instant::now().checked_add(limits.lifetime).ok_or_else(|| deny("invalid_network_limits"))?;
        let state = Arc::new(LeaseState { authority: self.authority.clone(), pinned, authority_header: authority_header(&endpoint)?, original: destination.endpoint, endpoint, paths, provider: destination.provider, addresses, credentials, client, expires, limits, requests: AtomicU32::new(0), handles: AtomicUsize::new(1), token: CancellationToken::new(), socket, directory, socket_device: socket_metadata.dev(), socket_inode: socket_metadata.ino() });
        tokio::spawn(serve(listener, state.clone()));
        Ok(NetworkLease { state })
    }
}
fn validate_authority(state: &NetworkAuthority, run: RunId, route: &RoutePin) -> Result<()> {
    if state.run_id != run || state.route != *route || state.project_id != state.policy.project_id
        || route.connection_id != state.connection.id
        || !state.policy.allowed_connection_ids.contains(&state.connection.id)
        || !state.connection.allowed_projects.contains(&state.project_id)
        || !state.connection.models.iter().any(|m| m.id == route.model_id)
        || state.policy.max_requests == 0 { return Err(deny("route_policy_denied")); }
    if matches!(state.connection.auth_ref.as_ref(), Some(AuthRef::NativeSession { .. })) { return Err(deny("native_auth_unqualified")); }
    // The original endpoint is authoritative; never classify the private loopback rewrite.
    let endpoint = parse_endpoint(&state.connection.endpoint)?;
    if state.policy.local_only && !literal_loopback(&endpoint) { return Err(deny("local_only_route_denied")); }
    Ok(())
}
fn parse_endpoint(text: &str) -> Result<Url> {
    let url = Url::parse(text).map_err(|_| deny("invalid_endpoint"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() || url.query().is_some() || text.contains('%') || text.contains('\\') { return Err(deny("invalid_endpoint")); }
    if literal_loopback(&url) {
        let raw = text.split_once("://").ok_or_else(|| deny("invalid_endpoint"))?.1.split('/').next().unwrap_or("");
        let canonical = if matches!(url.host(), Some(url::Host::Ipv4(_))) { "127.0.0.1" } else { "[::1]" };
        if raw != canonical && !raw.strip_prefix(&format!("{canonical}:")).and_then(|port| port.parse::<u16>().ok()).is_some_and(|port| Some(port) == url.port_or_known_default()) { return Err(deny("loopback_alias_denied")); }
    }
    Ok(url)
}
fn literal_loopback(url: &Url) -> bool { matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip == std::net::Ipv4Addr::LOCALHOST) || matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip == std::net::Ipv6Addr::LOCALHOST) }
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => { let [a,b,c,_] = ip.octets(); !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_multicast() || ip.is_unspecified() || ip.is_broadcast() || ip.is_documentation() || a == 0 || a >= 240 || (a == 100 && (64..=127).contains(&b)) || (a == 192 && b == 0) || (a == 198 && (b == 18 || b == 19)) || (a == 192 && b == 88 && c == 99)) }
        IpAddr::V6(ip) => { let segments = ip.segments(); segments[0] & 0xe000 == 0x2000 && !(segments[0] == 0x2001 && (segments[1] < 0x0200 || segments[1] == 0x0db8)) }
    }
}
async fn resolve(endpoint: &Url, local_only: bool) -> Result<Vec<SocketAddr>> {
    let port = endpoint.port_or_known_default().ok_or_else(|| deny("invalid_port"))?;
    let addresses: BTreeSet<SocketAddr> = match endpoint.host().ok_or_else(|| deny("invalid_host"))? {
        url::Host::Ipv4(ip) => [SocketAddr::new(ip.into(), port)].into_iter().collect(),
        url::Host::Ipv6(ip) => [SocketAddr::new(ip.into(), port)].into_iter().collect(),
        url::Host::Domain(host) => {
            if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") || host.ends_with(".local") { return Err(deny("loopback_alias_denied")); }
            lookup_host((host, port)).await.map_err(|_| deny("dns_failed"))?.collect()
        }
    };
    if addresses.is_empty() || addresses.iter().any(|address| if literal_loopback(endpoint) { !address.ip().is_loopback() } else { local_only || !public_ip(address.ip()) }) { return Err(deny("dns_address_denied")); }
    if endpoint.scheme() != "https" && !literal_loopback(endpoint) { return Err(deny("cloud_tls_required")); }
    Ok(addresses.into_iter().collect())
}
fn path_query(url: &Url) -> String { match url.query() { Some(query) => format!("{}?{query}", url.path()), None => url.path().into() } }
fn authority_header(url: &Url) -> Result<String> {
    let host = match url.host().ok_or_else(|| deny("invalid_host"))? { url::Host::Ipv6(ip) => format!("[{ip}]"), other => other.to_string() };
    Ok(match url.port() { Some(port) => format!("{host}:{port}"), None => host })
}
fn provider_paths(endpoint: &Url, protocol: ProviderProtocol, model: &str) -> Result<Vec<String>> {
    let base = endpoint.path().trim_end_matches('/');
    let base = if base.is_empty() { match protocol { ProviderProtocol::Ollama => "/api", ProviderProtocol::Gemini => "/v1beta", _ => "/v1" } } else { base };
    Ok(match protocol {
        ProviderProtocol::OpenaiCompletions | ProviderProtocol::LlamaCpp => vec![format!("{base}/chat/completions")],
        ProviderProtocol::OpenaiResponses => vec![format!("{base}/responses")],
        ProviderProtocol::Anthropic => vec![format!("{base}/messages")],
        ProviderProtocol::Ollama => vec![format!("{base}/chat"), format!("{base}/generate")],
        ProviderProtocol::Gemini => {
            if model.is_empty() || !model.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')) { return Err(deny("invalid_model_path")); }
            vec![format!("{base}/models/{model}:generateContent"), format!("{base}/models/{model}:streamGenerateContent?alt=sse")]
        }
        ProviderProtocol::Custom => return Err(deny("protocol_unavailable")),
    })
}
async fn serve(listener: UnixListener, state: Arc<LeaseState>) {
    let mut tasks = JoinSet::new();
    let deadline = tokio::time::Instant::from_std(state.expires);
    loop {
        tokio::select! {
            biased;
            _ = state.token.cancelled() => break,
            _ = tokio::time::sleep_until(deadline) => { state.token.cancel(); break; },
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { state.token.cancel(); break; };
                // Bound concurrent connections separately from lifetime request budget.
                if tasks.len() >= 16 { drop(stream); continue; }
                let state = state.clone();
                tasks.spawn(async move {
                    let service = hyper::service::service_fn(move |request| handle(request, state.clone()));
                    let _ = hyper::server::conn::http1::Builder::new().keep_alive(false).max_buf_size(16 * 1024).serve_connection(TokioIo::new(stream), service).await;
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    drop(listener);
    let _ = std::fs::remove_file(&state.socket);
    let _ = std::fs::remove_dir(&state.directory);
}
fn response(status: StatusCode) -> Response<HttpBody> { let mut response = Response::new(full(Bytes::new())); *response.status_mut() = status; response }
fn live(state: &LeaseState) -> Result<()> {
    if state.token.is_cancelled() || Instant::now() >= state.expires { return Err(deny("network_lease_revoked")); }
    if (state.authority)()? != state.pinned { return Err(deny("stale_network_authority")); }
    Ok(())
}
async fn handle(request: Request<Incoming>, state: Arc<LeaseState>) -> std::result::Result<Response<HttpBody>, std::convert::Infallible> {
    if live(&state).is_err() { return Ok(response(StatusCode::FORBIDDEN)); }
    if request.method() == hyper::Method::GET && request.uri().path() == "/__ironbark_lease_alive" && request.uri().query().is_none() {
        let token = state.token.clone();
        let stream = stream::unfold((token, true), |(token, first)| async move {
            if !first { tokio::select! { _ = token.cancelled() => return None, _ = tokio::time::sleep(Duration::from_secs(1)) => {} } }
            Some((Ok::<_, BodyError>(Frame::data(Bytes::from_static(b"."))), (token, false)))
        });
        return Ok(Response::new(StreamBody::new(stream).boxed_unsync()));
    }
    let target = request.uri().path_and_query().map(|path| path.as_str()).unwrap_or("");
    let expected_method = if state.provider { hyper::Method::POST } else { hyper::Method::GET };
    if request.method() != expected_method || request.uri().scheme().is_some() || request.uri().authority().is_some()
        || !state.paths.iter().any(|path| path == target)
        || request.headers().get_all(HOST).iter().count() != 1
        || request.headers().get(HOST).and_then(|h| h.to_str().ok()) != Some(state.authority_header.as_str()) {
        return Ok(response(StatusCode::FORBIDDEN));
    }
    let mut outgoing = HeaderMap::new();
    // No Cookie, proxy auth, forwarding, model-controlled origin, custom account, or hop headers.
    for name in ["content-type", "accept", "anthropic-version", "anthropic-beta"] {
        if let Some(value) = request.headers().get(name) { outgoing.insert(HeaderName::from_static(name), value.clone()); }
    }
    if state.provider {
        for (name, approved) in state.credentials.headers.iter() {
            if request.headers().get_all(name).iter().count() == 1 && request.headers().get(name) == Some(approved) { outgoing.insert(name.clone(), approved.clone()); }
        }
    }
    let target = target.to_owned();
    let body = match tokio::time::timeout(state.limits.request_timeout, Limited::new(request.into_body(), state.limits.max_request_bytes).collect()).await {
        Ok(Ok(body)) => body.to_bytes(), _ => return Ok(response(StatusCode::PAYLOAD_TOO_LARGE)),
    };
    if !state.provider && !body.is_empty() { return Ok(response(StatusCode::FORBIDDEN)); }
    if state.provider {
        if state.pinned.connection.protocol == ProviderProtocol::Gemini {
            let json: serde_json::Value = match serde_json::from_slice(&body) { Ok(value) => value, Err(_) => return Ok(response(StatusCode::FORBIDDEN)) };
            if !json.is_object() { return Ok(response(StatusCode::FORBIDDEN)); }
        } else {
            // Derived field deserialization rejects duplicate model keys, unlike Value's last-wins.
            #[derive(serde::Deserialize)]
            struct ModelBinding<'a> { #[serde(borrow)] model: std::borrow::Cow<'a, str> }
            let binding: ModelBinding<'_> = match serde_json::from_slice(&body) { Ok(value) => value, Err(_) => return Ok(response(StatusCode::FORBIDDEN)) };
            if binding.model.as_ref() != state.pinned.route.model_id.as_str() { return Ok(response(StatusCode::FORBIDDEN)); }
        }
    }
    if state.requests.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| if count < state.limits.max_requests { Some(count + 1) } else { None }).is_err() { return Ok(response(StatusCode::TOO_MANY_REQUESTS)); }
    let upstream = tokio::select! {
        biased;
        _ = state.token.cancelled() => return Ok(response(StatusCode::FORBIDDEN)),
        result = forward(&state, &target, outgoing, body) => result,
    };
    let upstream = match upstream { Ok(value) => value, Err(_) => return Ok(response(StatusCode::BAD_GATEWAY)) };
    if upstream.status().is_redirection() || upstream.content_length().is_some_and(|n| n > state.limits.max_response_bytes as u64) { return Ok(response(StatusCode::BAD_GATEWAY)); }
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let token = state.token.clone();
    let limit = state.limits.max_response_bytes;
    let body = stream::unfold((upstream.bytes_stream(), 0usize, token), move |(mut upstream, total, token)| async move {
        let next = tokio::select! { biased; _ = token.cancelled() => Some(Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "network revoked").into())), next = upstream.next() => next.map(|next| next.map_err(|_| -> BodyError { std::io::Error::other("upstream body failed").into() })) };
        match next {
            None => None,
            Some(Ok(bytes)) if total.checked_add(bytes.len()).is_some_and(|sum| sum <= limit) => { let count = total + bytes.len(); Some((Ok(Frame::data(bytes)), (upstream, count, token))) },
            Some(Ok(_)) => Some((Err::<Frame<Bytes>, BodyError>(std::io::Error::other("response budget exceeded").into()), (upstream, limit, token))),
            Some(Err(error)) => Some((Err(error), (upstream, total, token))),
        }
    });
    let mut response = Response::new(StreamBody::new(body).boxed_unsync());
    *response.status_mut() = status;
    for name in ["content-type", "content-length", "content-encoding", "retry-after"] { if let Some(value) = headers.get(name) { response.headers_mut().insert(HeaderName::from_static(name), value.clone()); } }
    Ok(response)
}
async fn forward(state: &LeaseState, target: &str, headers: HeaderMap, body: Bytes) -> Result<reqwest::Response> {
    live(state)?;
    let addresses = tokio::time::timeout(state.limits.request_timeout, resolve(&state.endpoint, state.pinned.policy.local_only)).await.map_err(|_| deny("dns_timeout"))??;
    if addresses != state.addresses { return Err(deny("dns_pin_changed")); }
    live(state)?;
    let mut url = state.endpoint.clone();
    let (path, query) = target.split_once('?').map_or((target, None), |(path, query)| (path, Some(query)));
    url.set_path(path); url.set_query(query);
    let method = if state.provider { reqwest::Method::POST } else { reqwest::Method::GET };
    state.client.request(method, url).headers(headers).body(body).send().await.map_err(|_| deny("upstream_failed"))
}
