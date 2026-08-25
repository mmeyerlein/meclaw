//! W8 Task 8 (GH #380): components are data, and the `pages` table is the router.
//!
//! Two claims, and the second is the one the spec called a risk.
//!
//! **A component can be defined at runtime, by message.** That is what makes
//! components data rather than code — a model can grow the vocabulary of a
//! display without a release. The template is parsed *at definition*, so an
//! unknown form is answered to whoever wrote it.
//!
//! **`cell.surface` dies with `/surface/*`, and its replacement must not be a
//! second grammar for the same thing** (spec risk 2). It is discharged by
//! construction here: the `pages` table is the only route source, and there is
//! no code path in the `web` cell that reads a surface declaration. The route
//! grammar refuses the two names that could shadow the cell's own paths.

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, Path};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

/// A cell with one component and one page, so there is a starting point.
fn seed(cell_dir: &std::path::Path) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("seed dir");
    std::fs::write(
        seed.join("components.jsonl"),
        concat!(
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
            "\n",
            r#"{"name":"stack","template":"<main>{{children}}</main>","prop_schema":"{}","editable":"[]","layer":"content"}"#,
            "\n"
        ),
    )
    .expect("components");
    std::fs::write(
        seed.join("objects.jsonl"),
        concat!(
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
            "\n",
            r#"{"id":"root","parent":null,"component":"stack","ord":0,"props":"{}"}"#,
            "\n"
        ),
    )
    .expect("objects");
    std::fs::write(
        seed.join("pages.jsonl"),
        concat!(
            r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
            "\n",
            r#"{"route":"/","root":"root","title":"Home"}"#,
            "\n"
        ),
    )
    .expect("pages");
}

struct Live {
    port: u16,
    mailbox: mpsc::Sender<meclaw_core::Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

async fn start(cell_dir: &std::path::Path) -> Live {
    let port = free_port();
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory)
        .spawn_cell(
            Path::new("/web"),
            json!({ "port": port }),
            out_tx,
            cell_dir.to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("spawn");
    let SpawnedCellKind::Active {
        join,
        sender,
        stop_tx,
        ..
    } = spawned
    else {
        panic!("Active");
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(r) = reqwest::get(format!("http://127.0.0.1:{port}/")).await
            && r.status().is_success()
        {
            break;
        }
        assert!(Instant::now() < deadline, "the cell never served its page");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Live {
        port,
        mailbox: sender,
        out_rx,
        _stop: stop_tx,
        join,
    }
}

async fn call(live: &mut Live, args: Value) -> Value {
    let msg = MessageBuilder::new(Path::new("/web"))
        .body(Body::Inline(json!({
            "messages": [{"origin":"tool","type":"tool_call","text": args.to_string(),"id":"c"}]
        })))
        .reply_to(Path::new("/caller"))
        .build();
    live.mailbox.send(msg).await.expect("mailbox");
    tokio::time::timeout(Duration::from_secs(30), live.out_rx.recv())
        .await
        .expect("the cell must answer")
        .expect("emission")
        .content
}

/// GET a route, waiting for the listener to exist.
///
/// The cell binds its port in the I/O half, which starts when the colony
/// spawns it — so the first request after boot can arrive before there is
/// anything listening, and `ConnectionRefused` is not an answer, it is the
/// absence of one. Under a full parallel workspace run that gap is wide enough
/// to lose the race, which is exactly what it did (W8): green in isolation,
/// red once as a load flake. The connect error is therefore retried against
/// the repo's 30-second failure-marker deadline; a *status* is returned as it
/// comes, because a 404 IS an answer and this suite asserts on it.
async fn get(port: u16, route: &str) -> (u16, String) {
    let url = format!("http://127.0.0.1:{port}{route}");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match reqwest::get(&url).await {
            Ok(r) => {
                let status = r.status().as_u16();
                return (status, r.text().await.expect("text"));
            }
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    panic!("{url} never accepted a connection: {e}");
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_component_defined_by_message_can_be_used_immediately() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let r = call(
        &mut live,
        json!({
            "op": "component.define",
            "name": "banner",
            "template": "<h1 class=\"b\">{{title}}</h1>",
            "prop_schema": {"title": "text"}
        }),
    )
    .await;
    assert_eq!(r["header"]["operation"], json!("component.define"), "{r}");
    assert!(r["header"].get("error_code").is_none(), "{r}");

    let r = call(
        &mut live,
        json!({"op": "object.create", "id": "b1", "parent": "root",
               "component": "banner", "props": {"title": "Hello"}}),
    )
    .await;
    assert!(r["header"].get("error_code").is_none(), "{r}");

    let (status, html) = get(live.port, "/").await;
    assert_eq!(status, 200);
    assert!(
        html.contains("<h1 class=\"b\">Hello</h1>"),
        "the new component rendered: {html}"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_template_form_the_language_does_not_have_is_refused_at_definition() {
    // The one-parser rule: the same parser the renderer uses, run early, so the
    // answer reaches whoever wrote the template.
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let r = call(
        &mut live,
        json!({
            "op": "component.define",
            "name": "loopy",
            "template": "{{#each items}}<li>{{item}}</li>{{/each}}",
            "prop_schema": {"items": "text"}
        }),
    )
    .await;
    assert_eq!(r["header"]["error_code"], json!("invalid_input"), "{r}");
    let text = r["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("not a form"),
        "the refusal says what the language has: {text}"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_page_set_by_message_is_served_at_its_route() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    // Before: nothing declares it.
    assert_eq!(get(live.port, "/about").await.0, 404);

    call(
        &mut live,
        json!({"op": "component.define", "name": "para",
               "template": "<p>{{body}}</p>", "prop_schema": {"body": "text"}}),
    )
    .await;
    call(
        &mut live,
        json!({"op": "object.create", "id": "about-root", "component": "para",
               "props": {"body": "about us"}}),
    )
    .await;
    let r = call(
        &mut live,
        json!({"op": "page.set", "route": "/about", "root": "about-root", "title": "About"}),
    )
    .await;
    assert!(r["header"].get("error_code").is_none(), "{r}");

    let (status, html) = get(live.port, "/about").await;
    assert_eq!(status, 200, "the new route is served");
    assert!(html.contains("about us"), "{html}");
    assert!(html.contains("<title>About</title>"), "{html}");

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_reserved_routes_cannot_be_shadowed() {
    // `live` is the transport's and `@…` is the cell's own files'. A page at
    // either would shadow something the cell needs to keep serving.
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    for bad in [
        "/live",
        "/live/websocket",
        "/@client",
        "/Upper",
        "/a/",
        "no-slash",
        "/a b",
    ] {
        let r = call(
            &mut live,
            json!({"op": "page.set", "route": bad, "root": "root"}),
        )
        .await;
        assert_eq!(
            r["header"]["error_code"],
            json!("invalid_input"),
            "route {bad:?} must be refused, got {r}"
        );
    }

    // And the transport still answers, which is what the refusal protects.
    let (status, _) = get(live.port, "/live/websocket").await;
    assert_eq!(
        status, 400,
        "the websocket path is still the websocket path (400 = right path, wrong request)"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_page_pointing_at_no_object_is_refused() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let r = call(
        &mut live,
        json!({"op": "page.set", "route": "/ghost", "root": "nobody"}),
    )
    .await;
    assert_eq!(r["header"]["error_code"], json!("unknown_object"), "{r}");
    assert_eq!(get(live.port, "/ghost").await.0, 404);

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redefining_a_component_redraws_the_pages_using_it() {
    // A component definition changes how every object using it renders. The
    // cell does not track which objects those are, so it re-materialises every
    // route — a page that quietly kept drawing the old template would be the
    // worse outcome.
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    call(
        &mut live,
        json!({"op": "component.define", "name": "tag",
               "template": "<span>{{label}}</span>", "prop_schema": {"label": "text"}}),
    )
    .await;
    call(
        &mut live,
        json!({"op": "object.create", "id": "t1", "parent": "root",
               "component": "tag", "props": {"label": "x"}}),
    )
    .await;
    assert!(get(live.port, "/").await.1.contains("<span>x</span>"));

    // Same component, different markup.
    call(
        &mut live,
        json!({"op": "component.define", "name": "tag",
               "template": "<b data-tag>{{label}}</b>", "prop_schema": {"label": "text"}}),
    )
    .await;

    let (_, html) = get(live.port, "/").await;
    assert!(
        html.contains("<b data-tag>x</b>"),
        "the redefinition took effect without touching the object: {html}"
    );
    assert!(!html.contains("<span>x</span>"), "{html}");

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn editable_must_name_props_the_component_declares() {
    // An `editable` prop that is not declared could never be written anyway —
    // the write would be refused as undeclared — so naming one is a mistake
    // worth reporting rather than a harmless extra.
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let r = call(
        &mut live,
        json!({"op": "component.define", "name": "node",
               "template": "<i>{{label}}</i>", "prop_schema": {"label": "text"},
               "editable": ["x", "y"]}),
    )
    .await;
    assert_eq!(r["header"]["error_code"], json!("invalid_input"), "{r}");
    let text = r["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains('x'), "the refusal names the offender: {text}");

    live.join.abort();
}
