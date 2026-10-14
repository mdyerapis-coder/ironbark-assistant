//! P3 invariants: source revisions are immutable and content-addressed.
//! - Re-ingesting identical bytes is idempotent (returns the existing revision).
//! - New bytes publish a NEW revision; an old citation still resolves to its
//!   exact historic text (never a silent current-revision substitution).
//! - A failed extraction publishes no revision and no chunk (no partial publish).
//! - An encrypted PDF is refused, not indexed as a blank success.

use ironbark_core::contracts::{
    Citation, CitationLocator, CitationView, DisclosurePolicy, ProjectId, SourceInput,
};
use ironbark_core::error::CoreError;
use ironbark_core::knowledge::{locators, KnowledgeService};
use ironbark_core::store::Store;
use tempfile::tempdir;

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    project: ProjectId,
    service: KnowledgeService,
}

async fn setup() -> Fixture {
    let dir = tempdir().unwrap();
    let store = Store::open(&dir.path().join("core.db")).await.unwrap();
    let project = ProjectId::new();
    let root = dir.path().to_string_lossy().into_owned();
    store
        .with_transaction(move |tx| {
            tx.execute(
                "INSERT INTO projects (id,name,root,created_at) VALUES (?1,'p',?2,0)",
                rusqlite::params![project.to_string(), root],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let service = KnowledgeService::new(store.clone(), dir.path().join("objects"));
    Fixture { _dir: dir, store, project, service }
}

/// The locator of the first real content unit published for a revision.
async fn first_locator(store: &Store, source_id: &str, digest: &str) -> CitationLocator {
    let source = source_id.to_string();
    let digest = digest.to_string();
    let raw: String = store
        .with_connection(move |conn| {
            conn.query_row(
                "SELECT c.locator FROM chunks c
                 JOIN source_revisions sr ON sr.id=c.revision_id
                 WHERE sr.source_id=?1 AND sr.digest=?2 AND c.locator != ?3
                 ORDER BY c.rowid ASC LIMIT 1",
                rusqlite::params![source, digest, locators::META_LOCATOR],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .await
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn ingest_idempotent_same_bytes_returns_same_revision() {
    let f = setup().await;
    let file = f._dir.path().join("source.txt");
    tokio::fs::write(&file, b"alpha beta gamma delta epsilon").await.unwrap();
    let input = || SourceInput::File { path: file.to_string_lossy().into_owned() };

    let rev1 = f.service.ingest(f.project, input(), DisclosurePolicy::ExplicitPublic).await.unwrap();
    let rev2 = f.service.ingest(f.project, input(), DisclosurePolicy::ExplicitPublic).await.unwrap();

    assert_eq!(rev1.digest, rev2.digest, "identical bytes must resolve to the same revision");
    assert_eq!(rev1.source_id, rev2.source_id);
    assert_eq!(rev1.locators, rev2.locators);
    let published: i64 = f
        .store
        .with_connection(move |conn| {
            conn.query_row("SELECT count(*) FROM source_revisions", [], |row| row.get(0))
                .map_err(Into::into)
        })
        .await
        .unwrap();
    assert_eq!(published, 1, "re-ingesting identical bytes must not publish a second revision");
}

#[tokio::test]
async fn new_bytes_publish_new_revision_and_old_citation_stays_exact() {
    let f = setup().await;
    let file = f._dir.path().join("source.txt");
    tokio::fs::write(&file, b"first version").await.unwrap();
    let input = || SourceInput::File { path: file.to_string_lossy().into_owned() };

    let rev1 = f.service.ingest(f.project, input(), DisclosurePolicy::ExplicitPublic).await.unwrap();
    let citation = Citation {
        source_id: rev1.source_id.clone(),
        revision_digest: rev1.digest.clone(),
        locator: first_locator(&f.store, &rev1.source_id, rev1.digest.as_str()).await,
    };

    tokio::fs::write(&file, b"second version with different content").await.unwrap();
    let rev2 = f.service.ingest(f.project, input(), DisclosurePolicy::ExplicitPublic).await.unwrap();
    assert_ne!(rev1.digest, rev2.digest, "new bytes must publish a new revision digest");
    assert_eq!(rev1.source_id, rev2.source_id, "same uri is the same source");

    let view = f.service.open_citation_in(f.project, &citation).await.unwrap();
    match view {
        CitationView::Content { text } => {
            assert!(text.contains("first version"), "old citation resolves to its exact historic text, got {text:?}")
        }
        other => panic!("old citation must resolve to Content, got {other:?}"),
    }
    // A foreign project cannot open another project's citation.
    let stranger = ProjectId::new();
    assert_eq!(
        f.service.open_citation_in(stranger, &citation).await.unwrap(),
        CitationView::Missing,
        "a citation from another project must not resolve here"
    );
}

#[tokio::test]
async fn failed_extraction_publishes_no_partial_revision() {
    let f = setup().await;
    let empty = f._dir.path().join("empty.txt");
    tokio::fs::write(&empty, b"").await.unwrap();

    let err = f
        .service
        .ingest(
            f.project,
            SourceInput::File { path: empty.to_string_lossy().into_owned() },
            DisclosurePolicy::ExplicitPublic,
        )
        .await
        .expect_err("an empty source cannot validate");
    assert_eq!(err.code, "extraction_empty", "the failure names the real validation reason");

    let (revisions, chunks): (i64, i64) = f
        .store
        .with_connection(move |conn| {
            Ok((
                conn.query_row("SELECT count(*) FROM source_revisions", [], |r| r.get(0))?,
                conn.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(revisions, 0, "a failed extraction publishes no revision");
    assert_eq!(chunks, 0, "a failed extraction publishes no chunk");
}

#[tokio::test]
async fn encrypted_pdf_is_not_indexed_as_blank_success() {
    let f = setup().await;
    let pdf = f._dir.path().join("secret.pdf");
    // A %PDF header (so the sniff path engages) whose trailer names /Encrypt.
    let mut body = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
    body.extend_from_slice(b"trailer\n<< /Encrypt 7 0 R >>\n%%EOF\n");
    tokio::fs::write(&pdf, &body).await.unwrap();

    let err = f
        .service
        .ingest(
            f.project,
            SourceInput::File { path: pdf.to_string_lossy().into_owned() },
            DisclosurePolicy::ExplicitPublic,
        )
        .await
        .expect_err("an encrypted PDF must be refused");
    assert_eq!(err.code, "source_encrypted");

    let revisions: i64 = f
        .store
        .with_connection(move |conn| {
            conn.query_row("SELECT count(*) FROM source_revisions", [], |r| r.get(0))
                .map_err(Into::into)
        })
        .await
        .unwrap();
    assert_eq!(revisions, 0, "an encrypted source must not be published");
}

#[tokio::test]
async fn local_only_source_cannot_be_fetched_over_the_network() {
    let f = setup().await;
    let err = f
        .service
        .ingest(
            f.project,
            SourceInput::Fetched {
                url: "https://example.invalid/doc".into(),
                receipt: ironbark_core::contracts::Digest::parse(&"a".repeat(64)).unwrap(),
            },
            DisclosurePolicy::LocalOnly,
        )
        .await
        .expect_err("LocalOnly content must never ride a network fetch");
    assert_eq!(err.code, "disclosure_conflict");
}

#[test]
fn extraction_error_codes_are_typed() {
    // Guards the error contract the ingest path depends on.
    let err = CoreError::new("extraction_empty", "no units");
    assert_eq!(format!("{err}"), "extraction_empty: no units");
}
