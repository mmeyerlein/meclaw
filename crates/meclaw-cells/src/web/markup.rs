//! GH #869: where in the markup a substitution stands, and what may stand there.
//!
//! Escaping answers one question: can this value close the element or the
//! attribute it was put into? It cannot. It does not answer the second one: what
//! does the value MEAN where it stands? Three places give an escaped value a
//! meaning of its own, and each was reachable from a prop a model or a browser
//! writes:
//!
//! - a LiveView binding attribute (`phx-click`, `phx-keyup`, …). The client reads
//!   a value that starts with `[` as a list of JS commands (`exec`,
//!   `set_attr`, `navigate`), so `[["exec",{"attr":"onclick"}]]` is a program,
//!   escaped or not;
//! - a URL attribute (`href`, `src`, …), where `javascript:` is a scheme;
//! - a `style` attribute, where `0;background:url(//x)` is two declarations
//!   and a request.
//!
//! So the parser remembers, for every `{{prop}}`, which of these places it
//! stands in (a [`Slot`]), and the renderer asks the value to fit that place.
//! A value that does not fit renders **empty** rather than refusing the page:
//! the same trade as the one in `render.rs` for an undeclared raw prop — a
//! missing word is the harmless failure, a page nobody can see is not.
//!
//! `phx-value-*` and `phx-target` are deliberately NOT binding attributes. The
//! client executes nothing it finds there, and the display writes object ids
//! into `phx-value-for` that a narrower grammar would cut, so a tap would lose
//! its target.
//!
//! The scanner is a small cut of the WHATWG HTML tokenizer: data, start and
//! end tags, attribute names and values in all three quotings, bogus comments
//! (`<?…>`, `</ …>`), comments with their start and end states, and the text
//! of the elements that switch the tokenizer to text (`script`, `style`,
//! `textarea`, `title` and the other RAWTEXT elements, see [`Contexts`]).
//! It reads only the template's own text; a substituted value never moves it,
//! because an escaped value cannot contain the characters that do. Where the
//! cut is coarser than the tokenizer, the template is refused rather than
//! guessed at: `<plaintext>`, CDATA, a script or style inside svg or math,
//! and a template that does not end between tags (review I2); SVG animation,
//! `srcdoc` and `<!--` in a script (review M3/M6/M7/M8/M9), places where a
//! value loads or runs that the scanner would have to read as a second
//! language.
//!
//! That last sentence holds only while a substitution stands where the
//! template's own text cannot run on through it. Blanked, `<scr{{x}}ipt>` is
//! two harmless words; rendered with an empty `x` it is `<script>`, and
//! `<a h{{n}}="{{v}}">` becomes an `href` whose value nobody fitted. So the
//! parser refuses a `{{…}}` at every such place (review I1 of #868/#869, see
//! [`Contexts::place`]): inside a tag or attribute name, inside a tag outside
//! quotes, in a comment, in the end tag of a script or style, and in a URL
//! attribute before the template's own text has settled the scheme.

use meclaw_core::serde_json::Value;

/// Where one substitution stands in the markup of its template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    /// Between tags: an HTML text node, or inside a comment.
    Text,
    /// Inside a quoted attribute value. The attribute's name, lowercased.
    Attr(String),
    /// Inside a tag but outside any quoted value: a tag or attribute name, or
    /// an unquoted value. Also what a place the scanner cannot decide reads as
    /// (see [`Contexts::slot`]), because its grammar is the narrowest.
    Tag,
    /// Inside the text of a `<script>` or `<style>` element.
    RawText,
}

impl Slot {
    /// Whether a raw (`{{&prop}}`, schema `"html"`) value may stand here as
    /// markup. Only where markup is what the place holds: between tags, and in
    /// the body of a `<script>`/`<style>` (the display's own hook script and
    /// font faces). In an attribute or a tag a raw value would be a way out of
    /// the quotes, so there it is escaped and fitted like any other value.
    pub fn takes_markup(&self) -> bool {
        matches!(self, Slot::Text | Slot::RawText)
    }
}

/// The LiveView attributes whose value the client reads as an event name or
/// as a list of JS commands. `phx-window-*` is matched as a prefix.
const BINDINGS: &[&str] = &[
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
];

/// Attributes a browser reads as a URL it may load or navigate to.
const URL_ATTRS: &[&str] = &[
    "href",
    "src",
    "action",
    "formaction",
    "poster",
    "xlink:href",
    // What an `<object>` loads and shows (review M7 of #868/#869).
    "data",
];

/// The schemes a URL attribute may carry. A value without a scheme is relative
/// and always fits.
const URL_SCHEMES: &[&str] = &["http", "https", "mailto", "tel"];

/// Whether `name` is a binding attribute (see [`BINDINGS`]).
pub fn is_binding(name: &str) -> bool {
    BINDINGS.contains(&name) || name.starts_with("phx-window-")
}

/// Whether `name` is an attribute a browser reads as a URL.
pub fn is_url_attr(name: &str) -> bool {
    URL_ATTRS.contains(&name)
}

/// An event name as a binding attribute may carry it: `[A-Za-z0-9_.:/-]{0,64}`.
/// No `[`, so no JSON command list; no quote, space or comma either.
pub fn is_event_name(v: &str) -> bool {
    v.len() <= 64
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'/' | b'-'))
}

/// A URL as a URL attribute may carry it: relative, or with one of
/// [`URL_SCHEMES`].
///
/// A control character anywhere is refused outright: a browser's URL parser
/// drops tabs and newlines before it reads the scheme, so `java\tscript:` IS
/// `javascript:` to it. Leading and trailing spaces are trimmed the way the
/// browser trims them. The scheme is what stands before the first `:` when
/// that `:` comes before any `/`, `?` or `#`; anything else is a path.
pub fn is_safe_url(v: &str) -> bool {
    if v.chars().any(|c| c.is_control()) {
        return false;
    }
    let v = v.trim_matches(' ');
    match v.find([':', '/', '?', '#']) {
        Some(at) if v.as_bytes()[at] == b':' => {
            let scheme = v[..at].to_ascii_lowercase();
            URL_SCHEMES.contains(&scheme.as_str())
        }
        _ => true,
    }
}

/// A CSS value as a `style` attribute may carry it inside a declaration the
/// template wrote: `[A-Za-z0-9 _.,%#+-]{0,128}`. No `;` (a second
/// declaration), no `:` (a property), no `(` (a function such as `url()`), no
/// quote and no backslash (an escape).
pub fn is_css_value(v: &str) -> bool {
    v.len() <= 128
        && v.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b' ' | b'_' | b'.' | b',' | b'%' | b'#' | b'+' | b'-')
        })
}

/// A plain token: `[A-Za-z0-9_.-]{0,64}`. What a value inside a tag but outside
/// quotes may be — it can name neither an event handler with its code nor a
/// URL with a scheme, and it cannot end the tag.
pub fn is_token(v: &str) -> bool {
    v.len() <= 64
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// Whether `value` may stand in `slot`. Text always may: it is escaped.
pub fn fits(slot: &Slot, value: &str) -> bool {
    match slot {
        Slot::Text => true,
        Slot::Tag => is_token(value),
        Slot::RawText => is_css_value(value),
        Slot::Attr(name) if is_binding(name) => is_event_name(value),
        Slot::Attr(name) if is_url_attr(name) => is_safe_url(value),
        Slot::Attr(name) if name == "style" => is_css_value(value),
        Slot::Attr(_) => true,
    }
}

/// A prop typed `"int"` as the text it renders: a JSON integer, or a string of
/// one to fifteen digits with an optional minus. Absent and `null` are empty.
/// Anything else is `None` — the caller renders nothing. An `int` used to
/// render any string, and the display puts its ints inside `style`.
pub fn int_text(v: Option<&Value>) -> Option<String> {
    match v {
        None | Some(Value::Null) => Some(String::new()),
        Some(Value::Number(n)) => n
            .as_i64()
            .map(|i| i.to_string())
            .or_else(|| n.as_u64().map(|u| u.to_string())),
        Some(Value::String(s)) => {
            let digits = s.strip_prefix('-').unwrap_or(s);
            ((1..=15).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.clone())
        }
        Some(_) => None,
    }
}

/// What a `{{…}}` puts where it stands (review I1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sub {
    /// `{{prop}}` or `{{&prop}}`: a value.
    Value,
    /// `{{#if prop}}` or `{{/if}}`: nothing itself, but the template's text on
    /// either side of it meets when one branch renders empty.
    Branch,
    /// `{{children}}`: markup.
    Children,
}

/// Why a `{{…}}` may not stand where it does; completes "`{{x}}` stands in …".
const IN_NAME: &str = "a tag or attribute name";
const IN_UNQUOTED: &str = "an unquoted attribute value";
const IN_COMMENT: &str = "a comment";
const IN_RAW_END: &str = "the end tag of a script, style or text element";
const IN_SCRIPT_ESCAPE: &str = "a `<!--` inside a script";
const IN_OPEN_URL: &str = "a URL attribute whose scheme the template's own text has not settled";
const OUTSIDE_TEXT: &str = "a place other than between tags";
const IN_DOUBT: &str = "a place the parser cannot tell apart";
/// Why static text after a value at the start of a URL attribute is refused.
pub(crate) const AFTER_WHOLE_URL: &str = "a value stands at the start of a URL attribute \
     and the template's own text goes on after it — a URL takes one whole value, \
     or starts with the template's own `/`, `?`, `#` or scheme";
/// Why a `<plaintext>` is refused (review I2).
pub(crate) const PLAINTEXT: &str = "the template opens a `<plaintext>` element — a browser \
     reads everything after it as text, and nothing closes it";
/// Why a CDATA section is refused (review I2).
pub(crate) const CDATA: &str = "the template carries a `<![CDATA[` section — inside svg or \
     math a browser reads it to `]]>`, elsewhere to the first `>`, and a template cannot \
     tell which";
/// Why a script or style inside svg or math is refused (review I2 point 4).
pub(crate) const FOREIGN_RAW: &str = "a `<script>` or `<style>` stands inside an svg or math \
     element — there it is no raw text but markup, and its content would be read as tags";
/// Why an SVG animation element is refused (review M3/M8 of #868/#869).
pub(crate) const SVG_ANIMATION: &str = "the template carries an svg animation element \
     (`animate`, `set`, `animateMotion`, `animateTransform`) — it sets the attribute \
     `attributeName` names to a value from `to`, `from`, `by` or `values`, which no rule \
     reads as that attribute: `<set attributeName=\"href\" to=\"javascript:…\">` is a link \
     that runs code";
/// Why a `srcdoc` attribute is refused (review M7 of #868/#869).
pub(crate) const SRCDOC: &str = "the template gives an element a `srcdoc` — its value is a \
     whole document the browser reads as markup again, and the rules read it as one \
     attribute value";
/// Why `<!--` in a script is refused (review M6/M9 of #868/#869).
pub(crate) const SCRIPT_ESCAPE: &str = "the template writes `<!--` in the text of a script — \
     from there a browser reads escaped script data, where `<script>` and `</script>` \
     move it in ways this parser does not follow, up to past the template's end";
/// Why a template that does not end between tags is refused (review I2).
pub(crate) const UNFINISHED: &str = "the template ends inside a tag, an attribute value, a \
     comment, a text element or an svg or math element — what follows it on the page would \
     be read on from there; a template ends between tags, as it began";

/// The elements whose start tag switches a browser's tokenizer to text until
/// their own end tag (WHATWG § 13.2.6.2, "generic raw text / RCDATA element
/// parsing"): RAWTEXT — `script` data reads the same here — and RCDATA
/// (`textarea`, `title`).
///
/// Whether the tokenizer switches depends on the tree around the element, not
/// on the tokenizer: inside svg or math (foreign content) it does not, and an
/// `<img>` in the text breaks out of the svg again as HTML; `noscript` switches
/// only with scripting on; an older parser ignores `<style>` in a `<select>`. A
/// template cannot know which parent it lands in (a `{{children}}` inside
/// someone else's `<svg>`), so it is read both ways from each of these
/// ([`Scanner::close_tag`]) and a `{{…}}` has to fit in both (review I2).
/// The SVG animation elements (lowercased as the tokenizer lowers a tag
/// name). Each sets the attribute its `attributeName` names to a value from
/// `to`, `from`, `by` or `values`: `href` among them, so `javascript:` could
/// reach a link through an attribute no rule reads as a URL, and `values` is a
/// list no single-URL grammar reads (review M3/M8). No shipped template uses
/// one, so they are refused rather than read.
const SVG_ANIMATION_ELEMENTS: &[&str] = &["animate", "set", "animatemotion", "animatetransform"];

const TEXT_ELEMENTS: &[&str] = &[
    "script", "style", "xmp", "iframe", "noembed", "noframes", "noscript", "textarea", "title",
];

/// Where the quoted value of a URL attribute stands with its scheme.
///
/// A browser reads the scheme from what comes before the first `:`, and only
/// the template's own text is checked for `javascript:`. So a value may stand
/// in a URL attribute once that text has settled the scheme — a `:`, `/`, `?`
/// or `#` of its own came first — or as the whole attribute value, where
/// [`is_safe_url`] reads all of it. `href="java{{x}}script:…"` is neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UrlValue {
    /// Not in the quoted value of a URL attribute.
    No,
    /// At its start, nothing read yet.
    Empty,
    /// The template's text began it without a `:`, `/`, `?` or `#`.
    Open,
    /// The template's own text settled the scheme.
    Settled,
    /// A value stood at the start: it has to be the whole attribute value.
    Whole,
}

/// The tokenizer states the scanner distinguishes (WHATWG § 13.2.5, cut to
/// what moves a place).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    Data,
    /// Just read `<`.
    Lt,
    /// Just read `</`: a letter starts an end tag, `>` is nothing, anything
    /// else is a bogus comment to the next `>`.
    EndTagOpen,
    TagName,
    InTag,
    AttrName,
    AfterAttrName,
    BeforeValue,
    Dq,
    Sq,
    Unquoted,
    /// `<!`, `<!-` and `<![` (matching `[CDATA[`).
    Bang,
    BangDash,
    BangCdata,
    /// The comment states: its start (`<!--` read; `>` ends it), the start
    /// dash (`<!---`; `>` ends it), its text, and its end (`-`, `--`, `--!`).
    CommentStart,
    CommentStartDash,
    Comment,
    CommentEndDash,
    CommentEnd,
    CommentEndBang,
    /// `<!x…>`, `<?…>` or `</ …>`: skipped to the next `>`.
    Bogus,
    /// The text of a [`TEXT_ELEMENTS`] element, and the three steps of its end
    /// tag.
    Raw,
    RawLt,
    RawLtSlash,
    RawEnd,
    /// `<!` and `<!-` in the text of a script: one more `-` opens the escaped
    /// script data of the tokenizer, which the scanner does not follow.
    RawBang,
    /// The scanner gave up (see [`Contexts`]); every place reads as [`Slot::Tag`].
    Lost,
}

/// One reading of a template's markup, character by character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Scanner {
    st: St,
    /// The name of the tag being read, lowercased.
    tag: String,
    end_tag: bool,
    /// The name of the attribute being read, lowercased.
    attr: String,
    /// The [`TEXT_ELEMENTS`] name while in [`St::Raw`].
    raw: &'static str,
    /// Characters of `[CDATA[` or of the raw end tag matched.
    n: usize,
    /// Where a URL attribute's quoted value stands with its scheme.
    url: UrlValue,
    /// A `/` was the last character read in the tag: a `>` now makes it
    /// self-closing (`<svg/>` opens no svg).
    slash: bool,
    /// How many `<svg>`/`<math>` start tags their end tags have not closed,
    /// counted from this template's own text.
    foreign: u32,
    /// Set for good once the template's own text is refused
    /// ([`AFTER_WHOLE_URL`], [`PLAINTEXT`], [`CDATA`], [`FOREIGN_RAW`],
    /// [`SVG_ANIMATION`], [`SRCDOC`], [`SCRIPT_ESCAPE`]).
    refused: Option<&'static str>,
}

impl Scanner {
    fn start() -> Self {
        Self {
            st: St::Data,
            tag: String::new(),
            end_tag: false,
            attr: String::new(),
            raw: "",
            n: 0,
            url: UrlValue::No,
            slash: false,
            foreign: 0,
            refused: None,
        }
    }

    /// A reading that gave up, keeping a refusal one of the readings had.
    fn lost(refused: Option<&'static str>) -> Self {
        Self {
            st: St::Lost,
            refused,
            ..Self::start()
        }
    }

    /// Between tags again. What the last tag was matters no more, so two
    /// readings that got here by different ways are one reading.
    fn enter_data(&mut self) {
        self.st = St::Data;
        self.tag.clear();
        self.end_tag = false;
        self.attr.clear();
        self.raw = "";
        self.n = 0;
        self.url = UrlValue::No;
        self.slash = false;
    }

    fn refuse(&mut self, why: &'static str) {
        self.refused.get_or_insert(why);
    }

    /// A tag ends at `>`. Returns the second reading when a start tag of a
    /// [`TEXT_ELEMENTS`] element leaves two: this one in its text, the returned
    /// one between tags, as if the element had not switched the tokenizer.
    fn close_tag(&mut self) -> Option<Scanner> {
        let start = !self.end_tag;
        let self_closing = self.slash;
        let tag = std::mem::take(&mut self.tag);
        self.enter_data();
        if tag == "svg" || tag == "math" {
            if !start {
                self.foreign = self.foreign.saturating_sub(1);
            } else if !self_closing {
                self.foreign = self.foreign.saturating_add(1);
            }
            return None;
        }
        if !start {
            return None;
        }
        if tag == "plaintext" {
            self.refuse(PLAINTEXT);
            return None;
        }
        if SVG_ANIMATION_ELEMENTS.contains(&tag.as_str()) {
            self.refuse(SVG_ANIMATION);
            return None;
        }
        let raw = TEXT_ELEMENTS.iter().copied().find(|t| *t == tag)?;
        if self.foreign > 0 && matches!(raw, "script" | "style") {
            self.refuse(FOREIGN_RAW);
        }
        let markup = self.clone();
        self.st = St::Raw;
        self.raw = raw;
        Some(markup)
    }

    /// Read one character; the second reading it leaves, if it leaves two
    /// (see [`Scanner::close_tag`]).
    fn step(&mut self, ch: char) -> Option<Scanner> {
        let ws = matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}');
        match self.st {
            St::Data => {
                if ch == '<' {
                    self.st = St::Lt;
                }
            }
            St::Lt => {
                if ch.is_ascii_alphabetic() {
                    self.tag.clear();
                    self.tag.push(ch.to_ascii_lowercase());
                    self.end_tag = false;
                    self.st = St::TagName;
                } else if ch == '/' {
                    self.st = St::EndTagOpen;
                } else if ch == '!' {
                    self.st = St::Bang;
                } else if ch == '?' {
                    self.st = St::Bogus;
                } else if ch != '<' {
                    self.enter_data();
                }
            }
            St::EndTagOpen => {
                if ch.is_ascii_alphabetic() {
                    self.tag.clear();
                    self.tag.push(ch.to_ascii_lowercase());
                    self.end_tag = true;
                    self.st = St::TagName;
                } else if ch == '>' {
                    self.enter_data();
                } else {
                    self.st = St::Bogus;
                }
            }
            St::TagName => {
                if ws || ch == '/' {
                    self.st = St::InTag;
                    self.slash = ch == '/';
                } else if ch == '>' {
                    return self.close_tag();
                } else {
                    self.tag.push(ch.to_ascii_lowercase());
                }
            }
            St::InTag => {
                if ch == '>' {
                    return self.close_tag();
                }
                self.slash = ch == '/';
                if !(ws || ch == '/') {
                    self.attr.clear();
                    self.attr.push(ch.to_ascii_lowercase());
                    self.st = St::AttrName;
                }
            }
            St::AttrName => {
                if ch == '=' {
                    self.st = St::BeforeValue;
                } else if ws {
                    self.st = St::AfterAttrName;
                } else if ch == '>' {
                    return self.close_tag();
                } else if ch == '/' {
                    self.st = St::InTag;
                    self.slash = true;
                } else {
                    self.attr.push(ch.to_ascii_lowercase());
                }
            }
            St::AfterAttrName => {
                if ch == '=' {
                    self.st = St::BeforeValue;
                } else if ch == '>' {
                    return self.close_tag();
                } else if ch == '/' {
                    self.st = St::InTag;
                    self.slash = true;
                } else if !ws {
                    self.attr.clear();
                    self.attr.push(ch.to_ascii_lowercase());
                    self.st = St::AttrName;
                }
            }
            St::BeforeValue => {
                // Review M7: a `srcdoc` value is a document, markup again.
                if ch != '>' && !ws && self.attr == "srcdoc" {
                    self.refuse(SRCDOC);
                }
                if ch == '"' || ch == '\'' {
                    self.st = if ch == '"' { St::Dq } else { St::Sq };
                    self.url = if is_url_attr(&self.attr) {
                        UrlValue::Empty
                    } else {
                        UrlValue::No
                    };
                } else if ch == '>' {
                    return self.close_tag();
                } else if !ws {
                    self.st = St::Unquoted;
                }
            }
            St::Dq | St::Sq => {
                let quote = if self.st == St::Dq { '"' } else { '\'' };
                if ch == quote {
                    self.st = St::InTag;
                    self.url = UrlValue::No;
                } else {
                    match self.url {
                        UrlValue::Whole => self.refuse(AFTER_WHOLE_URL),
                        UrlValue::Empty | UrlValue::Open => {
                            self.url = if matches!(ch, ':' | '/' | '?' | '#') {
                                UrlValue::Settled
                            } else {
                                UrlValue::Open
                            };
                        }
                        UrlValue::No | UrlValue::Settled => {}
                    }
                }
            }
            St::Unquoted => {
                if ws {
                    self.st = St::InTag;
                } else if ch == '>' {
                    return self.close_tag();
                }
            }
            St::Bang => match ch {
                '-' => self.st = St::BangDash,
                '[' => {
                    self.n = 1;
                    self.st = St::BangCdata;
                }
                '>' => self.enter_data(),
                _ => self.st = St::Bogus,
            },
            St::BangCdata => {
                const WANT: &[u8] = b"[cdata[";
                if ch.is_ascii() && ch.to_ascii_lowercase() as u8 == WANT[self.n] {
                    self.n += 1;
                    if self.n == WANT.len() {
                        self.refuse(CDATA);
                        self.n = 0;
                        self.st = St::Bogus;
                    }
                } else if ch == '>' {
                    self.enter_data();
                } else {
                    self.n = 0;
                    self.st = St::Bogus;
                }
            }
            St::BangDash => match ch {
                '-' => self.st = St::CommentStart,
                '>' => self.enter_data(),
                _ => self.st = St::Bogus,
            },
            // A comment ends where the WHATWG tokenizer ends it: `<!-->` and
            // `<!--->` at its start, later `-->` and `--!>`. `<!--!>` and
            // `<!---!>` end nothing (review I2 point 3).
            St::CommentStart => match ch {
                '-' => self.st = St::CommentStartDash,
                '>' => self.enter_data(),
                _ => self.st = St::Comment,
            },
            St::CommentStartDash => match ch {
                '-' => self.st = St::CommentEnd,
                '>' => self.enter_data(),
                _ => self.st = St::Comment,
            },
            St::Comment => {
                if ch == '-' {
                    self.st = St::CommentEndDash;
                }
            }
            St::CommentEndDash => {
                self.st = if ch == '-' {
                    St::CommentEnd
                } else {
                    St::Comment
                };
            }
            St::CommentEnd => match ch {
                '>' => self.enter_data(),
                '!' => self.st = St::CommentEndBang,
                '-' => {}
                _ => self.st = St::Comment,
            },
            St::CommentEndBang => match ch {
                '-' => self.st = St::CommentEndDash,
                '>' => self.enter_data(),
                _ => self.st = St::Comment,
            },
            St::Bogus => {
                if ch == '>' {
                    self.enter_data();
                }
            }
            St::Raw => {
                if ch == '<' {
                    self.st = St::RawLt;
                }
            }
            St::RawLt => {
                if ch == '/' {
                    self.n = 0;
                    self.st = St::RawLtSlash;
                } else if ch == '!' && self.raw == "script" {
                    self.n = 0;
                    self.st = St::RawBang;
                } else if ch != '<' {
                    self.st = St::Raw;
                }
            }
            St::RawLtSlash => {
                let want = self.raw.as_bytes();
                if self.n < want.len()
                    && ch.is_ascii()
                    && ch.to_ascii_lowercase() as u8 == want[self.n]
                {
                    self.n += 1;
                    if self.n == want.len() {
                        self.st = St::RawEnd;
                    }
                } else {
                    self.st = if ch == '<' { St::RawLt } else { St::Raw };
                }
            }
            St::RawEnd => {
                // `</title` is an end tag only when the name ends here.
                if ch == '>' {
                    self.enter_data();
                } else if ws || ch == '/' {
                    self.tag = self.raw.to_string();
                    self.end_tag = true;
                    self.raw = "";
                    self.n = 0;
                    self.slash = ch == '/';
                    self.st = St::InTag;
                } else {
                    self.st = if ch == '<' { St::RawLt } else { St::Raw };
                }
            }
            // Review M6/M9: `<!--` in script data opens its escaped state, in
            // which `<script>` and `</script>` move the tokenizer on ways the
            // scanner does not read; `<script><!--<script></script>-->` still
            // stands in the script after the template ends.
            St::RawBang => {
                if ch == '-' {
                    self.n += 1;
                    if self.n == 2 {
                        self.refuse(SCRIPT_ESCAPE);
                        self.n = 0;
                        self.st = St::Raw;
                    }
                } else {
                    self.n = 0;
                    self.st = if ch == '<' { St::RawLt } else { St::Raw };
                }
            }
            St::Lost => {}
        }
        None
    }

    fn slot(&self) -> Slot {
        match self.st {
            St::Data
            | St::Bang
            | St::BangDash
            | St::BangCdata
            | St::CommentStart
            | St::CommentStartDash
            | St::Comment
            | St::CommentEndDash
            | St::CommentEnd
            | St::CommentEndBang
            | St::Bogus => Slot::Text,
            St::Dq | St::Sq => Slot::Attr(self.attr.clone()),
            // The text of a script or style is code; that of the other text
            // elements is text.
            St::Raw | St::RawLt | St::RawLtSlash | St::RawEnd | St::RawBang => {
                if matches!(self.raw, "script" | "style") {
                    Slot::RawText
                } else {
                    Slot::Text
                }
            }
            // `<{{x}}` makes the value a tag name.
            St::Lt
            | St::EndTagOpen
            | St::TagName
            | St::InTag
            | St::AttrName
            | St::AfterAttrName
            | St::BeforeValue
            | St::Unquoted
            | St::Lost => Slot::Tag,
        }
    }

    /// Whether `sub` may stand here, and the reading after it.
    fn place(&mut self, sub: Sub) -> Result<(), &'static str> {
        use St::*;
        let why = match (sub, self.st) {
            (Sub::Children, Data) => None,
            (Sub::Children, Lost) => Some(IN_DOUBT),
            (Sub::Children, _) => Some(OUTSIDE_TEXT),
            (_, Lost) => Some(IN_DOUBT),
            (_, Lt | EndTagOpen | TagName | AttrName) => Some(IN_NAME),
            (_, RawLt | RawLtSlash | RawEnd) => Some(IN_RAW_END),
            (_, RawBang) => Some(IN_SCRIPT_ESCAPE),
            (_, Bang | BangDash | BangCdata) => Some(IN_COMMENT),
            (_, BeforeValue | Unquoted) => Some(IN_UNQUOTED),
            (Sub::Value, InTag | AfterAttrName) => Some(IN_NAME),
            (
                Sub::Value,
                CommentStart | CommentStartDash | Comment | CommentEndDash | CommentEnd
                | CommentEndBang,
            ) => Some(IN_COMMENT),
            (_, Dq | Sq) => match (sub, self.url) {
                (_, UrlValue::No | UrlValue::Settled) => None,
                (Sub::Value, UrlValue::Empty) => {
                    self.url = UrlValue::Whole;
                    None
                }
                _ => Some(IN_OPEN_URL),
            },
            (
                _,
                Data | InTag | AfterAttrName | CommentStart | CommentStartDash | Comment
                | CommentEndDash | CommentEnd | CommentEndBang | Bogus | Raw,
            ) => None,
        };
        match why {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }
}

/// How many readings the parser carries before it gives up on telling them
/// apart. Each `{{#if}}` whose body does not return to where it started adds
/// one, and each open text element one until its end tag; a template needs
/// about as many of those as it has unbalanced conditionals and nested text
/// elements, which in every shipped template is at most one.
const MAX_READINGS: usize = 16;

/// Every reading of a template that is still possible at one point of it.
///
/// A conditional is one reason there can be more than one: after
/// `{{#if a}}…{{/if}}` the markup is where it was before the `#if` when `a` was
/// false, and where the body left it when `a` was true. Most bodies end where
/// they began, and then the two readings are one. A text element is the other
/// (see [`TEXT_ELEMENTS`]): its text is read as text and as markup until its
/// end tag, where the two meet again. Where readings differ, a value is placed
/// by the narrowest grammar ([`Slot::Tag`]) rather than by a guess about which
/// one the browser takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Contexts(Vec<Scanner>);

impl Contexts {
    /// The start of a template: between tags.
    pub(crate) fn start() -> Self {
        Self(vec![Scanner::start()])
    }

    /// Read a piece of the template's own text.
    pub(crate) fn feed(&mut self, text: &str) {
        for ch in text.chars() {
            let mut forks = Vec::new();
            for s in &mut self.0 {
                if let Some(fork) = s.step(ch) {
                    forks.push(fork);
                }
            }
            if forks.is_empty() && self.0.len() == 1 {
                continue;
            }
            self.0.extend(forks);
            self.settle();
        }
    }

    /// Drop repeated readings; too many are one that gave up.
    fn settle(&mut self) {
        let mut unique: Vec<Scanner> = Vec::with_capacity(self.0.len());
        for s in self.0.drain(..) {
            if !unique.contains(&s) {
                unique.push(s);
            }
        }
        self.0 = unique;
        if self.0.len() > MAX_READINGS {
            self.0 = vec![Scanner::lost(self.refused())];
        }
    }

    /// The readings after a conditional: the ones before it and the ones its
    /// body ended in.
    pub(crate) fn join(&mut self, body: Contexts) {
        self.0.extend(body.0);
        self.settle();
    }

    /// Place a `{{…}}` here (review I1): the slot a value takes, or why no
    /// `{{…}}` of this kind may stand here in some reading of the template.
    pub(crate) fn place(&mut self, sub: Sub) -> Result<Slot, &'static str> {
        let slot = self.slot();
        for s in &mut self.0 {
            s.place(sub)?;
        }
        self.settle();
        Ok(slot)
    }

    /// Why the template's own text, read so far, is refused, if it is: it ran
    /// on after a value that had to stand alone ([`AFTER_WHOLE_URL`]), or it
    /// carries what no reading can follow ([`PLAINTEXT`], [`CDATA`],
    /// [`FOREIGN_RAW`], [`SVG_ANIMATION`], [`SRCDOC`], [`SCRIPT_ESCAPE`]).
    pub(crate) fn refused(&self) -> Option<&'static str> {
        self.0.iter().find_map(|s| s.refused)
    }

    /// Why the template, read to its end, is refused because of where it
    /// ended: anywhere but between tags and outside svg and math, in every
    /// reading ([`UNFINISHED`]). What the page puts after a template — a
    /// sibling, the rest of its parent — is read on from where it ends, so
    /// `<a title="` and a sibling `" o{{x}}nclick=…` would be a handler
    /// neither shows alone.
    pub(crate) fn unfinished(&self) -> Option<&'static str> {
        self.0
            .iter()
            .any(|s| s.st != St::Data || s.foreign > 0)
            .then_some(UNFINISHED)
    }

    /// Where a substitution at this point stands: the one slot every reading
    /// agrees on, or [`Slot::Tag`] when they disagree. Text and the text of a
    /// script or style agree on the narrower of the two: a value in a
    /// `<style>` is text only where the style is no style (in svg, say).
    pub(crate) fn slot(&self) -> Slot {
        let mut slots = self.0.iter().map(Scanner::slot);
        let Some(mut agreed) = slots.next() else {
            return Slot::Tag;
        };
        for slot in slots {
            agreed = match (agreed, slot) {
                (a, b) if a == b => a,
                (Slot::Text, Slot::RawText) | (Slot::RawText, Slot::Text) => Slot::RawText,
                _ => return Slot::Tag,
            };
        }
        agreed
    }
}

/// A template's static text with every `{{…}}` replaced by a space — the text
/// the definition rules read. The same blanking as the class scanner in
/// `ops.rs`: a tag that is not markup must not hide markup either side of it.
fn blank_tags(template: &str) -> String {
    let mut plain = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        plain.push_str(&rest[..open]);
        plain.push(' ');
        match rest[open + 2..].find("}}") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    plain.push_str(rest);
    plain
}

/// The first event-handler attribute in `text`, if any: a name that BEGINS
/// with `on` and continues with letters, then `=` —
/// `(?i)(?:^|[\s"'/<])on[a-z]+\s*=`. Anchored at the start of a name, so
/// `data-front=`, `data-tone=`, `data-cond=` and `aria-controls=` are not
/// handlers.
fn event_attribute(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let mut i = 0;
    while i + 2 < b.len() {
        let starts = i == 0
            || matches!(
                b[i - 1],
                b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'"' | b'\'' | b'/' | b'<'
            );
        if starts && b[i].eq_ignore_ascii_case(&b'o') && b[i + 1].eq_ignore_ascii_case(&b'n') {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_alphabetic() {
                j += 1;
            }
            if j > i + 2 {
                let name_end = j;
                while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\n' | b'\r' | b'\x0c') {
                    j += 1;
                }
                if j < b.len() && b[j] == b'=' {
                    return Some(text[i..name_end].to_ascii_lowercase());
                }
            }
        }
        i += 1;
    }
    None
}

/// `text` as a browser reads it for a scheme: numeric character references and
/// the three named ones that spell a scheme's punctuation decoded, tab,
/// newline and carriage return dropped (the URL parser drops them), lowercased.
/// So `java&#x09;script&colon;` reads as `javascript:`.
fn scheme_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let (decoded, used) = char_ref(tail);
        match decoded {
            Some(c) => out.push(c),
            None => out.push('&'),
        }
        rest = &tail[used..];
    }
    out.push_str(rest);
    out.chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// One character reference at the start of `s` (which starts with `&`):
/// the character and how many bytes it spanned, or `(None, 1)`.
///
/// Named references are case-sensitive, as in HTML (review M4): `&colon;`,
/// `&Tab;` and `&NewLine;` spell a scheme's punctuation, `&COLON;` and `&tab;`
/// are no references at all and `&Colon;` is U+2237. The display door reads
/// them with Python's `html.unescape`, which knows the same table.
fn char_ref(s: &str) -> (Option<char>, usize) {
    for (name, c) in [("&colon;", ':'), ("&Tab;", '\t'), ("&NewLine;", '\n')] {
        if s.starts_with(name) {
            return (Some(c), name.len());
        }
    }
    let Some(num) = s.strip_prefix("&#") else {
        return (None, 1);
    };
    let (radix, digits_at) = if num.starts_with(['x', 'X']) {
        (16, 3)
    } else {
        (10, 2)
    };
    let digits: String = s[digits_at..]
        .chars()
        .take_while(|c| c.is_digit(radix))
        .collect();
    if digits.is_empty() {
        return (None, 1);
    }
    let mut used = digits_at + digits.len();
    if s[used..].starts_with(';') {
        used += 1;
    }
    // A reference that names no character is no reference: the `&` stays
    // text and the scan goes on after it.
    match u32::from_str_radix(&digits, radix)
        .ok()
        .and_then(char::from_u32)
    {
        Some(c) => (Some(c), used),
        None => (None, 1),
    }
}

/// The definition rules on a template's markup (GH #869): no event-handler
/// attribute and no `javascript:` in its static text.
///
/// Asked at `component.define` and of every seeded component, from ONE
/// function, for the reason the glass rule is (see
/// [`crate::web::ops::check_glass_layer`]). `<script>` stays allowed: the
/// display's own shell carries its hook script that way, and whether a page
/// runs inline script is the Content-Security-Policy's decision, not this
/// parser's.
pub fn check_template_markup(name: &str, template: &str) -> Result<(), String> {
    let plain = blank_tags(template);
    if let Some(attr) = event_attribute(&plain) {
        return Err(format!(
            "component {name:?} carries the event-handler attribute {attr:?} — a component \
             reaches the cell through a binding (`phx-click` and friends), never through \
             inline script"
        ));
    }
    if scheme_text(&plain).contains("javascript:") {
        return Err(format!(
            "component {name:?} carries a `javascript:` URL — a link on a display leads to a \
             page, it does not run code"
        ));
    }
    Ok(())
}

/// The definition rule on `editable` (GH #869): a prop a browser may write
/// is never one the template renders raw. `editable` on an `"html"` prop would
/// make whatever one viewer typed arrive as markup at every other viewer.
pub fn check_editable(name: &str, prop_schema: &Value, editable: &Value) -> Result<(), String> {
    let Some(list) = editable.as_array() else {
        return Ok(());
    };
    for prop in list.iter().filter_map(Value::as_str) {
        if prop_schema.get(prop).and_then(Value::as_str) == Some("html") {
            return Err(format!(
                "component {name:?} makes {prop:?} editable, and {prop:?} is typed \"html\" — \
                 what a browser writes is text; type it \"text\" or drop it from editable"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn slot_after(text: &str) -> Slot {
        let mut c = Contexts::start();
        c.feed(text);
        c.slot()
    }

    #[test]
    fn the_scanner_names_the_place() {
        assert_eq!(slot_after("<p>"), Slot::Text);
        assert_eq!(slot_after(r#"<a href=""#), Slot::Attr("href".into()));
        assert_eq!(slot_after("<a HREF='"), Slot::Attr("href".into()));
        assert_eq!(
            slot_after(r#"<div style="--x: "#),
            Slot::Attr("style".into())
        );
        assert_eq!(slot_after("<div "), Slot::Tag);
        assert_eq!(slot_after("<div a="), Slot::Tag);
        assert_eq!(slot_after("<"), Slot::Tag);
        assert_eq!(slot_after("<!-- "), Slot::Text);
        assert_eq!(slot_after("<style>.a{color:"), Slot::RawText);
        // Where a script's text would be a tag if the element did not switch the
        // tokenizer (inside svg, say), the two readings disagree: a token.
        assert_eq!(slot_after("<script>var a = \"<b title='"), Slot::Tag);
        // Back out of raw text, and a quote in it moved nothing.
        assert_eq!(
            slot_after("<script>var a = \"<b>\";</script><i title=\""),
            Slot::Attr("title".into())
        );
        assert_eq!(slot_after("<script></scriptx><b title=\""), Slot::Tag);
        assert_eq!(slot_after(r#"<p class="a" data-x='y' z=w>"#), Slot::Text);
        // A comment ends where a browser ends it: `<!-->`, `<!--->` and
        // `--!>` close it too, so what follows is markup again.
        assert_eq!(slot_after(r#"<!--><a href=""#), Slot::Attr("href".into()));
        assert_eq!(slot_after(r#"<!---><a href=""#), Slot::Attr("href".into()));
        assert_eq!(
            slot_after(r#"<!-- a --!><a href=""#),
            Slot::Attr("href".into())
        );
        assert_eq!(slot_after(r#"<!-- a -> <a href=""#), Slot::Text);
        // Review I2: a text element is text to its own end tag, `</` before a
        // non-letter is a bogus comment, and `<!--!>` / `<!---!>` end nothing.
        assert_eq!(slot_after("<textarea>"), Slot::Text);
        assert_eq!(slot_after("<title>"), Slot::Text);
        assert_eq!(slot_after("<xmp><b>"), Slot::Text);
        assert_eq!(slot_after(r#"</ a=""#), Slot::Text);
        assert_eq!(slot_after(r#"</1 a=""#), Slot::Text);
        assert_eq!(
            slot_after(r#"</ a="><i title=""#),
            Slot::Attr("title".into())
        );
        assert_eq!(slot_after(r#"<!--!><a title=""#), Slot::Text);
        assert_eq!(slot_after(r#"<!---!><a title=""#), Slot::Text);
        assert_eq!(
            slot_after(r#"<!----!><a title=""#),
            Slot::Attr("title".into())
        );
    }

    /// Review I2 (#868/#869): the scanner is a cut of the WHATWG tokenizer, and
    /// every place it reads as a quoted value while a browser already reads
    /// markup lets `<scr{{x}}ipt>` back in. Each of these renders a script or a
    /// handler with an empty `x`.
    #[test]
    fn a_text_element_a_bogus_end_tag_and_a_comment_start_are_read_like_a_browser() {
        for bad in [
            // 1. RCDATA and RAWTEXT end at their own end tag only.
            r#"<textarea><b title="</textarea><scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>"></b></textarea>"#,
            r#"<title><b title="</title><scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>"></b></title>"#,
            r#"<xmp><b title="</xmp><scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>"></b></xmp>"#,
            r#"<noscript><b title="</noscript><img src=x on{{x}}error=window.__pwned=1>"></b></noscript>"#,
            r#"<noembed><b title="</noembed><a href=java{{x}}script:window.__pwned=1>t</a>"></b></noembed>"#,
            r#"<noframes><b title="</noframes><ifr{{x}}ame src=https://example.org></iframe>"></b></noframes>"#,
            r#"<iframe><b title="</iframe><scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>"></b></iframe>"#,
            // 2. `</` before a non-letter is a bogus comment to the first `>`.
            r#"</ a="><scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>">"#,
            r#"</1 a="><img src=x on{{x}}error=window.__pwned=1>">"#,
            "</{{x}}>",
            // 3. `<!--!>` and `<!---!>` do not end the comment they start.
            r#"<!--!><a title="--><scr{{x}}ipt>1</scr{{x}}ipt>">"#,
            r#"<!---!><a title="--><scr{{x}}ipt>1</scr{{x}}ipt>">"#,
            "<!--{{x}}-->",
            "<!---{{x}}-->",
        ] {
            assert!(crate::web::render::parse_template(bad).is_err(), "{bad}");
        }
        for ok in [
            "<textarea>{{t}}</textarea><!---->{{t}}</>{{t}}",
            "<title>{{t}}</title>",
            "<noscript>{{t}}</noscript>",
            "<!-->{{x}}",
            "<!--->{{x}}",
            "<!----!>{{x}}",
            "<!-- a --!>{{x}}",
            "<!-- a -- > b -->{{x}}",
            "</ {{x}}>",
            "<?x {{x}}>",
            "<script>a</scriptx>b</script>{{t}}",
            "<SCRIPT>x</Script >{{t}}",
            "<textarea>{{#if c}}</textarea>{{/if}}</textarea>",
        ] {
            assert!(
                crate::web::render::parse_template(ok).is_ok(),
                "{ok}: {:?}",
                crate::web::render::parse_template(ok)
            );
        }
    }

    /// Whether a text element switches the tokenizer depends on the tree
    /// around the template: not inside svg or math (where `<img>` breaks out
    /// again as HTML), `noscript` only with scripting on, and an older parser
    /// ignores `<style>` in a `<select>`. A template cannot know which of its
    /// parents it lands in, so its text is read both ways.
    #[test]
    fn a_text_element_is_read_both_ways() {
        for bad in [
            r#"<textarea><img src=x on{{x}}error=window.__pwned=1></textarea>"#,
            r#"<style><img src=x on{{x}}error=window.__pwned=1></style>"#,
            r#"<noscript><img src=x on{{x}}error=window.__pwned=1></noscript>"#,
            r#"<title><a href="java{{x}}script:1">t</a></title>"#,
        ] {
            let err = refused(bad);
            assert!(err.contains("stands"), "{bad}: {err}");
        }
        // A raw prop in a script or style body stays raw in both readings.
        let pieces =
            crate::web::render::parse_template("<style>{{&css}}</style><script>{{x}}</script>")
                .unwrap();
        let slots: Vec<Slot> = pieces
            .iter()
            .filter_map(|p| match p {
                crate::web::render::Piece::Raw { slot, .. }
                | crate::web::render::Piece::Prop { slot, .. } => Some(slot.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(slots, vec![Slot::RawText, Slot::RawText]);
    }

    /// Review I2 point 4: inside svg or math, `<style>` and `<script>` are no
    /// raw text but markup. `plaintext` never ends, and a CDATA section ends at
    /// `]]>` in svg but at the first `>` elsewhere.
    #[test]
    fn script_or_style_in_svg_plaintext_and_cdata_are_refused() {
        for bad in [
            r#"<svg><style><img src=x on{{x}}error=window.__pwned=1></style></svg>"#,
            "<svg><style>{{&css}}</style></svg>",
            "<math><script>{{&js}}</script></math>",
            "<svg><g><style>a{}</style></g></svg>",
            "<plaintext>{{x}}",
            "<plaintext>",
            r#"<svg><![CDATA[ x > <b title="]]><scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>"></b></svg>"#,
            "<p><![cdata[x]]></p>",
        ] {
            assert!(crate::web::render::parse_template(bad).is_err(), "{bad}");
        }
        for ok in [
            r#"<svg viewBox="0 0 1 1"><path d="M0 0"/></svg><script>{{&js}}</script>"#,
            "<svg/><style>{{&css}}</style>",
            r#"<svg><title>{{t}}</title></svg>"#,
            r#"<style>.a{background:url("data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg'></svg>")}</style>{{t}}"#,
            "<![if x]>{{t}}",
        ] {
            assert!(
                crate::web::render::parse_template(ok).is_ok(),
                "{ok}: {:?}",
                crate::web::render::parse_template(ok)
            );
        }
    }

    /// Review M3/M7/M8 and M6/M9 of #868/#869: places where a value loads or
    /// runs that no rule read as such. SVG animation sets an attribute to a
    /// value the rules read under another name (`to`, `values`), `srcdoc` is a
    /// whole document read as markup again, and `<!--` in a script puts the
    /// browser into escaped script data the scanner does not follow.
    #[test]
    fn svg_animation_srcdoc_and_a_script_escape_are_refused() {
        for bad in [
            r#"<svg><a><set attributeName="href" to="java{{x}}script:alert(1)"/><text>t</text></a></svg>"#,
            r#"<svg><a><animate attributeName="href" values="{{v}}"/><text>t</text></a></svg>"#,
            r#"<svg><a><animate attributeName="href" from="x" to="{{v}}"></animate></a></svg>"#,
            r#"<svg><animateMotion dur="1s"></animateMotion></svg>"#,
            r#"<svg><animateTransform attributeName="transform" by="{{b}}"/></svg>"#,
            r#"<SET attributeName="href" to="x">"#,
            r#"<iframe srcdoc="{{d}}"></iframe>"#,
            r#"<iframe srcdoc="<img src=x on&#101;rror=alert(1)>"></iframe>"#,
            r#"<iframe SRCDOC=x></iframe>"#,
            // M9: both readings end between tags, the browser is still in
            // the script (escaped script data) after the template ends.
            "<script><!--<script></script>-->",
            // M6: `x` stands in script data, not in text.
            "<script><!--<script></script>-->{{x}}</script>",
            "<script>a<!-- b --></script>",
            "<script>a<!{{x}}--</script>",
            "<script>a<!-{{x}}-</script>",
            "<script>a<!-{{#if c}}{{/if}}-</script>",
        ] {
            assert!(
                crate::web::render::parse_template(bad).is_err(),
                "{bad}: {:?}",
                crate::web::render::parse_template(bad)
            );
        }
        for ok in [
            r#"<svg viewBox="0 0 1 1"><g><path d="M0 0"/></g></svg>"#,
            r#"<object data="{{u}}"></object>"#,
            r#"<object data="/o/{{u}}"></object>"#,
            r#"<b data-x="{{v}}" data-set="{{v}}">{{v}}</b>"#,
            "<style><!-- a --></style>{{t}}",
            "<script>if (a <!x) {}</script>{{t}}",
            "<p><!-- a --></p><settle>{{t}}</settle>",
        ] {
            assert!(
                crate::web::render::parse_template(ok).is_ok(),
                "{ok}: {:?}",
                crate::web::render::parse_template(ok)
            );
        }
        // `data` loads what `<object>` shows: a URL attribute.
        assert!(
            crate::web::render::parse_template(r#"<object data="java{{x}}script:1"></object>"#)
                .is_err()
        );
        assert!(!fits(&Slot::Attr("data".into()), "javascript:x"));
        assert!(fits(&Slot::Attr("data".into()), "/pic.svg"));
    }

    /// A template renders next to others: what follows it on the page is read
    /// on from where it ends. `<a title="` then `" o{{x}}nclick=…` is a handler
    /// neither template shows alone. So a template ends between tags, outside
    /// svg and math, in every reading.
    #[test]
    fn a_template_ends_where_it_began() {
        for bad in [
            r#"<b title=""#,
            "<b title=\"{{t}}",
            "<textarea>",
            "<script>",
            "<!-- x",
            "<svg><g>",
            "<a href=x",
            "<",
            "<p>x</p><scr",
            "{{#if c}}<textarea>{{/if}}",
        ] {
            assert!(crate::web::render::parse_template(bad).is_err(), "{bad}");
        }
        for ok in ["", "text", "<p>{{t}}", "<svg></svg>", "<div><span>{{t}}"] {
            assert!(
                crate::web::render::parse_template(ok).is_ok(),
                "{ok}: {:?}",
                crate::web::render::parse_template(ok)
            );
        }
    }

    fn refused(t: &str) -> String {
        match crate::web::render::parse_template(t) {
            Ok(p) => panic!("{t:?} parsed: {p:?}"),
            Err(e) => e.0,
        }
    }

    /// Review I1 (#868/#869): a value, a conditional or `{{children}}` that
    /// stands where the template's own text is a NAME joins that text into a
    /// name the rules never read. Blanked, `<scr{{x}}ipt>` is two harmless
    /// words; rendered with an empty `x` it is `<script>`. So the parser
    /// refuses the place, not the value.
    #[test]
    fn a_substitution_never_stands_in_a_name() {
        for bad in [
            // The four the review rendered, and the attribute-name split.
            "<scr{{x}}ipt>window.__pwned=1</scr{{x}}ipt>",
            r#"<a on{{x}}click="window.__pwned=1">"#,
            r#"<a href="java{{x}}script:window.__pwned=1">"#,
            r#"<ifr{{x}}ame src="https://example.org"></iframe>"#,
            r#"<a h{{n}}="{{v}}">"#,
            // A whole name, with or without the `=` after it.
            "<{{x}}>",
            "<div {{a}}>",
            "<div {{a}}=alert(1)>",
            "<div {{a}} =alert(1)>",
            "<a x {{a}}=alert(1)>",
            // A conditional in the middle of a name.
            "<a o{{#if c}}n{{/if}}click=alert(1)>",
            "<scr{{#if c}}{{/if}}ipt>",
            // An unquoted value, a comment, a raw-text end tag.
            "<a href={{x}}>",
            "<a href=x{{x}}>",
            "<!-- {{x}} -->",
            "<!{{x}}- a -->",
            "<script>a</scr{{x}}ipt>",
            "<script>a<{{x}}/script>",
            // A URL whose scheme the static text has not settled yet.
            r#"<a href="{{x}}:alert(1)">"#,
            r#"<a href="{{a}}{{b}}">"#,
            r#"<a href="java{{#if c}}{{/if}}script:alert(1)">"#,
            r#"<a href="{{x}}&colon;alert(1)">"#,
            // Children are markup, and markup stands between tags only.
            r#"<a href="{{children}}">"#,
            "<script>{{children}}</script>",
        ] {
            let err = refused(bad);
            assert!(err.contains("stands"), "{bad}: {err}");
        }
        for ok in [
            r#"<a href="{{u}}" title="{{t}}">{{t}}</a>"#,
            r#"<a href="/c/{{id}}?x={{y}}#{{z}}">"#,
            r#"<a href="mailto:{{m}}">"#,
            r#"<b type="button"{{#if e}} phx-keyup="{{e}}"{{/if}}>"#,
            r#"<div class="s{{#if t}} thin{{/if}}" style="--s: {{s}}">{{children}}</div>"#,
            "<style>{{&faces}}</style><script>{{&js}}</script>",
            "<!-->{{x}}",
            "<p>{{#if a}}<i>{{a}}</i>{{/if}}</p>",
        ] {
            assert!(
                crate::web::render::parse_template(ok).is_ok(),
                "{ok}: {:?}",
                crate::web::render::parse_template(ok)
            );
        }
    }

    #[test]
    fn readings_that_disagree_take_the_narrowest_grammar() {
        let mut c = Contexts::start();
        c.feed("<b>");
        let mut body = c.clone();
        body.feed("<a href=\"");
        c.join(body);
        assert_eq!(c.slot(), Slot::Tag);
        // A body that ends where it began leaves one reading.
        let mut c = Contexts::start();
        c.feed("<b ");
        let mut body = c.clone();
        body.feed("class=\"x\"");
        c.join(body);
        c.feed(" title=\"");
        assert_eq!(c.slot(), Slot::Attr("title".into()));
    }

    #[test]
    fn a_binding_takes_an_event_name_and_no_command_list() {
        let at = Slot::Attr("phx-click".into());
        assert!(fits(&at, "tap"));
        assert!(fits(&at, "app:save/1-2_x.y"));
        assert!(fits(&at, ""));
        assert!(!fits(&at, r#"[["exec",{"attr":"onclick"}]]"#));
        assert!(!fits(&at, "a b"));
        assert!(!fits(&at, &"x".repeat(65)));
        assert!(fits(&Slot::Attr("phx-window-keyup".into()), "k"));
        assert!(!fits(&Slot::Attr("phx-window-keyup".into()), "[1]"));
        // Not bindings: the client executes nothing it finds there.
        assert!(fits(
            &Slot::Attr("phx-value-for".into()),
            r#"[["exec",{}]]"#
        ));
        assert!(fits(&Slot::Attr("phx-target".into()), "#a b"));
    }

    #[test]
    fn a_url_is_relative_or_names_an_allowed_scheme() {
        let at = Slot::Attr("src".into());
        for ok in [
            "pic.png",
            "/a/b?c=d#e",
            "https://example.org/x",
            "HTTP://example.org",
            "mailto:a@example.org",
            "tel:+491",
            "a/b:c",
            "?x=javascript:1",
            "",
            " https://example.org ",
        ] {
            assert!(fits(&at, ok), "{ok:?}");
        }
        for bad in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            " javascript:alert(1)",
            "java\tscript:alert(1)",
            "java\nscript:alert(1)",
            "data:text/html,x",
            "vbscript:x",
            "a:b",
        ] {
            assert!(!fits(&at, bad), "{bad:?}");
        }
        assert!(!fits(&Slot::Attr("xlink:href".into()), "javascript:x"));
        assert!(!fits(&Slot::Attr("formaction".into()), "javascript:x"));
    }

    #[test]
    fn a_style_value_cannot_start_a_declaration() {
        let at = Slot::Attr("style".into());
        for ok in ["1.5", "pane-1", "c.p", "10px 20%", "#fff", "-3", "a_b,c+d"] {
            assert!(fits(&at, ok), "{ok:?}");
        }
        for bad in [
            "0;background:url(//x)",
            "x\" style=\"a",
            "red}",
            "a\\62",
            "url(x)",
            "a:b",
        ] {
            assert!(!fits(&at, bad), "{bad:?}");
        }
        assert!(!fits(&at, &"1".repeat(129)));
        assert!(!fits(&Slot::RawText, "red;}body{x:url(y)"));
    }

    #[test]
    fn inside_a_tag_only_a_token_fits() {
        assert!(fits(&Slot::Tag, "data-x"));
        assert!(!fits(&Slot::Tag, "onclick=alert(1)"));
        assert!(!fits(&Slot::Tag, "javascript:x"));
        assert!(!fits(&Slot::Tag, "a b"));
    }

    #[test]
    fn an_int_is_an_integer_or_nothing() {
        assert_eq!(int_text(Some(&json!(42))), Some("42".into()));
        assert_eq!(int_text(Some(&json!(-7))), Some("-7".into()));
        assert_eq!(int_text(Some(&json!(u64::MAX))), Some(u64::MAX.to_string()));
        assert_eq!(
            int_text(Some(&json!("1790000000000"))),
            Some("1790000000000".into())
        );
        assert_eq!(int_text(Some(&json!("-3"))), Some("-3".into()));
        assert_eq!(int_text(None), Some(String::new()));
        assert_eq!(int_text(Some(&json!(null))), Some(String::new()));
        assert_eq!(int_text(Some(&json!(1.5))), None);
        assert_eq!(int_text(Some(&json!("4x"))), None);
        assert_eq!(int_text(Some(&json!(""))), None);
        assert_eq!(int_text(Some(&json!("-"))), None);
        assert_eq!(int_text(Some(&json!("1234567890123456"))), None);
        assert_eq!(int_text(Some(&json!("0;background:url(//x)"))), None);
        assert_eq!(int_text(Some(&json!(true))), None);
        assert_eq!(int_text(Some(&json!([1]))), None);
    }

    /// rev-3 I-3: the rule is anchored at the start of a name.
    #[test]
    fn an_event_handler_is_refused_and_a_name_ending_in_on_is_not() {
        for ok in [
            r#"<b data-front="{{f}}" data-tone="{{t}}" data-cond="c" aria-controls="x">"#,
            r#"<p>Turn it on = off</p>"#,
            r#"<button phx-click="{{e}}">on</button>"#,
            r#"<script>{{&js}}</script>"#,
        ] {
            assert!(check_template_markup("c", ok).is_ok(), "{ok}");
        }
        for bad in [
            r#"<a onclick="x()">"#,
            r#"<svg onload=x()>"#,
            r#"<img src=x OnError = "y">"#,
            r#"<b/onmouseover=x>"#,
            r#"<b {{a}}onfocus=x>"#,
        ] {
            let err = check_template_markup("c", bad).unwrap_err();
            assert!(err.contains("event-handler"), "{bad}: {err}");
        }
    }

    #[test]
    fn a_javascript_url_is_refused_however_it_is_spelled() {
        for bad in [
            r#"<a href="javascript:x()">"#,
            r#"<a href="JAVASCRIPT:x()">"#,
            "<a href=\"java\tscript:x()\">",
            r#"<a href="java&#x09;script&colon;x()">"#,
            r#"<a href="&#106;avascript:x()">"#,
            r#"<a href="java&Tab;script&colon;x()">"#,
            r#"<a href="java&NewLine;script:x()">"#,
        ] {
            let err = check_template_markup("c", bad).unwrap_err();
            assert!(err.contains("javascript"), "{bad}: {err}");
        }
        assert!(check_template_markup("c", r#"<a href="https://example.org">"#).is_ok());
    }

    /// Review M4: named character references are case-sensitive, as in HTML —
    /// `&COLON;` and `&tab;` are no references, `&Colon;` is U+2237. So the
    /// rule reads these as a browser does, and the display door (Python's
    /// `html.unescape`) reads them the same way.
    #[test]
    fn a_named_reference_is_read_with_its_case() {
        for ok in [
            r#"<a href="javascript&COLON;x()">"#,
            r#"<a href="javascript&Colon;x()">"#,
            r#"<a href="java&tab;script:x()">"#,
            r#"<a href="java&newline;script:x()">"#,
        ] {
            assert!(check_template_markup("c", ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn an_html_prop_is_never_editable() {
        let schema = json!({"body": "html", "v": "text"});
        assert!(check_editable("c", &schema, &json!(["v"])).is_ok());
        assert!(check_editable("c", &schema, &json!([])).is_ok());
        let err = check_editable("c", &schema, &json!(["body"])).unwrap_err();
        assert!(err.contains("\"body\""), "{err}");
    }
}
