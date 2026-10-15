//! GH #708 -- the sheet reads the attribute names the description gives it.
//!
//! `display-hive.md` § 2 strikes four words the sheet used to select on:
//! `plane` per output, `state` as a curator word, the hint `modal: true`, and
//! `screen` as the name of an output. The curator writes a window's step as
//! `rung` (§ 4.17-4.23) and its drawing level as `level` (§ 4.24), both
//! systemwide; a tile says whether its window is open (§ 6.10); the root says
//! which kind of output it is drawn for (§ 6.1, § 6.6).
//!
//! So the contract between curator and sheet is a list of names, and this file
//! is that list. Two halves:
//!
//!   * the SHEET half is strict and always runs: no struck name stands in a
//!     selector, and every `data-*` a selector reads is on the list below.
//!   * the TEMPLATE half runs as soon as the curator carries `ATTRS` -- the
//!     templates belong to the curator strand (plan § 6, task H1-T6), and the
//!     two halves of one wave meet at the integration merge, not before.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! library is skipped, never judged.

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";
const COMPOSE: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The sheet without its prose. Every sentence in this file that explains a
/// rule may name an attribute; only what a SELECTOR reads is the contract.
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

/// Every `data-<name>` in the text, in the order it appears.
fn data_attributes(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(at) = src[i..].find("data-") {
        let start = i + at;
        let mut end = start + "data-".len();
        while end < b.len() && (b[end].is_ascii_lowercase() || b[end] == b'-') {
            end += 1;
        }
        // A trailing hyphen belongs to nothing: `data-` alone is not a name.
        let name = src[start..end].trim_end_matches('-').to_string();
        if name.len() > "data-".len() {
            out.push(name);
        }
        i = end;
    }
    out
}

/// The names the sheet may select on, each with the sentence it serves.
/// Anything else in a selector is either a typo or a contract nobody wrote
/// down -- both are findings, which is why this list is closed.
const ALLOWED: &[(&str, &str)] = &[
    // The curator's words about a window (plan § 4, `ATTRS`).
    ("data-rung", "the window's step, systemwide (§ 4.17-4.23)"),
    ("data-level", "the drawing level 0-3, systemwide (§ 4.24)"),
    ("data-age", "fresh | settled | leaving (§ 4.35)"),
    ("data-open", "1 on the tile of an open window (§ 6.10)"),
    ("data-pinned", "1 on a pinned tile (§ 7.6)"),
    ("data-unread", "1 on a tile whose app says so (§ 7.2)"),
    ("data-unseen", "the count, on the OS mark (§ 4.33)"),
    // The root: which output this page is, and what it can take.
    (
        "data-exit",
        "tv | monitor | phone, on the root (§ 6.1, § 6.6)",
    ),
    (
        "data-inputs",
        "the profile's input words, on the root (§ 6.4)",
    ),
    ("data-dock", "shown | hidden, out of `dock_default` (§ 6.1)"),
    (
        "data-dock-open",
        "the one browser state with meaning (§ 3.2, § 5.5)",
    ),
    ("data-region", "the place on the canvas a view was sent to"),
    ("data-ground", "the operator's ground: day | night"),
    // Words an app or a component writes about its own content.
    ("data-view", "the view a wrapper belongs to"),
    ("data-topic", "what a window is about (§ 2)"),
    (
        "data-role",
        "who speaks a chat line; the mark's live region",
    ),
    ("data-phase", "the mark's phase (§ 6.8)"),
    ("data-size", "a value component's size"),
    ("data-kind", "a status or list component's kind"),
    ("data-ratio", "a media frame's aspect"),
    ("data-gap", "a stack's gap"),
    ("data-tone", "a window's tone"),
    ("data-position", "an overlay's corner"),
    (
        "data-page-state",
        "a page's own state: loading | ready | error | suspended (§ 7.9). \
         NOT `data-state` -- that word is the curator's and is struck",
    ),
    (
        "data-step-state",
        "a step's own state: done | running | todo | failed | blocked (GH #958). \
         NOT `data-state` -- that word is the curator's and is struck",
    ),
    // The client's own press ring, for the length of one movement (§ 5.7).
    ("data-zoomed", "the pressed tile, drawn by the scene hook"),
    // Not a selector: `attr(data-digits)` is the text of the timer's warm layer,
    // which repeats the digits and fades in by opacity instead of repainting the
    // colour of the digits every frame (GH #1209). The template writes it beside
    // the digits, the scene hook's tick rewrites both together.
    (
        "data-digits",
        "the timer's remaining digits, repeated for its alarm layer (GH #1209)",
    ),
    // The hook's word about its OWN channel, and the reason it puts beside the
    // address. `data-page-state` above says what the CELL is doing and belongs
    // to the curator; two writers on that one attribute made a sleeping page
    // look like a waking one (OR-G.g18.1).
    (
        "data-page-link",
        "up | down: whether the page's channel stands, drawn by the scene hook \
         (§ 7.9)",
    ),
    (
        "data-keys",
        "the page whose hidden field holds the focus, drawn by the scene hook (§ 7.9)",
    ),
    // The screen's OWN link, not screen state: the curator never writes it and
    // § 3.2 keeps meaning out of the browser. The scene hook sets it while
    // LiveView reports the channel gone, because a tap there is neither sent
    // nor drawn (K7, welle-testing tv-bn M10b) and the tiles must say so.
    (
        "data-offline",
        "true while the screen's own LiveView channel is gone, drawn by the scene hook: \
         no input is sent, so everything that takes a finger or a key loses its colour \
         (K7, OR-T10-10)",
    ),
    // A notification says its own class; `data-level` is the drawing level of
    // a WINDOW since 2.5.0, and one word says one thing (§ 2).
    ("data-notice", "the class of a notification (§ 7.7)"),
];

/// The words § 2 struck. None of them may stand in a selector of this sheet.
/// `data-profile` is not struck as a word -- a profile is the property list of
/// an output (§ 2) -- but it is not the name of the thing a selector asks
/// about either: the sheet asks which OUTPUT it draws for, and that is `exit`.
const STRUCK: &[(&str, &str)] = &[
    (
        "data-plane",
        "`plane` per output is struck (§ 2, § 4.24: `level`)",
    ),
    (
        "data-state",
        "`state` as a curator word is struck (§ 2: `rung`)",
    ),
    (
        "data-modal",
        "the hint `modal: true` is struck (§ 2: `layer`)",
    ),
    ("data-on-canvas", "a tile says `open` (§ 6.10)"),
    (
        "data-screen",
        "`screen` is the whole screen, not one output (§ 2)",
    ),
    (
        "data-profile",
        "the root names its exit, not its property list (§ 6.1)",
    ),
];

#[test]
fn no_struck_name_stands_in_a_selector_of_the_sheet() {
    if !library_ships() {
        eprintln!("skip: no template library in this tree");
        return;
    }
    let sheet = without_comments(&read(SHEET));
    for (name, why) in STRUCK {
        assert!(
            !data_attributes(&sheet).iter().any(|n| n == name),
            "{SHEET} still selects on `{name}`: {why}"
        );
    }
}

#[test]
fn every_attribute_the_sheet_reads_is_on_the_list() {
    if !library_ships() {
        eprintln!("skip: no template library in this tree");
        return;
    }
    let sheet = without_comments(&read(SHEET));
    for name in data_attributes(&sheet) {
        assert!(
            ALLOWED.iter().any(|(n, _)| *n == name),
            "{SHEET} selects on `{name}`, which is on no contract: \
             add it to ALLOWED with the sentence it serves, or stop reading it"
        );
    }
}

#[test]
fn the_sheet_reads_the_words_the_curator_writes() {
    if !library_ships() {
        eprintln!("skip: no template library in this tree");
        return;
    }
    let sheet = without_comments(&read(SHEET));
    let names = data_attributes(&sheet);
    // The four the rename brings in. A sheet that carried none of them would
    // pass the two tests above by saying nothing at all.
    for name in ["data-rung", "data-level", "data-open", "data-exit"] {
        assert!(
            names.iter().any(|n| n == name),
            "{SHEET} reads nothing about `{name}` -- the contract of plan § 4 \
             says the curator writes it on every window, tile or root"
        );
    }
}

/// One `NAME = (` … `\n)` block of the curator, as text.
fn template(src: &str, name: &str) -> String {
    let open = format!("{name} = (");
    let start = src.find(&open).unwrap_or_else(|| panic!("{name} is gone"));
    let end = src[start..]
        .find("\n)")
        .unwrap_or_else(|| panic!("{name} is not one parenthesised block"));
    src[start..start + end].to_string()
}

#[test]
fn the_templates_write_what_the_sheet_reads() {
    if !library_ships() {
        eprintln!("skip: no template library in this tree");
        return;
    }
    let compose = read(COMPOSE);
    // The curator strand owns the templates (plan § 6, H1-T6). Until its
    // `ATTRS` is in this tree the sheet is ahead of them by design, and this
    // half has nothing to judge; after the merge it is the coupling lock.
    if !compose.contains("ATTRS = {") {
        eprintln!("skip: the curator's ATTRS is not in this tree yet (H1)");
        return;
    }
    for name in [
        "PANE_TEMPLATE",
        "PANEL_TEMPLATE",
        "OVERLAY_TEMPLATE",
        "PROSE_TEMPLATE",
    ] {
        let t = template(&compose, name);
        for attr in ["data-rung=\"{{", "data-level=\"{{"] {
            assert!(t.contains(attr), "{name} writes no `{attr}`");
        }
        for attr in ["data-plane=", "data-state=", "data-modal="] {
            assert!(
                !t.contains(attr),
                "{name} still writes `{attr}` (§ 2, struck)"
            );
        }
    }
    let tile = template(&compose, "TILE_TEMPLATE");
    assert!(
        tile.contains("data-rung=\"{{"),
        "TILE_TEMPLATE writes no `data-rung`"
    );
    assert!(
        tile.contains("data-open=\"{{"),
        "TILE_TEMPLATE writes no `data-open`"
    );
    assert!(
        !tile.contains("data-on-canvas="),
        "TILE_TEMPLATE still writes `data-on-canvas` (§ 6.10: `open`)"
    );
    let shell = template(&compose, "SHELL_TEMPLATE");
    assert!(
        shell.contains("data-exit=\"{{"),
        "SHELL_TEMPLATE writes no `data-exit`"
    );
    assert!(
        !shell.contains("data-screen="),
        "SHELL_TEMPLATE still writes `data-screen` (§ 2: one screen, many exits)"
    );
}

/// welle-tv tv-bn-layout A (re-check A-M1): the pass that answered a tap says so on
/// the windows it acted on. `data-acted` grows on every touch of a window -- an app
/// write, a co-close, step 5 -- so a driver waiting for "a bigger stamp" returned on
/// the pass of an EARLIER tap: B-07 green before its tenth tap was answered, the
/// co-close judged the modal without its stamp. A tap may carry `phx-value-seq`; the
/// pass renders it back as `data-tap-seq` on the tapped window and on every modal it
/// co-closed (§ 5.3), never on a tap it refused, and never into the rest row. Test
/// observability like `ringed`: absent unless a tap carried one.
#[test]
fn the_pass_renders_the_seq_of_the_tap_it_answered() {
    if !library_ships() {
        eprintln!("SKIP: the display library is not in this tree");
        return;
    }
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             m.read_knobs({})\n\
             st = m.empty_state({'default_screen': 'tv'}, m.DEFAULT_SCREENS)\n\
             w = lambda st, oid, h, now: m.run_pass(st, {'kind': 'app_write', 'oid': oid, 'view': h}, now)\n\
             st = w(st, 'view.a.canvas', {'view_id': 'canvas', 'region': 'main', 'layer': 'canvas'}, 1000)\n\
             st = w(st, 'view.a.chat', {'view_id': 'chat', 'region': 'main', 'topic': 'chat', 'layer': 'modal'}, 2000)\n\
             plain = m.window_attrs(st['views']['view.a.chat'], 2000)\n\
             ev = m.tap_event({'for': 'view.a.chat', 'seq': '7'}, 'view.a.chat')\n\
             st = m.run_pass(st, ev, 3000); m.stamp_tap_seq(st, ev)\n\
             level = st['views']['view.a.chat']['curator'].get('level')\n\
             ev = m.tap_event({'for': 'view.a.canvas', 'seq': '9'}, 'view.a.canvas')\n\
             st = m.run_pass(st, ev, 4000); m.stamp_tap_seq(st, ev)\n\
             seqs = {o: m.window_attrs(v, 4000).get('tapseq') for o, v in st['views'].items()}\n\
             ev = m.tap_event({'for': 'view.a.absent', 'seq': '11'}, 'view.a.absent')\n\
             st = m.run_pass(st, ev, 5000); m.stamp_tap_seq(st, ev)\n\
             after = {o: m.window_attrs(v, 5000).get('tapseq') for o, v in st['views'].items()}\n\
             st = w(st, 'view.b.chat', {'view_id': 'chat', 'region': 'main', 'topic': 'chat', 'layer': 'modal'}, 5500)\n\
             ev = m.tap_event({'for': 'view.b.chat', 'seq': '13'}, 'view.b.chat')\n\
             st = m.run_pass(st, ev, 6000); m.stamp_tap_seq(st, ev)\n\
             b = st['views'].get('view.b.chat')\n\
             held = {'held': b is not None, 'present': bool(b and b['curator'].get('present')),\n\
                     'refused': ['view.b.chat', 'tap', 'no tile'] in [list(r) for r in st['pass'].get('refused', [])],\n\
                     'seq': b and m.window_attrs(b, 6000).get('tapseq')}\n\
             want = {}\n\
             m.add_tree(want, 'view.a.app', {'component': 'display-pane', 'props': {'pane_id': 'p', 'tapseq': '42'}}, 0, {}, attrs={}, region='main')\n\
             app = any('tapseq' in (n.get('props') or {}) for n in want.values())\n\
             print(json.dumps({'plain': 'tapseq' in plain, 'level': level, 'seqs': seqs, 'app': app,\n\
                               'refused': after, 'held': held, 'junk': m.tap_event({'seq': 'x'}, 'view.a.chat'),\n\
                               'rest': 'tap_seq' in json.dumps(m.rest_content(st, []))}))",
        )
        .arg(repo(COMPOSE))
        .output();
    let Ok(out) = out else {
        eprintln!("SKIP: no python3 on this host");
        return;
    };
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(said.trim()).expect("one JSON line");
    assert_eq!(
        v["plain"], false,
        "a window nobody tapped with a seq wears one: {v}"
    );
    assert_eq!(v["level"], 2, "the chat did not open as a modal: {v}");
    assert_eq!(
        v["seqs"]["view.a.canvas"], "9",
        "the tapped window does not carry its tap's seq: {v}"
    );
    assert_eq!(
        v["seqs"]["view.a.chat"], "9",
        "the modal the tap co-closed does not carry the tap's seq (§ 5.3): {v}"
    );
    assert_eq!(
        v["refused"], v["seqs"],
        "a tap the pass refused (no tile) moved a window's seq: {v}"
    );
    // A refused tap on a window the state HOLDS (re-check A fix1 H2): a second app's
    // chat is a topic duplicate, held but not present, so its tile is gone and the pass
    // refuses the tap -- the `refused` branch of `stamp_tap_seq`, which the absent
    // window above never reached (`views.get(o) is None` answered for it).
    assert!(
        v["held"]["held"] == true && v["held"]["present"] == false && v["held"]["refused"] == true,
        "the duplicate chat is no refused tap on a window the state holds: {v}"
    );
    assert!(
        v["held"]["seq"].is_null(),
        "a tap the pass refused on a window it holds moved that window's seq: {v}"
    );
    assert!(
        v["junk"].get("seq").is_none(),
        "a seq that is no number rode along: {v}"
    );
    assert_eq!(v["rest"], false, "the seq reached the rest row: {v}");
    assert_eq!(
        v["app"], false,
        "an application's own `tapseq` reaches the window: the seq is the screen's answer to a tap, no word of the contract: {v}"
    );
    for name in [
        "PANE_TEMPLATE",
        "PANEL_TEMPLATE",
        "OVERLAY_TEMPLATE",
        "PROSE_TEMPLATE",
    ] {
        assert!(
            template(&read(COMPOSE), name)
                .contains("{{#if tapseq}} data-tap-seq=\"{{tapseq}}\"{{/if}}"),
            "{name} does not render the seq back"
        );
    }
}
