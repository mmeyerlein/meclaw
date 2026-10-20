//! GH #1260 (strand W1) -- the tools hive answers a weather question.
//!
//! The quick path of a turn carries the tools its class is for, and a weather
//! question is the plainest of them: until #1260 the tools hive had no weather
//! occupant at all, so a `simple` turn asking for tomorrow's rain could only
//! search the web. `templates/tools/weather` is a `code` cell against
//! Open-Meteo (free, no key): the geocoder names the place, the forecast
//! answers for its coordinates.
//!
//! Everything here runs the SHIPPED `params.script_inline` against a local
//! mock on loopback -- never the real service: the property under test (the
//! two requests, the answer a voice can speak, the named refusals, the bound
//! on a silent endpoint) is the same whoever answers. The second half pins the
//! registration: one occupant directory, two edges, one schema row, and the
//! talky collector asking for the tool by name.

use meclaw_testing::deadline;
use meclaw_testing::mock_http::{MockResponse, start_capture};
use serde_json::{Map, Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// The two templates this file reads, each named in its path: both travel
/// with the export, and a path that names its template is what the export
/// audit can check (R2b).
fn tools_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/tools")
}

fn talky_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/talky")
}

fn assistant_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/assistant")
}

fn read_json(path: std::path::PathBuf) -> Value {
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn tools(rel: &str) -> Value {
    read_json(tools_dir().join(rel))
}

fn weather_config() -> Value {
    tools("weather/config.json")
}

/// The shipped params with `over` merged on top, minus the script source --
/// the object the substrate hands the child. A knob this file names that the
/// template does not carry is a failure, never a silently ignored line.
fn params(over: Value) -> Value {
    let cfg = weather_config();
    let mut p: Map<String, Value> = cfg["params"].as_object().expect("params").clone();
    p.remove("script_inline");
    for (k, v) in over.as_object().expect("overrides are an object") {
        assert!(
            p.contains_key(k),
            "unknown knob `{k}` -- the shipped params do not carry it"
        );
        p.insert(k.clone(), v.clone());
    }
    Value::Object(p)
}

/// The two endpoints pointed at the mock, the stamp fixed.
fn stub_params(addr: std::net::SocketAddr, over: Value) -> Value {
    let mut p = json!({
        "geocode_endpoint": format!("http://{addr}/v1/search"),
        "forecast_endpoint": format!("http://{addr}/v1/forecast"),
        "now_utc": "2026-10-19T12:00:00Z",
    });
    for (k, v) in over.as_object().expect("object") {
        p[k] = v.clone();
    }
    params(p)
}

fn call_doc(args: Value, params: Value) -> String {
    meclaw_testing::code_stdin(&json!({
        "header": {"hop": {"route": "tool_call", "tool_name": "weather",
                           "tool_call_id": "call-w"}},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-w",
                      "text": args.to_string()}],
        "params": params,
    }))
    .to_string()
}

/// The one emission of the cell: (header, the result text parsed as JSON).
async fn run(stdin_doc: &str) -> (Value, Value) {
    let script = weather_config()["params"]["script_inline"]
        .as_str()
        .expect("the weather cell ships a script")
        .to_string();
    // The script travels on stdin, never in argv (MAX_ARG_STRLEN, GH #279).
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(&script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn python3");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all(src.as_bytes()).await.expect("write");
    drop(stdin);
    let out = deadline::hang("the weather script exits", child.wait_with_output())
        .await
        .expect("python3 output");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "the cell must not die: {stderr}");
    let msg: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not one emission ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    let turns = msg["messages"].as_array().expect("messages");
    assert_eq!(turns.len(), 1, "ONE tool_result turn: {msg}");
    assert_eq!(turns[0]["type"], "tool_result");
    assert_eq!(
        turns[0]["id"], "call-w",
        "the result answers its call: {msg}"
    );
    let text = turns[0]["text"].as_str().expect("text");
    let parsed: Value = serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("the result text is a JSON object ({e}): {text}"));
    (msg["header"].clone(), parsed)
}

fn geocode_hit() -> MockResponse {
    MockResponse::ok_json(
        json!({"results": [{"name": "Berlin", "country": "Germany", "country_code": "DE",
                            "admin1": "Berlin", "latitude": 52.52, "longitude": 13.41,
                            "timezone": "Europe/Berlin"}],
               "generationtime_ms": 0.4})
        .to_string()
        .as_bytes(),
    )
}

fn forecast() -> MockResponse {
    MockResponse::ok_json(
        json!({"latitude": 52.52, "longitude": 13.41, "timezone": "Europe/Berlin",
               "current": {"time": "2026-10-19T14:00", "temperature_2m": 13.2,
                           "weather_code": 3},
               "daily": {"time": ["2026-10-19", "2026-10-20"],
                         "weather_code": [61, 0],
                         "temperature_2m_max": [15.4, 17.0],
                         "temperature_2m_min": [9.1, 8.0],
                         "precipitation_sum": [2.1, 0.0]}})
        .to_string()
        .as_bytes(),
    )
}

/// An answer the bounded attempt never outlives.
const NEVER: Duration = Duration::from_secs(24 * 3600);

// ═══════════════════════════════════════════════════════ 1. the cell

/// The whole road: the place goes to the geocoder, its coordinates to the
/// forecast, and the answer comes back as one sentence a voice can say plus
/// the fields it was made from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_place_goes_through_the_geocoder_to_its_forecast() {
    let mut mock = start_capture(vec![geocode_hit(), forecast()]).await;
    let p = stub_params(mock.addr, json!({}));
    let (header, body) = run(&call_doc(
        json!({"place": "Berlin", "days": 2, "language": "en"}),
        p,
    ))
    .await;

    let seen = mock.next_requests(2).await;
    assert!(
        seen[0].path.starts_with("/v1/search?") && seen[0].path.contains("name=Berlin"),
        "the place goes to the geocoder first: {}",
        seen[0].path
    );
    assert!(seen[0].path.contains("language=en"), "{}", seen[0].path);
    assert!(
        seen[1].path.starts_with("/v1/forecast?")
            && seen[1].path.contains("latitude=52.52")
            && seen[1].path.contains("longitude=13.41")
            && seen[1].path.contains("forecast_days=2"),
        "the forecast is asked for the geocoder's coordinates and the days of the call: {}",
        seen[1].path
    );
    for r in &seen {
        assert_eq!(
            r.headers.get("user-agent").map(String::as_str),
            Some("meclaw weather"),
            "a free service is told who calls it"
        );
    }

    assert_eq!(header["operation"], "weather", "{header}");
    assert!(
        header.get("error_code").is_none(),
        "a served call carries no error code: {header}"
    );
    assert!(
        header.get("tool_name").is_none(),
        "an answer never carries the key its door dispatches on (W7-R4): {header}"
    );
    assert!(
        body.get("error").is_none() && body.get("error_code").is_none(),
        "the curator's speaker rule reads an error object as unspeakable: {body}"
    );
    let summary = body["summary"].as_str().expect("summary");
    assert!(
        summary.starts_with("Berlin, Germany:"),
        "the place is named with its country, so a wrong town is audible: {summary}"
    );
    assert!(
        summary.contains("light rain") && summary.contains("clear sky"),
        "the weather codes are words, not numbers: {summary}"
    );
    assert_eq!(body["place"]["name"], "Berlin");
    assert_eq!(body["timezone"], "Europe/Berlin");
    assert_eq!(body["current"]["condition"], "overcast");
    let days = body["days"].as_array().expect("days");
    assert_eq!(days.len(), 2, "{body}");
    assert_eq!(days[0]["temp_min"], 9.1);
    assert_eq!(days[0]["temp_max"], 15.4);
    assert_eq!(days[0]["precipitation_mm"], 2.1);
    assert_eq!(days[1]["condition"], "clear sky");
    assert_eq!(
        body["fetched_at"], "2026-10-19T12:00:00Z",
        "the stamp override"
    );
}

/// The words come out of the language data, and the shipped default language
/// is the one the call gets when it names none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_words_come_from_the_language_table_of_the_default_language() {
    let mut mock = start_capture(vec![geocode_hit(), forecast()]).await;
    let p = stub_params(mock.addr, json!({}));
    let lang = p["default_language"]
        .as_str()
        .expect("default_language")
        .to_string();
    let rain = p["condition_lang"][&lang]["codes"]["61"]
        .as_str()
        .unwrap_or_else(|| panic!("the shipped table carries code 61 for `{lang}`"))
        .to_string();
    let (_, body) = run(&call_doc(json!({"place": "Berlin"}), p)).await;
    let seen = mock.next_requests(2).await;
    assert!(
        seen[0].path.contains(&format!("language={lang}")),
        "{}",
        seen[0].path
    );
    assert!(
        seen[1].path.contains("forecast_days=1"),
        "one day by default"
    );
    assert_eq!(body["days"][0]["condition"], json!(rain), "{body}");
}

/// GH #1260 (lab B-L4): `days: 0` is today, not a refusal. A model read the
/// old schema text "how many days from today" as an offset and sent 0 for
/// "today"; the refusal sent the round to the brain. Zero counts as one day.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_days_is_today() {
    let mut mock = start_capture(vec![geocode_hit(), forecast()]).await;
    let (header, body) = run(&call_doc(
        json!({"place": "Berlin", "days": 0, "language": "en"}),
        stub_params(mock.addr, json!({})),
    ))
    .await;
    assert!(header.get("error_code").is_none(), "{header}");
    let seen = mock.next_requests(2).await;
    assert!(
        seen[1].path.contains("forecast_days=1"),
        "zero asks for today only: {}",
        seen[1].path
    );
    assert!(
        body["days"].as_array().is_some_and(|d| !d.is_empty()),
        "{body}"
    );
}

/// GH #1260: a whole number written as a float means the same day count as
/// the integer -- `1.0` is one day and `0.0` is today, like `1` and `0`. A
/// review found `0.0` accepted and `1.0` refused; JSON numbers from a model
/// carry no reliable int/float distinction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_whole_number_float_counts_like_the_integer() {
    for (days, want) in [(json!(1.0), 1), (json!(2.0), 2), (json!(0.0), 1)] {
        let mut mock = start_capture(vec![geocode_hit(), forecast()]).await;
        let (header, _body) = run(&call_doc(
            json!({"place": "Berlin", "days": days, "language": "en"}),
            stub_params(mock.addr, json!({})),
        ))
        .await;
        assert!(header.get("error_code").is_none(), "{days}: {header}");
        let seen = mock.next_requests(2).await;
        assert!(
            seen[1].path.contains(&format!("forecast_days={want}&"))
                || seen[1].path.ends_with(&format!("forecast_days={want}")),
            "{days} asks for {want} day(s): {}",
            seen[1].path
        );
    }
}

/// GH #1260 (lab B-L4): the schema says `days` counts today, so a model
/// cannot read it as an offset from today.
#[test]
fn the_days_of_the_schema_count_today() {
    let schemas = tools("schemas/config.json");
    let script = schemas["params"]["script_inline"].as_str().expect("script");
    assert!(
        script.contains("number of days including today; 1 = today only"),
        "the days description"
    );
    assert!(
        !script.contains("how many days from today"),
        "the old offset wording"
    );
}

/// A place the geocoder does not know is a named refusal, not an empty result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_place_is_a_named_refusal() {
    let mock = start_capture(vec![MockResponse::ok_json(
        br#"{"generationtime_ms": 0.2}"#,
    )])
    .await;
    let (header, body) = run(&call_doc(
        json!({"place": "Nowhereville"}),
        stub_params(mock.addr, json!({})),
    ))
    .await;
    assert_eq!(header["error_code"], "place_not_found", "{header}");
    assert_eq!(body["error_code"], "place_not_found", "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("Nowhereville")),
        "{body}"
    );
}

/// A call without a place, or with days out of range, is refused before any
/// request leaves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_without_a_place_is_invalid_input() {
    let mock = start_capture(vec![geocode_hit()]).await;
    for args in [
        json!({}),
        json!({"place": "Berlin", "days": 9}),
        json!({"place": "Berlin", "days": 1.5}),
        json!({"place": "Berlin", "days": 9.0}),
    ] {
        let (header, body) = run(&call_doc(args.clone(), stub_params(mock.addr, json!({})))).await;
        assert_eq!(header["error_code"], "invalid_input", "{args}: {header}");
        assert_eq!(body["error_code"], "invalid_input", "{args}: {body}");
    }
}

/// An endpoint that never answers ends on the cell's own operation timeout as
/// a `timeout` result -- the marker is the code, never a wall-clock verdict.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_service_is_a_timeout_result_not_a_hang() {
    let mock = start_capture(vec![geocode_hit().with_delay(NEVER)]).await;
    let (header, body) = run(&call_doc(
        json!({"place": "Berlin"}),
        stub_params(mock.addr, json!({"external_timeout_ms": 3000})),
    ))
    .await;
    assert_eq!(header["error_code"], "timeout", "{header}");
    assert_eq!(body["error_code"], "timeout", "{body}");
}

/// No connection at all is `io_error`, and a non-2xx answer `upstream_error`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_or_failing_service_is_named() {
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("addr")
    };
    let (header, _) = run(&call_doc(
        json!({"place": "Berlin"}),
        stub_params(closed, json!({})),
    ))
    .await;
    assert_eq!(header["error_code"], "io_error", "{header}");

    let mock = start_capture(vec![MockResponse::server_error()]).await;
    let (header, _) = run(&call_doc(
        json!({"place": "Berlin"}),
        stub_params(mock.addr, json!({})),
    ))
    .await;
    assert_eq!(header["error_code"], "upstream_error", "{header}");
}

/// Review M1 (fix round 1): an answer the script did not foresee -- a `NaN`
/// where a temperature stands (Python's JSON reader takes it, `round` does
/// not) -- is an ordinary `upstream_error` result carrying `hop.operation`,
/// never a dead script: a `script_failed` carries no operation, the hive's
/// exit edge (`hop.operation == 'weather'`) does not match it, and the round
/// would wait out its window.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unforeseen_answer_is_an_upstream_error_not_a_dead_script() {
    let nan_forecast = MockResponse::ok_json(
        br#"{"timezone": "Europe/Berlin",
             "current": {"time": "2026-10-19T14:00", "temperature_2m": NaN, "weather_code": 3},
             "daily": {"time": ["2026-10-19"], "weather_code": [61],
                       "temperature_2m_max": [NaN], "temperature_2m_min": [9.1],
                       "precipitation_sum": [2.1]}}"#,
    );
    let mock = start_capture(vec![geocode_hit(), nan_forecast]).await;
    let (header, body) = run(&call_doc(
        json!({"place": "Berlin"}),
        stub_params(mock.addr, json!({})),
    ))
    .await;
    assert_eq!(header["operation"], "weather", "{header}");
    assert_eq!(header["error_code"], "upstream_error", "{header}");
    assert_eq!(body["error_code"], "upstream_error", "{body}");
}

/// The same net catches a language table an operator got wrong: a `day`
/// phrase with the wrong number of `%s` slots is a named refusal, not a
/// crash.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broken_phrase_in_the_language_data_is_an_upstream_error() {
    let mock = start_capture(vec![geocode_hit(), forecast()]).await;
    let mut lang = weather_config()["params"]["condition_lang"].clone();
    lang["en"]["phrases"]["day"] = json!("%s");
    let (header, body) = run(&call_doc(
        json!({"place": "Berlin", "language": "en"}),
        stub_params(mock.addr, json!({"condition_lang": lang})),
    ))
    .await;
    assert_eq!(header["operation"], "weather", "{header}");
    assert_eq!(header["error_code"], "upstream_error", "{header}");
    assert_eq!(body["error_code"], "upstream_error", "{body}");
}

// ═══════════════════════════════════════════════════════ 2. the registration

/// The shipped cell needs no key and reaches the public service by default:
/// both endpoints are params with an env default naming Open-Meteo, the
/// sandbox allows the network it exists for, and the contract says so.
#[test]
fn the_shipped_cell_names_its_endpoints_and_its_reach() {
    let cfg = weather_config();
    assert_eq!(cfg["cell"]["type"], "code", "no new cell type (W1)");
    let p = &cfg["params"];
    assert_eq!(
        p["geocode_endpoint"],
        "${WEATHER_GEOCODE_ENDPOINT:-https://geocoding-api.open-meteo.com/v1/search}"
    );
    assert_eq!(
        p["forecast_endpoint"],
        "${WEATHER_FORECAST_ENDPOINT:-https://api.open-meteo.com/v1/forecast}"
    );
    assert_eq!(p["sandbox"]["network"], "allow");
    assert_eq!(p["sandbox"]["trust"], "restricted");
    let caps = cfg["contract"]["capabilities"].as_array().expect("caps");
    assert!(caps.contains(&json!("network:http")), "{caps:?}");
    let timeout = p["external_timeout_ms"]
        .as_u64()
        .expect("external_timeout_ms");
    let backstop = cfg["cell"]["message_timeout"]
        .as_u64()
        .expect("message_timeout");
    assert!(
        backstop > timeout,
        "B generous, A precise (AGENTS rule 12): {backstop} vs {timeout}"
    );

    let t = tools("template.json");
    for var in ["WEATHER_GEOCODE_ENDPOINT", "WEATHER_FORECAST_ENDPOINT"] {
        assert_eq!(
            t["requires"]["env"][var]["required"],
            json!(false),
            "{var} is declared and not required, like SEARCH_ENDPOINT"
        );
    }
}

/// One occupant directory, two edges, one schema row -- and the talky
/// collector asks for the tool by its name (a name without `_` folds into no
/// family, so the menu shows it as itself).
#[test]
fn the_hive_routes_weather_and_talky_asks_for_it() {
    let edges = tools("config.json")["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    assert!(
        edges.iter().any(|e| e["from"] == "."
            && e["to"] == "./weather"
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'tool_call'")
                    && c.contains("hop.tool_name == 'weather'"))),
        "the door to ./weather"
    );
    assert!(
        edges.iter().any(|e| e["from"] == "./weather"
            && e["to"] == "."
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.operation == 'weather'"))
            && e["modifier"]["set_hop"]["route"] == "'tool_result'"
            && e["modifier"]["set_hop"]["tool_name"] == "'weather'"),
        "the exit of ./weather onto tool_result, naming its tool (GH #937)"
    );
    let schemas = tools("schemas/config.json");
    let script = schemas["params"]["script_inline"].as_str().expect("script");
    assert!(script.contains("\"weather\": {"), "the schema row");
    let t = tools("template.json");
    assert!(t["reentrancy"]["weather"]["reentrant"].is_boolean());
    assert!(
        t["tags"]
            .as_array()
            .expect("tags")
            .contains(&json!("weather"))
    );

    let talky = read_json(talky_dir().join("collector/config.json"));
    assert!(
        talky["override_params"]["assemble"]["tools"]
            .as_array()
            .expect("tools")
            .contains(&json!("weather")),
        "talky asks for the weather tool"
    );
}

/// Review M2 (fix round 1): both assistant generations override the
/// composite's tool list wholesale (OR-WG.W.1), so they are the lists a live
/// turn actually carries -- each must ask for `weather` itself, or the quick
/// path's `quick_tools` names a tool no menu holds.
#[test]
fn both_assistant_generations_ask_for_weather() {
    for generation in ["talky", "talky-chat"] {
        let cfg = read_json(assistant_dir().join(generation).join("config.json"));
        let tools = cfg["override_params"]["collector/assemble"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("assistant/{generation} overrides the tool list"));
        assert!(
            tools.contains(&json!("weather")),
            "assistant/{generation} asks for the weather tool: {tools:?}"
        );
    }
}
