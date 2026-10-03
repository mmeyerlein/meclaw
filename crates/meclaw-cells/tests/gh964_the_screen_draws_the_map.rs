//! GH #964 -- `display-map`: the screen works out the tiles, the application only
//! names the place.
//!
//! A map is a point (`lat`, `lon`), a `zoom` and a size in tiles (`w` x `h`). The
//! template language does no arithmetic and the `web` cell renders what it is
//! given, so the COMPOSE cell turns the point into tile indices when it builds the
//! object tree (`add_tree`, outside the pass of § 4) and writes one
//! `display-map-tile` child per tile, whose address it builds from the operator's
//! setting `map_tiles` and nothing else. An application never sends a tile and
//! never sends an address: the door refuses a tile it names, a coordinate that is
//! not a number in range (a URL in `lat` included), and a size or zoom out of
//! range; a credit line it claims is overwritten by the screen's.
//!
//! Without `map_tiles` -- the shipped default -- there is no tile and no fetch: the
//! map shows its name and its coordinates.
//!
//! Asked of `compose.py` directly, without a colony: these are pure functions and
//! one tree walk. The running screen is `gh964_a_map_stands_on_a_running_screen`,
//! the two engines are `gh964_a_map_draws_tiles_only_from_the_setting_browser`.

use meclaw_core::serde_json::{self, Value, json};

const COMPOSE: &str = "templates/display/compose/compose.py";
const HOST: &str = "https://tiles.example.test/{z}/{x}/{y}.png";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn library_ships() -> bool {
    repo(COMPOSE).is_file()
}

/// Run `prog` against the module `m` and return what it leaves in `out`.
fn ask(prog: &str) -> Option<Value> {
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(format!(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             {prog}\n\
             print(json.dumps(out))"
        ))
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(serde_json::from_slice(&out.stdout).expect("the answer is JSON"))
}

/// The slippy-map tile of a point, worked out here a second time and independently
/// of `compose.py`: `x` from the longitude, `y` from the Mercator latitude.
fn tile_of(lat: f64, lon: f64, zoom: u32) -> (f64, f64) {
    let n = f64::from(1u32 << zoom);
    let x = (lon + 180.0) / 360.0 * n;
    let phi = lat.to_radians();
    let y = (1.0 - (phi.tan() + 1.0 / phi.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

/// (1) The pure function, as a table. The grid is centred on the point: its first
/// column is `floor(x - w/2)`, its first row `floor(y - h/2)`; columns wrap round the
/// world, rows above the top or below the bottom of the world have no tile; the pin
/// is where the point lies inside the grid, in thousandths.
#[test]
fn the_tiles_are_a_function_of_the_place() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // (lat, lon, zoom, w, h)
    let cases: [(f64, f64, u32, i64, i64); 7] = [
        (52.52, 13.405, 12, 2, 2),      // a capital, the lock's own case
        (52.52, 13.405, 12, 1, 1),      // one tile: the one the point is on
        (-33.8688, 151.2093, 10, 3, 2), // southern and eastern
        (40.7128, -74.006, 15, 4, 4),   // western, the largest grid
        (0.0, 179.99, 3, 3, 1),         // at the date line: the columns wrap
        (85.0, 0.0, 3, 2, 2),           // at the top of the world: no row above it
        (0.0, 0.0, 17, 2, 2),           // the deepest zoom
    ];
    let prog = format!(
        "out = [m.map_tiles_of(*c) for c in {}]",
        serde_json::to_string(
            &cases
                .iter()
                .map(|(a, b, z, w, h)| json!([a, b, z, w, h]))
                .collect::<Vec<_>>()
        )
        .unwrap()
    );
    let Some(got) = ask(&prog) else {
        return;
    };
    for (i, (lat, lon, zoom, w, h)) in cases.iter().enumerate() {
        let n = 1i64 << zoom;
        let (x, y) = tile_of(*lat, *lon, *zoom);
        let col0 = (x - *w as f64 / 2.0).floor() as i64;
        let row0 = (y - *h as f64 / 2.0).floor() as i64;
        let mut want = Vec::new();
        for row in 0..*h {
            for col in 0..*w {
                let ty = row0 + row;
                if ty < 0 || ty >= n {
                    continue;
                }
                let tx = (col0 + col).rem_euclid(n);
                want.push(json!({"x": tx, "y": ty, "col": col + 1, "row": row + 1}));
            }
        }
        let g = &got[i];
        assert_eq!(
            g["tiles"],
            Value::Array(want),
            "case {i} ({lat}, {lon}) z{zoom} {w}x{h}: the tiles"
        );
        let pin_x = ((x - col0 as f64) / *w as f64 * 1000.0).round() as i64;
        let pin_y = ((y - row0 as f64) / *h as f64 * 1000.0).round() as i64;
        assert_eq!(
            (g["pin_x"].as_i64(), g["pin_y"].as_i64()),
            (Some(pin_x.clamp(1, 999)), Some(pin_y.clamp(1, 999))),
            "case {i}: the pin"
        );
    }
    // And the one case a person can check on any tile map: the capital at zoom 12
    // lies on tile 2200/1342.
    assert_eq!(
        got[1]["tiles"],
        json!([{"x": 2200, "y": 1342, "col": 1, "row": 1}])
    );
}

/// (2) The operator's setting. A pattern is `https://` with a host (display-hive.md § 6.18) and all three of
/// `{z}`, `{x}`, `{y}`, and nothing in it that could leave an attribute; anything else
/// is read as no setting at all -- no tile, no fetch -- rather than as half a URL.
#[test]
fn only_a_clean_pattern_is_a_setting() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let Some(got) = ask("out = [m.map_setting(v) for v in [\
           'https://tiles.example.test/{z}/{x}/{y}.png',\
           'http://127.0.0.1:8080/{z}/{x}/{y}.png',\
           '', None, 42,\
           'javascript:alert(1)//{z}/{x}/{y}',\
           '//tiles.example.test/{z}/{x}/{y}.png',\
           'https://tiles.example.test/{z}/{x}.png',\
           'https://tiles.example.test/{z}/{x}/{y}.png\" onerror=\"x',\
           'https:///{z}/{x}/{y}.png',\
           'https://tiles.example.test/{z}/{x}/{y}.png?key=<k>',\
           ' https://tiles.example.test/{z}/{x}/{y}.png']] \
         + [m.tile_url('https://tiles.example.test/{z}/{x}/{y}.png', 12, 2200, 1342)]")
    else {
        return;
    };
    assert_eq!(
        got,
        json!([
            "https://tiles.example.test/{z}/{x}/{y}.png",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "https://tiles.example.test/12/2200/1342.png"
        ])
    );
}

/// (3) The tree walk. With a setting, a map gets its tiles as children -- addresses
/// from the setting and integer indices only -- its pin, the setting's credit line in
/// place of whatever the application claimed, and its size and zoom written out. A
/// tile the application put in is not one of them. Without a setting there is no
/// child, no pin and no credit line, and the name and the coordinates stay.
#[test]
fn the_screen_writes_the_tiles_and_nothing_else_does() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let Some(got) = ask(&format!(
        "node = {{'component': 'display-map', 'props': {{'lat': '52.52', 'lon': 13.405,\
                 'zoom': 12, 'label': 'Capital', 'attribution': 'mine', 'pin_x': 7}},\
                 'children': [{{'component': 'display-map-tile',\
                   'props': {{'src': 'https://elsewhere.example/evil.png', 'col': 1, 'row': 1}}}},\
                   {{'component': 'display-mark', 'props': {{'x': 500, 'y': 250, 'label': 'here'}}}}]}}\n\
         out = {{}}\n\
         for name, setting in (('set', '{HOST}'), ('unset', '')):\n\
         \x20   m.MAP_TILES = m.map_setting(setting)\n\
         \x20   m.MAP_ATTRIBUTION = 'Tiles: example data' if setting else ''\n\
         \x20   want = {{}}\n\
         \x20   m.add_tree(want, 'v', json.loads(json.dumps(node)), 0)\n\
         \x20   out[name] = want"
    )) else {
        return;
    };
    let set = got["set"].as_object().expect("objects");
    let map = &set["v/0"];
    assert_eq!(map["component"], "display-map");
    let p = &map["props"];
    assert_eq!(p["attribution"], "Tiles: example data", "{p}");
    assert_eq!(
        (p["zoom"].as_i64(), p["w"].as_i64(), p["h"].as_i64()),
        (Some(12), Some(2), Some(2)),
        "{p}"
    );
    assert_eq!(
        (p["lat"].as_str(), p["label"].as_str()),
        (Some("52.52"), Some("Capital")),
        "{p}"
    );
    let (x, y) = tile_of(52.52, 13.405, 12);
    let (col0, row0) = ((x - 1.0).floor() as i64, (y - 1.0).floor() as i64);
    assert_eq!(
        p["pin_x"].as_i64(),
        Some(((x - col0 as f64) / 2.0 * 1000.0).round() as i64),
        "the pin is the screen's, not the 7 the application said: {p}"
    );
    let mut tiles: Vec<(String, i64, i64)> = set
        .iter()
        .filter(|(_, o)| o["component"] == "display-map-tile")
        .map(|(id, o)| {
            assert_eq!(o["parent"], "v/0", "{id} hangs under the map");
            (
                o["props"]["src"].as_str().unwrap_or("").to_string(),
                o["props"]["col"].as_i64().unwrap_or(0),
                o["props"]["row"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    tiles.sort();
    // OR-DP-71: the application's mark stays on the map, behind the screen's tiles.
    let marks: Vec<(&String, &Value)> = set
        .iter()
        .filter(|(_, o)| o["component"] == "display-mark")
        .collect();
    assert_eq!(marks.len(), 1, "the mark stands on the map: {set:?}");
    let (mid, mark) = marks[0];
    assert_eq!(mark["parent"], "v/0", "{mid} hangs under the map");
    assert_eq!(
        (mark["props"]["x"].as_i64(), mark["props"]["y"].as_i64()),
        (Some(500), Some(250)),
        "in thousandths of the map, as sent: {mark}"
    );
    let last_tile = set
        .values()
        .filter(|o| o["component"] == "display-map-tile")
        .filter_map(|o| o["ord"].as_i64())
        .max()
        .expect("tiles");
    assert!(
        mark["ord"].as_i64() > Some(last_tile),
        "drawn after the tiles, so it stands on them: {mark}"
    );
    let mut want: Vec<(String, i64, i64)> = Vec::new();
    for row in 0..2 {
        for col in 0..2 {
            want.push((
                format!(
                    "https://tiles.example.test/12/{}/{}.png",
                    col0 + col,
                    row0 + row
                ),
                col + 1,
                row + 1,
            ));
        }
    }
    want.sort();
    assert_eq!(
        tiles, want,
        "four tiles from the setting, none from the application"
    );

    let unset = got["unset"].as_object().expect("objects");
    assert!(
        unset.values().all(|o| o["component"] != "display-map-tile"),
        "no setting, no tile: {unset:?}"
    );
    assert_eq!(
        unset
            .values()
            .filter(|o| o["component"] == "display-mark")
            .count(),
        1,
        "the mark is the application's, with or without a setting: {unset:?}"
    );
    let p = &unset["v/0"]["props"];
    assert_eq!(p["attribution"], "", "no tiles, no credit line: {p}");
    assert_eq!(p["pin_x"].as_i64(), Some(0), "no tiles, no pin: {p}");
    assert_eq!(
        (p["lat"].as_str(), p["lon"].as_f64(), p["label"].as_str()),
        (Some("52.52"), Some(13.405), Some("Capital")),
        "{p}"
    );
}

/// (4) The door. A coordinate is a number in range -- digits as text or a JSON
/// number -- and nothing else, so an address in `lat` is refused before it is
/// written anywhere; zoom and size are in range; a tile is the screen's to write.
#[test]
fn the_door_refuses_what_is_not_a_place() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let Some(got) = ask("def mp(**props):\n\
         \x20   base = {'lat': '52.52', 'lon': '13.405'}\n\
         \x20   base.update(props)\n\
         \x20   return {'component': 'display-pane', 'children': [{'component': 'display-map', 'props': base}]}\n\
         out = [m.typed_refusal(t) for t in [\n\
         \x20   mp(),\n\
         \x20   mp(lat=52.52, lon=-13, zoom='3', w=4, h=1),\n\
         \x20   mp(lat='https://tiles.example.test/1/1/1.png'),\n\
         \x20   mp(lon='181'),\n\
         \x20   mp(lat='-90.5'),\n\
         \x20   mp(lat=True),\n\
         \x20   mp(lat='1e3'),\n\
         \x20   mp(zoom=2),\n\
         \x20   mp(zoom=18),\n\
         \x20   mp(w=0),\n\
         \x20   mp(h=5),\n\
         \x20   {'component': 'display-map', 'props': {'lon': '1'}},\n\
         \x20   {'component': 'display-pane', 'children': [{'component': 'display-map',\n\
         \x20     'props': {'lat': '1', 'lon': '1'}, 'children': [{'component': 'display-map-tile',\n\
         \x20     'props': {'src': 'https://tiles.example.test/1/1/1.png', 'col': 1, 'row': 1}}]}]},\n\
         \x20   {'component': 'display-pane', 'children': [{'component': 'display-map-tile',\n\
         \x20     'props': {'src': 'https://tiles.example.test/1/1/1.png'}}]},\n\
         \x20   {'component': 'display-pane', 'children': [{'component': 'display-map',\n\
         \x20     'props': {'lat': '1', 'lon': '1'}, 'children': [{'component': 'display-text',\n\
         \x20     'props': {'body': 'x'}}]}]},\n\
         \x20   {'component': 'display-pane', 'children': [{'component': 'display-map',\n\
         \x20     'props': {'lat': '1', 'lon': '1'}, 'children': [{'component': 'display-mark',\n\
         \x20     'props': {'x': 500, 'y': 250, 'label': 'here'}}]}]},\n\
         ]]")
    else {
        return;
    };
    assert_eq!(
        got,
        json!([
            null,
            null,
            "display-map.lat: number -90..90",
            "display-map.lon: number -180..180",
            "display-map.lat: number -90..90",
            "display-map.lat: number -90..90",
            "display-map.lat: number -90..90",
            "display-map.zoom: int 3..17",
            "display-map.zoom: int 3..17",
            "display-map.w: int 1..4",
            "display-map.h: int 1..4",
            "display-map.lat: required text",
            "display-map-tile: written by the screen",
            "display-map-tile: written by the screen",
            "display-map.children: display-map-tile|display-mark",
            // OR-DP-71: a mark is what an application may stand on its map.
            null
        ])
    );
}
