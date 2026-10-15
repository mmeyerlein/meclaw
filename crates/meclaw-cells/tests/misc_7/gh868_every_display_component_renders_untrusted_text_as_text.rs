//! GH #868 / #869 -- every display component renders untrusted text as text.
//!
//! What a model says reaches the screen as props, and until this lock the only
//! thing between a model's sentence and the page's markup was whether the
//! application that wrote the view escaped it. Escaping keeps a value inside
//! its quotes; it does not decide what the value means where it lands -- a
//! JSON list in a `phx-click` is a program to the LiveView client, a
//! `javascript:` in a `src` is a scheme, `0;background:url(//x)` in a `style`
//! is a second declaration. So every component of `components()` (new ones
//! join by themselves) is fed hostile text in every prop its schema declares,
//! and rendered the way the `web` cell renders one object:
//!
//! - `text`, `int`, `boolean` props directly;
//! - `html` props the application writes through `sanitize_markup`, the
//!   display's own allowlist; `html` props the screen writes (`faces`, the two
//!   `client_js`) are no application's to fill, so they are pinned, not fed.
//!
//! Measured per component, prop and payload:
//!
//! 1. the sequence of element and attribute names equals the render with a
//!    harmless `x` -- for an `html` prop, outside the elements its allowlist
//!    admits;
//! 2. no `<script>` element, no `on…=` attribute and no `javascript:` URL that
//!    the template does not carry itself;
//! 3. every LiveView binding holds an event name, every URL attribute a
//!    relative URL or an `http(s)`/`mailto`/`tel` one, and every `style` value
//!    gained no character a CSS value may not carry;
//! 4. the set of `html` props is pinned WITH its writer -- a new one is red
//!    until it says who writes it;
//! 5. clean tables, bar and line charts and links go through `sanitize_markup`
//!    byte for byte, because a sanitiser that fills a chart's line is a broken
//!    chart that no hostile-vs-`x` comparison would ever see.
//!
//! Plus the door: an application's own component with `<script>`, an
//! event-handler attribute, a `javascript:` URL or an `html` prop is
//! `invalid_view`, and a raw prop the screen writes does not survive an
//! application's tree.
//!
//! The grammars in 3 are written out here rather than borrowed from the cell:
//! this file is the oracle the cell is measured against. Skips when `python3`
//! is absent or the templates do not ship (R2b).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_cells::web::render::render_pieces_plain;
use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
/// The one application in the tree that brings its own code to the screen.
const COLONY_VIEW: &str = "templates/colony-view/layout/layout.py";

/// Every `html` prop of the catalogue and who writes it: `screen` is the
/// display's own, `app` is filled by an application and passes the allowlist.
const PINNED: [(&str, &str, &str); 8] = [
    ("display-shell", "faces", "screen"),
    ("display-shell", "client_js", "screen"),
    ("display-os", "client_js", "screen"),
    ("display-table", "head", "app"),
    ("display-table", "rows", "app"),
    ("display-media", "figure", "app"),
    ("display-chart", "figure", "app"),
    ("display-document", "body", "app"),
];

/// The hostile text, one payload per way in.
fn payloads() -> Vec<String> {
    vec![
        r#""'><script>window.__pwned=1</script>"#.to_string(),
        "<img src=x onerror=window.__pwned=1>".to_string(),
        r#""><svg onload=window.__pwned=1>"#.to_string(),
        "javascript:window.__pwned=1".to_string(),
        r#"[["exec",{"attr":"onclick"}]]"#.to_string(),
        "[x](javascript:window.__pwned=1)".to_string(),
        "{{&x}}{{children}}".to_string(),
        "0;background:url(//x)".to_string(),
        r#"x" style="background:url(//x)"#.to_string(),
        "\u{202e}x".to_string(),
        "a\u{0}b".to_string(),
        format!("{}\"><script>window.__pwned=1</script>", "x".repeat(4096)),
    ]
}

/// Clean markup as the applications write it: must pass byte for byte.
fn fixtures() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "bar chart",
            "figure",
            concat!(
                r#"<svg viewBox="0 0 320 120" role="img">"#,
                r#"<line stroke="currentColor" stroke-opacity="0.25" stroke-width="1" x1="0" y1="96.00" x2="320" y2="96.00"/>"#,
                r#"<rect fill="currentColor" x="12.40" y="24.00" width="99.20" height="72.00"/>"#,
                r#"<rect fill="currentColor" x="119.07" y="96.00" width="99.20" height="24.00"/></svg>"#
            ),
        ),
        (
            "line chart",
            "figure",
            concat!(
                r#"<svg viewBox="0 0 320 120" role="img">"#,
                r#"<polyline fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round""#,
                r#" stroke-linecap="round" vector-effect="non-scaling-stroke""#,
                r#" points="0.00,120.00 160.00,60.00 320.00,0.00"/></svg>"#
            ),
        ),
        (
            "table head",
            "head",
            r#"<tr><th>Name</th><th>&quot;A&quot; &amp; B</th></tr>"#,
        ),
        (
            "table rows",
            "rows",
            r#"<tr><td>1</td><td>it&#39;s &lt;fine&gt;</td></tr><tr><td colspan="2">2</td></tr>"#,
        ),
        (
            "link",
            "figure",
            r#"<a href="https://example.org/?a=1&amp;b=2" target="_blank" rel="noopener noreferrer">open in browser</a>"#,
        ),
        (
            "document",
            "body",
            r#"<h3>Title</h3><p>One <strong>two</strong> <em>three</em><br>four</p><ul><li><code>x</code></li></ul>"#,
        ),
    ]
}

/// What the script is asked, all at once: `components()`, the table of raw
/// props, the allowlists, every payload and fixture through the sanitiser, the
/// door on a list of application components, and `add_tree` on a list of
/// nodes. What the script does not have (yet) comes back as `null`.
const HELPER: &str = r#"
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location('compose', sys.argv[1])
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
ask = json.load(sys.stdin)
sanitize = getattr(m, 'sanitize_markup', None)
raw = getattr(m, 'RAW_PROPS', None)
allowed = getattr(m, 'RAW_MARKUP', None)
out = {
    'components': m.components(),
    'raw_props': None if raw is None else sorted([c, p, w] for (c, p), w in raw.items()),
    'allowed': None if allowed is None else dict((p, sorted(v)) for p, v in allowed.items()),
    'sanitized': None if sanitize is None else dict(
        (p, [sanitize(p, x) for x in ask['payloads']]) for p in ask['props']),
    'fixtures': None if sanitize is None else [sanitize(p, x) for p, x in ask['fixtures']],
    'door': [list(m.check_components([c], v)[1:]) for c, v in ask['door']],
    'trees': [],
}
for node in ask['trees']:
    want = {}
    m.add_tree(want, 'w', node, 0)
    out['trees'].append(want['w/0']['props'])
cv = sys.argv[2]
if cv:
    spec = importlib.util.spec_from_file_location('layout', cv)
    lay = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(lay)
    out['colony_view'] = list(m.check_components(lay.components(), lay.VIEW_ID)[1:])
print(json.dumps(out))
"#;

/// The script's answer. `None` when there is no `python3` on this host.
fn ask(question: &Value) -> Option<Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(HELPER)
        .arg(repo(COMPOSE))
        .arg(if repo(COLONY_VIEW).is_file() {
            repo(COLONY_VIEW)
        } else {
            std::path::PathBuf::new()
        })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(question.to_string().as_bytes())
        .expect("the question reaches python");
    let out = child.wait_with_output().expect("python ends");
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(meclaw_core::serde_json::from_slice(&out.stdout).expect("the answer is JSON"))
}

// --- a small tokenizer: the element and attribute structure of a render ---

/// One tag of a render. Text is not structure and is not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
    },
    End(String),
}

/// `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#39;` and numeric references decoded:
/// an attribute value as the browser reads it.
fn decode(v: &str) -> String {
    let mut out = String::new();
    let mut rest = v;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(semi) = tail.find(';').filter(|s| *s <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..semi];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name.strip_prefix('#').and_then(|n| {
                let code = if let Some(h) = n.strip_prefix(['x', 'X']) {
                    u32::from_str_radix(h, 16).ok()
                } else {
                    n.parse::<u32>().ok()
                };
                code.and_then(char::from_u32)
            }),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn tokens(html: &str) -> Vec<Tok> {
    let b: Vec<char> = html.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}');
    while i < b.len() {
        if b[i] != '<' {
            i += 1;
            continue;
        }
        // Comment.
        if b[i..].starts_with(&['<', '!', '-', '-']) {
            let s: String = b[i..].iter().collect();
            i += s
                .find("-->")
                .map_or(b.len() - i, |e| s[..e].chars().count() + 3);
            continue;
        }
        let end = i + 1 < b.len() && b[i + 1] == '/';
        let mut j = i + 1 + usize::from(end);
        if j >= b.len() || !b[j].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let mut name = String::new();
        while j < b.len() && !ws(b[j]) && b[j] != '/' && b[j] != '>' {
            name.push(b[j].to_ascii_lowercase());
            j += 1;
        }
        let mut attrs = Vec::new();
        loop {
            while j < b.len() && (ws(b[j]) || b[j] == '/') {
                j += 1;
            }
            if j >= b.len() || b[j] == '>' {
                j += 1;
                break;
            }
            let mut an = String::new();
            while j < b.len() && !ws(b[j]) && !matches!(b[j], '=' | '>' | '/') {
                an.push(b[j].to_ascii_lowercase());
                j += 1;
            }
            while j < b.len() && ws(b[j]) {
                j += 1;
            }
            let mut av = String::new();
            if j < b.len() && b[j] == '=' {
                j += 1;
                while j < b.len() && ws(b[j]) {
                    j += 1;
                }
                if j < b.len() && (b[j] == '"' || b[j] == '\'') {
                    let q = b[j];
                    j += 1;
                    while j < b.len() && b[j] != q {
                        av.push(b[j]);
                        j += 1;
                    }
                    j += 1;
                } else {
                    while j < b.len() && !ws(b[j]) && b[j] != '>' {
                        av.push(b[j]);
                        j += 1;
                    }
                }
            }
            attrs.push((an, decode(&av)));
        }
        i = j;
        if end {
            out.push(Tok::End(name));
        } else {
            let raw = name == "script" || name == "style";
            out.push(Tok::Start {
                name: name.clone(),
                attrs,
            });
            if raw {
                let s: String = b[i..].iter().collect::<String>().to_ascii_lowercase();
                let close = format!("</{name}");
                i += s
                    .find(&close)
                    .map_or(b.len() - i, |e| s[..e].chars().count());
            }
        }
    }
    out
}

/// The structure as a list of names, without the elements in `skip`.
fn shape(toks: &[Tok], skip: &BTreeSet<String>) -> Vec<String> {
    toks.iter()
        .filter_map(|t| match t {
            Tok::Start { name, attrs } if !skip.contains(name) => Some(format!(
                "<{name}{}>",
                attrs
                    .iter()
                    .map(|(a, _)| format!(" {a}"))
                    .collect::<String>()
            )),
            Tok::End(name) if !skip.contains(name) => Some(format!("</{name}>")),
            _ => None,
        })
        .collect()
}

// --- the grammars, written out as the oracle ---

fn is_binding(a: &str) -> bool {
    [
        "phx-click",
        "phx-click-away",
        "phx-submit",
        "phx-change",
        "phx-blur",
        "phx-focus",
        "phx-keydown",
        "phx-keyup",
        "phx-mounted",
        "phx-remove",
        "phx-connected",
        "phx-disconnected",
        "phx-viewport-top",
        "phx-viewport-bottom",
    ]
    .contains(&a)
        || a.starts_with("phx-window-")
}

fn is_url_attr(a: &str) -> bool {
    [
        "href",
        "src",
        "action",
        "formaction",
        "poster",
        "xlink:href",
        "data",
    ]
    .contains(&a)
}

fn event_name_ok(v: &str) -> bool {
    v.len() <= 64
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.:/-".contains(c))
}

fn url_ok(v: &str) -> bool {
    if v.chars().any(char::is_control) {
        return false;
    }
    let v = v.trim_matches(' ');
    match v.find([':', '/', '?', '#']) {
        Some(at) if v.as_bytes()[at] == b':' => {
            ["http", "https", "mailto", "tel"].contains(&v[..at].to_ascii_lowercase().as_str())
        }
        _ => true,
    }
}

/// A `style` value with every character a CSS value may carry removed: what
/// is left is the template's own punctuation, and a hostile value must not
/// add to it.
fn style_skeleton(v: &str) -> String {
    v.chars()
        .filter(|c| !(c.is_ascii_alphanumeric() || " _.,%#+-".contains(*c)))
        .collect()
}

fn script_url(v: &str) -> bool {
    v.chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect::<String>()
        .trim()
        .to_ascii_lowercase()
        .starts_with("javascript:")
}

fn attrs_of(toks: &[Tok]) -> Vec<(String, String, String)> {
    toks.iter()
        .filter_map(|t| match t {
            Tok::Start { name, attrs } => Some(
                attrs
                    .iter()
                    .map(|(a, v)| (name.clone(), a.clone(), v.clone()))
                    .collect::<Vec<_>>(),
            ),
            Tok::End(_) => None,
        })
        .flatten()
        .collect()
}

/// Every way `hostile` differs from `benign` that this lock forbids.
fn findings(benign: &str, hostile: &str, skip: &BTreeSet<String>) -> Vec<String> {
    let (bt, ht) = (tokens(benign), tokens(hostile));
    let mut out = Vec::new();
    // 1. structure
    if shape(&bt, skip) != shape(&ht, skip) {
        out.push("structure".to_string());
    }
    // 2. script, handlers, script URLs the template does not carry
    let scripts = |t: &[Tok]| {
        t.iter()
            .filter(|x| matches!(x, Tok::Start { name, .. } if name == "script"))
            .count()
    };
    if scripts(&ht) > scripts(&bt) {
        out.push("script".to_string());
    }
    let (ba, ha) = (attrs_of(&bt), attrs_of(&ht));
    let benign_names: BTreeSet<&str> = ba.iter().map(|(_, a, _)| a.as_str()).collect();
    for (_, a, v) in &ha {
        if a.starts_with("on") && !benign_names.contains(a.as_str()) {
            out.push(format!("handler {a}"));
        }
        if is_url_attr(a) && script_url(v) {
            out.push(format!("script-url {a}"));
        }
        // 3. grammars
        if is_binding(a) && !event_name_ok(v) {
            out.push(format!("binding {a}={v:?}"));
        }
        if is_url_attr(a) && !url_ok(v) {
            out.push(format!("url {a}={v:?}"));
        }
    }
    let styles = |a: &[(String, String, String)]| {
        a.iter()
            .filter(|(_, n, _)| n == "style")
            .map(|(_, _, v)| style_skeleton(v))
            .collect::<Vec<_>>()
    };
    if styles(&ba) != styles(&ha) {
        out.push("style".to_string());
    }
    out
}

fn render(template: &str, props: &Value, schema: &Value) -> String {
    render_pieces_plain(template, props, schema)
        .unwrap_or_else(|e| panic!("the web cell would refuse this template: {e}"))
}

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The components the door is asked about, with the view that brings them and
/// whether they pass. A view in `code_views` (shipped: `colony-view`) may bring
/// script, style and `html` props, and nothing else beyond the rule.
fn door_cases() -> Vec<(Value, &'static str, bool)> {
    let c = |template: &str, schema: Value| json!({"name": "v-card", "template": template, "prop_schema": schema});
    let own = |template: &str, schema: Value| json!({"name": "colony-view-card", "template": template, "prop_schema": schema});
    let mut cases: Vec<(Value, &'static str, bool)> = vec![
        (
            own(
                "<style>{{&css}}</style><script>{{&js}}</script>",
                json!({"css": "html", "js": "html"}),
            ),
            "colony-view",
            true,
        ),
        (
            own(r#"<iframe src="{{t}}"></iframe>"#, json!({"t": "text"})),
            "colony-view",
            false,
        ),
        // Review I2 point 4: in svg a style is markup, and `<img>` breaks out
        // of the svg as HTML with its handler.
        (
            own(
                r#"<svg><style><img src=x on{{x}}error=window.__pwned=1></style></svg>"#,
                json!({"x": "text"}),
            ),
            "colony-view",
            false,
        ),
        (
            own("<svg><style>{{&css}}</style></svg>", json!({"css": "html"})),
            "colony-view",
            false,
        ),
        // The same style anywhere: the view cannot know it lands in no svg.
        (
            own(
                r#"<style><img src=x on{{x}}error=window.__pwned=1></style>"#,
                json!({"x": "text"}),
            ),
            "colony-view",
            false,
        ),
        // The display's own shape: an svg that is closed, then a script.
        (
            own(
                r#"<svg viewBox="0 0 1 1"><path d="M0 0"/></svg><script>{{&js}}</script>"#,
                json!({"js": "html"}),
            ),
            "colony-view",
            true,
        ),
        (
            own(r#"<a onclick="x()">{{t}}</a>"#, json!({"t": "text"})),
            "colony-view",
            false,
        ),
        // Review M3/M8: SVG animation sets `href` from a value no rule reads
        // as a URL -- refused for a view with its own code as well.
        (
            own(
                r#"<svg><a><set attributeName="href" to="java{{x}}script:alert(1)"/><text>t</text></a></svg>"#,
                json!({"x": "text"}),
            ),
            "colony-view",
            false,
        ),
        // Review M6/M9: `<!--` in a script leaves the browser in escaped
        // script data, and a sibling template after it is read as script.
        (
            own("<script><!--<script></script>-->", json!({})),
            "colony-view",
            false,
        ),
    ];
    cases.extend(vec![
        (
            c(
                r#"<b data-front="{{t}}" data-tone="x" data-cond="y" aria-controls="z" phx-click="{{t}}">{{t}}</b>"#,
                json!({"t": "text"}),
            ),
            true,
        ),
        (c("<p>{{t}}</p><script>window.__pwned=1</script>", json!({"t": "text"})), false),
        (c("<p>{{&t}}</p>", json!({"t": "html"})), false),
        (c(r#"<a onclick="x()">{{t}}</a>"#, json!({"t": "text"})), false),
        (c(r#"<svg onload=x()></svg>"#, json!({})), false),
        (c(r#"<a href="javascript:x()">{{t}}</a>"#, json!({"t": "text"})), false),
        (c(r#"<iframe src="{{t}}"></iframe>"#, json!({"t": "text"})), false),
        (c(r#"<style>*{}</style>"#, json!({})), false),
        (c(r#"<base href="/x/">"#, json!({})), false),
        // Review I1: a value standing inside a name joins the static text into
        // the name the rule refuses -- empty, `<scr{{x}}ipt>` is `<script>`.
        (
            c("<scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>", json!({"x": "text"})),
            false,
        ),
        (
            c(r#"<a on{{x}}click="window.__pwned=1">t</a>"#, json!({"x": "text"})),
            false,
        ),
        (
            c(r#"<a href="java{{x}}script:window.__pwned=1">t</a>"#, json!({"x": "text"})),
            false,
        ),
        (
            c(r#"<ifr{{x}}ame src="https://example.org"></iframe>"#, json!({"x": "text"})),
            false,
        ),
        (
            c(r#"<a h{{n}}="{{v}}">t</a>"#, json!({"n": "text", "v": "text"})),
            false,
        ),
        // Review I2: where the scanner read a quoted value, a browser already
        // read markup -- a text element ends at its own end tag, `</` before a
        // non-letter is a bogus comment, `<!--!>` ends no comment.
        (
            c(
                r#"<textarea><b title="</textarea><scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>"></b></textarea>"#,
                json!({"x": "text"}),
            ),
            false,
        ),
        (
            c(
                r#"<noscript><b title="</noscript><img src=x on{{x}}error=window.__pwned=1>"></b></noscript>"#,
                json!({"x": "text"}),
            ),
            false,
        ),
        (
            c(
                r#"</ a="><scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>">"#,
                json!({"x": "text"}),
            ),
            false,
        ),
        (
            c(
                r#"<!--!><a title="--><scr{{x}}ipt>1</scr{{x}}ipt>">"#,
                json!({"x": "text"}),
            ),
            false,
        ),
        (
            c(
                r#"<!---!><a title="--><scr{{x}}ipt>1</scr{{x}}ipt>">"#,
                json!({"x": "text"}),
            ),
            false,
        ),
        // A text element inside an svg of the page is no text element.
        (
            c(
                r#"<textarea><img src=x on{{x}}error=window.__pwned=1></textarea>"#,
                json!({"x": "text"}),
            ),
            false,
        ),
        // CDATA ends at `]]>` in svg and at the first `>` elsewhere.
        (
            c(
                r#"<svg><![CDATA[ x > <b title="]]><scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>"></b></svg>"#,
                json!({"x": "text"}),
            ),
            false,
        ),
        (c("<plaintext>{{t}}", json!({"t": "text"})), false),
        // What follows a template is read on from where it ends: `<b title="`
        // and a sibling `" o{{x}}nclick=…` are a handler together.
        (c(r#"<b title=""#, json!({})), false),
        (
            c(
                "<textarea>{{t}}</textarea><title>{{t}}</title><!---->{{t}}</>{{t}}",
                json!({"t": "text"}),
            ),
            true,
        ),
        (
            c(
                r#"<svg><title>{{t}}</title><path d="M0 0"/></svg><!-- a --!>{{t}}"#,
                json!({"t": "text"}),
            ),
            true,
        ),
        // Review M3/M8: `to`, `from`, `by` and `values` set the attribute
        // `attributeName` names, so the animation elements are refused.
        (
            c(
                r#"<svg><a><set attributeName="href" to="java{{x}}script:alert(1)"/><text>t</text></a></svg>"#,
                json!({"x": "text"}),
            ),
            false,
        ),
        (
            c(
                r#"<svg><a><animate attributeName="href" values="{{v}}"/><text>t</text></a></svg>"#,
                json!({"v": "text"}),
            ),
            false,
        ),
        (
            c(
                r#"<svg><animateTransform attributeName="transform" by="{{b}}"/></svg>"#,
                json!({"b": "text"}),
            ),
            false,
        ),
        // Review M4: the handler rule reads the tokenizer's ASCII whitespace
        // and ASCII letters, as the `web` cell does. A vertical tab is no
        // space to a browser, `ſ` no `s`: neither is a handler attribute.
        (
            c("<b title=x\u{b}onclick=y>{{t}}</b>", json!({"t": "text"})),
            true,
        ),
        (
            c("<b title=x on\u{17f}tart=y>{{t}}</b>", json!({"t": "text"})),
            true,
        ),
    ]
    .into_iter()
    .map(|(v, ok)| (v, "v", ok)));
    cases
}

#[test]
fn every_display_component_renders_untrusted_text_as_text() {
    if !library_ships() {
        return;
    }
    let payloads = payloads();
    let fixtures = fixtures();
    let html_props: Vec<&str> = ["head", "rows", "figure", "body"].to_vec();
    let question = json!({
        "payloads": payloads,
        "props": html_props,
        "fixtures": fixtures.iter().map(|(_, p, x)| json!([p, x])).collect::<Vec<_>>(),
        "door": door_cases().into_iter().map(|(c, v, _)| json!([c, v])).collect::<Vec<_>>(),
        "trees": [
            {"component": "display-os", "props": {"mount": "voice", "client_js": "window.__pwned=1"}},
            {"component": "display-shell", "props": {"faces": "</style><script>window.__pwned=1</script>"}},
            {"component": "display-table", "props": {"rows": "<tr><td>1</td></tr><script>window.__pwned=1</script>"}},
            {"component": "display-chart", "props": {"figure": ["<script>window.__pwned=1</script>"]}},
        ],
    });
    let Some(answer) = ask(&question) else {
        return;
    };
    let all = answer["components"]
        .as_array()
        .expect("components()")
        .clone();
    let writer: BTreeMap<(&str, &str), &str> =
        PINNED.iter().map(|(c, p, w)| ((*c, *p), *w)).collect();
    let mut red: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut flag = |at: String, why: String| {
        red.entry(at).or_default().insert(why);
    };

    // 4. the raw props, pinned with their writer.
    let mut declared = BTreeSet::new();
    for c in &all {
        let name = c["name"].as_str().expect("a name");
        for (prop, ty) in c["prop_schema"].as_object().expect("a schema") {
            if ty == "html" {
                declared.insert((name.to_string(), prop.clone()));
                if !writer.contains_key(&(name, prop.as_str())) {
                    flag(
                        format!("{name}.{prop}"),
                        "html prop without a writer in PINNED".into(),
                    );
                }
            }
        }
    }
    for (c, p, _) in PINNED {
        if !declared.contains(&(c.to_string(), p.to_string())) {
            flag(
                format!("{c}.{p}"),
                "pinned but no longer an html prop".into(),
            );
        }
    }
    let pinned_rows: BTreeSet<String> = PINNED
        .iter()
        .map(|(c, p, w)| json!([c, p, w]).to_string())
        .collect();
    let shipped_rows: Option<BTreeSet<String>> = answer["raw_props"]
        .as_array()
        .map(|rows| rows.iter().map(Value::to_string).collect());
    if shipped_rows.as_ref() != Some(&pinned_rows) {
        flag(
            "RAW_PROPS".into(),
            format!(
                "compose.py's table disagrees with PINNED: {}",
                answer["raw_props"]
            ),
        );
    }

    // 1-3, every component, every prop, every payload.
    for c in &all {
        let name = c["name"].as_str().expect("a name");
        let template = c["template"].as_str().expect("a template");
        let schema = &c["prop_schema"];
        let mut base = json!({});
        for (prop, ty) in schema.as_object().expect("a schema") {
            base[prop] = match ty.as_str() {
                Some("int") => json!(1),
                Some("boolean") => json!(true),
                Some("html") if writer.get(&(name, prop.as_str())) == Some(&"screen") => json!(""),
                _ => json!("x"),
            };
        }
        let benign = render(template, &base, schema);
        for (prop, ty) in schema.as_object().expect("a schema") {
            let is_html = ty == "html";
            if is_html && writer.get(&(name, prop.as_str())) != Some(&"app") {
                continue;
            }
            let skip: BTreeSet<String> = if is_html {
                answer["allowed"][prop]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                BTreeSet::new()
            };
            for (i, payload) in payloads.iter().enumerate() {
                let value = if is_html {
                    answer["sanitized"][prop][i]
                        .as_str()
                        .map_or_else(|| payload.clone(), str::to_string)
                } else {
                    payload.clone()
                };
                let mut props = base.clone();
                props[prop] = json!(value);
                let hostile = render(template, &props, schema);
                // A raw prop the allowlist emptied switches its `{{#if}}` off,
                // and an absent `<thead>` is no injection: the reference is then
                // the component with that prop empty.
                let reference = if value.is_empty() {
                    let mut empty = base.clone();
                    empty[prop] = json!("");
                    render(template, &empty, schema)
                } else {
                    benign.clone()
                };
                for why in findings(&reference, &hostile, &skip) {
                    flag(format!("{name}.{prop}"), format!("{why} (payload {i})"));
                }
            }
        }
    }

    // 5. clean markup passes byte for byte.
    for (i, (label, _, markup)) in fixtures.iter().enumerate() {
        if answer["fixtures"][i].as_str() != Some(*markup) {
            flag(
                format!("sanitize_markup {label}"),
                format!("not byte-equal: {}", answer["fixtures"][i]),
            );
        }
    }

    // The door, and the guard on an application's tree.
    for (i, (_, _, accepted)) in door_cases().iter().enumerate() {
        let code = answer["door"][i][0].clone();
        let refused = code == json!("invalid_view");
        if refused == *accepted {
            flag(
                format!("check_components case {i}"),
                format!(
                    "expected {}, got {}",
                    if *accepted {
                        "accepted"
                    } else {
                        "invalid_view"
                    },
                    answer["door"][i]
                ),
            );
        }
    }
    if repo(COLONY_VIEW).is_file() && answer["colony_view"] != json!([null, null]) {
        flag(
            "check_components colony-view".into(),
            format!(
                "the shipped colony-view is refused: {}",
                answer["colony_view"]
            ),
        );
    }
    let trees = &answer["trees"];
    if trees[0].get("client_js").is_some() {
        flag(
            "add_tree display-os".into(),
            "an app wrote client_js".into(),
        );
    }
    if trees[1].get("faces").is_some() {
        flag("add_tree display-shell".into(), "an app wrote faces".into());
    }
    if trees[2]["rows"] != json!("<tr><td>1</td></tr>") {
        flag(
            "add_tree display-table".into(),
            format!("rows not sanitised: {}", trees[2]["rows"]),
        );
    }
    if trees[3].get("figure").is_some() {
        flag(
            "add_tree display-chart".into(),
            "a raw prop that is not a string survived".into(),
        );
    }

    assert!(
        red.is_empty(),
        "untrusted text is not text in {} place(s): {}\n{:#?}",
        red.len(),
        red.keys().cloned().collect::<Vec<_>>().join(", "),
        red
    );
}
