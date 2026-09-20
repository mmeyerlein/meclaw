//! The I/O half of the `browser` cell: the browser, its pages and its viewers.
//!
//! Everything that touches the child process lives here, and the handler half
//! never touches a pipe. What crosses between them are two channels — commands
//! outwards, events inwards — which is what keeps "one task per actor" true for
//! a cell with a whole browser hanging off it.
//!
//! **A1′**: [`run_io`] runs for the cell's whole life. It returns only when the
//! handler closes a channel, which is the cell as a whole going away.

use crate::browser::cdp::{self, CapSite, CdpEvent, CdpPipe, SandboxVerdict};
use crate::browser::cell::{BrowserCommand, BrowserEvent};
use crate::browser::error::BrowserError;
use crate::browser::input::{self, Input, reshapes_the_page};
use crate::browser::pages::{PageReport, PageState, Register, Viewer, Viewport};
use crate::browser::params::BrowserParams;
use crate::browser::service::{BrowserLinkOpener, head, viewport_of_join};
use crate::sandbox::SandboxProfile;
use crate::stdio_child::ChildReaper;
use meclaw_colony::{Link, LinkFrame, LinkRefused, Registration, SurfaceEntry};
use meclaw_core::Path;
use meclaw_core::serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// The longest the cell watches the browser's child chain before it decides.
///
/// Bounded by `external_timeout_ms` so a test can shorten it, and capped here
/// so a generous operator timeout does not turn a refusal into a wait. A
/// packaged browser forks its zygote and its crash handler during startup, and
/// the fork plus the `unshare` is milliseconds — this is generous.
const SANDBOX_WATCH_CAP: Duration = Duration::from_secs(5);

/// How often the sandbox check looks while it waits.
const SANDBOX_POLL: Duration = Duration::from_millis(50);

/// What an operator does about a browser that did not start.
///
/// One sentence, used by every refusal that ends this way, because they all end
/// the same way for the reader: install a browser from the distribution's
/// packages and point the param at it (R-G11). meclaw ships none.
pub const PACKAGED_BROWSER: &str = "install a Chromium-based browser from your distribution's packages and point \
     params.chromium_path at it";

/// How long a page has to be unused before an interaction is told again.
///
/// The throttle sits in the CELL and not in the client (OR-G13): two outputs
/// watching one page would each throttle their own half of the story, and a
/// client sends inputs and nothing else.
pub const INTERACTION_QUIET_MS: i64 = 20_000;

/// One browser, with its pages and the pipe it is reached over.
///
/// It is a plain value owned by one task. The register is a field rather than
/// shared state, which is why nothing here is behind a lock.
pub struct Browser {
    /// The pipe. Every call on it carries the A-timeout.
    pub pipe: CdpPipe,
    /// What ends the child, through the pid this cell remembers.
    pub reaper: ChildReaper,
    /// Pages and contexts.
    pub register: Register,
    /// The params this life runs on.
    pub params: BrowserParams,
    /// The profile directory this life owns and takes with it (T9).
    pub profile_dir: PathBuf,
    /// Where the ceiling of this browser lives, read while it was still
    /// alive.
    ///
    /// The number that tells a memory cap apart from a crash is only worth
    /// anything after the death, and after the death the pid is gone.
    pub cgroup: Option<CapSite>,
    /// The next link id. A plain counter in the one task that owns it.
    next_link: u64,
    /// Reports that belong to no caller, waiting for the event lane.
    ///
    /// A verb answers its own caller through the receipt the handler writes,
    /// and that is the whole story for the page the verb NAMED. A displaced
    /// page is a second page, and the app that owns its card never asked about
    /// it — so its `closed` report cannot ride the answer. It is drained after
    /// every command, on the failing path too: the register has already given
    /// the page up, and a report that went missing there would leave a row in
    /// `cell.db` that the next life reopens.
    out_of_band: Vec<PageReport>,
}

/// One admitted viewer: what goes back to the door, and what stays here.
pub struct Admitted {
    /// What the socket door hands its client.
    pub link: Link,
    /// The id this viewer is known by here.
    pub id: u64,
    /// What the client sends, which the I/O half pumps into its own loop.
    pub from_client: mpsc::Receiver<LinkFrame>,
    /// The state change this join caused, if it caused one.
    pub report: Option<PageReport>,
}

/// What a viewer's socket asks of the I/O half.
pub enum LinkCommand {
    /// Somebody wants to watch a page.
    Join {
        /// The page, which is the topic suffix.
        page: String,
        /// The join payload minus the mount, one level deep (OR-G32).
        params: Value,
        /// Where the link, or the refusal, goes.
        answer: oneshot::Sender<Result<Link, LinkRefused>>,
    },
    /// A viewer sent something. Text only: a page never sends a picture.
    Frame {
        /// Which viewer.
        link: u64,
        /// Which page.
        page: String,
        /// What it said.
        text: String,
    },
    /// A viewer went away.
    Left {
        /// Which viewer.
        link: u64,
        /// Which page.
        page: String,
    },
}

impl Browser {
    /// Start a browser. Synchronous and await-free.
    pub fn start(
        params: BrowserParams,
        profile_dir: PathBuf,
    ) -> Result<(Self, mpsc::Receiver<CdpEvent>), BrowserError> {
        let (pipe, events, reaper) = cdp::spawn_browser(&params, &profile_dir)?;
        Ok((
            Self {
                pipe,
                reaper,
                register: Register::default(),
                params,
                profile_dir,
                cgroup: None,
                next_link: 1,
                out_of_band: Vec::new(),
            },
            events,
        ))
    }

    /// Wait for the browser to answer, then look at how it is confined.
    ///
    /// Fail-closed in both halves, and the second half is the one R-G13 put
    /// here: a browser whose own sandbox is not doing its work does not get a
    /// page. There is no knob to say otherwise (R-G11) — the operator's answer
    /// to this refusal is to install a browser from their distribution's
    /// packages.
    pub async fn ready(&mut self) -> Result<Value, BrowserError> {
        let version = match tokio::time::timeout(
            Duration::from_millis(self.params.startup_timeout_ms),
            self.pipe.call("Browser.getVersion", json!({}), None),
        )
        .await
        {
            Err(_) => {
                return Err(BrowserError::StartupTimeout {
                    ms: self.params.startup_timeout_ms,
                });
            }
            // A browser that dies before it answers is not a crash of a running
            // browser: it is a browser that never started. Exit 133 with "No
            // usable sandbox" on its stderr is the shape this takes when a
            // packaged browser is asked to run where its own sandbox cannot,
            // and an operator reading `browser_crashed` would go looking for a
            // page that never existed.
            Ok(Err(BrowserError::BrowserCrashed(cause))) => {
                // With the number, when the child has already been reaped:
                // "exit code 133" is what an operator looks up, and "the
                // browser is gone" is not.
                let how = match self.reaper.exited() {
                    Some(exit) => format!("{cause} ({})", exit.detail()),
                    None => cause,
                };
                return Err(BrowserError::SpawnFailed(format!(
                    "{how}; {PACKAGED_BROWSER}"
                )));
            }
            Ok(other) => other?,
        };

        let pid = self
            .reaper
            .pid()
            .ok_or_else(|| BrowserError::SpawnFailed(PACKAGED_BROWSER.to_string()))?;
        let watch = SANDBOX_WATCH_CAP.min(Duration::from_millis(self.params.external_timeout_ms));
        let deadline = std::time::Instant::now() + watch;
        let mut last;
        loop {
            match cdp::renderers_are_sandboxed(pid) {
                // One child elsewhere is the whole answer, and the first one
                // that is settles it. The ceiling is placed HERE and not
                // before the wait: a browser that re-homes itself does so
                // before it forks the child this verdict reads, so the window
                // this loop already keeps is the same window the ceiling
                // needs (measured 2026-09-20, five runs of the packaged
                // browser: re-homed after 46-62 ms, first foreign-namespace
                // child at 159-194 ms).
                SandboxVerdict::Sandboxed => {
                    self.the_ceiling_follows_the_browser(pid).await?;
                    return Ok(version);
                }
                // Not yet an answer. A child that has been forked but has not
                // reached its `unshare` is in this namespace for a few
                // milliseconds, and refusing on that first look would refuse
                // every browser some of the time.
                other => {
                    last = other;
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(SANDBOX_POLL).await;
                }
            }
        }
        Err(BrowserError::SpawnFailed(match last {
            SandboxVerdict::NotSandboxed => "the browser runs without its sandbox: install a \
                 packaged browser from your distribution and point params.chromium_path at it"
                .to_string(),
            // Nothing was observed, so nothing is asserted. A browser that
            // forked no process at all is one whose confinement cannot be
            // measured, and an unmeasured boundary is refused rather than
            // assumed.
            _ => "the browser started no process of its own, so whether its sandbox holds could \
                 not be observed; install a packaged browser from your distribution and point \
                 params.chromium_path at it"
                .to_string(),
        }))
    }

    /// Put the declared ceiling where the browser actually is (R-G7, #766).
    ///
    /// `params.sandbox.limits` is written onto a cgroup this cell creates and
    /// the child joins before `exec`. A child that re-homes itself afterwards
    /// takes none of it with it: measured on the packaged browser,
    /// 2026-09-20, five runs of five, the browser is in a scope of the
    /// service manager's making 46-62 ms after the spawn, and there
    /// `memory.max` reads `max`, `pids.max` `38416` and `cpu.max` `max`
    /// (finding B-G9). So the ceiling follows: the cell reads where the child
    /// went and puts the ceiling there -- by ASKING the service manager for
    /// it, not by writing the cgroup files. A scope is systemd's node, and
    /// the files a stranger writes into it last until the next
    /// `daemon-reload` and no longer (measured 2026-09-20, strand g11).
    ///
    /// Fail-closed (the contract, § 2). A ceiling that cannot be placed ends the
    /// start, because R-G7 — the browser never displaces the colony — hangs
    /// on nothing else. The one deliberate way past it is an explicitly
    /// written `{"trust": "trusted"}` (OR-G56), which declares no `limits` and
    /// so never reaches this code.
    async fn the_ceiling_follows_the_browser(&mut self, pid: u32) -> Result<(), BrowserError> {
        let limits = match &self.params.sandbox {
            Some(SandboxProfile::Restricted {
                limits: Some(l), ..
            }) => *l,
            // No ceiling declared: nothing to follow, and nothing to refuse.
            _ => {
                self.cgroup = cdp::cgroup_of(pid).map(CapSite::at);
                return Ok(());
            }
        };
        let witness = crate::sandbox::follow_process(
            pid,
            self.reaper.sandbox_dir(),
            &limits,
            Duration::from_millis(self.params.external_timeout_ms),
        )
        .await
        .map_err(|e| BrowserError::SpawnFailed(e.to_string()))?;
        // Read AFTER the move, not before it: the cgroup an operator needs
        // named after the death is the one the browser ended up in.
        self.cgroup = cdp::cgroup_of(pid).map(|dir| CapSite { dir, witness });
        Ok(())
    }

    /// The browser context id for `name`, minting one if this life has none.
    ///
    /// Contexts are ephemeral by design (R-G3): a context is an identity, and
    /// an identity that survived a browser restart would be a login this cell
    /// persisted without saying so.
    async fn context_id(&mut self, name: &str) -> Result<String, BrowserError> {
        if let Some(id) = self.register.contexts.get(name) {
            return Ok(id.clone());
        }
        let made = self
            .pipe
            .call("Target.createBrowserContext", json!({}), None)
            .await?;
        let id = made
            .get("browserContextId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BrowserError::SpawnFailed(
                    "the browser answered Target.createBrowserContext without an id".to_string(),
                )
            })?
            .to_string();
        self.register.contexts.insert(name.to_string(), id.clone());
        Ok(id)
    }

    /// Open a page, or make sure the one that stands there is at this address.
    ///
    /// Idempotent on purpose: the same page at the same address is a receipt
    /// and not a second page. An app that re-emits a card must not open a
    /// second browser tab for it.
    pub async fn in_open(
        &mut self,
        page: &str,
        url: &str,
        context: &str,
        viewport: Option<Viewport>,
    ) -> Result<PageReport, BrowserError> {
        if let Some(entry) = self.register.pages.get(page) {
            if entry.url == url && entry.state != PageState::Suspended {
                return self
                    .register
                    .report(page)
                    .ok_or_else(|| BrowserError::UnknownPage(page.to_string()));
            }
            return self.in_navigate(page, url).await;
        }
        if u32::try_from(self.register.pages.len()).unwrap_or(u32::MAX) >= self.params.max_pages {
            match self.register.oldest_suspended() {
                Some(spare) => {
                    self.close_target_of(&spare).await;
                    // The second half of OR-G7. A displacement that says
                    // nothing leaves the app holding a card whose page is
                    // gone, and leaves the ROW behind — `db::delete_page`
                    // hangs off the `closed` state — so the next life of this
                    // cell reopens the page it just gave up and spends the
                    // place again.
                    self.register.set_state(&spare, PageState::Closed);
                    if let Some(report) = self.register.report(&spare) {
                        self.out_of_band.push(report);
                    }
                    self.register.forget(&spare);
                }
                // Never a silent displacement (OR-G7): a page somebody may be
                // looking at is not given up to make room.
                None => {
                    return Err(BrowserError::TooManyPages {
                        max: self.params.max_pages,
                    });
                }
            }
        }
        self.open_fresh(page, url, context, viewport.unwrap_or_default())
            .await
    }

    /// Mint a target for a page this browser is not holding.
    ///
    /// Its own function, and not an arm of [`Browser::in_open`], because a
    /// suspended page is reopened from [`Browser::in_navigate`] as well — and
    /// two `async fn`s that call each other would need a box to do it.
    pub async fn open_fresh(
        &mut self,
        page: &str,
        url: &str,
        context: &str,
        shape: Viewport,
    ) -> Result<PageReport, BrowserError> {
        let context_id = self.context_id(context).await?;
        let made = self
            .pipe
            .call(
                "Target.createTarget",
                json!({
                    "url": "about:blank",
                    "browserContextId": context_id,
                    "width": shape.width,
                    "height": shape.height,
                    // A window of its own, and the size is why (wave G, g7,
                    // findings B-G13 and B-G17). Measured 2026-09-20 against
                    // Chromium 153.0.8010.36 over this pipe: a `width`/`height`
                    // on `Target.createTarget` is accepted only for the target
                    // that MAKES a window, so the first page of a context
                    // opened and the second was refused with `Target position
                    // can only be set for new windows` — which is why a
                    // restart brought back one row of the `pages` table out of
                    // three, and why a cell that holds more than one page per
                    // context held only one. Three pages in three windows,
                    // each with a screencast and every frame acknowledged,
                    // draw 30 frames in 6 s each: a window per page costs the
                    // picture nothing.
                    "newWindow": true,
                }),
                None,
            )
            .await?;
        let target = made
            .get("targetId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BrowserError::NavigateFailed(
                    "the browser answered Target.createTarget without a target id".to_string(),
                )
            })?
            .to_string();
        let attached = self
            .pipe
            .call(
                "Target.attachToTarget",
                json!({"targetId": target, "flatten": true}),
                None,
            )
            .await?;
        let session = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BrowserError::NavigateFailed(
                    "the browser attached to the target without a session id".to_string(),
                )
            })?
            .to_string();

        self.register.put(
            page,
            url,
            context,
            shape,
            Some(target),
            Some(session.clone()),
            PageState::Opening,
        );
        self.pipe
            .call("Page.enable", json!({}), Some(&session))
            .await?;
        self.apply_viewport(page).await?;
        self.pipe
            .call("Page.navigate", json!({"url": url}), Some(&session))
            .await?;
        self.register
            .report(page)
            .ok_or_else(|| BrowserError::UnknownPage(page.to_string()))
    }

    /// Render the page at the shape its register entry says (R-G5).
    ///
    /// A viewport that is reported and not applied is a lie the INPUT path
    /// pays for: the contract calls a pointer's coordinates "CSS pixels of the
    /// page viewport", and a page still rendered at the browser's own default
    /// turns every tap into a tap somewhere else. So the two move together —
    /// the register is written and the page is switched in the same breath.
    async fn apply_viewport(&mut self, page: &str) -> Result<(), BrowserError> {
        let Some((session, shape)) = self
            .register
            .pages
            .get(page)
            .and_then(|e| e.session.clone().map(|s| (s, e.viewport)))
        else {
            return Ok(());
        };
        self.pipe
            .call(
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": shape.width,
                    "height": shape.height,
                    "deviceScaleFactor": shape.dpr,
                    "mobile": shape.mobile,
                }),
                Some(&session),
            )
            .await
            .map(|_| ())
    }

    /// Send an open page somewhere else.
    pub async fn in_navigate(&mut self, page: &str, url: &str) -> Result<PageReport, BrowserError> {
        let session = match self.register.pages.get(page) {
            None => return Err(BrowserError::UnknownPage(page.to_string())),
            Some(entry) => entry.session.clone(),
        };
        let Some(session) = session else {
            // The page is suspended: its target is gone and its row is not.
            // Re-opening it is what a navigation means then, and the context
            // survives, so the identity does too.
            return self.reopen_at(page, Some(url)).await;
        };
        self.pipe
            .call("Page.navigate", json!({"url": url}), Some(&session))
            .await?;
        if let Some(entry) = self.register.pages.get_mut(page) {
            entry.url = url.to_string();
            entry.state = PageState::Opening;
            entry.updated_at = Register::now_ms();
        }
        self.register
            .report(page)
            .ok_or_else(|| BrowserError::UnknownPage(page.to_string()))
    }

    /// Bring a page whose target is gone back, keeping row and identity.
    ///
    /// `url` moves it somewhere else; `None` puts it back at the address its
    /// own row remembers. The register entry is UPDATED rather than forgotten
    /// and remade, so whoever is watching the page keeps watching it — and so
    /// the lookup is a `get` rather than an index, which in the I/O half of a
    /// long-running cell is the difference between a refusal and a panic that
    /// takes the browser with it.
    async fn reopen_at(
        &mut self,
        page: &str,
        url: Option<&str>,
    ) -> Result<PageReport, BrowserError> {
        let Some(entry) = self.register.pages.get(page) else {
            return Err(BrowserError::UnknownPage(page.to_string()));
        };
        let context = entry.context.clone();
        let shape = entry.viewport;
        let known = entry.url.clone();
        let target = url.unwrap_or(&known).to_string();
        self.open_fresh(page, &target, &context, shape).await
    }

    /// Close a page. The context stays: closing a window is not logging out.
    pub async fn in_close(&mut self, page: &str) -> Result<PageReport, BrowserError> {
        if !self.register.has(page) {
            return Err(BrowserError::UnknownPage(page.to_string()));
        }
        self.close_target_of(page).await;
        self.register.set_state(page, PageState::Closed);
        let report = self
            .register
            .report(page)
            .ok_or_else(|| BrowserError::UnknownPage(page.to_string()))?;
        self.register.forget(page);
        Ok(report)
    }

    /// Close a context — which is what logging out is here.
    pub async fn in_context_close(
        &mut self,
        context: &str,
    ) -> Result<Vec<PageReport>, BrowserError> {
        let pages = self.register.of_context(context);
        let mut reports = Vec::with_capacity(pages.len());
        for page in pages {
            self.close_target_of(&page).await;
            self.register.set_state(&page, PageState::Closed);
            if let Some(report) = self.register.report(&page) {
                reports.push(report);
            }
            self.register.forget(&page);
        }
        if let Some(id) = self.register.contexts.remove(context) {
            self.pipe
                .call(
                    "Target.disposeBrowserContext",
                    json!({"browserContextId": id}),
                    None,
                )
                .await?;
        }
        Ok(reports)
    }

    /// Every report that belongs to the event lane rather than to a caller.
    pub fn take_out_of_band(&mut self) -> Vec<PageReport> {
        std::mem::take(&mut self.out_of_band)
    }

    /// Admit one viewer to one page, and start the picture if it is the first.
    ///
    /// The screencast switch is `viewers` empty ↔ not empty, and that is the
    /// ONLY thing that turns it (R-G4). It is a rendering question — how many
    /// sockets want this picture — and never a curator's: whether a window is
    /// open is said by its level, and a level is system-wide.
    pub async fn join(&mut self, page: &str, params: &Value) -> Result<Admitted, LinkRefused> {
        if !self.register.has(page) {
            return Err(LinkRefused {
                status: 404,
                detail: format!("no page {page:?} is open on this browser"),
            });
        }
        // Contract § 2 names TWO ways back out of `suspended`, and this is the
        // second one. Without it a viewer was admitted onto a page with no
        // target: `start_cast` left at `session == None`, `set_state(Active)`
        // said the opposite, and `idle_since = None` kept `due_suspends` from
        // ever looking at the page again — no picture, and no way out.
        if self
            .register
            .pages
            .get(page)
            .is_some_and(|e| e.session.is_none())
            && let Err(e) = self.reopen_at(page, None).await
        {
            return Err(LinkRefused {
                status: 503,
                detail: e.detail(),
            });
        }
        let id = self.next_link;
        self.next_link += 1;
        let (to_cell_tx, to_cell_rx) =
            mpsc::channel::<LinkFrame>(meclaw_colony::surfaces::LINK_QUEUE);
        let (from_cell_tx, from_cell_rx) =
            mpsc::channel::<LinkFrame>(meclaw_colony::surfaces::LINK_QUEUE);
        let viewport = viewport_of_join(params);
        let Some(entry) = self.register.pages.get_mut(page) else {
            // Unreachable through the check above, and written as a refusal
            // rather than as an `expect`: this runs in the I/O half of a
            // long-running cell, where a panic takes the browser with it.
            return Err(LinkRefused {
                status: 404,
                detail: format!("no page {page:?} is open on this browser"),
            });
        };
        let was_empty = {
            let was_empty = entry.viewers.is_empty();
            entry.viewers.insert(
                id,
                Viewer {
                    out: from_cell_tx,
                    viewport,
                },
            );
            entry.idle_since = None;
            was_empty
        };
        let report = if was_empty {
            self.start_cast(page).await;
            self.register.set_state(page, PageState::Active);
            self.register.report(page)
        } else {
            // The state did not change, but the picture still has to reach
            // whoever just arrived (B-G3).
            self.keyframe(page).await;
            None
        };
        Ok(Admitted {
            link: Link {
                to_cell: to_cell_tx,
                from_cell: from_cell_rx,
            },
            id,
            from_client: to_cell_rx,
            report,
        })
    }

    /// One viewer went. The last one going stops the picture, not the page.
    pub async fn left(&mut self, page: &str, link: u64) -> Option<PageReport> {
        let now_empty = {
            let entry = self.register.pages.get_mut(page)?;
            entry.viewers.remove(&link);
            entry.viewers.is_empty()
        };
        if !now_empty {
            return None;
        }
        self.stop_cast(page).await;
        if let Some(entry) = self.register.pages.get_mut(page) {
            entry.idle_since = Some(Instant::now());
        }
        self.register.set_state(page, PageState::Background);
        self.register.report(page)
    }

    /// Ask the browser for this page's picture.
    async fn start_cast(&mut self, page: &str) {
        let (session, casting) = match self.register.pages.get(page) {
            None => return,
            Some(entry) => (entry.session.clone(), entry.casting),
        };
        let Some(session) = session else { return };
        if casting {
            return;
        }
        let s = &self.params.screencast;
        let asked = self
            .pipe
            .call(
                "Page.startScreencast",
                json!({
                    "format": s.format,
                    "quality": s.quality,
                    "maxWidth": s.max_width,
                    "maxHeight": s.max_height,
                }),
                Some(&session),
            )
            .await;
        match asked {
            Ok(_) => {
                if let Some(entry) = self.register.pages.get_mut(page) {
                    entry.casting = true;
                }
            }
            // A refusal used to be swallowed here, and that is why three proof
            // runs read "the page says `active`, and the journal carries no
            // screencast error" (wave G, g8, B-G20). It is a real answer and a
            // frequent one: measured 2026-09-20 against Chromium
            // 153.0.8010.36 over this pipe, `Page.startScreencast` sent in the
            // same breath as `Page.navigate` answers `-32000 Not attached to
            // an active page` and then sends NOTHING, for ever. A cell that
            // says nothing about it leaves the finding to a marker.
            Err(e) => {
                tracing::warn!(
                    page = %page,
                    detail = %e.detail(),
                    "browser: the screencast did not start"
                );
            }
        }
    }

    /// Make the browser draw one picture for somebody who just arrived.
    ///
    /// A screencast sends on CHANGE, and a page that stands still changes
    /// nothing. Measured 2026-09-19 against Chromium 153.0.8010.36 over this
    /// pipe (wave G, g5, finding B-G3): `Page.startScreencast` yields exactly
    /// ONE frame and then nothing at all while the page stands still; a second
    /// `Page.startScreencast` without a stop is refused (`-32000 Screencast is
    /// already active`) and yields nothing; `Page.stopScreencast` followed by
    /// `Page.startScreencast` yields one frame again. So the second and the
    /// third output to join a standing page used to see nothing — which is why
    /// G2, G5, G11, G13 and F10 all measured a dead canvas.
    ///
    /// Stop-and-start rather than `Page.captureScreenshot`: the frame then
    /// travels the ONE path a picture travels, carrying the metadata that
    /// fills the sixteen-byte head (device size and scroll offset) and under
    /// the same acknowledgement pacing. A screenshot would be a second picture
    /// path and a head this cell made up. Everybody already watching gets the
    /// extra frame too, which is a repaint and not a lie.
    ///
    /// ONE form for every case, and it is measured for every case (wave G, g8,
    /// B-G20, 2026-09-20, Chromium 153.0.8010.36 over this pipe): on a cast
    /// that is running, stop-and-start costs nothing — 90 frames in 5 s, then
    /// 89, then 90 across two keyframes; on a cast that was refused, the same
    /// pair repairs it — 90 frames in the 5 s after it. The suspicion this
    /// strand set out to test (a second start answering `-32000 Screencast is
    /// already active` and killing the stream) never happens, because the cell
    /// stops before it starts. What used to happen instead is the branch that
    /// is gone from here: `keyframe` returned in silence when `casting` was
    /// false, so the second and third output to arrive on a page whose cast
    /// had been refused repaired nothing and saw nothing, for ever (g5 review,
    /// minor M2, which predicted exactly this).
    async fn keyframe(&mut self, page: &str) {
        self.stop_cast(page).await;
        self.start_cast(page).await;
    }

    /// Make sure a page that somebody is watching is actually casting.
    ///
    /// The one repair point for a cast that never started. `casting` is the
    /// CELL's belief, and it is false both when nothing was ever asked and
    /// when the asking was refused; a page with viewers and no cast is a black
    /// canvas that reports `active`, which is the whole of B-G20.
    async fn cast_if_watched(&mut self, page: &str) {
        let needs = self
            .register
            .pages
            .get(page)
            .is_some_and(|entry| !entry.viewers.is_empty() && !entry.casting);
        if needs {
            self.keyframe(page).await;
        }
    }

    /// Stop asking. The page stays where it is.
    ///
    /// The browser is asked whenever the page has a session, and not only when
    /// this cell believes a cast is running. The two can differ — a refused
    /// start leaves `casting` false while a start that raced a `stopScreencast`
    /// would leave it true — and the cell's belief is the half that is wrong.
    /// Measured 2026-09-20 (wave G, g8): `Page.stopScreencast` on a page that
    /// is not casting answers `{}` and costs one round trip.
    async fn stop_cast(&mut self, page: &str) {
        let session = match self.register.pages.get(page) {
            None => return,
            Some(entry) => entry.session.clone(),
        };
        if let Some(session) = session {
            let _ = self
                .pipe
                .call("Page.stopScreencast", json!({}), Some(&session))
                .await;
        }
        if let Some(entry) = self.register.pages.get_mut(page) {
            entry.casting = false;
        }
    }

    /// One picture out of the browser, on its way to everybody watching.
    ///
    /// Distribution is `try_send` and nothing else: **the cell never waits for
    /// a viewer**. One that has stopped reading is given up with `4408` and the
    /// others keep their picture — the alternative, waiting, would make one
    /// slow browser tab the pace of every other screen.
    pub async fn on_frame(&mut self, event: &CdpEvent) -> Vec<PageReport> {
        let Some(session) = event.session_id.as_deref() else {
            return Vec::new();
        };
        let Some(page) = self.register.page_of_session(session) else {
            return Vec::new();
        };
        let meta = event.params.get("metadata").cloned().unwrap_or(json!({}));
        let dim = |key: &str, fallback: u32| -> u32 {
            meta.get(key)
                .and_then(Value::as_f64)
                .map(|v| v.max(0.0) as u32)
                .unwrap_or(fallback)
        };
        let frame_id = event
            .params
            .get("sessionId")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let picture = event
            .params
            .get("data")
            .and_then(Value::as_str)
            .map(crate::store::query::hamming::decode_base64)
            .transpose();
        let picture = match picture {
            Ok(Some(bytes)) => bytes,
            // A frame this cell cannot decode is not one to repair. The
            // acknowledgement still goes, or the browser would stop sending.
            _ => Vec::new(),
        };
        let shape = self.register.pages.get(&page).map(|e| e.viewport);
        let viewport = shape.unwrap_or_default();
        let mut wire = head(
            dim("deviceWidth", viewport.width),
            dim("deviceHeight", viewport.height),
            dim("scrollOffsetX", 0),
            dim("scrollOffsetY", 0),
        )
        .to_vec();
        wire.extend_from_slice(&picture);

        let now = Instant::now();
        let mut reports = Vec::new();
        let mut gave_up = Vec::new();
        let throttled;
        {
            let Some(entry) = self.register.pages.get_mut(&page) else {
                return reports;
            };
            for (id, viewer) in entry.viewers.iter() {
                if viewer
                    .out
                    .try_send(LinkFrame::Binary(wire.clone()))
                    .is_err()
                {
                    gave_up.push(*id);
                }
            }
            for id in &gave_up {
                if let Some(viewer) = entry.viewers.remove(id) {
                    let _ = viewer.out.try_send(LinkFrame::Close {
                        code: 4408,
                        reason: BrowserError::ClientTooSlow.detail(),
                    });
                }
            }
            // The busy window: frames that keep arriving without a pause. A gap
            // of more than two intervals is a page that went quiet, and the
            // window starts again from there.
            let interval = ack_interval(&self.params, entry.state == PageState::Throttled);
            match entry.last_frame {
                Some(last) if now.duration_since(last) <= interval * 2 => {
                    entry.busy_since.get_or_insert(now);
                }
                _ => entry.busy_since = Some(now),
            }
            entry.last_frame = Some(now);
            entry.ack_pending = Some((session.to_string(), frame_id));
            entry.ack_due = Some(entry.last_ack.map_or(now, |last| last + interval));
            throttled = entry.state != PageState::Throttled
                && entry.busy_since.is_some_and(|since| {
                    now.duration_since(since)
                        >= Duration::from_millis(self.params.throttle_after_ms)
                });
        }
        if !gave_up.is_empty() {
            tracing::warn!(page = %page, gave_up = gave_up.len(), "browser: a viewer stopped reading");
        }
        if throttled {
            self.register.set_state(&page, PageState::Throttled);
            if let Some(report) = self.register.report(&page) {
                reports.push(report);
            }
        }
        if self
            .register
            .pages
            .get(&page)
            .is_some_and(|e| e.viewers.is_empty())
        {
            self.stop_cast(&page).await;
            if let Some(entry) = self.register.pages.get_mut(&page) {
                entry.idle_since = Some(now);
            }
            self.register.set_state(&page, PageState::Background);
            if let Some(report) = self.register.report(&page) {
                reports.push(report);
            }
        }
        reports
    }

    /// One thing a viewer did, on its way into the page.
    ///
    /// Two things come out of it besides the page moving. The viewport of the
    /// page becomes the profile of the output that sent this (R-G5) — two
    /// screens watching one page have to agree on one, and the one somebody is
    /// touching is the one that is right. And the cell tells the topology that
    /// the page is being used, at most once every [`INTERACTION_QUIET_MS`]
    /// (OR-G13): it rides `active_at` on a `page` emission, never a fourth
    /// route and never a curator event.
    pub async fn on_input(
        &mut self,
        page: &str,
        link: u64,
        text: &str,
    ) -> (Option<BrowserError>, Vec<PageReport>) {
        let Some(session) = self
            .register
            .pages
            .get(page)
            .and_then(|e| e.session.clone())
        else {
            return (
                Some(BrowserError::UnknownPage(page.to_string())),
                Vec::new(),
            );
        };
        let parsed = match Input::parse(text) {
            Ok(parsed) => parsed,
            Err(e) => return (Some(BrowserError::InvalidInput(e)), Vec::new()),
        };
        let mut reports = Vec::new();
        // The input FIRST, the sender's shape after it (wave G, g7, finding
        // B-G16). A pointer's coordinates are CSS pixels of the page viewport,
        // and the viewport the sender measured them against is the one the
        // picture it holds was drawn at — never the one this call is about to
        // switch the page to. Resizing first put every FIRST tap of every
        // output into a layout that had just moved under it: measured
        // 2026-09-20 at Chromium 153.0.8010.36 through `run_io` and the real
        // door, a tap on a field at `left: 50%` of a page opened at 1280
        // missed it once the page had been switched to a phone's 390, focused
        // nothing, and the `Input.insertText` behind it went nowhere — which
        // is B-G16 and the same line G3 measured as "the click never reached
        // the page". The shape still takes effect here and not later, so the
        // NEXT frame is drawn for whoever is touching (R-G5); only the order
        // within the one call changed.
        for (method, params) in input::calls(&parsed) {
            if let Err(e) = self.pipe.call(method, params, Some(&session)).await {
                return (Some(e), reports);
            }
        }
        // And the shape only on an input that does NOT carry coordinates, or on
        // the end of a gesture (wave G, g9, finding B-G16). g7 put the input
        // before the shape inside ONE call, and that is right but not enough: a
        // click is THREE frames -- `move`, `down`, `up` -- and the viewer
        // computes all three against the ONE picture it holds. Reshaping on the
        // `move` therefore moves the layout out from under the `down` that is
        // already on its way. Measured 2026-09-20 in a colony, monitor profile
        // 1920x1080 on a page the app opened at 1280x800: the first click of
        // that output went to 640,400, the page was 1920 wide by the time it
        // arrived, the field at `left: 50%` now sat at 960 and was missed, and
        // the `Input.insertText` behind it went nowhere. The SECOND click of the
        // same output -- computed against a frame already 1920 wide -- put
        // `ZWEITER Versuch 42` into the field and into the page's own echo.
        // Holding the shape until the release keeps R-G5 (the shape is the
        // profile of whoever last touched, and the next frame is drawn for them)
        // and costs one gesture of delay.
        if reshapes_the_page(&parsed)
            && let Some(theirs) = self.register.viewport_of_link(page, link)
            && self.register.set_viewport(page, theirs)
            && let Err(e) = self.apply_viewport(page).await
        {
            return (Some(e), reports);
        }
        let now = Register::now_ms();
        if self
            .register
            .note_interaction(page, now, INTERACTION_QUIET_MS)
            && let Some(mut report) = self.register.report(page)
        {
            // The state has not changed, and this emission is not about one: it
            // carries `active_at`, which is how the app on the other end holds
            // its own window open (the document's § 5.11 sentence).
            report.active_at = Some(now);
            reports.push(report);
        }
        (None, reports)
    }

    /// Ask the page what it turned out to be, after a load finished.
    ///
    /// Under the A-timeout like every round trip. A question nobody answers
    /// leaves the field empty rather than guessing `text/html` — the app's PDF
    /// anchor hangs on this value, and a guess would put the anchor on every
    /// page.
    pub async fn note_content_type(&mut self, page: &str) -> Option<PageReport> {
        let session = self
            .register
            .pages
            .get(page)
            .and_then(|e| e.session.clone())?;
        let answered = self
            .pipe
            .call(
                "Runtime.evaluate",
                json!({"expression": "document.contentType", "returnByValue": true}),
                Some(&session),
            )
            .await
            .ok()?;
        let content_type = answered
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(Value::as_str)?;
        self.register
            .note_content_type(page, content_type)
            .then(|| self.register.report(page))
            .flatten()
    }

    /// Everything the document that just arrived says about itself.
    ///
    /// Both halves hang off `Page.loadEventFired`, and the title is PULLED
    /// rather than waited for. Measured 2026-09-19 against Chromium
    /// 153.0.8010.36 over this very pipe (wave G, g5, finding B-G1): with no
    /// discovery enabled `Target.targetInfoChanged` arrives **not once**, and
    /// with `Target.setDiscoverTargets` it arrives at navigation START
    /// carrying the URL in its `title` field and never again with the settled
    /// one. A cell that waits for that event reports an empty title for the
    /// whole life of every page — which is what G3, G4 and G7 read their proof
    /// back through, and why all three were red.
    ///
    /// `Target.getTargetInfo` after the load answers with the settled title
    /// AND the full address including the fragment, so OR-G39 is kept rather
    /// than bent: the address still comes from a target's info and never from
    /// `Page.frameNavigated`. The event arm in `translate` stays — a browser
    /// that does announce a rename is still heard.
    pub async fn after_load(&mut self, page: &str) -> Option<PageReport> {
        // The moment the refusal of B-G20 stops being true. `Page.navigate`
        // and a join land in the same breath when three outputs draw a card
        // the instant it appears, and a `Page.startScreencast` sent then is
        // answered `-32000 Not attached to an active page` (measured
        // 2026-09-20, Chromium 153.0.8010.36 over this pipe) — the page is not
        // an active page YET. This event is when it became one, and a viewer
        // that arrived alone has no second join to repair the cast with.
        self.cast_if_watched(page).await;
        let typed = self.note_content_type(page).await.is_some();
        let named = self.note_title(page).await;
        (typed || named)
            .then(|| self.register.report(page))
            .flatten()
    }

    /// Ask the browser what this page calls itself. `true` when it was news.
    async fn note_title(&mut self, page: &str) -> bool {
        let Some(target) = self.register.pages.get(page).and_then(|e| e.target.clone()) else {
            return false;
        };
        let Ok(answered) = self
            .pipe
            .call("Target.getTargetInfo", json!({"targetId": target}), None)
            .await
        else {
            return false;
        };
        let info = answered.get("targetInfo");
        let url = info
            .and_then(|i| i.get("url"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let title = info
            .and_then(|i| i.get("title"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.register
            .note_target_info(&target, url, title)
            .is_some()
    }

    /// Open again what a previous life was holding (OR-G17).
    ///
    /// Not streaming: a reopened page has no viewer yet, and a picture nobody
    /// asked for is bandwidth for nothing. The context is minted fresh, because
    /// a context is an identity and an identity that survived a browser restart
    /// would be a login this cell had persisted without saying so.
    pub async fn reopen(
        &mut self,
        rows: &[crate::browser::db::PageRow],
    ) -> Result<Vec<PageReport>, BrowserError> {
        let mut reports = Vec::with_capacity(rows.len());
        let mut refused: Option<BrowserError> = None;
        for row in rows {
            let shape = Viewport {
                width: row.viewport_w,
                height: row.viewport_h,
                dpr: row.viewport_dpr,
                mobile: row.mobile,
            };
            // One row that will not open is one page, not the recovery: a
            // single dead address used to cost a member every other window the
            // last life was holding.
            if let Err(e) = self
                .open_fresh(&row.page, &row.url, &row.context, shape)
                .await
            {
                tracing::warn!(page = %row.page, error = %e.detail(), "browser: a row did not reopen");
                refused.get_or_insert(e);
                continue;
            }
            self.register.set_state(&row.page, PageState::Reopened);
            if let Some(report) = self.register.report(&row.page) {
                reports.push(report);
            }
        }
        match refused {
            Some(e) if reports.is_empty() => Err(e),
            _ => Ok(reports),
        }
    }

    /// Give up the pages nobody has looked at for `suspend_after_ms`.
    ///
    /// The target goes and the CONTEXT stays: what is expensive about a page is
    /// its renderer, and what is precious about it is the identity it was
    /// opened under. The row stays too — it is what brings the page back.
    pub async fn suspend_due(&mut self) -> (Vec<PageReport>, Option<Instant>) {
        let after = Duration::from_millis(self.params.suspend_after_ms);
        let (due, next) = self.register.due_suspends(Instant::now(), after);
        let mut reports = Vec::with_capacity(due.len());
        for page in due {
            self.stop_cast(&page).await;
            self.close_target_of(&page).await;
            self.register.set_state(&page, PageState::Suspended);
            if let Some(report) = self.register.report(&page) {
                reports.push(report);
            }
        }
        (reports, next)
    }

    /// When the next page's idle time runs out, if one is counting.
    pub fn next_suspend(&self) -> Option<Instant> {
        self.register
            .due_suspends(
                Instant::now(),
                Duration::from_millis(self.params.suspend_after_ms),
            )
            .1
    }

    /// When the next acknowledgement is due, if one is.
    pub fn next_ack(&self) -> Option<Instant> {
        self.register
            .pages
            .values()
            .filter(|e| e.ack_pending.is_some())
            .filter_map(|e| e.ack_due)
            .min()
    }

    /// Acknowledge every frame whose moment has come.
    ///
    /// The acknowledgement goes out after the frame reached everybody AND after
    /// the interval has passed (OR-G8), which is the whole pace: the browser's
    /// next frame is the answer to this call.
    pub async fn flush_acks(&mut self) {
        let now = Instant::now();
        let due: Vec<(String, String, u64)> = self
            .register
            .pages
            .values()
            .filter(|e| e.ack_due.is_some_and(|due| due <= now))
            .filter_map(|e| {
                e.ack_pending
                    .as_ref()
                    .map(|(session, frame)| (e.page.clone(), session.clone(), *frame))
            })
            .collect();
        for (page, session, frame) in due {
            let _ = self
                .pipe
                .call(
                    "Page.screencastFrameAck",
                    json!({"sessionId": frame}),
                    Some(&session),
                )
                .await;
            if let Some(entry) = self.register.pages.get_mut(&page) {
                entry.ack_pending = None;
                entry.ack_due = None;
                entry.last_ack = Some(Instant::now());
            }
        }
    }

    /// Say something to one viewer, on its own link.
    async fn tell_viewer(&mut self, page: &str, link: u64, e: &BrowserError) {
        let told = json!({"type": "error", "error_code": e.error_code(), "detail": e.detail()});
        if let Some(entry) = self.register.pages.get(page)
            && let Some(viewer) = entry.viewers.get(&link)
        {
            let _ = viewer.out.try_send(LinkFrame::Text(told.to_string()));
        }
    }

    /// Close whatever target a page has, ignoring a browser that says no.
    ///
    /// A target this cell cannot close is one that is already gone, and the
    /// register is what says whether the page exists — not the browser.
    async fn close_target_of(&mut self, page: &str) {
        let target = self.register.pages.get(page).and_then(|e| e.target.clone());
        if let Some(target) = target {
            let _ = self
                .pipe
                .call("Target.closeTarget", json!({"targetId": target}), None)
                .await;
        }
        if let Some(entry) = self.register.pages.get_mut(page) {
            entry.target = None;
            entry.session = None;
            entry.casting = false;
        }
    }
}

/// The owned I/O state of a `browser` cell.
pub struct BrowserIo {
    /// The params this life runs on.
    pub params: BrowserParams,
    /// The profile directory this life's browser owns, and takes with it.
    pub profile_dir: PathBuf,
    /// The path the mount registers under.
    pub cell_path: Path,
    /// The process's mount table (ADR-0031).
    pub surfaces: std::sync::Arc<meclaw_colony::SurfaceRegistry>,
    /// The cell's OWN command channel, for what `on_start` has to ask before
    /// the substrate's sender exists anywhere (`BrowserCell::new`).
    pub from_handler: Option<mpsc::Receiver<BrowserCommand>>,
}

impl BrowserIo {
    /// The I/O state of one life.
    pub fn new(
        params: BrowserParams,
        profile_dir: PathBuf,
        cell_path: Path,
        surfaces: std::sync::Arc<meclaw_colony::SurfaceRegistry>,
    ) -> Self {
        Self {
            params,
            profile_dir,
            cell_path,
            surfaces,
            from_handler: None,
        }
    }
}

/// The I/O loop.
///
/// One round for the whole life (A1′). The browser is started here rather than
/// in the factory, because starting it is `async` and the factory's corridor is
/// not — and because a browser that could not start is a thing the handler has
/// to hear about, which needs a channel that exists.
pub async fn run_io(
    io: BrowserIo,
    events_tx: mpsc::Sender<BrowserEvent>,
    mut commands: mpsc::Receiver<BrowserCommand>,
) {
    let mut io = io;
    // Two command seams, one loop. The substrate hands `run_io` its own
    // receiver and gives the matching sender to `handle` alone, and `on_start`
    // runs before that sender exists anywhere — so the cell mints a pair of its
    // own. Either one closing means the handler is gone, which is the only
    // thing that ends this function (A1′).
    let mut from_handler = io.from_handler.take();
    // The links channel exists before the mount does: whoever finds the name on
    // the table must find something that answers.
    let (links_tx, mut links_rx) = mpsc::channel::<LinkCommand>(64);
    // The mount goes on the table once per life, at the top of it (ADR-0031). A
    // name another cell holds is an operator's mistake to read, not a reason to
    // tear this cell down: the cell then serves nobody and says so.
    let _guard = match io
        .surfaces
        .register(
            &io.params.mount,
            SurfaceEntry {
                kind: "browser",
                cell_path: io.cell_path.clone(),
                links: Some(Arc::new(BrowserLinkOpener {
                    links: links_tx.clone(),
                    external_timeout: Duration::from_millis(io.params.external_timeout_ms),
                })),
            },
        )
        .await
    {
        Ok((_handoff, registration)) => Some(MountGuard {
            surfaces: Arc::clone(&io.surfaces),
            mount: io.params.mount.clone(),
            registration,
        }),
        Err(e) => {
            let _ = events_tx
                .send(BrowserEvent::Failed(BrowserError::SpawnFailed(format!(
                    "the mount could not be taken: {e}"
                ))))
                .await;
            None
        }
    };

    // The four things this half owns are dropped in the reverse of the order
    // they are declared in, and that order is the decision (voice/io.rs is the
    // precedent). The child goes first, so nothing writes into the profile
    // while it is being removed; the profile goes next; the mount goes last, so
    // a join that arrives during the teardown is refused by this cell rather
    // than falling through to the listener's 404.
    let _profile = ProfileDir::at(io.profile_dir.clone());
    let started = Browser::start(io.params.clone(), io.profile_dir.clone());
    let (mut browser, mut cdp_events) = match started {
        Ok(pair) => pair,
        Err(e) => return park(events_tx, commands, from_handler, e).await,
    };
    if let Err(e) = browser.ready().await {
        return park(events_tx, commands, from_handler, e).await;
    }

    loop {
        let ack_at = browser.next_ack();
        let suspend_at = browser.next_suspend();
        tokio::select! {
            command = commands.recv() => match command {
                // The handler is gone. That is the only way out.
                None => break,
                Some(command) => {
                    let reports = serve(&mut browser, command).await;
                    if !send_all(&events_tx, reports).await {
                        break;
                    }
                }
            },
            own = recv_own(&mut from_handler) => match own {
                None => break,
                Some(command) => {
                    let reports = serve(&mut browser, command).await;
                    if !send_all(&events_tx, reports).await {
                        break;
                    }
                }
            },
            link = links_rx.recv() => match link {
                // Never: this half holds a sender of its own.
                None => break,
                Some(link) => {
                    let reports = serve_link(&mut browser, link, &links_tx).await;
                    if !send_all(&events_tx, reports).await {
                        break;
                    }
                }
            },
            event = cdp_events.recv() => match event {
                None => {
                    // The cap's own verdict travels with the death, while the
                    // cgroup still exists: `oom_kill=<n>` is what tells a memory
                    // cap apart from a crash.
                    let detail =
                        cdp::death_detail(browser.cgroup.as_ref(), "the browser's pipe closed");
                    // `Died` and NOT `Failed`, and then a park instead of a
                    // return (wave G, g5, finding B-G2). A return here ends
                    // `run_io` with `Ok(())`; `cell_task_long_running` then
                    // aborts the handler and `spawn_watcher` classifies the
                    // exit as `DeathKind::Normal`, so `handle_cell_died`
                    // REMOVES the cell instead of restarting it — measured at
                    // the real browser on 18.09. as
                    // `cell ended normally, removing from registry
                    // path=/browser` (`colony.rs:1561`), with `restart_count`
                    // still 0 and no page ever reopened. Parking keeps this
                    // half alive long enough for the handler to see `Died`,
                    // emit the `error` and panic, which is the one exit the
                    // supervisor restarts a long-running cell on.
                    let _ = events_tx
                        .send(BrowserEvent::Died(BrowserError::BrowserCrashed(detail)))
                        .await;
                    refuse_until_the_handler_goes(commands, from_handler).await;
                    return;
                }
                Some(event) => {
                    if event.method == "Page.screencastFrame" {
                        let reports = browser.on_frame(&event).await;
                        if !send_all(&events_tx, reports).await {
                            break;
                        }
                    } else if event.method == "Page.loadEventFired" {
                        // Once per load, and only then: what a page IS and
                        // what it CALLS ITSELF are both properties of the
                        // document that just arrived, and `after_load` asks
                        // for both in one go. It is `after_load` and not
                        // `note_content_type` alone since the fix round of
                        // 19.09.: the title half was built, measured at a real
                        // Chromium and then never wired in here, so the cell
                        // kept shipping the defect B-G1 described while the
                        // arm that proved it drove a second copy of this loop
                        // (`tests/gh766_the_packaged_browser_answers_for_itself.rs`,
                        // review C1). That arm now drives THIS function.
                        let page = event
                            .session_id
                            .as_deref()
                            .and_then(|s| browser.register.page_of_session(s));
                        if let Some(page) = page {
                            let report = browser.after_load(&page).await;
                            if !send_all(&events_tx, report.into_iter().collect()).await {
                                break;
                            }
                        }
                    } else if let Some(translated) = translate(&mut browser, event)
                        && events_tx.send(translated).await.is_err()
                    {
                        break;
                    }
                }
            },
            () = sleep_until(ack_at) => browser.flush_acks().await,
            () = sleep_until(suspend_at) => {
                let (reports, _next) = browser.suspend_due().await;
                if !send_all(&events_tx, reports).await {
                    break;
                }
            }
        }
    }
    // The ordinary end. Every other end — a panic, an abort, the backstop —
    // reaches the same three teardowns through `Drop`, which is why this is the
    // only line after the loop: a line that runs on one path out of two is a
    // line that hides the other one.
    browser.reaper.terminate(Duration::from_millis(2_000)).await;
}

/// The pace a page's frames are acknowledged at.
///
/// The whole flow control, and it is an acknowledgement rather than a timer: a
/// browser sends the next frame when the last one was acknowledged, so the cell
/// sets the pace by WHEN it answers and never by dropping pictures. Under the
/// brake it is two a second (OR-G18).
fn ack_interval(params: &BrowserParams, throttled: bool) -> Duration {
    if throttled {
        Duration::from_millis(500)
    } else {
        Duration::from_millis(1_000 / u64::from(params.screencast.max_fps).max(1))
    }
}

/// The cell's own command seam, or a wait that never ends when there is none.
async fn recv_own(
    from_handler: &mut Option<mpsc::Receiver<BrowserCommand>>,
) -> Option<BrowserCommand> {
    match from_handler {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Wait for a moment that may not exist. A branch with nothing to wait for
/// waits forever, which is what keeps it out of the way of the others.
async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
        None => std::future::pending().await,
    }
}

/// Every report the handler should hear about. `false` when it is gone.
async fn send_all(events_tx: &mpsc::Sender<BrowserEvent>, reports: Vec<PageReport>) -> bool {
    for report in reports {
        if events_tx.send(BrowserEvent::Page(report)).await.is_err() {
            return false;
        }
    }
    true
}

/// Answer every verb with the same refusal, for a cell that has no browser.
///
/// A1′: the half stays up rather than returning. A voluntary return would
/// silence this side while the handler kept running, and the substrate's outer
/// `select!` would then abort the surviving handler on the I/O completion.
async fn park(
    events_tx: mpsc::Sender<BrowserEvent>,
    commands: mpsc::Receiver<BrowserCommand>,
    from_handler: Option<mpsc::Receiver<BrowserCommand>>,
    e: BrowserError,
) {
    let _ = events_tx.send(BrowserEvent::Failed(e)).await;
    refuse_until_the_handler_goes(commands, from_handler).await;
}

/// Answer every verb with a refusal until the handler goes, and never return
/// before it does.
///
/// A1', in one place, for the two halves that need it: the browser that never
/// started ([`park`]) and the browser that died under the cell's feet. A
/// voluntary return is `DeathKind::Normal` to the watcher, and `Normal`
/// removes a cell from the registry instead of restarting it (B-G2).
async fn refuse_until_the_handler_goes(
    mut commands: mpsc::Receiver<BrowserCommand>,
    mut from_handler: Option<mpsc::Receiver<BrowserCommand>>,
) {
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => return,
                Some(command) => refuse_everything(command),
            },
            own = recv_own(&mut from_handler) => match own {
                None => return,
                Some(command) => refuse_everything(command),
            },
        }
    }
}

/// Owns the profile directory for exactly as long as the browser that uses it.
///
/// A browser leaves a profile behind: a cache, a session store, cookies, a
/// journal of what it visited. A cell whose contexts are ephemeral by design
/// (R-G3) must not leave that on disk for the next one, so the directory goes
/// with the life that made it — through `Drop`, which is the only code that
/// still runs on the paths a cancelled task takes.
///
/// It is **never** the cell directory. What this removes is
/// `params.user_data_dir`, `/tmp/meclaw-browser-<cell_id>` by default, and the
/// No-Delete policy is about `{root}`, which this is not.
pub struct ProfileDir {
    path: PathBuf,
}

impl ProfileDir {
    /// Take ownership of `path` for the caller's lifetime.
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for ProfileDir {
    fn drop(&mut self) {
        let path = std::mem::take(&mut self.path);
        // The cheap half first, and INLINE. `remove_dir` is one `rmdir`, it
        // succeeds exactly when the directory is empty, and empty is the shape
        // this cell keeps meeting: a snap-confined browser has a private
        // `/tmp`, so the profile it fills is invisible to the host and what
        // the cell made stands there empty (wave G, g5, finding B-G8 —
        // `/tmp/meclaw-browser-welle-g` and `/tmp/meclaw-browser-noexe`, 4,0 K
        // each, still standing after the colony went). Doing it here rather
        // than in a task is the whole point: a real colony tears this half
        // down by ABORTING it, and a task spawned from the `Drop` of an
        // aborted future is a task the runtime may never poll.
        if std::fs::remove_dir(&path).is_ok() {
            return;
        }
        // A browser profile is a cache directory: thousands of files, and
        // removing them is blocking I/O. In a `Drop` on a runtime thread that
        // stalls every other task on that worker, which in this cell includes
        // the watchdog of a colony. `MountGuard` next door already does its
        // teardown off the drop path; this does the same.
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(move || remove_profile(&path));
            }
            // No runtime — a process on its way out, or a plain `fn` test.
            // There is nobody left to stall, so it happens right here rather
            // than not at all.
            Err(_) => remove_profile(&path),
        }
    }
}

/// Take a profile directory away, saying so when it will not go.
fn remove_profile(path: &std::path::Path) {
    if let Err(e) = std::fs::remove_dir_all(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "browser: the profile directory could not be removed"
        );
    }
}

/// Holds a mount for exactly as long as the I/O half that registered it.
///
/// The ordinary end is this half returning, and this covers every other one: a
/// panic, the backstop, an abort. An entry that stayed behind would keep a live
/// opener over a browser nobody is serving, and a page joining in that window
/// would be admitted and then see nothing.
struct MountGuard {
    surfaces: Arc<meclaw_colony::SurfaceRegistry>,
    mount: String,
    registration: Registration,
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        // A `Drop` cannot await and the registry is behind an `Arc`, so the
        // removal is a task of its own — and only on a runtime thread, because
        // a process without one has no mount table left to keep tidy either.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let surfaces = Arc::clone(&self.surfaces);
        let mount = std::mem::take(&mut self.mount);
        let registration = self.registration;
        handle.spawn(async move {
            surfaces.unregister(&mount, &registration).await;
        });
    }
}

/// Answer one thing a viewer's socket asked.
async fn serve_link(
    browser: &mut Browser,
    command: LinkCommand,
    links_tx: &mpsc::Sender<LinkCommand>,
) -> Vec<PageReport> {
    match command {
        LinkCommand::Join {
            page,
            params,
            answer,
        } => match browser.join(&page, &params).await {
            Err(refused) => {
                let _ = answer.send(Err(refused));
                Vec::new()
            }
            Ok(admitted) => {
                let reports = admitted.report.into_iter().collect();
                // One pump per viewer, so the loop above selects over ONE
                // channel however many sockets are watching.
                tokio::spawn(pump(
                    admitted.id,
                    page,
                    admitted.from_client,
                    links_tx.clone(),
                ));
                let _ = answer.send(Ok(admitted.link));
                reports
            }
        },
        LinkCommand::Frame { link, page, text } => {
            let (refused, reports) = browser.on_input(&page, link, &text).await;
            if let Some(e) = refused {
                // On the LINK, not as a message: whoever sent an unreadable
                // frame is the one who has to read about it, and a message per
                // bad keystroke would be a message per keystroke.
                browser.tell_viewer(&page, link, &e).await;
            }
            reports
        }
        LinkCommand::Left { link, page } => browser.left(&page, link).await.into_iter().collect(),
    }
}

/// Carry one viewer's frames into the loop, and say when it goes.
async fn pump(
    id: u64,
    page: String,
    mut from_client: mpsc::Receiver<LinkFrame>,
    links: mpsc::Sender<LinkCommand>,
) {
    while let Some(frame) = from_client.recv().await {
        let text = match frame {
            LinkFrame::Text(text) => text,
            // A client sends pointers and keys, never a picture: the socket
            // door drops a binary push on a `page:` topic before it gets here,
            // and anything that did would be nothing this cell can read.
            LinkFrame::Binary(_) => continue,
            LinkFrame::Close { .. } => break,
        };
        if links
            .send(LinkCommand::Frame {
                link: id,
                page: page.clone(),
                text,
            })
            .await
            .is_err()
        {
            return;
        }
    }
    let _ = links.send(LinkCommand::Left { link: id, page }).await;
}

/// Answer one command against the browser.
async fn serve(browser: &mut Browser, command: BrowserCommand) -> Vec<PageReport> {
    match command {
        BrowserCommand::Open {
            page,
            url,
            context,
            viewport,
            answer,
        } => {
            let out = browser.in_open(&page, &url, &context, viewport).await;
            let _ = answer.send(out.map(|r| vec![r]));
        }
        BrowserCommand::Navigate { page, url, answer } => {
            let out = browser.in_navigate(&page, &url).await;
            let _ = answer.send(out.map(|r| vec![r]));
        }
        BrowserCommand::Close { page, answer } => {
            let out = browser.in_close(&page).await;
            let _ = answer.send(out.map(|r| vec![r]));
        }
        BrowserCommand::ContextClose { context, answer } => {
            let out = browser.in_context_close(&context).await;
            let _ = answer.send(out);
        }
        BrowserCommand::Reopen { rows, answer } => {
            let out = browser.reopen(&rows).await;
            let _ = answer.send(out);
        }
    }
    // The verbs answer their caller through the receipt the handler writes; a
    // state change worth an emission of its own comes from the browser, not
    // from the verb. The one exception is a page the verb did not name — a
    // displacement — and that is what this lane carries.
    browser.take_out_of_band()
}

/// Answer one command on a browser that never started.
fn refuse_everything(command: BrowserCommand) {
    let refusal = Err(BrowserError::SpawnFailed(
        "this cell has no browser: the one it was configured with did not start".to_string(),
    ));
    match command {
        BrowserCommand::Open { answer, .. } => {
            let _ = answer.send(refusal);
        }
        BrowserCommand::Navigate { answer, .. } => {
            let _ = answer.send(refusal);
        }
        BrowserCommand::Close { answer, .. } => {
            let _ = answer.send(refusal);
        }
        BrowserCommand::ContextClose { answer, .. } => {
            let _ = answer.send(refusal);
        }
        BrowserCommand::Reopen { answer, .. } => {
            let _ = answer.send(refusal);
        }
    }
}

/// The CDP-event seam, as a method so a test can reach it (I8/I9).
///
/// It used to be a private free function, which meant the only way to measure
/// `Target.targetInfoChanged -> Page` and `Inspector.targetCrashed ->
/// suspended` was to call the REGISTER directly and assert on what one had
/// just written. Both claims now have a seam a test drives from outside.
impl Browser {
    /// Turn one CDP event into something the handler can emit, or into nothing.
    ///
    /// `url` and `title` come from `Target.targetInfoChanged` and never from
    /// `Page.frameNavigated` (OR-G39): a frame's `url` does not carry the
    /// fragment, which lives separately in `urlFragment`, and the PDF anchor of
    /// the app on the other end is a fragment. A page whose address arrived
    /// without it would be navigated again on every emission.
    pub fn on_event(&mut self, event: CdpEvent) -> Option<BrowserEvent> {
        translate(self, event)
    }
}

fn translate(browser: &mut Browser, event: CdpEvent) -> Option<BrowserEvent> {
    match event.method.as_str() {
        "Target.targetInfoChanged" => {
            let info = event.params.get("targetInfo")?;
            let target = info.get("targetId").and_then(Value::as_str)?;
            let url = info.get("url").and_then(Value::as_str).unwrap_or_default();
            let title = info
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let page = browser.register.note_target_info(target, url, title)?;
            browser.register.report(&page).map(BrowserEvent::Page)
        }
        "Inspector.targetCrashed" => {
            let session = event.session_id.as_deref()?;
            let page = browser.register.page_of_session(session)?;
            // Renderer isolation is per SITE, not per page (OR-G29), so a crash
            // is handled for the page it reached and says nothing about the
            // others — they keep answering.
            browser.register.set_state(&page, PageState::Suspended);
            if let Some(entry) = browser.register.pages.get_mut(&page) {
                entry.target = None;
                entry.session = None;
                entry.casting = false;
                // The viewers go with the renderer. Left in place they are
                // watching nothing, `join` never sees an empty set again — so
                // no rejoin restarts the picture — and `idle_since` stays
                // `None`, which keeps `due_suspends` from ever looking at the
                // page either. They are told why and given back; a fresh join
                // is the way back (contract § 2).
                for (_, viewer) in entry.viewers.drain() {
                    let _ = viewer.out.try_send(LinkFrame::Close {
                        code: 4410,
                        reason: "the page's renderer crashed; join again to open it".to_string(),
                    });
                }
                entry.idle_since = Some(Instant::now());
            }
            browser
                .register
                .report(&page)
                .map(|report| BrowserEvent::Crashed { report })
        }
        _ => None,
    }
}
