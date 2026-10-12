//! Namespace-local bridge. Private bootstrap is an inherited sealed fd, never model JSON.
use super::broker::{full, HttpBody};
use crate::error::{CoreError, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use hyper::{body::Incoming, header::{HeaderValue, HOST}, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::{fs::File, io::{Read, Seek, SeekFrom, Write}, os::{fd::{AsRawFd, FromRawFd, RawFd}, unix::ffi::OsStrExt}, path::{Path, PathBuf}, sync::Arc};
use tokio::{net::{TcpListener, UnixStream}, task::JoinSet};
use tokio_util::sync::CancellationToken;
const MAGIC: &[u8; 8] = b"IBNET001";
const BIND: &str = "127.0.0.1:43187";
fn error() -> CoreError { CoreError::new("namespace_bridge_denied", "private network bridge unavailable") }

pub(crate) fn bootstrap(socket: &Path, authority: &str) -> Result<File> {
    let path = socket.as_os_str().as_bytes();
    if path.len() > 256 || authority.len() > 512 || path.contains(&0) || authority.bytes().any(|b| !b.is_ascii_graphic()) { return Err(error()); }
    let name = c"ironbark-network-bootstrap";
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 { return Err(error()); }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(MAGIC)?;
    file.write_all(&(path.len() as u16).to_be_bytes())?;
    file.write_all(&(authority.len() as u16).to_be_bytes())?;
    file.write_all(path)?;
    file.write_all(authority.as_bytes())?;
    file.seek(SeekFrom::Start(0))?;
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK;
    if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } < 0 { return Err(error()); }
    Ok(file)
}
struct Config { socket: PathBuf, authority: HeaderValue }
fn read_config(fd: RawFd) -> Result<Config> {
    if fd < 3 { return Err(error()); }
    // Own and close the inherited descriptor immediately after reading. Reject ordinary files/pipes.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let required = libc::F_SEAL_SEAL | libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK;
    let seals = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
    if seals < 0 || seals & required != required || file.metadata()?.len() > 780 { return Err(error()); }
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    if data.len() < 12 || &data[..8] != MAGIC { return Err(error()); }
    let path_len = u16::from_be_bytes([data[8],data[9]]) as usize;
    let authority_len = u16::from_be_bytes([data[10],data[11]]) as usize;
    if path_len > 256 || authority_len > 512 || data.len() != 12 + path_len + authority_len { return Err(error()); }
    let socket = std::str::from_utf8(&data[12..12+path_len]).map_err(|_| error())?;
    if socket != "/run/ironbark-net/broker.sock" { return Err(error()); }
    let authority = HeaderValue::from_bytes(&data[12+path_len..]).map_err(|_| error())?;
    Ok(Config { socket: socket.into(), authority })
}
fn response(status: StatusCode) -> Response<HttpBody> { let mut response = Response::new(full(Bytes::new())); *response.status_mut() = status; response }
async fn connect(config: &Config) -> Result<hyper::client::conn::http1::SendRequest<HttpBody>> {
    let socket = UnixStream::connect(&config.socket).await?;
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(socket)).await.map_err(|_| error())?;
    tokio::spawn(async move { let _ = connection.await; });
    Ok(sender)
}
async fn proxy(mut request: Request<Incoming>, config: Arc<Config>) -> std::result::Result<Response<HttpBody>, std::convert::Infallible> {
    // No model-supplied origin rewrite or absolute-form proxy traffic is trusted.
    if request.method() == hyper::Method::CONNECT || request.uri().scheme().is_some() || request.uri().authority().is_some()
        || request.headers().get_all(HOST).iter().count() != 1
        || request.headers().get(HOST).and_then(|v| v.to_str().ok()) != Some(BIND)
        || request.uri().path() == "/__ironbark_lease_alive" {
        return Ok(response(StatusCode::FORBIDDEN));
    }
    request.headers_mut().insert(HOST, config.authority.clone());
    request.headers_mut().insert(hyper::header::CONNECTION, HeaderValue::from_static("close"));
    // Backpressure is maintained by hyper, no unbounded buffering; broker enforces its tighter cap.
    let request = request.map(|body| Limited::new(body, 4 * 1024 * 1024).map_err(|_| -> Box<dyn std::error::Error + Send + Sync> { std::io::Error::other("bridge body budget exceeded").into() }).boxed_unsync());
    let mut sender = match connect(&config).await { Ok(sender) => sender, Err(_) => return Ok(response(StatusCode::BAD_GATEWAY)) };
    let upstream = match sender.send_request(request).await { Ok(value) => value, Err(_) => return Ok(response(StatusCode::BAD_GATEWAY)) };
    Ok(upstream.map(|body| body.map_err(|_| -> Box<dyn std::error::Error + Send + Sync> { std::io::Error::other("broker stream interrupted").into() }).boxed_unsync()))
}
/// Run only as the staged binary INSIDE bwrap's private network namespace.
/// The native parent provides fd via explicit descriptor mapping; worker/model argv cannot mint it.
/// No provider credentials are injected here. SDK credential fd must be isolated separately.
pub async fn run_from_fd(fd: RawFd) -> Result<()> {
    // pre_exec alone is insufficient: this executes after the bridge exec boundary.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } < 0 { return Err(error()); }
    let config = Arc::new(read_config(fd)?);
    let mut control = connect(&config).await?;
    let request = Request::builder().method("GET").uri("/__ironbark_lease_alive").header(HOST, config.authority.clone()).body(full(Bytes::new())).map_err(|_| error())?;
    let control_response = control.send_request(request).await.map_err(|_| error())?;
    if control_response.status() != StatusCode::OK { return Err(error()); }
    let mut monitor = control_response.into_body();
    // Do not publish an unrestricted/stale listener when bootstrap broker is already dead.
    if !matches!(monitor.frame().await, Some(Ok(_))) { return Err(error()); }
    let listener = TcpListener::bind(BIND).await?;
    let cancel = CancellationToken::new();
    let stopped = cancel.clone();
    tokio::spawn(async move { while let Some(frame) = monitor.frame().await { if frame.is_err() { break; } } stopped.cancel(); });
    let mut requests = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = requests.join_next(), if !requests.is_empty() => {},
            accepted = listener.accept() => {
                let (socket, _) = accepted?;
                if requests.len() >= 16 { drop(socket); continue; }
                let config = config.clone();
                requests.spawn(async move {
                    let service = hyper::service::service_fn(move |request| proxy(request, config.clone()));
                    let _ = hyper::server::conn::http1::Builder::new().keep_alive(false).max_buf_size(16 * 1024).serve_connection(TokioIo::new(socket), service).await;
                });
            }
        }
    }
    requests.abort_all();
    while requests.join_next().await.is_some() {}
    drop(listener);
    Ok(())
}
