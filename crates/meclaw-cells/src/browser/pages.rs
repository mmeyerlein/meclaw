//! The page register: what this browser is holding, as state rather than as a
//! question anybody has to ask the browser.
//!
//! The browser is a cache and this is the truth (R-G4). A page is a row in
//! `cell.db` and an entry here; the browser's target and session ids are what
//! this life happens to have for it, and they are gone with the browser.

use crate::browser::db::PageRow;
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

/// The shape of one page, as an output profile describes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// CSS pixels across.
    pub width: u32,
    /// CSS pixels down.
    pub height: u32,
    /// Device pixel ratio.
    pub dpr: f64,
    /// Whether the page is emulating a phone.
    pub mobile: bool,
}

impl Default for Viewport {
    /// What a page gets when nobody said: the screencast's own ceiling at 1×.
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            dpr: 1.0,
            mobile: false,
        }
    }
}

impl Viewport {
    /// `"<w>x<h>@<dpr>"` — the one string form, used on the wire and in a row.
    pub fn as_text(&self) -> String {
        viewport_text(self.width, self.height, self.dpr)
    }
}

/// The one spelling of a viewport on the wire.
///
/// A free function because the `page` body builds it from a ROW, which carries
/// the three numbers and not a [`Viewport`]; two `format!`s of the same shape
/// in two files is one place too many for a contract string to drift.
pub fn viewport_text(width: u32, height: u32, dpr: f64) -> String {
    format!("{width}x{height}@{dpr}")
}

/// One viewer of one page: where its frames go and what shape it is.
pub struct Viewer {
    /// Where this viewer's frames go.
    pub out: tokio::sync::mpsc::Sender<meclaw_colony::LinkFrame>,
    /// The output's own profile, when the join brought one.
    pub viewport: Option<Viewport>,
}

/// The seven states of a page. There is no `gone` (OR-G17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    /// Opening, or navigating: a target exists, the picture does not yet.
    Opening,
    /// At least one viewer is watching.
    Active,
    /// No viewer. The page stays open and stops casting.
    Background,
    /// The target was closed after `suspend_after_ms`. The context remains.
    Suspended,
    /// This cell's previous life was holding it, and this life opened it again.
    Reopened,
    /// The page is producing more than `max_fps` and is being acked slower.
    Throttled,
    /// Closed on request. The row is gone with it.
    Closed,
}

impl PageState {
    /// The contract string. What travels on a `page` emission.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::Active => "active",
            Self::Background => "background",
            Self::Suspended => "suspended",
            Self::Reopened => "reopened",
            Self::Throttled => "throttled",
            Self::Closed => "closed",
        }
    }
}

/// One page of this browser.
pub struct PageEntry {
    /// The page id: the app's card id and the topic suffix (OR-G3).
    pub page: String,
    /// The full address, fragment included (OR-G39).
    pub url: String,
    /// The browser context — an identity, not a window.
    pub context: String,
    /// The shape the page is rendered at. `mobile` lives in here and nowhere
    /// else: two places for one fact is two places to get it wrong.
    pub viewport: Viewport,
    /// The attached CDP session, while there is a target.
    pub session: Option<String>,
    /// The CDP target, while there is one.
    pub target: Option<String>,
    /// The page's own title, as `Target.targetInfoChanged` last said it.
    pub title: String,
    /// `document.contentType` of the last load. Empty until one finished.
    pub content_type: String,
    /// What this page is doing.
    pub state: PageState,
    /// Everybody watching. Empty ↔ not empty is the screencast switch (T6).
    pub viewers: HashMap<u64, Viewer>,
    /// Whether a screencast is running for this page.
    pub casting: bool,
    /// Since when nobody has been watching — the suspend deadline's anchor.
    pub idle_since: Option<Instant>,
    /// When the last screencast frame was acknowledged.
    pub last_ack: Option<Instant>,
    /// A frame waiting to be acknowledged: the CDP session and the frame's own
    /// id, plus when the acknowledgement is due (OR-G8).
    pub ack_pending: Option<(String, u64)>,
    /// When the pending acknowledgement is due.
    pub ack_due: Option<Instant>,
    /// When the last frame arrived, for the busy window below.
    pub last_frame: Option<Instant>,
    /// Since when this page has been producing frames without a pause. Past
    /// `throttle_after_ms` the cell slows its acknowledgements (OR-G18).
    pub busy_since: Option<Instant>,
    /// When this cell last told the topology that somebody used this page.
    pub interaction_told_at: Option<i64>,
    /// Unix milliseconds of the first open, kept across navigations.
    pub opened_at: i64,
    /// Unix milliseconds of the last change.
    pub updated_at: i64,
}

impl PageEntry {
    /// The row this page is, for `cell.db`.
    pub fn row(&self) -> PageRow {
        PageRow {
            page: self.page.clone(),
            url: self.url.clone(),
            context: self.context.clone(),
            viewport_w: self.viewport.width,
            viewport_h: self.viewport.height,
            viewport_dpr: self.viewport.dpr,
            mobile: self.viewport.mobile,
            state: self.state.as_str().to_string(),
            opened_at: self.opened_at,
            updated_at: self.updated_at,
        }
    }
}

/// Everything a `page` emission says about one page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageReport {
    /// The row, which carries page, url, context, viewport and state.
    pub row: PageRow,
    /// The state, as the enum rather than as its string.
    pub state: PageState,
    /// The page's title, empty when nothing said one yet.
    pub title: String,
    /// `document.contentType`, empty when no load has finished.
    pub content_type: String,
    /// Epoch milliseconds of the interaction this emission reports, if any.
    pub active_at: Option<i64>,
}

/// The pages and the contexts of one browser.
#[derive(Default)]
pub struct Register {
    /// Pages by id, ordered so "the oldest suspended one" has a meaning.
    pub pages: BTreeMap<String, PageEntry>,
    /// Context name → the browser context id this life minted for it.
    pub contexts: HashMap<String, String>,
}

impl Register {
    /// Unix milliseconds, the one clock this register reads.
    pub fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or_default()
    }

    /// Whether this page is known here.
    pub fn has(&self, page: &str) -> bool {
        self.pages.contains_key(page)
    }

    /// Move a page to `state`, returning the state it left — or `None` when the
    /// state did not change, which is what makes a `page` emission a report of
    /// a CHANGE rather than a heartbeat.
    pub fn set_state(&mut self, page: &str, state: PageState) -> Option<PageState> {
        let entry = self.pages.get_mut(page)?;
        if entry.state == state {
            return None;
        }
        let was = entry.state;
        entry.state = state;
        entry.updated_at = Self::now_ms();
        Some(was)
    }

    /// Everything a `page` emission needs about one page.
    pub fn report(&self, page: &str) -> Option<PageReport> {
        let entry = self.pages.get(page)?;
        Some(PageReport {
            row: entry.row(),
            state: entry.state,
            title: entry.title.clone(),
            content_type: entry.content_type.clone(),
            active_at: None,
        })
    }

    /// Every page whose idle time is up, and when the next one's will be.
    ///
    /// A deadline rather than a tick (R-G4): the loop sleeps until the earliest
    /// one instead of waking every second to ask. `0` never suspends, which is
    /// what an operator means by turning it off.
    pub fn due_suspends(&self, now: Instant, after: Duration) -> (Vec<String>, Option<Instant>) {
        if after.is_zero() {
            return (Vec::new(), None);
        }
        let mut due = Vec::new();
        let mut next: Option<Instant> = None;
        for entry in self.pages.values() {
            if entry.state == PageState::Suspended || entry.state == PageState::Closed {
                continue;
            }
            let Some(since) = entry.idle_since else {
                continue;
            };
            let at = since + after;
            if at <= now {
                due.push(entry.page.clone());
            } else {
                next = Some(next.map_or(at, |n: Instant| n.min(at)));
            }
        }
        (due, next)
    }

    /// The oldest page that is suspended, if there is one.
    ///
    /// What gets given up when `max_pages` is reached — and only that. A page
    /// somebody could be looking at is never displaced silently (OR-G7).
    pub fn oldest_suspended(&self) -> Option<String> {
        self.pages
            .values()
            .filter(|e| e.state == PageState::Suspended)
            .min_by_key(|e| e.updated_at)
            .map(|e| e.page.clone())
    }

    /// Note a fresh page, or update the one that stands under this id.
    #[allow(clippy::too_many_arguments)]
    pub fn put(
        &mut self,
        page: &str,
        url: &str,
        context: &str,
        viewport: Viewport,
        target: Option<String>,
        session: Option<String>,
        state: PageState,
    ) {
        let now = Self::now_ms();
        match self.pages.get_mut(page) {
            Some(entry) => {
                entry.url = url.to_string();
                entry.viewport = viewport;
                entry.target = target;
                entry.session = session;
                entry.state = state;
                entry.updated_at = now;
            }
            None => {
                self.pages.insert(
                    page.to_string(),
                    PageEntry {
                        page: page.to_string(),
                        url: url.to_string(),
                        context: context.to_string(),
                        viewport,
                        session,
                        target,
                        title: String::new(),
                        content_type: String::new(),
                        state,
                        viewers: HashMap::new(),
                        casting: false,
                        idle_since: Some(Instant::now()),
                        last_ack: None,
                        ack_pending: None,
                        ack_due: None,
                        last_frame: None,
                        busy_since: None,
                        interaction_told_at: None,
                        opened_at: now,
                        updated_at: now,
                    },
                );
            }
        }
    }

    /// Forget a page. Its row goes with it (`db::delete_page` is the caller's).
    pub fn forget(&mut self, page: &str) -> Option<PageEntry> {
        self.pages.remove(page)
    }

    /// Every page of one context, oldest first.
    pub fn of_context(&self, context: &str) -> Vec<String> {
        self.pages
            .values()
            .filter(|e| e.context == context)
            .map(|e| e.page.clone())
            .collect()
    }

    /// The shape the output that sent this input renders at (R-G5).
    ///
    /// Two screens of different sizes watching one page have to agree on one
    /// viewport, and the one somebody is touching is the one that is right.
    pub fn viewport_of_link(&self, page: &str, link: u64) -> Option<Viewport> {
        self.pages.get(page)?.viewers.get(&link)?.viewport
    }

    /// Give a page a new shape. `true` when it actually moved.
    pub fn set_viewport(&mut self, page: &str, viewport: Viewport) -> bool {
        let Some(entry) = self.pages.get_mut(page) else {
            return false;
        };
        if entry.viewport == viewport {
            return false;
        }
        entry.viewport = viewport;
        entry.updated_at = Self::now_ms();
        true
    }

    /// Note that somebody used this page, and say whether it is worth telling.
    ///
    /// The throttle sits HERE and not in the client (OR-G13): two outputs on
    /// one page would each throttle their own half of the story, and the client
    /// sends inputs and nothing else. `quiet` is how long the page has to have
    /// been unused before the topology hears about it again.
    pub fn note_interaction(&mut self, page: &str, now_ms: i64, quiet: i64) -> bool {
        let Some(entry) = self.pages.get_mut(page) else {
            return false;
        };
        let told = !matches!(entry.interaction_told_at, Some(last) if now_ms - last < quiet);
        if told {
            entry.interaction_told_at = Some(now_ms);
        }
        entry.updated_at = now_ms;
        told
    }

    /// Write down what `Target.targetInfoChanged` said, and say whether it was
    /// news. The page id it belongs to, or `None` for a target nobody holds.
    ///
    /// The address comes from here and never from `Page.frameNavigated`
    /// (OR-G39): a frame's `url` does not carry the fragment — that lives
    /// separately in `urlFragment` — and an app whose anchor was dropped
    /// navigates the page again on every emission it reads.
    pub fn note_target_info(&mut self, target: &str, url: &str, title: &str) -> Option<String> {
        let page = self.page_of_target(target)?;
        let entry = self.pages.get_mut(&page)?;
        let moved = !url.is_empty() && entry.url != url;
        let renamed = entry.title != title;
        if !moved && !renamed {
            return None;
        }
        if moved {
            entry.url = url.to_string();
        }
        if renamed {
            entry.title = title.to_string();
        }
        entry.updated_at = Self::now_ms();
        Some(page)
    }

    /// Write down what the last load turned out to be. `true` when it changed.
    pub fn note_content_type(&mut self, page: &str, content_type: &str) -> bool {
        let Some(entry) = self.pages.get_mut(page) else {
            return false;
        };
        if entry.content_type == content_type {
            return false;
        }
        entry.content_type = content_type.to_string();
        entry.updated_at = Self::now_ms();
        true
    }

    /// Which page owns this CDP session, if any.
    pub fn page_of_session(&self, session: &str) -> Option<String> {
        self.pages
            .values()
            .find(|e| e.session.as_deref() == Some(session))
            .map(|e| e.page.clone())
    }

    /// Which page owns this CDP target, if any.
    pub fn page_of_target(&self, target: &str) -> Option<String> {
        self.pages
            .values()
            .find(|e| e.target.as_deref() == Some(target))
            .map(|e| e.page.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register_with(pages: &[(&str, PageState)]) -> Register {
        let mut r = Register::default();
        for (n, (page, state)) in pages.iter().enumerate() {
            r.put(
                page,
                "https://example.com/",
                "default",
                Viewport::default(),
                Some(format!("tgt-{n}")),
                Some(format!("sess-{n}")),
                *state,
            );
        }
        r
    }

    #[test]
    fn a_state_that_did_not_change_is_not_a_change() {
        let mut r = register_with(&[("card-1", PageState::Active)]);
        assert_eq!(
            r.set_state("card-1", PageState::Background),
            Some(PageState::Active)
        );
        assert_eq!(
            r.set_state("card-1", PageState::Background),
            None,
            "a `page` emission reports a change, not a heartbeat"
        );
        assert_eq!(r.set_state("card-2", PageState::Active), None);
    }

    #[test]
    fn only_a_suspended_page_is_ever_given_up() {
        let mut r = register_with(&[
            ("card-1", PageState::Active),
            ("card-2", PageState::Background),
        ]);
        assert_eq!(
            r.oldest_suspended(),
            None,
            "nothing suspended means nothing to displace (OR-G7)"
        );
        r.set_state("card-2", PageState::Suspended);
        assert_eq!(r.oldest_suspended().as_deref(), Some("card-2"));
    }

    #[test]
    fn the_viewport_lives_in_one_place_and_reads_back_as_one_string() {
        let v = Viewport {
            width: 960,
            height: 600,
            dpr: 2.0,
            mobile: true,
        };
        assert_eq!(v.as_text(), "960x600@2");
        let mut r = Register::default();
        r.put(
            "card-1",
            "https://example.com/",
            "default",
            v,
            None,
            None,
            PageState::Opening,
        );
        let row = r.pages["card-1"].row();
        assert_eq!(
            (row.viewport_w, row.viewport_h, row.viewport_dpr),
            (960, 600, 2.0)
        );
        assert!(row.mobile, "mobile is the viewport's, not a second field");
    }
}
