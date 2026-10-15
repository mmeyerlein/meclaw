//! GH #867: what a strict `script-src` sees on a served HTML page.
//!
//! A proxy in front of a colony may set `script-src 'self'` without `'unsafe-inline'`
//! or `'unsafe-hashes'`. Under it an inline script runs only when its sha256 is listed,
//! and an inline event handler or a `javascript:` URL never runs. Every lock that reads a
//! page the listener serves -- the pages the templates compose and the pages Rust serves
//! itself (the `web` shell, the voice test page, the operator UI under `/ui/`) -- reads it
//! with ONE reader, [`read_pages`], so a form one lock knows is a form every lock knows
//! (re-check tv-bn M9e, B-M1: two string searches beside the parser missed `<SCRIPT>`,
//! `on*` handlers, `javascript:` and `<script/>`).
//!
//! The reader reads every page twice and is red where the two readings differ -- in the
//! text of any inline script, not only in their number:
//!
//! * python's own HTML parser (`html.parser`) tells a `<script>` in a comment, a
//!   stylesheet or an attribute from a script element, hashes each inline script the way a
//!   browser does, and reports handlers, `javascript:` URLs (character references and URL
//!   whitespace resolved), `srcdoc`, self-closed `<script/>`, and the comment and CDATA
//!   forms a browser reads differently from python;
//! * a scan of the text reads every `<script` start tag without `src` with its text up to
//!   the next `</script`, and every `on…`, `srcdoc` and `javascript:` outside script text.
//!
//! Both halves read a `<style>` as a stylesheet, whose text is no markup, only where a
//! browser does: an element named exactly `style` in the HTML namespace with no raw-text,
//! svg, math or noscript element open. In svg and math, and under `<noscript>`, its text is
//! markup to both (OR-T10-18; re-check tv-bn M9h, M1: a style read as raw text everywhere
//! hid a `<script>` and an `onerror` a browser runs, from both halves alike).
//!
//! Each half alone was green on forms a browser runs (re-check tv-bn M9f, A-M1, probe of
//! 49 inputs): the parser on a CDATA section at an svg/math integration point and on a
//! `</noscript>` inside an attribute, the scan on `java&#9;script:`, on a `>` inside a
//! quoted attribute value before a handler and on `<<script>`. Together they read every
//! form of that table red, and a page with one gap of each half too: the script only one
//! half saw is a text the other half lacks (re-check tv-bn M9g, A-M1). A gap of the scan
//! that encloses one of the parser's -- a style the scan skips, holding a form the parser
//! misses -- is red the same way: both halves report the text of every stylesheet, and a
//! stylesheet only one half drew is a text the other lacks (re-check tv-bn M9n, M1).
//! `python3` is required: a host without it, or with a python
//! older than the one the form table is proven on (3.12), is red, never a skip -- the gate
//! that runs these locks is python itself.

use meclaw_core::serde_json::{self, Value, json};

/// What the reader read off one page.
#[derive(Debug, Clone, Default)]
pub struct PageRead {
    /// The text of every script element without `src`, line breaks normalised.
    pub scripts: Vec<String>,
    /// `'sha256-<base64>'` of each entry of `scripts`, as a CSP source expression.
    pub hashes: Vec<String>,
    /// Every inline handler, `javascript:` URL, `srcdoc`, and comment or CDATA form a
    /// browser would read as markup where python does not -- code no hash admits.
    pub inline_code: Vec<String>,
    /// Self-closed `<script/>` without `src`: a browser opens it.
    pub closed: u64,
    /// `<style>` elements.
    pub styles: u64,
    /// The text of every `<style>` read as a stylesheet (raw text, no markup), as it stands
    /// on the page. Both halves report it and must agree (re-check tv-bn M9n, M1).
    pub sheets: Vec<String>,
}

/// The interpreter every read runs.
const PYTHON: &str = "python3";

/// The parser, as python source. It reads a JSON list of pages on stdin and prints one
/// JSON object per page. `MIN` is the python the form table in the tests below is proven
/// on: `html.parser` has read comments and CDATA differently across versions (the
/// `<!-->` comment below), so an older one is refused rather than trusted (re-check tv-bn
/// M9f, A-M2).
const PAGE_SCRIPTS: &str = r#"
import base64, hashlib, json, re, sys
from html.parser import HTMLParser

MIN = (3, 12)
if sys.version_info < MIN:
    sys.exit('page_scripts needs python >= %d.%d, this is %s' % (MIN + (sys.version.split()[0],)))

C0 = ''.join(map(chr, range(0x21)))

# A `<style>` is raw text only in the HTML namespace with none of these open (OR-T10-18):
# in svg and math it is a foreign element whose text is markup, under `<noscript>` (a
# browser runs scripts) and under a raw-text or RCDATA element it is that element's text.
# The scan below reads it the same way.
FOREIGN = ('svg', 'math')
STYLE_AS_MARKUP = FOREIGN + ('noscript', 'textarea', 'title', 'xmp', 'iframe', 'noembed',
                             'noframes', 'plaintext')

def as_url(v):
    # A browser drops tab and line breaks from a URL and leading C0 and space, so
    # `java&#9;script:` is `javascript:` to it (re-check tv-bn M9e, A-H2).
    return re.sub(r'[\t\n\r]', '', v).lstrip(C0).lower()

class Page(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.scripts, self.inline_code, self.closed, self.styles = [], [], 0, 0
        self.text, self.open = None, dict.fromkeys(STYLE_AS_MARKUP, 0)
        self.sheets, self.sheet = [], None
    def attrs_of(self, tag, attrs):
        for k, v in attrs:
            v = v or ''
            if k.startswith('on') or k == 'srcdoc' or as_url(v).startswith('javascript:'):
                self.inline_code.append('<%s %s=%r>' % (tag, k, v[:60]))
    def handle_starttag(self, tag, attrs):
        self.attrs_of(tag, attrs)
        if tag in self.open:
            self.open[tag] += 1
        if tag == 'style':
            self.styles += 1
        if tag == 'script' and not any(k == 'src' for k, _ in attrs):
            self.text = []
    def handle_startendtag(self, tag, attrs):
        self.attrs_of(tag, attrs)
        if tag == 'style':
            self.styles += 1
        # `<svg/>` and `<math/>` stay shut; an HTML element ignores `/>` and opens.
        if tag in self.open and tag not in FOREIGN:
            self.open[tag] += 1
        if tag == 'script' and not any(k == 'src' for k, _ in attrs):
            self.closed += 1
    def handle_data(self, data):
        if self.text is not None:
            self.text.append(data)
        if self.sheet is not None:
            self.sheet.append(data)
    def handle_endtag(self, tag):
        if self.open.get(tag):
            self.open[tag] -= 1
        if tag == 'script' and self.text is not None:
            self.done()
        if tag == 'style' and self.sheet is not None:
            self.sheet_done()
    def handle_comment(self, data):
        # A browser ends `<!-->` and `<!--->` right there; python 3.12 reads on to the
        # next `-->` and hides whatever stands between.
        if data.startswith('>') or data.startswith('->'):
            self.inline_code.append('<!--%s' % data[:60])
    def unknown_decl(self, data):
        # Outside svg and math a browser reads `<![CDATA[` as a bogus comment up to the
        # first `>`, and what follows as markup.
        if not (self.open['svg'] or self.open['math']):
            self.inline_code.append('<![%s' % data[:60])
    def set_cdata_mode(self, elem, **kw):
        # python reads a `<style>` as raw text wherever it stands; a browser only where
        # none of STYLE_AS_MARKUP is open.
        if elem == 'style' and any(self.open.values()):
            return
        if elem == 'style':
            self.sheet = []
        super().set_cdata_mode(elem, **kw)
    def done(self):
        self.scripts.append(''.join(self.text).replace('\r\n', '\n').replace('\r', '\n'))
        self.text = None
    def sheet_done(self):
        self.sheets.append(''.join(self.sheet))
        self.sheet = None

out = []
for html in json.load(sys.stdin):
    p = Page()
    p.feed(html)
    p.close()
    if p.text is not None:
        p.done()
    if p.sheet is not None:
        # A stylesheet the page never ends: python holds its text back, a browser reads it
        # to the end of the page.
        p.sheet.append(p.rawdata)
        p.sheet_done()
    out.append({'scripts': p.scripts, 'inline_code': p.inline_code, 'closed': p.closed,
                'styles': p.styles, 'sheets': p.sheets,
                'hashes': ["'sha256-" + base64.b64encode(hashlib.sha256(t.encode('utf-8')).digest())
                           .decode() + "'" for t in p.scripts]})
print(json.dumps(out))
"#;

/// Read every page, parser and scan, and panic on a page the two read differently, on a
/// python that fails, is missing or is older than 3.12, or on output that is not one
/// object per page: a reader that cannot read is red, never empty.
pub fn read_pages(pages: &[String]) -> Vec<PageRead> {
    read_pages_with(PYTHON, pages)
}

/// [`read_pages`] with a named interpreter (the tests name one that is not there).
fn read_pages_with(python: &str, pages: &[String]) -> Vec<PageRead> {
    read_checked(python, pages)
        .into_iter()
        .zip(pages)
        .map(|(read, page)| {
            read.unwrap_or_else(|e| {
                panic!(
                    "{e} -- a page the two readers read differently is red; it starts: {:?}",
                    page.chars().take(200).collect::<String>()
                )
            })
        })
        .collect()
}

/// The lock for a page that may carry no inline code at all: [`read_pages`], and red on any
/// inline script, handler, `javascript:` URL or self-closed `<script/>`. Returns the read.
pub fn assert_no_inline_code(label: &str, html: &str) -> PageRead {
    let read = read_pages(&[html.to_string()]).remove(0);
    assert!(
        read.scripts.is_empty() && read.inline_code.is_empty() && read.closed == 0,
        "{label} carries inline code no `script-src 'self'` runs: scripts {:?}, handlers \
         and URLs {:?}, self-closed {}",
        read.scripts,
        read.inline_code,
        read.closed
    );
    read
}

/// Parser and scan per page: the parser's read where both read the same inline scripts --
/// the same texts, sorted, so the same hashes, and the same number self-closed -- the same
/// stylesheets, and both see code or none, else what each saw. Panics when the parser
/// cannot run.
///
/// The stylesheets, texts sorted like the scripts: a stylesheet's text is the one part of
/// a page the scan reads over unseen, so where the halves draw a different stylesheet --
/// one skips text the other reads as markup -- the page is red whatever that text holds.
/// Without it a gap of the parser inside a text the scan skipped read nothing on either
/// side (re-check tv-bn M9n, M1).
///
/// The texts, not their number: a gap of each half on one page (the parser misses one
/// script, the scan another) leaves the counts equal and the hash list short of the script
/// only one half saw (re-check tv-bn M9g, A-M1). Code is compared as present or absent
/// only; that is enough because every caller requires none (`inline_code` empty).
fn read_checked(python: &str, pages: &[String]) -> Vec<Result<PageRead, String>> {
    let parsed = run_parser(python, PAGE_SCRIPTS, pages).unwrap_or_else(|e| panic!("{e}"));
    parsed
        .into_iter()
        .zip(pages)
        .map(|(read, html)| {
            let scan = scan(html);
            let mut parser_texts = read.scripts.clone();
            parser_texts.sort();
            let mut scan_texts = scan.texts.clone();
            scan_texts.sort();
            let mut parser_sheets = read.sheets.clone();
            parser_sheets.sort();
            let mut scan_sheets = scan.sheets.clone();
            scan_sheets.sort();
            if scan_texts == parser_texts
                && scan_sheets == parser_sheets
                && u64::try_from(scan.closed).ok() == Some(read.closed)
                && scan.code.is_empty() == read.inline_code.is_empty()
            {
                Ok(read)
            } else {
                Err(format!(
                    "parser and scan disagree: the parser read the inline script(s) {:?}, {} \
                     self-closed, code {:?}, stylesheet(s) {:?}; the scan read the inline \
                     script(s) {:?}, {} self-closed, code {:?}, stylesheet(s) {:?}",
                    read.scripts,
                    read.closed,
                    read.inline_code,
                    read.sheets,
                    scan.texts,
                    scan.closed,
                    scan.code,
                    scan.sheets
                ))
            }
        })
        .collect()
}

/// Run the parser `script` under `python` over `pages`.
fn run_parser(python: &str, script: &str, pages: &[String]) -> Result<Vec<PageRead>, String> {
    use std::io::Write;
    let mut child = std::process::Command::new(python)
        .arg("-I")
        .arg("-c")
        .arg(script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "`{python}` does not start ({e}): every page lock needs it, a reader that \
                 cannot read is red"
            )
        })?;
    let input = json!(pages).to_string();
    let mut stdin = child.stdin.take().ok_or("the parser has no stdin")?;
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child
        .wait_with_output()
        .map_err(|e| format!("`{python}` did not end: {e}"))?;
    let written = writer.join().map_err(|_| "the stdin writer panicked")?;
    if !out.status.success() {
        return Err(format!(
            "the page parser failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    written.map_err(|e| format!("write the pages to the parser: {e}"))?;
    let read: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("the parser printed no JSON: {e}"))?;
    let read = read.as_array().ok_or("the parser printed no list")?;
    if read.len() != pages.len() {
        return Err(format!(
            "the parser read {} pages of {}: one object per page",
            read.len(),
            pages.len()
        ));
    }
    let strings = |v: &Value, key: &str| -> Result<Vec<String>, String> {
        v[key]
            .as_array()
            .ok_or_else(|| format!("no `{key}` in {v}"))?
            .iter()
            .map(|s| {
                s.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("`{key}` holds a non-string in {v}"))
            })
            .collect()
    };
    let count = |v: &Value, key: &str| v[key].as_u64().ok_or_else(|| format!("no `{key}` in {v}"));
    read.iter()
        .map(|v| {
            Ok(PageRead {
                scripts: strings(v, "scripts")?,
                hashes: strings(v, "hashes")?,
                inline_code: strings(v, "inline_code")?,
                closed: count(v, "closed")?,
                styles: count(v, "styles")?,
                sheets: strings(v, "sheets")?,
            })
        })
        .collect()
}

/// What the scan read off one page.
#[derive(Debug, Default)]
struct Scan {
    /// Every `<script` start tag (any case) without `src`, self-closed or not.
    scripts: Vec<String>,
    /// The text of each of those not self-closed, from the end of its start tag to the next
    /// `</script` (or the end of the page), line breaks normalised as the parser does.
    texts: Vec<String>,
    /// How many of them are self-closed (`<script/>`).
    closed: usize,
    /// Every attribute named `on…` or `srcdoc`, every self-closed `<script src=…/>`, and a
    /// `javascript:` anywhere outside script text (tab and line breaks dropped first).
    code: Vec<String>,
    /// The text of each `<style>` the scan reads over as a stylesheet, from the end of its
    /// start tag to its end tag (or the end of the page), as it stands.
    sheets: Vec<String>,
}

/// The elements under which a `<style>` is no stylesheet: svg and math (foreign content,
/// first, a self-closed one stays shut), noscript (a browser runs scripts) and the raw-text
/// and RCDATA elements (OR-T10-18). The parser's `STYLE_AS_MARKUP`, in the same order.
const STYLE_AS_MARKUP: [&str; 10] = [
    "svg",
    "math",
    "noscript",
    "textarea",
    "title",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "plaintext",
];

/// A tag's name as a browser reads it: up to whitespace, `/` or `>` (`<style-x>` is no
/// `style`, `<script-x>` no `script`).
fn tag_name(after_lt: &str) -> String {
    after_lt
        .chars()
        .take_while(|c| !c.is_ascii_whitespace() && *c != '/' && *c != '>')
        .collect()
}

/// Whether a start tag is self-closed as a browser and python read it: its last character
/// before `>` is a `/` that stands between attributes -- not one followed by whitespace
/// (`<svg/ >`), not the end of an unquoted value (`<svg a=b/>`) and not inside a quoted one.
/// The states follow the tokenizer (tag name, attribute name, before/after value, quoted
/// and unquoted value; python's `tagfind_tolerant`/`attrfind_tolerant` read the same):
/// re-check tv-bn M9l, M1 -- a `trim_end().ends_with('/')` called both forms self-closed,
/// left svg and math shut where both open them, and hid the script after them.
fn self_closed(tag: &str) -> bool {
    #[derive(Clone, Copy)]
    enum At {
        Between,
        Name,
        AfterName,
        BeforeValue,
        Quoted(char),
        Unquoted,
    }
    let body = tag.strip_prefix('<').unwrap_or(tag);
    let body = body.strip_suffix('>').unwrap_or(body);
    let body = &body[tag_name(body).len()..];
    let ws = |c: char| c.is_ascii_whitespace();
    let mut at = At::Between;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        let last = chars.peek().is_none();
        at = match at {
            At::Between | At::AfterName if ws(c) => at,
            At::Between | At::Name | At::AfterName if c == '/' => {
                if last {
                    return true;
                }
                At::Between
            }
            At::AfterName | At::Name if c == '=' => At::BeforeValue,
            At::Name if ws(c) => At::AfterName,
            At::Name => At::Name,
            At::Between | At::AfterName => At::Name,
            At::BeforeValue if ws(c) => At::BeforeValue,
            At::BeforeValue if c == '"' || c == '\'' => At::Quoted(c),
            At::BeforeValue => At::Unquoted,
            At::Unquoted if ws(c) => At::Between,
            At::Unquoted => At::Unquoted,
            At::Quoted(q) if c == q => At::Between,
            At::Quoted(q) => At::Quoted(q),
        };
    }
    false
}

/// The scan: text, not markup, so it errs towards red -- every `<…>` is a tag up to its
/// first `>`. After a `<script` start tag it reads on at the next `</script`, as a browser
/// and python do: the text of a listed inline script (`a<b`, `x.onload`) is no markup
/// (re-check tv-bn M9f, A-M1). After a `<style` start tag it reads on at its end tag
/// only where a browser reads a stylesheet: in the HTML namespace with none of
/// [`STYLE_AS_MARKUP`] open (OR-T10-18; re-check tv-bn M9g, A-M1 and M9h, M1). Elsewhere it
/// reads the style's text as markup, as the parser does.
///
/// "Errs towards red" does not hold for what the scan skips as a stylesheet. Its tag ends
/// at the first `>` and it reads no comment, so it can skip a style a browser and the
/// parser read as markup: after an end tag of svg or math behind an inner `>` (in a quoted
/// value, a comment or a CDATA section), after a `<style` start tag hidden the same way,
/// or with an svg or math start tag swallowed by a `<` before it (`<<svg>`). The scan alone
/// reads less there, and the parser alone has gaps of its own (a CDATA section behind a
/// breakout tag). So both halves report each stylesheet's text and [`read_checked`]
/// compares them: a style one half skips and the other reads is red by the texts
/// disagreeing, whatever the style holds -- pinned in `RUNS` and the hidden-tag table
/// with its content axis (re-check tv-bn M9m, M1 and M9n, M1). Under a raw-text ancestor
/// the plain boundary is the browser's.
fn scan(html: &str) -> Scan {
    let lower = html.to_ascii_lowercase();
    let mut found = Scan::default();
    let mut open = [0usize; STYLE_AS_MARKUP.len()];
    let mut outside = String::new();
    let mut seg = 0;
    let mut at = 0;
    while let Some(off) = lower[at..].find('<') {
        let start = at + off;
        let end = lower[start..]
            .find('>')
            .map_or(lower.len(), |e| start + e + 1);
        let tag = &lower[start..end];
        let snippet: String = html[start..end].chars().take(80).collect();
        let name = tag_name(&tag[1..]);
        let self_closed = self_closed(tag);
        if let Some(i) = STYLE_AS_MARKUP.iter().position(|n| *n == name)
            && !(self_closed && i < 2)
        {
            open[i] += 1;
        }
        if let Some(i) = tag
            .strip_prefix("</")
            .and_then(|t| STYLE_AS_MARKUP.iter().position(|n| *n == tag_name(t)))
        {
            open[i] = open[i].saturating_sub(1);
        }
        if !name.is_empty() {
            for attr in attr_names(tag) {
                if attr.starts_with("on") || attr == "srcdoc" {
                    found
                        .code
                        .push(format!("the attribute `{attr}`: {snippet}"));
                }
            }
        }
        at = end;
        // A stylesheet's text is no markup either: a `<script>` in a CSS comment is text
        // (colony-view's stylesheet carries one). Its text stays in the `javascript:` search
        // below. A self-closed `<style/>` python reads as an empty element and the text
        // after it as markup; a browser opens it, so the scan reads over with python
        // rather than skip what the parser reads (the differential table below).
        if name == "style" && !self_closed && open.iter().all(|n| *n == 0) {
            at = style_end(&lower, end);
            found.sheets.push(html[end..at].to_string());
        }
        if name == "script" {
            outside.push_str(&lower[seg..end]);
            at = lower[end..]
                .find("</script")
                .map_or(lower.len(), |c| end + c);
            seg = at;
            if !has_attr(tag, "src") {
                found.scripts.push(snippet);
                if self_closed {
                    found.closed += 1;
                } else {
                    found
                        .texts
                        .push(html[end..at].replace("\r\n", "\n").replace('\r', "\n"));
                }
            } else if self_closed {
                found.code.push(format!("a self-closed script: {snippet}"));
            }
        }
        if at >= lower.len() {
            break;
        }
    }
    outside.push_str(&lower[seg.min(lower.len())..]);
    let squeezed: String = outside
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    if squeezed.contains("javascript:") {
        found.code.push("a `javascript:` URL".to_string());
    }
    found
}

/// Where a stylesheet that starts at `from` ends: at the first `</style` followed by
/// whitespace, `/` or `>`, as a browser and python end raw text (`</stylex>` is text), or
/// at the end of the page.
fn style_end(lower: &str, from: usize) -> usize {
    let mut at = from;
    while let Some(c) = lower[at..].find("</style") {
        let after = at + c + "</style".len();
        if matches!(
            lower.as_bytes().get(after),
            Some(b'\t' | b'\n' | b'\r' | b'\x0c' | b' ' | b'/' | b'>')
        ) {
            return at + c;
        }
        at = after;
    }
    lower.len()
}

fn has_attr(tag: &str, want: &str) -> bool {
    attr_names(tag).iter().any(|a| a == want)
}

/// The attribute names of one tag (`<name a="1" b c='2'>`), quoted values skipped.
fn attr_names(tag: &str) -> Vec<String> {
    let body = tag.trim_start_matches('<').trim_end_matches('>');
    let mut chars = body.chars().peekable();
    // The tag name.
    while chars
        .peek()
        .is_some_and(|c| !c.is_whitespace() && *c != '/')
    {
        chars.next();
    }
    let mut names = Vec::new();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == '/') {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == '=' || c == '/' {
                break;
            }
            name.push(c);
            chars.next();
        }
        if name.is_empty() && chars.peek().is_none() {
            break;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek() == Some(&'=') {
            chars.next();
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            match chars.peek().copied() {
                Some(q @ ('"' | '\'')) => {
                    chars.next();
                    for c in chars.by_ref() {
                        if c == q {
                            break;
                        }
                    }
                }
                _ => {
                    while chars.peek().is_some_and(|c| !c.is_whitespace()) {
                        chars.next();
                    }
                }
            }
        }
        if !name.is_empty() {
            names.push(name);
        } else if chars.peek().is_some() {
            // A lone `=` or quote: step over it rather than loop on it.
            chars.next();
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::{
        PAGE_SCRIPTS, PYTHON, PageRead, assert_no_inline_code, read_checked, read_pages,
        read_pages_with, run_parser, scan,
    };

    /// Every form a browser runs as code (re-check tv-bn M9f, A-M1, its probe table).
    const RUNS: &[&str] = &[
        "<script>a=1</script>",
        "<SCRIPT>a=1</SCRIPT>",
        "<script/>",
        "<body onload=\"go()\">",
        "<img src=x ONERROR=go()>",
        "<p/onclick=go()>x</p>",
        "<a href=\"javascript:go()\">x</a>",
        "<a href=\"JavaScript:go()\">x</a>",
        "<!--><script>a=1</script>-->",
        "<!---><script>a=1</script>-->",
        "<!-- x --!><script>a=1</script>-->",
        "<![CDATA[><script>a=1</script>]]>",
        "<iframe srcdoc=\"&lt;script&gt;a=1&lt;/script&gt;\"></iframe>",
        "<svg><script>a=1</script></svg>",
        "<style></style x><script>a=1</script>",
        "<textarea></textarea x><script>a=1</script>",
        "<a href=\"java&#9;script:go()\">x</a>",
        "<a href=\"&#106;avascript:go()\">x</a>",
        "<a href=\"javascript&colon;go()\">x</a>",
        "<a href=\"javas&#99;ript:go()\">x</a>",
        "<p title=\"a>b\" onclick=\"go()\">x</p>",
        "<<script>a=1</script>",
        "<1<script>a=1</script>",
        "< <script>a=1</script>",
        "<=<script>a=1</script>",
        "<svg><foreignObject><![CDATA[><script>a=1</script>]]></foreignObject></svg>",
        "<svg><desc><![CDATA[><script>a=1</script>]]></desc></svg>",
        "<svg><p><![CDATA[><script>a=1</script>]]></p></svg>",
        "<math><mtext><![CDATA[><script>a=1</script>]]></mtext></math>",
        "<noscript><p title=\"</noscript><script>a=1</script>\"></noscript>",
        // A `<style>` is raw text only in the HTML namespace with no raw-text, svg, math or
        // noscript element open (OR-T10-18; re-check tv-bn M9h, M1 and H1): in svg and math
        // it is a foreign element whose text is markup, under `<noscript>` the noscript
        // ends at `</noscript>`, under a raw-text or RCDATA element it is that element's text.
        "<svg><style><script>a=1</script></style></svg>",
        "<svg><g><style><script>a=1</script></style></g></svg>",
        "<svg></math><style><script>a=1</script></style></svg>",
        "<svg><svg></svg><style><script>a=1</script></style></svg>",
        "</svg><svg><style><script>a=1</script></style></svg>",
        "<svg><style><img src=x onerror=go()></style></svg>",
        "<svg/ ><style><script>a=1</script></style></svg>",
        "<svg a=b/><style><script>a=1</script></style></svg>",
        "<math/ ><style><img src=x onerror=go()></style></math>",
        "<math a=b/><style><img src=x onerror=go()></style></math>",
        "<math><style><img src=x onerror=go()></style></math>",
        // An end tag of svg or math behind an inner `>` -- in a quoted value, a comment or a
        // CDATA section -- is text to a browser, which keeps the element open (re-check tv-bn
        // M9m, M1). The scan shuts it there: red only by the halves disagreeing.
        "<svg><g title=\"a></svg>\"><style><script>a=1</script></style></svg>",
        "<svg><!-- > </svg> --><style><script>a=1</script></style></svg>",
        "<svg><![CDATA[ > </svg> ]]><style><script>a=1</script></style></svg>",
        "<math><mrow title=\"a></math>\"><style><img src=x onerror=go()></style></math>",
        "<math><!-- > </math> --><style><img src=x onerror=go()></style></math>",
        "<math><![CDATA[ > </math> ]]><style><img src=x onerror=go()></style></math>",
        // A style the scan skips as a stylesheet, holding a form the parser misses (a CDATA
        // section behind the breakout tag `p`, the `</noscript>` in an attribute): neither
        // half reads a script, and only the stylesheet texts disagree (re-check tv-bn M9n,
        // M1). (a) the end tag of svg or math behind an inner `>`, (b) a `<style` start tag
        // hidden behind an inner `>`, (c) an svg or math start tag swallowed by a `<`.
        "<svg><g title=\"a></svg>\"><style><p><![CDATA[><script>a=1</script>]]></p></style></svg>",
        "<svg><!-- > </svg> --><style><p><![CDATA[><script>a=1</script>]]></p></style></svg>",
        "<svg><![CDATA[ > </svg> ]]><style><p><![CDATA[><script>a=1</script>]]></p></style></svg>",
        "<math><mrow title=\"a></math>\"><style><p><![CDATA[><script>a=1</script>]]></p></style></math>",
        "<math><!-- > </math> --><style><p><![CDATA[><script>a=1</script>]]></p></style></math>",
        "<math><![CDATA[ > </math> ]]><style><p><![CDATA[><script>a=1</script>]]></p></style></math>",
        "<svg><g title=\"a></svg>\"><style><desc><![CDATA[><script>a=1</script>]]></desc></style></svg>",
        "<math><mrow title=\"a></math>\"><style><mtext><![CDATA[><script>a=1</script>]]></mtext></style></math>",
        "<!-- > <style> --><svg><p><![CDATA[><script>a=1</script>]]></p></svg>",
        "<p title=\"a><style>\">x</p><svg><p><![CDATA[><script>a=1</script>]]></p></svg>",
        "<!-- > <style> --><noscript><p title=\"</noscript><script>a=1</script>\"></noscript>",
        "<p title=\"a><style>\">x</p><noscript><p title=\"</noscript><script>a=1</script>\"></noscript>",
        "<<svg><style><p><![CDATA[><script>a=1</script>]]></p></style></svg>",
        "<1<svg><style><p><![CDATA[><script>a=1</script>]]></p></style></svg>",
        "< <svg><style><p><![CDATA[><script>a=1</script>]]></p></style></svg>",
        "<=<svg><style><p><![CDATA[><script>a=1</script>]]></p></style></svg>",
        "<<math><style><p><![CDATA[><script>a=1</script>]]></p></style></math>",
        "<noscript><style></noscript><script>a=1</script></style>",
        "<noscript><style></noscript><img src=x onerror=go()></style>",
        "<noscript/><style></noscript><script>a=1</script></style>",
        "<style-x><noscript><p title=\"</noscript><script>a=1</script>\"></noscript>",
        "<textarea><style></textarea><script>a=1</script></style>",
        "<textarea/><style></textarea><script>a=1</script></style>",
        "<title><style></title><script>a=1</script></style>",
        "<xmp><style></xmp><script>a=1</script></style>",
        "<iframe><style></iframe><script>a=1</script></style>",
        "<noembed><style></noembed><script>a=1</script></style>",
        "<noframes><style></noframes><script>a=1</script></style>",
    ];

    /// Pages a browser runs no code on.
    const CLEAN: &[&str] = &[
        "<!doctype html><html><head><script type=\"module\" src=\"test.js\"></script>\
         <link rel=\"stylesheet\" href=\"a.css\"></head><body><a href=\"/ui/\">x</a>\
         <p title='one=\"two\"'>monitor</p></body></html>",
        "<svg><![CDATA[<script>a=1</script>]]></svg>",
        "<p title=\"a > b\">x</p>",
        "<!-- <b>note</b> -->",
        "<style>a>b{color:red}</style>",
        // A stylesheet in the HTML namespace: a `<script>` in it is CSS text (colony-view's
        // stylesheet carries one in a comment), in a template too, and after a self-closed
        // `<svg/>`, which a browser does not leave open.
        "<style>/* a `<script>` in a comment */</style>",
        "<template><style><script>a=1</script></style></template>",
        "<svg/><style><script>a=1</script></style>",
        "<math/><style><script>a=1</script></style>",
        "<svg></svg><style><script>a=1</script></style>",
    ];

    /// Whether `read_pages` calls the page red: it panics, or it reads code on it.
    fn red_in_read_pages(page: &str) -> bool {
        let page = page.to_string();
        match std::panic::catch_unwind(move || read_pages(&[page]).remove(0)) {
            Ok(r) => !clean(&r),
            Err(_) => true,
        }
    }

    #[test]
    fn every_form_a_browser_runs_is_red_in_the_one_reader() {
        let green: Vec<&str> = RUNS
            .iter()
            .copied()
            .filter(|form| {
                let page = form.to_string();
                !red_in_read_pages(form)
                    || std::panic::catch_unwind(move || assert_no_inline_code("a form", &page))
                        .is_ok()
            })
            .collect();
        assert!(
            green.is_empty(),
            "a browser runs code on these pages, and the reader calls them clean: {green:#?}"
        );
        for page in CLEAN {
            assert!(!red_in_read_pages(page), "{page} is clean and read red");
        }
    }

    /// A listed inline script is one script with its hash, and its text is no markup to
    /// the scan: `a<b`, `x.onload` and a `javascript:` string inside it are script text.
    #[test]
    fn an_inline_script_reads_as_one_script_and_its_text_as_text() {
        let page = "<p>x</p><script>if (a<b && c.onload) x = \"javascript:\";\r\n</script>";
        let read = read_pages(&[page.to_string()]).remove(0);
        assert_eq!(
            read.scripts,
            vec!["if (a<b && c.onload) x = \"javascript:\";\n".to_string()]
        );
        assert_eq!(read.hashes.len(), 1);
        assert!(read.hashes[0].starts_with("'sha256-") && read.hashes[0].ends_with("='"));
        assert!(read.inline_code.is_empty() && read.closed == 0);
    }

    /// The two halves reading one page differently is red by itself, whatever each half
    /// calls the page.
    #[test]
    fn a_page_the_two_halves_read_differently_is_red() {
        let pages: Vec<String> = [
            "<svg><desc><![CDATA[><script>a=1</script>]]></desc></svg>",
            "<a href=\"java&#9;script:go()\">x</a>",
            "<p>clean</p>",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let read = read_checked(PYTHON, &pages);
        for r in &read[..2] {
            let e = r.as_ref().expect_err("the halves disagree");
            assert!(e.contains("parser and scan disagree"), "{e}");
        }
        assert!(read[2].is_ok(), "{:?}", read[2]);
    }

    /// A gap of each half on one page does not cancel out: the parser misses one script,
    /// the scan another, and the counts agree. The halves compare the scripts themselves,
    /// so the script only one of them saw -- whose hash no list holds -- is red (re-check
    /// tv-bn M9g, A-M1, probe of three pages).
    #[test]
    fn a_gap_of_each_half_on_one_page_does_not_cancel_out() {
        let pages: Vec<String> = [
            "<noscript><p title=\"</noscript><script>evil()</script>\"></noscript>\
             <1<script>listed()</script>",
            "<svg><p><![CDATA[><script>evil()</script>]]></p></svg><<script>listed()</script>",
            "<script>listed()</script><noscript><p title=\"</noscript><script>evil()</script>\">\
             </noscript><<script>listed()</script>",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for (page, read) in pages.iter().zip(read_checked(PYTHON, &pages)) {
            let e = read.expect_err(page);
            assert!(e.contains("parser and scan disagree"), "{page}: {e}");
        }
    }

    /// Both halves read a `<style>` in svg, math or under noscript as markup (OR-T10-18), so
    /// they agree and the script in it is a script whose hash no list holds -- next to a
    /// listed one too (re-check tv-bn M9h, M1). An integration point (`foreignObject`,
    /// `mi`) and a MathML `<script>` are read the same way: red in the safe direction.
    #[test]
    fn a_style_in_svg_math_or_noscript_is_markup_to_both_halves() {
        let pages: Vec<String> = [
            "<script>listed()</script><svg><style><script>evil()</script></style></svg>",
            "<svg><g><style><script>evil()</script></style></g></svg>",
            "<noscript><style></noscript><script>evil()</script></style>",
            "<svg><foreignObject><style><script>evil()</script></style></foreignObject></svg>",
            "<math><mi><style><script>evil()</script></style></mi></math>",
            "<math><style><script>evil()</script></style></math>",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for (page, read) in pages.iter().zip(read_checked(PYTHON, &pages)) {
            let read = read.unwrap_or_else(|e| panic!("{page}: {e}"));
            assert!(
                read.scripts.contains(&"evil()".to_string()),
                "{page}: {:?}",
                read.scripts
            );
        }
    }

    #[test]
    fn a_host_without_python_is_red() {
        let e = run_parser(
            "python3-not-on-this-host",
            PAGE_SCRIPTS,
            &["<p>x</p>".into()],
        )
        .expect_err("no interpreter, no read");
        assert!(e.contains("does not start"), "{e}");
        let read = std::panic::catch_unwind(|| {
            read_pages_with("python3-not-on-this-host", &["<p>x</p>".to_string()])
        });
        assert!(
            read.is_err(),
            "a host without python reads a page: {read:?}"
        );
    }

    #[test]
    fn a_python_older_than_the_floor_is_red() {
        assert_eq!(PAGE_SCRIPTS.matches("MIN = (3, 12)").count(), 1);
        let newer = PAGE_SCRIPTS.replace("MIN = (3, 12)", "MIN = (99, 0)");
        let e = run_parser(PYTHON, &newer, &["<p>x</p>".into()])
            .expect_err("a python below the floor reads no page");
        assert!(e.contains("page_scripts needs python >= 99.0"), "{e}");
        // And the python of this host is at the floor or above it.
        run_parser(PYTHON, PAGE_SCRIPTS, &["<p>x</p>".into()]).expect("this python reads");
    }

    // ── The parser half alone, branch by branch (re-check tv-bn M9f, A-M2) ──

    fn parse(page: &str) -> PageRead {
        run_parser(PYTHON, PAGE_SCRIPTS, &[page.to_string()])
            .expect("the parser reads")
            .remove(0)
    }

    fn clean(r: &PageRead) -> bool {
        r.scripts.is_empty() && r.inline_code.is_empty() && r.closed == 0
    }

    /// `as_url`: tab and line breaks dropped, leading C0 and space stripped, any case.
    #[test]
    fn the_parser_reads_a_url_as_a_browser_does() {
        for page in [
            "<a href=\"java&#9;script:go()\">x</a>",
            "<a href=\"java\nscr\ript:go()\">x</a>",
            "<a href=\" \u{1}JavaScript:go()\">x</a>",
        ] {
            assert_eq!(parse(page).inline_code.len(), 1, "{page:?}");
        }
        assert!(clean(&parse("<a href=\"/javascript:\">x</a>")));
    }

    /// `attrs_of`: every `on…` attribute, `srcdoc`, and a `javascript:` value.
    #[test]
    fn the_parser_reads_every_handler_srcdoc_and_javascript_url() {
        for page in [
            "<body onload=\"go()\">",
            "<p/onclick=go()>x</p>",
            "<iframe srcdoc=\"&lt;b&gt;\"></iframe>",
            "<form action=\"javascript:go()\"></form>",
        ] {
            assert_eq!(parse(page).inline_code.len(), 1, "{page:?}");
        }
        assert!(clean(&parse("<p title=\"onload\" data-x=\"srcdoc\">x</p>")));
    }

    /// `handle_startendtag`: `<script/>` without `src` is a script a browser opens.
    #[test]
    fn the_parser_counts_a_self_closed_script() {
        assert_eq!(parse("<script/>").closed, 1);
        assert_eq!(parse("<script type=module />").closed, 1);
        assert_eq!(parse("<script src=\"a.js\"/>").closed, 0);
    }

    /// `handle_comment`: `<!-->` and `<!--->` end at once for a browser; python reads on.
    #[test]
    fn the_parser_reads_the_short_comments_a_browser_ends() {
        for page in [
            "<!--><script>a=1</script>-->",
            "<!---><script>a=1</script>-->",
        ] {
            assert!(!clean(&parse(page)), "{page:?}");
        }
        assert!(clean(&parse("<!-- <script>a=1</script> -->")));
    }

    /// `unknown_decl` and the svg/math counter: CDATA outside foreign content is markup to
    /// a browser, inside svg or math it is text.
    #[test]
    fn the_parser_reads_cdata_outside_svg_and_math() {
        assert!(!clean(&parse("<![CDATA[><script>a=1</script>]]>")));
        assert!(!clean(&parse(
            "<svg></svg><![CDATA[><script>a=1</script>]]>"
        )));
        assert!(clean(&parse("<svg><![CDATA[<b>x</b>]]></svg>")));
        assert!(clean(&parse("<math><![CDATA[<b>x</b>]]></math>")));
    }

    /// The style rule (OR-T10-18): a `<style>` is raw text only in the HTML namespace with no
    /// raw-text, svg, math or noscript element open; svg and math count by name and by depth,
    /// never below zero, so a stray `</math>` in svg, an inner `</svg>` or a `</svg>` before
    /// the `<svg>` ends nothing (re-check tv-bn M9k, M1); a self-closed `<svg/>` or `<math/>`
    /// stays shut, a self-closed `<noscript/>` opens (an HTML element ignores `/>`). So does a
    /// self-closed raw-text or RCDATA element, for which python enters no raw-text mode: only
    /// the rule reads the style after it as markup (re-check tv-bn M9i, M1).
    #[test]
    fn the_parser_reads_a_style_as_markup_in_svg_math_noscript_and_raw_text() {
        for page in [
            "<svg><style><script>a</script></style></svg>",
            "<math><style><script>a</script></style></math>",
            "<noscript><style><script>a</script></style></noscript>",
            "<noscript/><style><script>a</script></style>",
            "<svg></math><style><script>a</script></style></svg>",
            "<svg><svg></svg><style><script>a</script></style></svg>",
            "</svg><svg><style><script>a</script></style></svg>",
            "<xmp/><style><script>a</script></style>",
            "<textarea/><style><script>a</script></style>",
            "<title/><style><script>a</script></style>",
            "<iframe/><style><script>a</script></style>",
            "<noembed/><style><script>a</script></style>",
            "<noframes/><style><script>a</script></style>",
            "<plaintext/><style><script>a</script></style>",
            "<svg/ ><style><script>a</script></style>",
            "<svg a=b/><style><script>a</script></style>",
            "<math/ ><style><script>a</script></style>",
            "<math a=b/><style><script>a</script></style>",
            "<style/><script>a</script></style>",
        ] {
            assert_eq!(parse(page).scripts, vec!["a".to_string()], "{page:?}");
        }
        for page in [
            "<style><script>a</script></style>",
            "<svg></svg><style><script>a</script></style>",
            "<svg/><style><script>a</script></style>",
            "<math/><style><script>a</script></style>",
            "<svg a=\"b\"/><style><script>a</script></style>",
            "<template><style><script>a</script></style></template>",
        ] {
            assert!(clean(&parse(page)), "{page:?}");
        }
        assert!(clean(&parse("<svg></math><![CDATA[<b>x</b>]]></svg>")));
    }

    /// `styles`: every `<style>` element, any case.
    #[test]
    fn the_parser_counts_style_blocks() {
        assert_eq!(parse("<style>a{}</style><STYLE></STYLE>").styles, 2);
        assert_eq!(parse("<style/>").styles, 1);
        assert_eq!(parse("<link rel=stylesheet href=a.css>").styles, 0);
    }

    /// `handle_starttag`/`done`: the text of each script without `src`, line breaks
    /// normalised, an unclosed one too, and one hash per script.
    #[test]
    fn the_parser_reads_each_script_text_and_hashes_it() {
        let r = parse("<script>a\r\nb\rc</script><script src=x.js>y</script><script>d");
        assert_eq!(r.scripts, vec!["a\nb\nc".to_string(), "d".to_string()]);
        assert_eq!(r.hashes.len(), 2);
    }

    // ── The scan half ──

    /// The scan reads each script's text as the parser does: raw, case kept, line breaks
    /// normalised, and a self-closed one counted, not read.
    #[test]
    fn the_scan_reads_each_script_text_as_the_parser_does() {
        let page =
            "<SCRIPT type=module>a<b&amp;\r\nC\r</SCRIPT><script/><script src=x.js></script>";
        let s = scan(page);
        assert_eq!(
            (s.texts.clone(), s.closed),
            (vec!["a<b&amp;\nC\n".to_string()], 1)
        );
        assert_eq!(
            parse(page).scripts,
            s.texts,
            "and the parser reads the same text"
        );
    }

    #[test]
    fn the_scan_finds_every_form_and_passes_a_clean_page() {
        let s = scan(CLEAN[0]);
        assert!(s.scripts.is_empty() && s.code.is_empty(), "{s:?}");
        for (bad, scripts, code) in [
            ("<script>a=1</script>", 1, 0),
            ("<SCRIPT>a=1</SCRIPT>", 1, 0),
            ("<Script type=module>a</Script>", 1, 0),
            ("<script/>", 1, 0),
            ("<script src=\"x.js\"/>", 0, 1),
            ("<body onload=\"go()\">", 0, 1),
            ("<img src=x ONERROR=go()>", 0, 1),
            ("<a href=\"javascript:go()\">x</a>", 0, 1),
            ("<a href=\"java\tscript:go()\">x</a>", 0, 1),
            ("<iframe srcdoc=\"&lt;b&gt;\"></iframe>", 0, 1),
            ("<scriptx>", 0, 0),
            ("<style>/* a `<script>` in a comment */</style>", 0, 0),
            ("<STYLE>p{}</STYLE><script>a</script>", 1, 0),
            ("<style-x><script>a</script>", 1, 0),
            ("<script-x><p onclick=x></script>", 0, 1),
            ("<svg><style><script>a</script></style></svg>", 1, 0),
            ("<math><style><p onclick=x></style></math>", 0, 1),
            ("<noscript><style><p onclick=x></style>", 0, 1),
            // The counters as in the parser test: by name, by depth, never below zero, and
            // a closed svg releases (re-check tv-bn M9k, M1, H2).
            ("<svg></math><style><script>a</script></style></svg>", 1, 0),
            (
                "<svg><svg></svg><style><script>a</script></style></svg>",
                1,
                0,
            ),
            ("</svg><svg><style><script>a</script></style></svg>", 1, 0),
            ("<svg></svg><style><script>a</script></style>", 0, 0),
            ("<xmp><style></xmp><script>a</script>", 1, 0),
            ("<textarea><style></textarea><script>a</script>", 1, 0),
            ("<title><style></title><script>a</script>", 1, 0),
            ("<iframe><style></iframe><script>a</script>", 1, 0),
            ("<noembed><style></noembed><script>a</script>", 1, 0),
            ("<noframes><style></noframes><script>a</script>", 1, 0),
            // Reads over on purpose (a browser runs no script after `<plaintext>` or after an
            // unclosed `<N/>`): the rows pin the rule, not the browser (re-check tv-bn M9j).
            ("<plaintext><style><script>a</script>", 1, 0),
            ("<noscript/><style><script>a</script></style>", 1, 0),
            ("<xmp/><style><script>a</script></style>", 1, 0),
            ("<textarea/><style><script>a</script></style>", 1, 0),
            ("<title/><style><script>a</script></style>", 1, 0),
            ("<iframe/><style><script>a</script></style>", 1, 0),
            ("<noembed/><style><script>a</script></style>", 1, 0),
            ("<noframes/><style><script>a</script></style>", 1, 0),
            ("<plaintext/><style><script>a</script></style>", 1, 0),
            ("<svg/><style><script>a</script></style>", 0, 0),
            ("<math/><style><script>a</script></style>", 0, 0),
            // Self-closed only where `/` stands right before `>` between attributes: a
            // blank after it or an unquoted value ending in it leaves svg and math open
            // (re-check tv-bn M9l, M1); a self-closed style reads over with the parser.
            ("<svg/ ><style><script>a</script></style>", 1, 0),
            ("<svg a=b/><style><script>a</script></style>", 1, 0),
            ("<math/ ><style><script>a</script></style>", 1, 0),
            ("<math a=b/><style><script>a</script></style>", 1, 0),
            ("<svg a=\"b\"/><style><script>a</script></style>", 0, 0),
            ("<style/><script>a</script></style>", 1, 0),
            ("<script>a<b onload=x</script><p onclick=y>", 1, 1),
        ] {
            let s = scan(bad);
            assert_eq!(
                (s.scripts.len(), s.code.len()),
                (scripts, code),
                "{bad}: {s:?}"
            );
        }
    }

    // ── Both halves, every start-tag form (OR-T11-11) ──

    /// One table instead of one re-check round per edge case: every start-tag form of svg,
    /// math, style and script the table below generates -- blank before or after `/`,
    /// quoted and unquoted values, an attribute without one, `/` in a value, any case,
    /// nested, and stray or odd end tags -- is read alike by the parser and the scan: the
    /// same scripts, the same number self-closed (re-check tv-bn M9l, M1: four rounds had
    /// found one pair each on this scan). A form the two read differently is a red row
    /// naming the form. The table holds forms either way, open and shut, so it cannot pass
    /// by reading nothing.
    #[test]
    fn the_two_halves_read_every_start_tag_form_alike() {
        const ATTRS: [&str; 15] = [
            "",
            " a=b=c/",
            "\x0b",
            " a\x0b",
            "/ a",
            " a",
            " a=b",
            " a=b/",
            " a='b'",
            " a=\"b/\"",
            " a=/",
            " a= /",
            " a = b",
            "/a",
            " a=\"b\"c",
        ];
        const BEFORE: [&str; 6] = ["", " ", "\t", "\n", "\x0c", "\r"];
        const SLASH: [&str; 3] = ["", "/", "/ "];
        const ENDS: [&str; 8] = [
            "</{}>", "</{} >", "</{}/>", "</{} a>", "</ {}>", "</{}\t>", "</{}x>", "</{}/ >",
        ];
        let starts = |el: &str| -> Vec<String> {
            let mut cap = el[..1].to_ascii_uppercase();
            cap.push_str(&el[1..]);
            let mut forms = Vec::new();
            for name in [el.to_string(), el.to_ascii_uppercase(), cap] {
                for a in ATTRS {
                    for b in BEFORE {
                        for s in SLASH {
                            forms.push(format!("<{name}{a}{b}{s}>"));
                        }
                    }
                }
            }
            forms
        };
        let style = "<style><script>a</script></style>";
        let mut pages = Vec::new();
        for (el, other) in [("svg", "math"), ("math", "svg")] {
            for f in starts(el) {
                pages.push(format!("{f}{style}"));
                pages.push(format!("<{el}>{f}</{el}>{style}"));
                pages.push(format!("{f}</{other}>{style}"));
            }
            for end in ENDS {
                let end = end.replace("{}", el);
                pages.push(format!("<{el}>{end}{style}"));
                pages.push(format!("<{el}>{}{style}", end.to_ascii_uppercase()));
            }
        }
        for f in starts("style") {
            pages.push(format!("{f}<script>a</script></style>"));
            pages.push(format!("<svg></svg>{f}<script>a</script></style>"));
            pages.push(format!("<svg>{f}<script>a</script></style></svg>"));
        }
        for f in starts("script") {
            pages.push(format!("{f}a</script>"));
        }
        let (mut read_script, mut read_none, mut red) = (0, 0, Vec::new());
        for (page, read) in pages.iter().zip(read_checked(PYTHON, &pages)) {
            match read {
                Ok(r) if r.scripts.is_empty() && r.closed == 0 => read_none += 1,
                Ok(_) => read_script += 1,
                Err(e) => red.push(format!("{page:?}: {e}")),
            }
        }
        assert!(
            red.is_empty(),
            "{} of {} start-tag forms read differently by the two halves:\n{}",
            red.len(),
            pages.len(),
            red.join("\n")
        );
        assert_eq!(pages.len(), 8132, "the table's size");
        assert!(
            read_script > 1000 && read_none > 1000,
            "the table holds forms either way: {read_script} read a script, {read_none} none"
        );
    }

    /// The table's two missing axes (re-check tv-bn M9m, M1 and H3; OR-T11-16): a `>` in a
    /// quoted value, and a comment or CDATA section as context. The scan ends every tag at
    /// its first `>` and reads no comment; a browser and the parser read quotes, comments and
    /// CDATA. Each form carries the browser's verdict on the style after it: `true` where svg
    /// or math is still open there, so its text is markup and the `<script>` in it a tag. On
    /// such a form a reading of no script that both halves share is silent green -- the
    /// table's red row. A form the halves read differently is red, which is what the scan's
    /// blind spot comes to (fail closed); the count of those is pinned, so a change shows.
    /// Forms the browser reads as a stylesheet are green, or red the safe way (pinned too).
    ///
    /// Each form runs with every style content of `CONTENTS` (OR-T11-19; re-check tv-bn
    /// M9n, M1): with a fixed content the table was blind to a style the scan skips that
    /// holds a form the parser misses. Where the halves agree they must draw the browser's
    /// stylesheets -- none where it reads markup, the content where it reads a stylesheet --
    /// and where the content runs code a shared reading of none is silent green.
    #[test]
    fn no_hidden_tag_in_a_quote_comment_or_cdata_reads_silently_green() {
        // (content, whether it runs code where a browser reads it as markup in svg or math)
        const CONTENTS: [(&str, bool); 7] = [
            ("<script>a</script>", true),
            ("", false),
            ("<b title=x>y</b>", false),
            ("<![CDATA[><script>a</script>]]>", false),
            ("<!--><script>a</script>-->", true),
            ("</stylex><script>a</script>", true),
            ("<p><![CDATA[><script>a</script>]]></p>", true),
        ];
        let style = "\u{1}";
        let mut forms: Vec<(String, bool)> = Vec::new();
        for el in ["svg", "math"] {
            let up = el.to_ascii_uppercase();
            let ends = [
                format!("</{el}>"),
                format!("</{up}>"),
                format!("</{el} x>"),
                format!("</{el}/>"),
            ];
            for end in &ends {
                // `>` in a quoted value, on the element's own start tag and on a child's.
                for q in ['"', '\''] {
                    for pre in ["", "a>", " > ", ">>"] {
                        let v = format!("{q}{pre}{end}{q}");
                        for attr in [format!(" t={v}"), format!(" a t={v}")] {
                            forms.push((format!("<{el}{attr}>{style}</{el}>"), true));
                            forms.push((format!("<{el}><x{attr}>{style}</{el}>"), true));
                        }
                        let shut = format!("{q}{pre}b{q}");
                        forms.push((format!("<{el} t={shut}>{end}{style}"), false));
                        forms.push((format!("<{el}><x t={shut}>{end}{style}"), false));
                    }
                }
                // A comment in svg or math: the end tag in it is text, after it a tag.
                for open in ["", "<x>"] {
                    for pre in ["> ", "a>b ", "-- > ", "--!x> "] {
                        for close in ["-->", "--!>"] {
                            forms.push((
                                format!("<{el}>{open}<!-- {pre}{end} {close}{style}</{el}>"),
                                true,
                            ));
                            forms.push((
                                format!("<{el}>{open}<!-- {pre}{close}{end}{style}"),
                                false,
                            ));
                        }
                    }
                    for shut in ["<!---->", "<!-->", "<!--->"] {
                        forms.push((format!("<{el}>{open}{shut}{end}{style}"), false));
                    }
                    // A CDATA section in svg or math: the same, closed by `]]>`.
                    for pre in [" > ", "a>", "]>", "]] >"] {
                        forms.push((
                            format!("<{el}>{open}<![CDATA[{pre}{end}]]>{style}</{el}>"),
                            true,
                        ));
                        forms.push((format!("<{el}>{open}<![CDATA[{pre}]]>{end}{style}"), false));
                    }
                }
            }
            // The start tag hidden instead: a browser opens no element (a stylesheet) -- the
            // scan opens it and reads over; outside svg and math `<![CDATA[` is a bogus
            // comment up to its first `>`, so the start tag after it is one.
            for start in [format!("<{el}>"), format!("<{up} a>")] {
                forms.push((format!("<!-- > {start} -->{style}"), false));
                forms.push((format!("<p title=\"a>{start}\">x</p>{style}"), false));
                forms.push((format!("<p title='{start}'>x</p>{style}"), false));
                forms.push((format!("<![CDATA[ > {start} ]]>{style}</{el}>"), true));
            }
        }
        assert_eq!(forms.len(), 832, "the table's forms");
        let none = |r: &PageRead| r.scripts.is_empty() && r.closed == 0 && r.inline_code.is_empty();
        let (mut silent, mut wrong_sheet, mut counts) = (Vec::new(), Vec::new(), Vec::new());
        for (content, runs) in CONTENTS {
            let sheet = format!("<style>{content}</style>");
            let pages: Vec<String> = forms
                .iter()
                .map(|(p, _)| p.replace(style, &sheet))
                .collect();
            let (mut red_markup, mut read_markup, mut red_sheet, mut green_sheet) = (0, 0, 0, 0);
            for ((page, (_, markup)), read) in
                pages.iter().zip(&forms).zip(read_checked(PYTHON, &pages))
            {
                let drawn: Vec<String> = if *markup {
                    vec![]
                } else {
                    vec![content.to_string()]
                };
                match (read, *markup) {
                    (Ok(r), _) if r.sheets != drawn => wrong_sheet.push((page.clone(), r.sheets)),
                    (Ok(r), true) if runs && none(&r) => silent.push(page.clone()),
                    (Ok(_), true) => read_markup += 1,
                    (Err(_), true) => red_markup += 1,
                    (Ok(r), false) if none(&r) => green_sheet += 1,
                    (_, false) => red_sheet += 1,
                }
            }
            counts.push((content, red_markup, read_markup, red_sheet, green_sheet));
        }
        assert!(
            silent.is_empty(),
            "a browser reads the style's text as markup, it runs code, and both halves read \
             none: {silent:#?}"
        );
        assert!(
            wrong_sheet.is_empty(),
            "both halves agree on stylesheets a browser does not draw: {wrong_sheet:#?}"
        );
        // Per content: (red by disagreement, read alike as markup, red on a stylesheet,
        // green stylesheet); the fixed content of M9n first. The 388 styles the scan skips
        // where a browser reads markup are red on every content, the empty one too: the
        // stylesheet texts differ. On the 64 forms the scan keeps open the halves read the
        // content alike, except a `<script` in a CDATA section or comment, which the scan
        // reads as a script and python does not (452). A content with `-->` behind a
        // `<!-->` or `<!--->` is red on 32 stylesheets more: python reads that comment on
        // to the `-->` and calls it code (the safe way).
        assert_eq!(
            counts,
            vec![
                ("<script>a</script>", 388, 64, 8, 372),
                ("", 388, 64, 8, 372),
                ("<b title=x>y</b>", 388, 64, 8, 372),
                ("<![CDATA[><script>a</script>]]>", 452, 0, 8, 372),
                ("<!--><script>a</script>-->", 452, 0, 40, 340),
                ("</stylex><script>a</script>", 388, 64, 8, 372),
                ("<p><![CDATA[><script>a</script>]]></p>", 452, 0, 8, 372),
            ],
            "the table per style content"
        );
    }
}
