//! GH #1001 — a write reaches the socket as the part it changed.
//!
//! Measured on a deployed page (G1): every root child was one HTML string, so a
//! child update inside a container of 128 children sent the whole container,
//! ≈ 20 KB, and a join of a 4 000-figure pool carried every figure's markup in
//! full. These locks run the lab page (`support/web_fixture.rs`) through a real
//! cell and real sockets, and feed what the sockets received to the vendored
//! LiveView client in node (`support/lv_client.mjs`), so the claim "the client
//! builds the served page" is the client's own, not a model of it.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use meclaw_core::serde_json::{Value, json};
use web_fixture::{Lab, ROOT, Shape, kid, update};

/// The vendored client bundle.
fn client_js() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../meclaw-surface/src/client/phoenix_live_view.min.js")
}

/// The driver.
fn driver() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/lv_client.mjs")
}

/// What the client builds from a join and a series of diffs, or `None` when
/// there is no node on this host (the lanes have one, `gate_plan.py`).
fn client_builds(join: &Value, diffs: &[Value]) -> Option<(Vec<String>, String)> {
    let td = tempfile::TempDir::new().expect("tempdir");
    let steps = td.path().join("steps.json");
    std::fs::write(&steps, json!({"join": join, "diffs": diffs}).to_string()).expect("steps");
    let out = std::process::Command::new("node")
        .arg(driver())
        .arg(client_js())
        .arg(&steps)
        .output()
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) {
        println!("{stderr}");
        return None;
    }
    assert!(out.status.success(), "the client driver failed: {stderr}");
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("driver JSON");
    let steps = v["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|s| s.as_str().expect("html").to_string())
        .collect();
    Some((steps, v["full"].as_str().expect("full").to_string()))
}

/// The markup with the client's own bookkeeping attributes taken out.
fn without_client_ids(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(" data-phx-id=\"") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + " data-phx-id=\"".len()..];
        let end = tail.find('"').expect("closing quote");
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The page body a GET serves, cut out of the shell.
fn served_body(page: &str) -> String {
    let open = "data-phx-static=\"\">\n";
    let start = page.find(open).expect("the shell's container") + open.len();
    let end = start
        + page[start..]
            .find("\n</div>\n<script")
            .expect("the container closes before the scripts");
    page[start..end].to_string()
}

/// The answer reports no error.
fn ok(answer: &Value) {
    let text = answer.to_string();
    assert!(
        !text.contains("\"error_code\""),
        "the write was refused: {text}"
    );
}

/// A small page for the client runs: big enough for every kind of part.
fn small() -> Shape {
    Shape {
        figures: 40,
        chunks: 4,
        kids: 16,
    }
}

/// T4. Before (deployed page, G1): ≈ 20 KB per child update in a 128-child chunk.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1001_a_child_update_sends_only_the_child() {
    let mut lab = Lab::start(Shape::default()).await;
    let mut viewer = lab.viewer().await;
    let answer = lab.call(vec![update(&kid(3, 7), json!({"x": 999}))]).await;
    ok(&answer);
    let (bytes, diff) = viewer.next_diff().await;
    println!("LAB child update in a 128-child chunk: {bytes} bytes");
    // The chunk is root child `figures + 3`; its template's dynamics are
    // `c`, `lod`, `children`, so the children part is dynamic 2.
    let slot = (lab.shape.figures + 3).to_string();
    let child = &diff[&slot]["2"]["7"];
    assert!(
        child.is_object(),
        "the diff addresses the child inside its chunk, {{\"{slot}\":{{\"2\":{{\"7\":…}}}}}}: {}",
        &diff.to_string()[..diff.to_string().len().min(300)]
    );
    assert!(
        bytes <= 1024,
        "a child update costs {bytes} bytes at the socket (target ≤ 1 KB)"
    );
}

/// T5. A redefined component is drawn with its new markup — the fallback to a
/// whole render, kept honest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1001_redefining_a_component_rerenders_its_pages() {
    let mut lab = Lab::start(small()).await;
    let mut viewer = lab.viewer().await;
    let answer = lab
        .call(vec![json!({
            "op": "component.define",
            "name": "tree",
            "template": r#"<div class="tree tree--new" data-x="{{x}}" data-y="{{y}}"></div>"#,
            "prop_schema": {"x": "int", "y": "int"},
        })])
        .await;
    ok(&answer);
    let (_, diff) = viewer.next_diff().await;
    assert!(
        diff.to_string().contains("tree tree--new"),
        "the frame carries the new markup"
    );
    let body = served_body(&lab.get_page().await);
    assert!(
        body.contains(r#"<div class="tree tree--new" data-x="16""#),
        "the served page is drawn with the new template"
    );
    if let Some((_, full)) = client_builds(&viewer.rendered, &[diff]) {
        assert_eq!(without_client_ids(&full), body);
    }
}

/// A tiny deterministic generator, so a failing series can be replayed.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n
    }
}

/// T6. Join plus 50 writes of every kind, through the client: the end state
/// is the page a GET serves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1001_the_client_builds_the_served_html_from_join_and_diffs() {
    let shape = small();
    let mut lab = Lab::start(shape).await;
    // A raw-html prop, so the series carries markup in a dynamic (GH #869).
    ok(&lab
        .call(vec![json!({
            "op": "object.create", "id": "note-1", "parent": ROOT,
            "component": "note", "ord": 9999,
            "props": {"body": "<b>hi</b> &amp; <i>there</i>"},
        })])
        .await);
    let mut viewer = lab.viewer().await;
    let mut rng = Lcg(1001);
    let mut diffs = Vec::new();
    let mut created: Vec<String> = Vec::new();
    for step in 0..50 {
        let op = if step == 25 {
            // Mid-series: new statics for a component already on the page.
            json!({
                "op": "component.define", "name": "house",
                "template": r#"<div class="house house--v2" data-x="{{x}}" data-y="{{y}}" data-roof="{{roof}}">{{#if lit}}<b class="lamp"></b>{{/if}}</div>"#,
                "prop_schema": {"x": "int", "y": "int", "roof": "text", "lit": "bool"},
            })
        } else {
            match rng.next(7) {
                0 => update(
                    &format!("fig-{}", rng.next(shape.figures)),
                    json!({"x": rng.next(4096), "y": rng.next(4096), "kind": "runner"}),
                ),
                1 => update(
                    &kid(rng.next(shape.chunks), 2 * rng.next(shape.kids / 2)),
                    json!({"lit": rng.next(2) == 1, "roof": "r<\"&>"}),
                ),
                2 => update(
                    &kid(rng.next(shape.chunks), 2 * rng.next(shape.kids / 2) + 1),
                    json!({"x": rng.next(999)}),
                ),
                3 => {
                    let id = format!("new-{step}");
                    let c = rng.next(shape.chunks);
                    created.push(id.clone());
                    json!({
                        "op": "object.create", "id": id, "parent": format!("chunk-{c}"),
                        "component": "tree", "ord": rng.next(40), "props": {"x": step, "y": 1},
                    })
                }
                4 if !created.is_empty() => {
                    let id = created.remove(rng.next(created.len()));
                    json!({"op": "object.delete", "id": id})
                }
                5 => update(
                    "note-1",
                    json!({"body": format!("<p>step {step}</p><script>x</script>")}),
                ),
                _ => update(
                    &format!("chunk-{}", rng.next(shape.chunks)),
                    json!({"lod": "far"}),
                ),
            }
        };
        ok(&lab.call(vec![op]).await);
        let (_, diff) = viewer.next_diff().await;
        diffs.push(diff);
    }
    let body = served_body(&lab.get_page().await);
    let Some((_, full)) = client_builds(&viewer.rendered, &diffs) else {
        return;
    };
    let built = without_client_ids(&full);
    if built != body {
        let at = built
            .bytes()
            .zip(body.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(built.len().min(body.len()));
        let from = at.saturating_sub(120);
        panic!(
            "the client built a different page at byte {at}:\nclient: {}\nserved: {}",
            &built[from..(at + 120).min(built.len())],
            &body[from..(at + 120).min(body.len())]
        );
    }
}

/// One route and what its viewer received since the join.
struct Watched {
    route: &'static str,
    viewer: web_fixture::Viewer,
    diffs: Vec<Value>,
}

/// T6b. Structure after the join, through the client, case by case: a move
/// inside a chunk, between chunks, out of and into the root's child list, a
/// root child created and deleted, a second route, a move across routes, and
/// bundles of several legs (the bundle merge passes every touched slot on,
/// GH #1001). After every case each route's client state is the page a GET
/// serves. `frames_to` names the routes a case pushes to: one frame each.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1001_structure_after_the_join_reaches_the_client() {
    let shape = small();
    let mut lab = Lab::start(shape).await;
    // A second page over a tree of its own: two root children and a box with
    // two children.
    let fig = |id: &str, parent: &str, ord: i64| {
        json!({"op": "object.create", "id": id, "parent": parent, "component": "fig",
               "ord": ord, "props": {"kind": "walker", "name": id, "x": ord, "y": 1, "dir": "n"}})
    };
    ok(&lab
        .call(vec![
            json!({"op": "object.create", "id": "stage2", "component": "stage",
                   "ord": 0, "props": {"w": 512}}),
            fig("two-0", "stage2", 0),
            fig("two-1", "stage2", 1),
            json!({"op": "object.create", "id": "box2", "parent": "stage2",
                   "component": "chunk", "ord": 2, "props": {"c": 99, "lod": "near"}}),
            fig("box2-0", "box2", 0),
            fig("box2-1", "box2", 1),
            json!({"op": "page.set", "route": "/two", "root": "stage2", "title": "Two"}),
        ])
        .await);

    let mut routes = Vec::new();
    for route in ["/", "/two"] {
        routes.push(Watched {
            route,
            viewer: lab.viewer_at(route).await,
            diffs: Vec::new(),
        });
    }

    let mv = |id: &str, parent: &str, ord: i64| json!({"op": "object.move", "id": id, "parent": parent, "ord": ord});
    let cases: Vec<(&str, Vec<Value>, &[usize])> = vec![
        (
            "a move inside a chunk",
            vec![mv(&kid(0, 3), "chunk-0", 100)],
            &[0],
        ),
        (
            "a move between chunks",
            vec![mv(&kid(1, 5), "chunk-2", 2)],
            &[0],
        ),
        (
            "a root child moved into a chunk",
            vec![mv("fig-4", "chunk-0", 1)],
            &[0],
        ),
        (
            "a chunk child moved to the root",
            vec![mv(&kid(2, 4), ROOT, 5)],
            &[0],
        ),
        (
            "a root child reordered",
            vec![mv("fig-1", ROOT, 9_000)],
            &[0],
        ),
        (
            "a root child created",
            vec![
                json!({"op": "object.create", "id": "root-new", "parent": ROOT,
                        "component": "tree", "ord": 3, "props": {"x": 3, "y": 3}}),
            ],
            &[0],
        ),
        (
            "a root child deleted",
            vec![json!({"op": "object.delete", "id": "fig-6"})],
            &[0],
        ),
        (
            "a write on the second route",
            vec![update("two-0", json!({"x": 77}))],
            &[1],
        ),
        (
            "a move across routes",
            vec![mv(&kid(3, 1), "box2", 1)],
            &[0, 1],
        ),
        (
            "a bundle of updates, creates, a move and a delete on two routes",
            vec![
                update("fig-0", json!({"x": 11})),
                json!({"op": "object.create", "id": "b-1", "parent": "chunk-1",
                       "component": "tree", "ord": 0, "props": {"x": 1, "y": 1}}),
                mv(&kid(0, 2), "chunk-3", 7),
                json!({"op": "object.delete", "id": "fig-7"}),
                update("two-1", json!({"x": 12})),
                fig("two-2", "stage2", 3),
            ],
            &[0, 1],
        ),
        (
            "a bundle that moves a root child and updates the chunk it went to",
            vec![
                mv("fig-8", "chunk-1", 4),
                update(&kid(1, 0), json!({"lit": true})),
            ],
            &[0],
        ),
        (
            "a bundle that writes a page root",
            vec![
                update(ROOT, json!({"w": 2048})),
                update("box2-0", json!({"x": 5})),
            ],
            &[0, 1],
        ),
    ];

    for (name, ops, frames_to) in cases {
        ok(&lab.call(ops).await);
        for &i in frames_to {
            let (_, diff) = routes[i].viewer.next_diff().await;
            routes[i].diffs.push(diff);
        }
        for w in &routes {
            let body = served_body(&lab.get_page_at(w.route).await);
            let Some((_, full)) = client_builds(&w.viewer.rendered, &w.diffs) else {
                return;
            };
            let built = without_client_ids(&full);
            assert!(
                built == body,
                "after {name}, route {}: the client built\n{built}\nthe GET serves\n{body}",
                w.route
            );
        }
    }
}

/// T7. One figure moves: the client skips every other root child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1001_an_unchanged_slot_is_skipped_by_the_client() {
    let shape = small();
    let mut lab = Lab::start(shape).await;
    let mut viewer = lab.viewer().await;
    ok(&lab.call(vec![update("fig-3", json!({"x": 77}))]).await);
    let (_, diff) = viewer.next_diff().await;
    let Some((steps, _)) = client_builds(&viewer.rendered, &[diff]) else {
        return;
    };
    let after = &steps[1];
    let root_children = shape.figures + shape.chunks;
    assert_eq!(
        after.matches("data-phx-skip").count(),
        root_children - 1,
        "every root child but slot 3 is a skipped shell: {}",
        &after[..after.len().min(600)]
    );
    assert!(
        after.contains("data-x=\"77\""),
        "slot 3 is drawn with its new value"
    );
}

/// T8. A component with two root elements cannot be a skippable part: its
/// slot stays an HTML string and the page is correct.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1001_a_template_with_two_roots_falls_back_to_a_string() {
    let mut lab = Lab::start(small()).await;
    ok(&lab
        .call(vec![json!({
            "op": "component.define", "name": "pair",
            "template": "<b>{{a}}</b><i>{{b}}</i>",
            "prop_schema": {"a": "text", "b": "text"},
        })])
        .await);
    ok(&lab
        .call(vec![json!({
            "op": "object.create", "id": "pair-1", "parent": ROOT,
            "component": "pair", "ord": 99999, "props": {"a": "one", "b": "two"},
        })])
        .await);
    let mut viewer = lab.viewer().await;
    let slot = (lab.shape.figures + lab.shape.chunks).to_string();
    assert_eq!(
        viewer.rendered[&slot],
        json!("<b>one</b><i>two</i>"),
        "the two-root slot is a string in the join"
    );
    ok(&lab.call(vec![update("pair-1", json!({"a": "uno"}))]).await);
    let (_, diff) = viewer.next_diff().await;
    assert_eq!(diff[&slot], json!("<b>uno</b><i>two</i>"));
    let body = served_body(&lab.get_page().await);
    assert!(body.contains("<b>uno</b><i>two</i></main>"));
    if let Some((_, full)) = client_builds(&viewer.rendered, &[diff]) {
        assert_eq!(without_client_ids(&full), body);
    }
}

/// The join of the 4 000-figure lab page in the flat form of web@2.2.0,
/// measured in the red run of this strand (gate host, 05.10.2026).
const FLAT_JOIN_BYTES: usize = 1_089_251;

/// Where a join's bytes go: `(statics, dynamics, structure)` of its rendered
/// tree, serialised. Statics are the strings of every `"s"` array and of
/// `"p"`; dynamics are every other string (the substituted values and string
/// slots); structure is the rest: keys, braces, colons, commas, the `"s"`
/// numbers and the `"r": 1` marks.
fn join_breakdown(rendered: &Value) -> (usize, usize, usize) {
    fn strings(v: &Value) -> usize {
        v.as_array()
            .map(|a| a.iter().map(|s| s.to_string().len()).sum())
            .unwrap_or(0)
    }
    fn walk(v: &Value, statics: &mut usize, dynamics: &mut usize) {
        match v {
            Value::String(_) => *dynamics += v.to_string().len(),
            Value::Object(m) => {
                for (k, x) in m {
                    match k.as_str() {
                        "p" => {
                            *statics += x.as_object().map_or(0, |p| p.values().map(strings).sum())
                        }
                        "s" => *statics += strings(x),
                        _ => walk(x, statics, dynamics),
                    }
                }
            }
            _ => {}
        }
    }
    let (mut statics, mut dynamics) = (0, 0);
    walk(rendered, &mut statics, &mut dynamics);
    let total = rendered.to_string().len();
    (statics, dynamics, total - statics - dynamics)
}

/// T9. The join carries each component's statics once.
///
/// Measured: 1 089 251 → 632 475 bytes (58 %). The plan asked for ≤ 35 %; the
/// part form cannot get there on this markup (OR-H4.W1.3): every part keeps
/// its dynamics as strings under numbered keys plus `"s"` and `"r"`, and a
/// figure here has seven values. The lock holds what the form gives, with
/// room for jitter in the values, not for a regression to the flat form.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1001_the_join_shares_statics() {
    let lab = Lab::start(Shape::with_figures(4_000)).await;
    let viewer = lab.viewer().await;
    println!(
        "LAB join of the 4000-figure page: {} bytes",
        viewer.join_bytes
    );
    let (statics, dynamics, structure) = join_breakdown(&viewer.rendered);
    println!(
        "LAB join breakdown: envelope {} bytes, statics {statics}, dynamics {dynamics}, \
         structure {structure}",
        viewer.join_bytes - viewer.rendered.to_string().len()
    );
    assert!(
        viewer.join_bytes * 100 <= FLAT_JOIN_BYTES * 65,
        "the join carries {} bytes, more than 65 % of the flat {FLAT_JOIN_BYTES}",
        viewer.join_bytes
    );
}
