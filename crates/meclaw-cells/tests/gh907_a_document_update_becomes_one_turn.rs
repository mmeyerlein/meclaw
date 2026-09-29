//! GH #907 -- the Telegram connector reads a document: `message.document` is
//! parsed (with or without caption, with or without `file_name`), its bytes
//! are fetched through `getFile` + download under the client's
//! `max_document_bytes` (default 20 MiB), committed to the colony blob store,
//! and the handler emits ONE turn that carries the caption and one
//! `attachments[]` blob reference with `hop.has_file = "1"` -- never the
//! bytes (R-FJ-1). Every
//! other update (photo, voice, sticker, edit) moves the cursor and emits
//! nothing -- before, it was never acknowledged and came back after every
//! restart.
//!
//! The road through a member into its file space is
//! `gh907_a_pdf_from_telegram_is_read_by_its_address.rs`.

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::db::{load_offset, setup_proxy_schema};
use meclaw_cells::proxy::emit::{build_document_turn_content, document_failure, store_document};
use meclaw_cells::proxy::io::{DocumentContent, ProxyEvent, ProxyReconfig, RunIoConfig, run_io};
use meclaw_cells::proxy::params::ProxyParams;
use meclaw_cells::proxy::telegram::{DEFAULT_MAX_DOCUMENT_BYTES, TelegramClient, parse_update};
use meclaw_colony::{DbConn, DiskBlobStore, LongRunningCell};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, OriginSink, Path};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn document_update(id: i64, doc: Value, caption: Option<&str>) -> Value {
    let mut m = json!({"message_id": 5, "chat": {"id": 100}, "from": {"id": 200},
                       "document": doc});
    if let Some(c) = caption {
        m["caption"] = json!(c);
    }
    json!({"update_id": id, "message": m})
}

#[test]
fn a_document_update_is_parsed_with_and_without_caption_and_name() {
    let ev = parse_update(&document_update(
        7,
        json!({"file_id": "F1", "file_name": "a.pdf", "mime_type": "application/pdf",
               "file_size": 1234}),
        Some("look at this"),
    ))
    .expect("an event");
    match ev {
        ProxyEvent::Document {
            update_id,
            chat_id,
            user_id,
            message_id,
            caption,
            file_id,
            name,
            mime,
            size,
            content,
        } => {
            assert_eq!(
                (update_id, chat_id, user_id, message_id),
                (7, 100, Some(200), Some(5))
            );
            assert_eq!(caption, "look at this");
            assert_eq!((file_id.as_str(), name.as_str()), ("F1", "a.pdf"));
            assert_eq!((mime.as_str(), size), ("application/pdf", Some(1234)));
            assert_eq!(content, DocumentContent::NotFetched);
        }
        other => panic!("expected a Document, got {other:?}"),
    }
    // Without caption and without file_name: an empty caption, a name made
    // from the mime type; without mime type, the octet-stream default.
    let ev = parse_update(&document_update(
        8,
        json!({"file_id": "F2", "mime_type": "application/pdf"}),
        None,
    ))
    .unwrap();
    let ProxyEvent::Document { caption, name, .. } = ev else {
        panic!("a Document");
    };
    assert_eq!((caption.as_str(), name.as_str()), ("", "document.pdf"));
    let ev = parse_update(&document_update(9, json!({"file_id": "F3"}), None)).unwrap();
    let ProxyEvent::Document { name, mime, .. } = ev else {
        panic!("a Document");
    };
    assert_eq!(
        (name.as_str(), mime.as_str()),
        ("document", "application/octet-stream")
    );
}

#[test]
fn every_other_update_is_skipped_by_its_id() {
    for (id, u) in [
        (
            1,
            json!({"update_id": 1, "message": {"chat": {"id": 1}, "photo": [{"file_id": "p"}]}}),
        ),
        (
            2,
            json!({"update_id": 2, "message": {"chat": {"id": 1}, "voice": {"file_id": "v"}}}),
        ),
        (
            3,
            json!({"update_id": 3, "message": {"chat": {"id": 1}, "sticker": {"file_id": "s"}}}),
        ),
        (
            4,
            json!({"update_id": 4, "edited_message": {"chat": {"id": 1}, "text": "x"}}),
        ),
        (5, json!({"update_id": 5, "callback_query": {"id": "q"}})),
    ] {
        match parse_update(&u) {
            Some(ProxyEvent::Skipped { update_id }) => assert_eq!(update_id, id),
            other => panic!("update {id}: expected Skipped, got {other:?}"),
        }
    }
    assert!(
        parse_update(&json!({"message": {}})).is_none(),
        "no id, nothing to move past"
    );
}

#[test]
fn the_ceiling_defaults_to_twenty_mib_and_refuses_nonsense() {
    let base = json!({"bot_token": "T", "emit_to": "/x"});
    let p = ProxyParams::parse(&base).unwrap();
    assert_eq!(p.max_document_bytes, DEFAULT_MAX_DOCUMENT_BYTES);
    // The Bot API's own `getFile` limit: the turn carries a blob reference,
    // so the size no longer multiplies through hop and context (R-FJ-1).
    assert_eq!(DEFAULT_MAX_DOCUMENT_BYTES, 20 * 1024 * 1024);
    let mut v = base.clone();
    v["max_document_bytes"] = json!(1000);
    assert_eq!(ProxyParams::parse(&v).unwrap().max_document_bytes, 1000);
    for bad in [json!(0), json!(-5), json!("big")] {
        v["max_document_bytes"] = bad.clone();
        assert!(ProxyParams::parse(&v).is_err(), "{bad} must be refused");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_document_is_fetched_through_get_file_and_the_file_path() {
    let (addr, _j, cap) = start_mock_server_capturing(vec![
        MockResponse::ok_json(
            br#"{"ok":true,"result":{"file_id":"F1","file_size":5,"file_path":"documents/a.pdf"}}"#,
        ),
        MockResponse::ok(b"%PDF-"),
    ])
    .await;
    let c = TelegramClient::new(&format!("http://{addr}"), "TOKEN").unwrap();
    let got = c
        .fetch_document("F1", Some(5), Duration::from_secs(5))
        .await;
    assert_eq!(got, DocumentContent::Bytes(b"%PDF-".to_vec()));
    let cap = cap.lock().await;
    assert_eq!(cap.len(), 2);
    assert!(
        cap[0].path.starts_with("/botTOKEN/getFile?file_id=F1"),
        "{}",
        cap[0].path
    );
    assert_eq!(cap[1].path, "/file/botTOKEN/documents/a.pdf");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_document_over_the_ceiling_is_never_downloaded_whole() {
    // Announced too large: no request at all.
    let (addr, _j, cap) = start_mock_server_capturing(vec![MockResponse::ok(b"x")]).await;
    let c = TelegramClient::new(&format!("http://{addr}"), "T")
        .unwrap()
        .with_max_document_bytes(4);
    assert_eq!(
        c.fetch_document("F", Some(5), Duration::from_secs(5)).await,
        DocumentContent::TooLarge
    );
    assert!(
        cap.lock().await.is_empty(),
        "no request for an announced oversize"
    );

    // Announced small by the update, large by getFile: no download.
    let (addr, _j, cap) = start_mock_server_capturing(vec![MockResponse::ok_json(
        br#"{"ok":true,"result":{"file_size":9,"file_path":"d/x"}}"#,
    )])
    .await;
    let c = TelegramClient::new(&format!("http://{addr}"), "T")
        .unwrap()
        .with_max_document_bytes(4);
    assert_eq!(
        c.fetch_document("F", None, Duration::from_secs(5)).await,
        DocumentContent::TooLarge
    );
    assert_eq!(cap.lock().await.len(), 1, "getFile only");

    // Announced nowhere, grown past the ceiling while downloading.
    let (addr, _j, _cap) = start_mock_server_capturing(vec![
        MockResponse::ok_json(br#"{"ok":true,"result":{"file_path":"d/x"}}"#),
        MockResponse::ok(b"0123456789"),
    ])
    .await;
    let c = TelegramClient::new(&format!("http://{addr}"), "T")
        .unwrap()
        .with_max_document_bytes(4);
    assert_eq!(
        c.fetch_document("F", None, Duration::from_secs(5)).await,
        DocumentContent::TooLarge
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_fetch_names_a_code_and_never_the_token() {
    let (addr, _j, _cap) = start_mock_server_capturing(vec![MockResponse::not_found()]).await;
    let c = TelegramClient::new(&format!("http://{addr}"), "SECRET").unwrap();
    let got = c.fetch_document("F", None, Duration::from_secs(5)).await;
    assert_eq!(got, DocumentContent::Failed("get_file_failed".into()));

    let (addr, _j, _cap) = start_mock_server_capturing(vec![
        MockResponse::ok_json(br#"{"ok":true,"result":{"file_path":"d/x"}}"#),
        MockResponse::server_error(),
    ])
    .await;
    let c = TelegramClient::new(&format!("http://{addr}"), "SECRET").unwrap();
    let got = c.fetch_document("F", None, Duration::from_secs(5)).await;
    assert_eq!(got, DocumentContent::Failed("download_failed".into()));

    // A download that hangs past the step timeout.
    let (addr, _j, _cap) = start_mock_server_capturing(vec![
        MockResponse::ok_json(br#"{"ok":true,"result":{"file_path":"d/x"}}"#),
        MockResponse::ok(b"late").with_delay(Duration::from_secs(5)),
    ])
    .await;
    let c = TelegramClient::new(&format!("http://{addr}"), "SECRET").unwrap();
    let got = c
        .fetch_document("F", None, Duration::from_millis(300))
        .await;
    assert_eq!(got, DocumentContent::Failed("timeout".into()));

    // The sentence a turn carries holds the code, not the URL.
    let why = document_failure(&got).expect("no bytes");
    let turn = build_document_turn_content(1, None, None, "", "a.pdf", Err(&why));
    assert!(!turn.to_string().contains("SECRET"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fetched_document_is_one_turn_with_the_caption_and_a_blob_reference() {
    let dir = tempfile::tempdir().unwrap();
    let store = DiskBlobStore::new(dir.path()).unwrap();
    let bytes = b"%PDF-1".to_vec();
    let blob = store_document(
        Some(&store),
        bytes.clone(),
        "application/pdf",
        "a.pdf",
        Duration::from_secs(5),
    )
    .await
    .expect("stored");
    let v =
        build_document_turn_content(100, Some(200), Some(5), "what is this?", "a.pdf", Ok(&blob));
    assert_eq!(v["header"]["has_file"], "1");
    assert_eq!(v["header"]["chat_id"], 100);
    assert_eq!(v["header"]["platform"], "telegram");
    // One caption turn, no file element.
    assert_eq!(
        v["messages"],
        json!([{"origin": "user", "type": "text", "text": "what is this?"}])
    );
    let att = v["attachments"].as_array().expect("an attachments slot");
    assert_eq!(att.len(), 1);
    let e = &att[0];
    assert_eq!(e["blob_id"], blob.blob_id.to_string());
    assert_eq!(e["mime_type"], "application/pdf");
    assert_eq!(e["filename"], "a.pdf");
    assert_eq!(e["size_bytes"], 6);
    assert_eq!(e["sha256"], sha256_hex(&bytes));
    let text = v.to_string();
    assert!(!text.contains("data_b64"), "no bytes in the turn: {text}");
    assert!(!text.contains("JVBERi0x"), "no base64 of the bytes: {text}");
    // The blob holds exactly the bytes.
    let id: meclaw_core::Uuid = e["blob_id"].as_str().unwrap().parse().unwrap();
    let (got, sidecar) = store.read_bytes(id).await.expect("the blob");
    assert_eq!(got, bytes);
    assert_eq!(sidecar.mime_type, "application/pdf");
}

/// AGENTS.md rule 13 (review I-FR2 m-2, OR-FJ-82): the SHA-256 over a
/// document of up to `max_document_bytes` (20 MiB) is CPU work. Run inline in
/// `store_document` it holds the one worker of a current-thread runtime for as
/// long as the hash takes -- every timer and every other task on it waits.
/// Measured against the same hash run inline on this thread: while the proxy
/// stores the document, the longest silence of a 1 ms ticker stays under half
/// of that.
#[tokio::test(flavor = "current_thread")]
async fn hashing_a_twenty_mib_document_leaves_the_worker_free() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;
    let dir = tempfile::tempdir().unwrap();
    let store = DiskBlobStore::new(dir.path()).unwrap();
    let len = u32::try_from(DEFAULT_MAX_DOCUMENT_BYTES).unwrap();
    let bytes: Vec<u8> = (0..len)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let started = Instant::now();
    let want = sha256_hex(&bytes);
    let inline = started.elapsed();

    let stop = Arc::new(AtomicBool::new(false));
    let ticker = {
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut last = Instant::now();
            let mut worst = Duration::ZERO;
            while !stop.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(1)).await;
                let now = Instant::now();
                worst = worst.max(now - last);
                last = now;
            }
            worst
        })
    };
    tokio::task::yield_now().await;
    let blob = store_document(
        Some(&store),
        bytes.clone(),
        "application/pdf",
        "big.pdf",
        Duration::from_secs(120),
    )
    .await
    .expect("stored");
    stop.store(true, Ordering::Relaxed);
    let worst = ticker.await.unwrap();
    assert_eq!(blob.sha256.as_deref(), Some(want.as_str()));
    assert!(
        worst < inline / 2,
        "the worker went silent for {worst:?} while the document was stored; the hash \
         alone takes {inline:?} inline"
    );
}

#[test]
fn a_document_without_bytes_still_reaches_the_turn_as_a_sentence() {
    let why = document_failure(&DocumentContent::TooLarge).unwrap();
    let v = build_document_turn_content(1, None, None, "see", "big.pdf", Err(&why));
    assert!(
        v["header"].get("has_file").is_none(),
        "no bytes, no file route"
    );
    assert!(v.get("attachments").is_none(), "no bytes, no reference");
    let msgs = v["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0]["text"],
        "see\n[file \"big.pdf\" could not be stored: document too large]"
    );
    let why = document_failure(&DocumentContent::Failed("download_failed".into())).unwrap();
    let v = build_document_turn_content(1, None, None, "", "x.bin", Err(&why));
    assert_eq!(
        v["messages"][0]["text"],
        "[file \"x.bin\" could not be stored: download_failed]"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_io_loop_moves_past_a_photo_and_fetches_a_document() {
    let r1 = MockResponse::ok_json(
        br#"{"ok":true,"result":[
        {"update_id":10,"message":{"chat":{"id":1},"photo":[{"file_id":"p"}]}},
        {"update_id":11,"message":{"chat":{"id":1},"document":{"file_id":"D","file_name":"n.txt","mime_type":"text/plain","file_size":3}}}
    ]}"#,
    );
    let r2 = MockResponse::ok_json(br#"{"ok":true,"result":{"file_path":"attachments/n.txt"}}"#);
    let r3 = MockResponse::ok(b"abc");
    let r4 = MockResponse::ok_json(br#"{"ok":true,"result":[]}"#);
    let (addr, _j, cap) = start_mock_server_capturing(vec![r1, r2, r3, r4]).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let (events_tx, mut events_rx) = mpsc::channel::<ProxyEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(8);
    let cfg = RunIoConfig {
        client,
        initial_offset: 0,
        long_poll_request_secs: 1,
        long_poll_timeout_ms: 2000,
        liveness: meclaw_colony::IoLivenessMark::disabled(),
    };
    let join = tokio::spawn(run_io(cfg, events_tx, rc_rx));
    let first = tokio::time::timeout(Duration::from_secs(30), events_rx.recv())
        .await
        .expect("an event")
        .unwrap();
    assert!(
        matches!(first, ProxyEvent::Skipped { update_id: 10 }),
        "{first:?}"
    );
    let second = tokio::time::timeout(Duration::from_secs(30), events_rx.recv())
        .await
        .expect("an event")
        .unwrap();
    match second {
        ProxyEvent::Document {
            update_id, content, ..
        } => {
            assert_eq!(update_id, 11);
            assert_eq!(content, DocumentContent::Bytes(b"abc".to_vec()));
        }
        other => panic!("expected the fetched Document, got {other:?}"),
    }
    // The next poll asks past both updates.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if cap.lock().await.len() >= 4 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no fourth request");
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        cap.lock().await[3].path.contains("offset=12"),
        "the offset moved past both"
    );
    drop(rc_tx);
    let _ = tokio::time::timeout(Duration::from_secs(30), join).await;
}

fn cell() -> ProxyCell {
    ProxyCell::new(
        TelegramClient::new("http://127.0.0.1:9", "T").unwrap(),
        Path::new("/dst"),
        0,
        35000,
        30,
        10000,
        5000,
        "http://127.0.0.1:9".into(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_skipped_update_is_persisted_past_and_emits_nothing() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = cell();
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/p"), 64);
    cell.handle_event(ProxyEvent::Skipped { update_id: 41 }, &sink, &mut db)
        .await;
    assert_eq!(db.call(|c| load_offset(c)).await.unwrap(), 42);
    assert!(
        origin_rx.try_recv().is_err(),
        "a skipped update emits nothing"
    );
}

fn document_event() -> ProxyEvent {
    ProxyEvent::Document {
        update_id: 50,
        chat_id: 100,
        user_id: None,
        message_id: None,
        caption: "".into(),
        file_id: "F".into(),
        name: "n.txt".into(),
        mime: "text/plain".into(),
        size: Some(3),
        content: DocumentContent::Bytes(b"abc".to_vec()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_document_event_is_stored_persisted_and_emitted_as_one_turn() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DiskBlobStore::new(dir.path()).unwrap());
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = cell().with_blob_store(Some(Arc::clone(&store)));
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/p"), 64);
    cell.handle_event(document_event(), &sink, &mut db).await;
    assert_eq!(db.call(|c| load_offset(c)).await.unwrap(), 51);
    let em = origin_rx.recv().await.expect("one emission");
    assert_eq!(em.target.as_str(), "/dst");
    assert_eq!(em.content["header"]["has_file"], "1");
    let e = &em.content["attachments"][0];
    assert_eq!(e["filename"], "n.txt");
    assert_eq!(e["mime_type"], "text/plain");
    assert_eq!(e["size_bytes"], 3);
    assert_eq!(e["sha256"], sha256_hex(b"abc"));
    assert_eq!(em.content["messages"].as_array().unwrap().len(), 1);
    assert!(!em.content.to_string().contains("data_b64"));
    let id: meclaw_core::Uuid = e["blob_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(store.read_bytes(id).await.unwrap().0, b"abc");
    assert!(origin_rx.try_recv().is_err(), "exactly one turn");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_document_event_without_a_blob_store_is_the_fallback_line() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = cell();
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/p"), 64);
    cell.handle_event(document_event(), &sink, &mut db).await;
    assert_eq!(db.call(|c| load_offset(c)).await.unwrap(), 51);
    let em = origin_rx.recv().await.expect("one emission");
    assert!(em.content["header"].get("has_file").is_none());
    assert!(em.content.get("attachments").is_none());
    assert_eq!(
        em.content["messages"],
        json!([{"origin": "user", "type": "text",
                "text": "[file \"n.txt\" could not be stored: no_blob_store]"}])
    );
    assert!(origin_rx.try_recv().is_err(), "exactly one turn");
}
