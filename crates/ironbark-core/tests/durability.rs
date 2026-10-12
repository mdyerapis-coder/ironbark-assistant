use ironbark_core::{contracts::*, ipc::authorize_peer, store::{RequestDisposition, Store}};
use tempfile::TempDir;
use uuid::Uuid;

fn temporary_store() -> (TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("core.sqlite3");
    (dir, path)
}

// Removing durable reply dedup would create two projects for this request.
#[tokio::test]
async fn duplicate_request_is_not_redispatched() {
    let (_dir, path) = temporary_store();
    let store = Store::open(&path).await.unwrap();
    let id = Uuid::new_v4();
    let command = CoreCommand::ProjectCreate { name: "one".into(), root: "/tmp".into() };
    assert_eq!(store.accept_request(id, &command).await.unwrap(), RequestDisposition::New);
    let first_reply = store.execute_accepted(id, &command).await.unwrap();
    let second = store.accept_request(id, &command).await.unwrap();
    assert_eq!(second, RequestDisposition::Existing(first_reply));
    assert_eq!(store.projects().await.unwrap().len(), 1);
    assert_eq!(store.events_after(0).await.unwrap().len(), 1);
    let different = CoreCommand::ProjectCreate { name: "two".into(), root: "/tmp".into() };
    assert_eq!(store.accept_request(id, &different).await.unwrap_err().code, "request_id_conflict");
}

// Removing event commit or replay ordering loses/reorders events after restart.
#[tokio::test]
async fn committed_events_survive_reopen() {
    let (_dir, path) = temporary_store();
    {
        let store = Store::open(&path).await.unwrap();
        for n in 0..3 {
            store.append(None, CoreEvent::Error { code: format!("e{n}"), message: "boundary".into(), details: serde_json::json!({}) }).await.unwrap();
        }
    }
    let store = Store::open(&path).await.unwrap();
    assert_eq!(store.events_after(0).await.unwrap().iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(store.events_after(1).await.unwrap().iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]);
}

// Accepting foreign UID or nonce before dispatch would expose all core operations.
#[test]
fn foreign_peer_and_bad_nonce_are_rejected() {
    assert!(authorize_peer(1000, 1001, 1, "correct", "correct").is_err());
    assert!(authorize_peer(1000, 1000, 1, "incorrect", "correct").is_err());
    assert!(authorize_peer(1000, 1000, 2, "correct", "correct").is_err());
    assert!(authorize_peer(1000, 1000, 1, "correct", "correct").is_ok());
}

// Serializing before redaction leaks the sentinel into SQLite/WAL bytes.
#[tokio::test]
async fn secret_redaction_precedes_event_commit() {
    let (dir, path) = temporary_store();
    let sentinel = "C1_CREDENTIAL_SENTINEL_6e8f1f9d";
    let store = Store::open(&path).await.unwrap();
    store.append(None, CoreEvent::Error { code: "upstream".into(), message: format!("Authorization: Bearer {sentinel}"), details: serde_json::json!({"headers":{"Authorization":sentinel}, "access_token":sentinel,"credential_lease_id":sentinel}) }).await.unwrap();
    let events = store.events_after(0).await.unwrap();
    assert!(!serde_json::to_string(&events).unwrap().contains(sentinel));
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() { assert!(!String::from_utf8_lossy(&std::fs::read(path).unwrap()).contains(sentinel)); }
    }
}

// Omitting text-header redaction leaks complete cookie values into replay and WAL.
#[tokio::test]
async fn cookie_header_text_is_redacted_before_persistence() {
    let (dir, path) = temporary_store();
    let store = Store::open(&path).await.unwrap();
    let sentinel = "C1_COOKIE_SENTINEL_516b27";
    let headers = [
        format!("Cookie: session={sentinel}; other={sentinel}"),
        format!("sEt-CoOkIe: session={sentinel}; HttpOnly; Secure"),
        format!("HTTP headers:\r\ncOoKiE: session={sentinel}; other={sentinel}\r\nX-Trace: keep"),
        format!("Set-Cookie: session=first;\r\n\tcontinued={sentinel}\r\nX-Trace: keep"),
    ];
    for header in headers {
        store.append(None, CoreEvent::Error {
            code: "header_dump".into(),
            message: header.clone(),
            details: serde_json::json!({"diagnostic":header}),
        }).await.unwrap();
    }
    let replay = serde_json::to_string(&store.events_after(0).await.unwrap()).unwrap();
    assert!(!replay.contains(sentinel), "cookie secret reached durable replay");
    assert!(replay.contains("X-Trace: keep"), "unrelated header was lost");
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            assert!(!String::from_utf8_lossy(&std::fs::read(path).unwrap()).contains(sentinel),
                "cookie secret reached persisted bytes");
        }
    }
}

// Leaf-only checking admits a replaceable parent and writes into its hierarchy.
#[tokio::test]
async fn unsafe_private_ancestry_is_rejected_before_creation() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
    let leaf = shared.join("private");
    assert!(ironbark_core::paths::private_directory(&leaf).is_err());
    assert!(!leaf.exists());
    assert!(Store::open(&shared.join("core.sqlite3")).await.is_err());
    assert!(!shared.join("core.sqlite3").exists());
    let actual = dir.path().join("actual");
    std::fs::create_dir(&actual).unwrap();
    let alias = dir.path().join("alias");
    symlink(&actual, &alias).unwrap();
    assert!(ironbark_core::paths::private_directory(&alias.join("private")).is_err());
    assert!(!actual.join("private").exists());
}

// A crash between durable acceptance and reply must not silently redispatch.
#[tokio::test]
async fn abandoned_requests_require_reconciliation() {
    let (_dir, path) = temporary_store();
    let id = Uuid::new_v4();
    let command = CoreCommand::Shutdown {};
    {
        let store = Store::open(&path).await.unwrap();
        assert_eq!(store.accept_request(id, &command).await.unwrap(), RequestDisposition::New);
    }
    let store = Store::open(&path).await.unwrap();
    assert_eq!(store.accept_request(id, &command).await.unwrap(), RequestDisposition::InFlight);
    assert_eq!(store.abandoned_requests().await.unwrap(), vec![id]);
}

#[test]
fn malformed_security_variants_are_rejected() {
    assert!(serde_json::from_str::<HarnessKind>("\"future_harness\"").is_err());
    assert!(serde_json::from_str::<Digest>("\"not-a-digest\"").is_err());
    for command in ["shutdown", "status", "project_list", "doctor"] {
        let unexpected = format!("{{\"type\":\"{command}\",\"untrusted\":true}}");
        assert!(serde_json::from_str::<CoreCommand>(&unexpected).is_err());
    }
    assert!(serde_json::from_str::<ironbark_core::contracts::ScoreEvidence>("{\"type\":\"none\",\"upper_error_bound\":0}").is_err());
    assert!(serde_json::from_str::<ironbark_core::policy::service::ApprovalDecision>("{\"decision\":\"deny\",\"allowed\":true}").is_err());
}

// Common provider credential headers must be scrubbed, not just Authorization.
#[tokio::test]
async fn provider_header_secrets_are_redacted_in_sqlite_bytes() {
    let (dir, path) = temporary_store();
    let store = Store::open(&path).await.unwrap();
    let sentinel = "C1_XAPI_SENTINEL_d33d47";
    store.append(None, CoreEvent::Error { code: "headers".into(), message: "upstream failed".into(), details: serde_json::json!({"headers":{"X-Api-Key":sentinel,"X-Goog-Api-Key":sentinel},"session_token":sentinel,"private_key":sentinel}) }).await.unwrap();
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() { assert!(!String::from_utf8_lossy(&std::fs::read(path).unwrap()).contains(sentinel)); }
    }
}

// Committing an unstreamable event would permanently wedge event recovery.
#[tokio::test]
async fn oversized_event_is_not_committed() {
    let (_dir, path) = temporary_store();
    let store = Store::open(&path).await.unwrap();
    let error = store.append(None, CoreEvent::TextDelta { message_id: MessageId::new(), delta: "x".repeat(ironbark_core::ipc::MAX_FRAME) }).await.unwrap_err();
    assert_eq!(error.code, "frame_too_large");
    assert!(store.events_after(0).await.unwrap().is_empty());
}

// Dropping oversized frames must happen before unbounded allocation/dispatch.
#[tokio::test]
async fn oversized_ipc_frames_are_rejected() {
    let mut bytes = vec![b'x'; ironbark_core::ipc::MAX_FRAME + 1];
    bytes.push(b'\n');
    let mut reader = std::io::Cursor::new(bytes);
    assert_eq!(ironbark_core::ipc::read_line(&mut reader).await.unwrap_err().code, "frame_too_large");
}

async fn socket_frame(reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>) -> ServerFrame {
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(5), ironbark_core::ipc::read_line(reader)).await.unwrap().unwrap().unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// Uses the actual authorized IPC sink, durable backlog and live notifications.
#[tokio::test]
async fn socket_nonce_gate_and_subscription_replay_are_real() {
    use ironbark_core::ipc::{serve, write_json};
    let (dir, path) = temporary_store();
    let store = Store::open(&path).await.unwrap();
    let socket = dir.path().join("core.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    // serve now also owns the run-less service dispatcher; this C1 test only needs a valid
    // RuntimePaths (the dispatcher is constructed but not exercised here).
    let paths = ironbark_core::paths::RuntimePaths { data_dir: dir.path().join("data"), state_dir: dir.path().join("state"), runtime_dir: dir.path().join("runtime"), database: path.clone(), socket: socket.clone(), nonce: dir.path().join("nonce"), lock: dir.path().join("lock") };
    let running = tokio::spawn(serve(listener, "boundary-nonce".into(), store.clone(), paths));
    let (read, mut write) = tokio::net::UnixStream::connect(&socket).await.unwrap().into_split();
    let mut reader = tokio::io::BufReader::new(read);
    write_json(&mut write, &ClientFrame::Hello { protocol: 1, session_nonce: "incorrect".into() }).await.unwrap();
    assert!(matches!(socket_frame(&mut reader).await, ServerFrame::Error { code, .. } if code == "bad_nonce"));
    assert!(store.projects().await.unwrap().is_empty());
    drop(write);
    for _ in 0..300 {
        store.append(None, CoreEvent::StateChanged { entity_id: "test".into(), state: "observed".into() }).await.unwrap();
    }
    let (read, mut write) = tokio::net::UnixStream::connect(&socket).await.unwrap().into_split();
    let mut reader = tokio::io::BufReader::new(read);
    write_json(&mut write, &ClientFrame::Hello { protocol: 1, session_nonce: "boundary-nonce".into() }).await.unwrap();
    assert!(matches!(socket_frame(&mut reader).await, ServerFrame::Hello { protocol: 1 }));
    write_json(&mut write, &ClientFrame::Subscribe { after_seq: 0 }).await.unwrap();
    for expected in 1..=300 {
        assert!(matches!(socket_frame(&mut reader).await, ServerFrame::Event { event } if event.seq == expected));
    }
    store.append(None, CoreEvent::StateChanged { entity_id: "test".into(), state: "live".into() }).await.unwrap();
    assert!(matches!(socket_frame(&mut reader).await, ServerFrame::Event { event } if event.seq == 301));
    drop(reader); drop(write);
    let (read, mut write) = tokio::net::UnixStream::connect(&socket).await.unwrap().into_split();
    let mut reader = tokio::io::BufReader::new(read);
    write_json(&mut write, &ClientFrame::Hello { protocol: 1, session_nonce: "boundary-nonce".into() }).await.unwrap();
    assert!(matches!(socket_frame(&mut reader).await, ServerFrame::Hello { protocol: 1 }));
    write_json(&mut write, &ClientFrame::Request { request_id: Uuid::new_v4(), command: CoreCommand::Shutdown {} }).await.unwrap();
    assert!(matches!(socket_frame(&mut reader).await, ServerFrame::Reply { reply: CoreReply::Ok { .. }, .. }));
    tokio::time::timeout(std::time::Duration::from_secs(5), running).await.unwrap().unwrap().unwrap();
}

// An event page must fit a reply while subsequent cursors retain every event.
#[tokio::test]
async fn byte_bounded_pages_do_not_drop_events() {
    let (_dir, path) = temporary_store();
    let store = Store::open(&path).await.unwrap();
    for _ in 0..3 {
        store.append(None, CoreEvent::TextDelta { message_id: MessageId::new(), delta: "x".repeat(400_000) }).await.unwrap();
    }
    let first = store.events_page(0, 256).await.unwrap();
    assert!(serde_json::to_vec(&first).unwrap().len() < ironbark_core::ipc::MAX_FRAME);
    assert_eq!(first.len(), 2);
    let next = store.events_page(first.last().unwrap().seq, 256).await.unwrap();
    assert_eq!(next.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3]);
    assert_eq!(store.events_after(0).await.unwrap().iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2, 3]);
}

// The unique request transaction, not a process-local mutex, gates concurrency.
#[tokio::test]
async fn concurrent_acceptance_has_one_dispatch_owner() {
    let (_dir, path) = temporary_store();
    let store = Store::open(&path).await.unwrap();
    let id = Uuid::new_v4();
    let command = CoreCommand::ProjectCreate { name: "once".into(), root: "/tmp".into() };
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let store = store.clone(); let command = command.clone();
        tasks.spawn(async move { store.accept_request(id, &command).await.unwrap() });
    }
    let mut owners = 0;
    while let Some(result) = tasks.join_next().await {
        match result.unwrap() {
            RequestDisposition::New => owners += 1,
            RequestDisposition::InFlight => {},
            RequestDisposition::Existing(_) => panic!("no request has executed yet"),
        }
    }
    assert_eq!(owners, 1);
    store.execute_accepted(id, &command).await.unwrap();
    assert_eq!(store.projects().await.unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_command_replies_are_durably_deduplicated() {
    let (_dir, path) = temporary_store();
    let store = Store::open(&path).await.unwrap();
    let id = Uuid::new_v4();
    let command = CoreCommand::Events { after: u64::MAX };
    assert_eq!(store.accept_request(id, &command).await.unwrap(), RequestDisposition::New);
    let reply = store.execute_accepted(id, &command).await.unwrap();
    assert!(matches!(&reply, CoreReply::Error { code, .. } if code == "invalid_cursor"));
    assert_eq!(store.accept_request(id, &command).await.unwrap(), RequestDisposition::Existing(reply));
}

// An actual SQLite trigger abort proves reply/events/project commit atomically.
#[tokio::test]
async fn failed_event_commit_rolls_back_project_and_reply() {
    let (_dir, path) = temporary_store();
    let store = Store::open(&path).await.unwrap();
    store.with_connection(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_events BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT,'injected persistence failure'); END;")?;
        Ok(())
    }).await.unwrap();
    let id = Uuid::new_v4();
    let command = CoreCommand::ProjectCreate { name: "atomic".into(), root: "/tmp".into() };
    assert_eq!(store.accept_request(id, &command).await.unwrap(), RequestDisposition::New);
    assert!(store.execute_accepted(id, &command).await.is_err());
    assert!(store.projects().await.unwrap().is_empty());
    assert!(store.events_after(0).await.unwrap().is_empty());
    assert_eq!(store.accept_request(id, &command).await.unwrap(), RequestDisposition::InFlight);
}
