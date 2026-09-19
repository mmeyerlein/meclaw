//! GH #708 -- no part of a tile is under 0.88 (`display-hive.md` § 6.11).
//!
//! "Tiles stand in front of the canvas and are readable above every window: no
//! part of a tile has an opacity below 0.88" (§ 6.11, from the acceptance of
//! 16.09.). Beside it stands § 6.10: a tile whose window is open is "slightly
//! dimmed". The two sentences together leave one span, 0.88 to 1.0, and the
//! floor IS the dimming -- so the number lives once, as a token, and the open
//! tile spends it (plan § 2, OR-H3; the browser proof is B-22, strand H5).
//!
//! Guarded like every template-reading test (GH #49).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";
const TOKEN: &str = "--tile-open-opacity";
const FLOOR: f64 = 0.88;

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn without_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            match src[i + 2..].find("*/") {
                Some(end) => i = i + 2 + end + 2,
                None => break,
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// Every plain rule of the sheet as `(selector, body)`, at-rules walked into.
/// CSS rules do not nest, so a body holds no brace of its own.
fn rules(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let (mut i, mut start) = (0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'{' => {
                let head = src[start..i].trim().to_string();
                if head.starts_with('@') {
                    // An at-rule: its own brace, and rules inside it.
                    i += 1;
                    start = i;
                    continue;
                }
                let end = src[i..].find('}').map(|e| i + e).unwrap_or(b.len());
                out.push((head, src[i + 1..end].to_string()));
                i = end + 1;
                start = i;
            }
            b'}' => {
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    out
}

/// Every `opacity: <value>` of one declaration body.
fn opacities(body: &str) -> Vec<String> {
    body.split(';')
        .filter_map(|d| {
            let (name, value) = d.split_once(':')?;
            if name.trim() == "opacity" {
                Some(
                    value
                        .trim()
                        .trim_end_matches("!important")
                        .trim()
                        .to_string(),
                )
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn a_tile_never_falls_below_the_floor() {
    if !library_ships() {
        eprintln!("skip: no template library in this tree");
        return;
    }
    let sheet = without_comments(&read(SHEET));
    let mut seen = 0;
    for (selector, body) in rules(&sheet) {
        if !selector.contains(".display-tile") {
            continue;
        }
        for value in opacities(&body) {
            seen += 1;
            if value == format!("var({TOKEN})") {
                continue;
            }
            let n: f64 = value.parse().unwrap_or_else(|_| {
                panic!(
                    "{selector} sets `opacity: {value}`, which is neither a number nor the token"
                )
            });
            assert!(
                n >= FLOOR,
                "{selector} sets `opacity: {value}` -- § 6.11: no part of a tile \
                 has an opacity below {FLOOR}"
            );
        }
    }
    assert!(
        seen > 0,
        "{SHEET} sets no opacity on a tile at all -- the rules moved"
    );
}

#[test]
fn the_dimming_is_the_floor_and_the_number_stands_once() {
    if !library_ships() {
        eprintln!("skip: no template library in this tree");
        return;
    }
    let raw = read(SHEET);
    let sheet = without_comments(&raw);
    assert!(
        sheet.contains(&format!("{TOKEN}: 0.88;")),
        "{SHEET} declares no `{TOKEN}: 0.88` -- § 2 `Token`: the number lives \
         in the sheet, and this one is § 6.11's floor"
    );
    assert_eq!(
        sheet.matches("0.88").count(),
        1,
        "the floor stands more than once in {SHEET}: one quantity, one token \
         (development-rules § 2d)"
    );
    let open = rules(&sheet)
        .into_iter()
        .find(|(s, _)| s == ".display-tile[data-open=\"1\"]")
        .expect("no rule for the tile of an open window (§ 6.10)");
    assert_eq!(
        opacities(&open.1),
        vec![format!("var({TOKEN})")],
        "the open tile spends something other than the token: § 6.10 dims it \
         slightly, § 6.11 says how far down that may go"
    );
}
