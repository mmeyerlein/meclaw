# Changelog

All notable changes to MeClaw are documented in this file. One entry per released
package. The format loosely follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows SemVer (0.x: minor/patch bumps for additive features).

The public contract is the HTTP API, the template DSL, the template ports, the
mount a surface cell owns (a `web` page, a `proxy` peer mount and its frame) and the
documented `error_code` strings ([docs/stability.md](docs/stability.md)). Anything that breaks one of them is
listed under **Breaking** in its release, with the migration named. The Rust
crates are internals and move without notice.

## [Unreleased]

## [0.44.0] — 2026-09-23

A colony that lifts, exports and installs the way it promises. A lift compares what makes the
instance — `cell`, `params`, `contract` — and names every database it sets aside; a colony keeps its
own copy of every template version it ever instantiated, so the way back needs nothing from the
repository; a session keeper is addressed by its path, so every talky's sessions travel; an app
declares how it attaches and the builder draws it; an answer that comes back from a consult or a
delegation carries the turn that asked, and says when it is late; a recall's dossier counts axes,
not rows; the memory hive clears its working keys at its own exit; `meclaw --env-report` names
the keys nothing binds; and a peer proxy carries a credential on its outgoing POST.

### Added

- **A peer proxy can carry a credential on its outgoing POST** ([#828](https://github.com/mmeyerlein/meclaw/issues/828)).
  A `proxy` cell on platform `meclaw` takes an optional `params.auth` in one of two standard forms: a
  static header whose value comes from `${VAR}`, or the OAuth 2.0 client-credentials grant against a
  `token_url`, whose token is cached until shortly before `expires_in` and sent as
  `Authorization: Bearer`. A `401` from the far side is answered with one fresh token and one retry; a second `401` is `peer_refused`
  with the far side's answer. A token endpoint that does not answer, or answers without a token, is
  the new code `auth_unavailable` on the sending side, with a `refused` receipt and no request to the
  peer. `value` and `client_secret` must be written as `${VAR}`: a literal is refused at boot and at
  the mutation door, naming the key — for that the substrate now hands every cell type its `params`
  as declared, next to the resolved ones (`CellFactory::validate_declared_params`). Neither secret
  nor token appears in `Debug`, a receipt, a log line or `cell.db`. The mount side and the frame are
  unchanged, and no shipped template declares `auth`.
- **A colony keeps every template version it has instantiated** ([#811](https://github.com/mmeyerlein/meclaw/issues/811)).
  After a committed `add_nodes`, `swap_nodes` or `replace_nodes`, at boot and before every rescan,
  each `name@version` a node was instantiated from (its own template and every hop of its chain) is
  copied to the colony's own `local/` directory (`<name>@<version>/`), and its row in the `templates`
  table is repointed there under the same `template_id`. That directory is `<library>/local` when the
  library lies under the colony root and `<root>/templates/local` when it does not: a library the
  colony is merely pointed at — a repository checkout, a shared library — is never written, neither by
  a kept copy nor by `add_templates`, and the scan reads the colony's directory beside it. A library swap no longer removes older versions, and lifting a
  node back to an earlier version needs no `add_templates`. The scanner lets the kept copy win over a
  shipped directory of the same version (an identical twin is dropped, a changed one is skipped with
  its reason instead of aborting the scan), in the same way whatever order the directories are listed
  in, and a copy a crash left half-made is removed at the next boot or rescan. `add_templates` with a changed tree under a stored version
  is refused as the new `template_version_immutable`; the same tree again stays `template_name_taken`.
  `/colony/templates` entries carry `scanned_at`, and with `?name=` also `used_by`; lift receipts name
  `from_template_id` / `to_template_id` per child. No schema change.
- **Apps are installed from what they declare** ([#599](https://github.com/mmeyerlein/meclaw/issues/599)).
  An app template carries its own wiring as a declaration under `app` in `template.json` — screen,
  listened lanes, offers, observed tool results and the devices it drives — and the builder's fifth
  fast-lane recipe, `install_app`, renders the mutation from it: unguarded observer edges with the
  channel-less exit beside them, the binding to the app, `withdraw` on the view edge, tool and menu
  v-lanes from both surfaces of the generation, and a device's road. A word outside the vocabulary is
  refused as `app_declaration_invalid`. `builder@1.12.2`, `tools@1.4.3` (`build_topology` offers
  `install_app`), `colony-view@1.1.4` (declares itself), `meclaw-os@1.8.13` (pin). The app loader
  around the recipe stays in the register (`reg:app-loader`).
- **`meclaw --env-report`** ([#826](https://github.com/mmeyerlein/meclaw/issues/826)). A new mode flag
  names the `.env` keys no `${…}` in the colony's tree substitutes and the strict `${KEY}` references
  whose key the `.env` lacks. Names only, never a value; it takes no root lease, opens no `colony.db`
  and writes no log, so it answers for a running colony. Exit 0 with findings.

### Fixed

- **A lift keeps a child whose version only rewrote its prose, and names every store it sets aside**
  ([#773](https://github.com/mmeyerlein/meclaw/issues/773)). `replace_nodes` judges a standing child by
  the three blocks the substrate reads from a `config.json` — `cell` (less `cell.id`/`cell.provenance`),
  `params`, `contract`; a rewritten `description` no longer replaces the child and parks its `cell.db`,
  and a child that comes in through a `ref` is judged the same way, so a moved reference hop alone
  replaces nothing. A `replaced` entry of the receipt's `changes` carries `parked_path` and
  `parked_store`; `kept`, `added` and `left` entries are unchanged on the wire. `/colony/graph` nodes
  carry `active` and `parked`. Carrying the database over to a successor stays deferred
  (`reg:lift-store-carry-over`).
- **Every session keeper of a generation travels** ([#712](https://github.com/mmeyerlein/meclaw/issues/712)).
  `session-keeper@2.2.2`: the porter files its ledger under its own path inside the generation
  (`<talky>/session-keeper`) instead of the constant hive name, and `export_done` / the import receipt
  name that path. `assistant@2.8.1` draws the four transfer lanes at both talkys from one rule, so the
  typed channel's sessions are exported and imported too. `member@1.9.3` routes a keeper part on its
  path; `talky@5.2.2` pins the keeper. `examples/memory-import/build_import.py` reads the new layout and
  still reads an older export as `talky/session-keeper`.
- **Late answers keep their turn** ([#728](https://github.com/mmeyerlein/meclaw/issues/728)).
  `collector@4.2.1`: a handed-over call leaves a `depart` round row; the round an advisor's answer or a
  delegation opens is keyed `<member turn>~<deadline>~<hex>`, and its `answer` carries the member's turn
  as `hop.turn_id`, the round key as `hop.round_id` and `hop.late` (past the new `late_after_ms`,
  default 30 s). `assistant@2.8.1`: the consult edges drop `context.turn_id` and both talky ref markers
  set `late_after_ms`. `voice@2.2.1`: a late answer for a turn the call has left is not spoken, and a
  sidecar `fact` for a delegation already closed by the fallback is dropped after a turn change. Pins
  follow: `talky@5.2.2`, `cogny@5.0.3`, `freeswitch@2.1.2`.
- **The conversation brains name their app at OpenRouter.** `talky@5.2.2` (and with it the typed
  channel's `talky-chat`) and `cogny@5.0.3`: `brain` sends `http_referer` / `x_title`, overridable by
  `OPENROUTER_HTTP_REFERER` / `OPENROUTER_X_TITLE`, in the form the memory-hive cells already use.
  Until now every request of a talky or a core reached the provider without an app attribution.
- **A regenerated builder corpus reaches a colony** ([#811](https://github.com/mmeyerlein/meclaw/issues/811)).
  `builder-librarian@2.2.1` carries the corpus regenerated from this release's templates and docs; a
  colony keeps every version it instantiated and a stored version does not change, so a new corpus has
  to be a new version. `builder@1.12.2` pins it.
- **A recall's dossier counts axes, not rows** ([#691](https://github.com/mmeyerlein/meclaw/issues/691)).
  `memory-hive@3.4.1`: the rows of one `(subject, predicate)` take one of the six dossier seats before
  any axis takes a second, and a multi axis such as `has_child` still enumerates. A fact another leg
  nominated only leaves the dossier when that leg votes. Duplicate episodes fold in the keyword and
  semantic legs to one rank position, represented by the newest copy, with the `(seen: N)` count kept
  true; the semantic leg looks `2 × tier1_leg_limit` deep, and a cut it makes is reported as a cap. No
  new knob.
- **A question no longer rides past the memory hive** ([#823](https://github.com/mmeyerlein/meclaw/issues/823)).
  Every one of `memory-hive@3.4.1`'s twelve exit edges deletes the five recall working keys
  (`recall_query`, `memory_tier`, `recall_as_of`, `recall_window_from`, `recall_window_to`), so an
  answer, a refusal or a tool result leaves the hive without the question that produced it.
  `member@1.9.3` pins the new hive.

## [0.43.0] — 2026-09-23

The display's curator keeps its state in memory. Until now every pass carried the whole
screen plan in a header and wrote the state back as one row of the store, which is what made
the message log of a running colony grow by the gigabyte. The compose cell now runs
`resident`, the store keeps what the applications said, and a restart rebuilds the same
screen out of the rows and the tree the display holds. And the colony view stands in the dock
and opens on a tap.

### Changed

- **`display@2.7.0`: the curator keeps its state, the store keeps objects, no header carries
  the screen plan** (GH #809). The compose cell runs `resident` and holds the curator's state,
  the rows it knows and the tree it last sent in memory; the `views` store holds the
  application rows and one small row of history, written when it changes and never per pass;
  one pass sends `web` at most one `patch` and no `read`; a start or a refused patch reads the
  tree once; `display_views` is gone from every header. Measured on the live colony before:
  2.3 MB of header per pass, 1-2 GB of log per hour; after, with the same traffic, 25.6 MB of
  log per hour and no header field of the display's own above 200 B. The scenario driver prints a fourth line,
  `REBUILD n/n`: it kills the curator after every scenario and compares the rebuilt state one
  stroke later.
- **The shipped pins follow** (`builder@1.12.1`, `member@1.9.2`, `meclaw-os@1.8.12`). The
  builder's recipes grow a member's screen as `display@2.7.0` and its colony view as
  `colony-view@1.1.3`; the member level names `display@2.7.0` in its description; the shell
  pins `builder@1.12.1`. Only pins and text move, which is why all three are third-digit bumps.

### Fixed

- **The colony view stands in the dock and opens on a tap** (`colony-view@1.1.3`, GH #808).
  The layout now sends its picture as a `display-pane` window with `context: system`,
  `relevance`, `pinned`, `topic: colony`, a dock tile carrying the cell count and `touched`
  set to the snapshot's moment, so the display's curator scores it above the bar, a tap opens
  it and every committed mutation lifts it onto the canvas; before, the shell was the root of
  the view, the curator found no window and read the picture at 0.25 below a bar of 0.3.

### Security

- **`rustls` 0.23.40 → 0.23.45** (lock only; RUSTSEC-2026-0285). The TLS client accepted
  TLS 1.3 handshake messages sent at the wrong encryption level when they followed a
  key-changing message in the same record; the handshake stayed authenticated. Every outgoing
  HTTPS call of the colony goes through it.

## [0.42.0] — 2026-09-23

A colony can now speak to another colony. The `proxy` cell gains a third platform, `meclaw`:
a peer mount on the colony's one listener, one POST out, a lane contract declared on both
sides, and a receipt for every crossing. Underneath, an ingress emission carries the trace and
the budget it was handed, so one conversation stays one trace across two message logs.
`affinity` learns that a directory is an audience the member decides. The peer mount and its
frame join the public contract (see [docs/stability.md](docs/stability.md)). Proven by a test
that runs two real colonies behind a reverse proxy.

### Added

- **A colony speaks to another colony over a declared lane** (GH #617, #814, #815, #818,
  ADR-0028). The `proxy` cell gains a third platform, `meclaw`: one instance per contract class
  mounts on the colony's one listener and dials the peer's mount with one POST carrying a
  wire-v1 frame; the lane contract lives in `params` on both sides, each side judges its own
  edge, a field the lane does not name is refused rather than stripped, `hop` never crosses,
  every crossing leaves a receipt on both sides and every refusal one on the side that refused
  and, once it went over the wire, on the sender as well. The sending colony's identity comes
  from the header a reverse proxy in front sets, never from the frame, so a frame that carries
  a sender field is `invalid_frame`, and so is one whose body would not be deliverable on the
  far side. The client follows no redirects. Nine `error_code` strings, three of them the words
  already in use; the canonical dead-letter list is untouched.
- **An ingress emission carries the trace and the budget it was handed** (GH #617, #813). A cell
  that declares `contract.ingress.carries_trace` may emit a message it did not originate with
  the `trace_id` and `ttl` from the wire; one conversation stays one trace across two message
  logs, and a crossing at zero is refused rather than made. A cell without the declaration has
  no handle, and a source emission without one still gets the colony's default budget.
- **`affinity@3.4.0`: a proposal names its audience, and a directory audience is never
  auto-accepted** (GH #617, #816). `propose` carries `audience`, and the member's verdict keeps
  it; when it begins with `directory:` the proposal stays `open` for the member to decide,
  whatever the caller asked. The `peer` slot of a brief now carries the address an agent entity
  holds under `mx.peer`. `member@1.9.1` pins the new version, and its README says that a
  channel value may be composed from the class and the counterpart, which is how one peer
  channel keeps one session per conversation partner (#817).

## [0.41.1] — 2026-09-22

A patch release: eight small defects, each one measured before it was fixed. Three test rigs
stopped measuring the host they ran on -- the screencast give-up arm has a floor of milliseconds
where it used to take three and a half seconds alone, the attachment timeout rig holds both ends
of its FIFO so no blocking task is left behind, and the quarantine entry that carried it went
with the fix. A browser the cell refuses after it has already started is ended before the
refusal is spoken, and the refusal says what happened to the process. Paging the message log by
cursor is a range read again. And three gates learned to read what they were looking at: a
roadmap horizon holds bullets only, a gate receipt carries the stations its run planned, and the
page-weight markers of the display lab pick their lid by the target they measure.
`browser@1.0.1` is the only version this release moves.

### Fixed

- **The browser cell ends a browser it refuses** (GH #772, `browser@1.0.1`). A refusal after
  the spawn -- a browser whose renderers never left the cell's user namespace, or whose ceiling
  could not be placed -- left the process running for as long as the cell stayed refused. The
  cell now ends the process group before it parks, and the refusal names the pid, the cgroup
  the browser sat in and how it ended.
- **Paging the message log by cursor is a range read again** (GH #770). The keyset predicate
  was written as an `OR`, which no planner could answer as a range: it read rows the page does
  not contain and sorted them, so every page cost more than the one before it. The predicate is
  a row value now, `message_log` carries an index on `(created_at, id)` -- picked up by an
  existing database on its next open -- and the query plan is pinned by a test.
- **The export audit reads a gate receipt against what the run planned** (GH #769). A gate
  receipt carries the planned station list, and a station the diff did not ask for is no longer
  read as a station that was skipped.
- **A paragraph under a roadmap horizon is a finding** (GH #774). The anchor gate read only
  bullets; a line under Now, Next, Later or Alongside that is not a bullet now fails the gate,
  and the gate has a test file of its own. Twelve such lines stood in the roadmap when the gate
  first looked.
- **The page-weight markers of the display lab carry two lids** (GH #738). A throwaway
  colony and a shipped instance weigh differently, and the marker now picks its lid by the
  target it measures.
- **Three test rigs measure a promise instead of a host** (GH #771, #804, #748). The
  screencast give-up arm has a floor of milliseconds instead of seconds -- 0.54 s where it
  used to take 3.36 s alone and run out of a two-minute window under load -- the attachment
  timeout rig holds both ends of its FIFO so no blocking task is ever orphaned, and its
  quarantine entry is gone with the fix. The named-work-item trip sits above the colony's own
  idle beat.

## [0.41.0] — 2026-09-22

A minor release: a `voice` cell can hold one session in which a model hears the
caller and answers in it. `voice@2.1.0` adds `DuplexProvider` beside recognition
and synthesis -- one session carrying audio in both directions, `gpt_live` over
one WebSocket or `echo` for the wire without a model -- and three lanes that
exist only there: `spoken` for the assistant side, `delegation` for the errand
the model hands the backend mid-call, and `in_advise` for a fact, a piece of
context or a correction brought into a running conversation. `voice@2.2.0` gives
that session a clock of its own, because the provider's running meter arrived at
no point inside 45 s on a line that heard only silence, keeps the socket alive
with a ping whose own pong resets the idle deadline, and closes a delegation
nobody answered with a single sentence. The road around the cell is drawn to
match: `freeswitch@2.1.0` puts a live model behind the media half, `member@1.9.0`
carries a delegation across to the assistants and the three advice sections back
down, `assistant@2.8.0` takes that errand in as a lane, `collector@4.2.0`
assembles for a brain that advises the model on the line rather than answering
the caller, and `talky@5.2.0` accepts the lane while `talky@5.2.1` lets a sidecar
section be the sentence itself. Beside that, the dead-letter read answers newest
first, a grown child hears the two lanes its level had just learned
(`builder@1.12.0`), and `scripts/test-tier.sh` builds where `scripts/gate.sh`
builds.

### Added

- **A duplex provider is the third seam of the `voice` cell** (`voice@2.1.0`,
  GH #779, GH #789, ADR-0023). `DuplexProvider` holds one session that carries
  audio in both directions and reports what was said beside it, so a model can
  hear the caller itself and answer with a voice where a recogniser, an
  assistant and a synthesis did the same work in three steps. Two
  implementations ship, `gpt_live` over one WebSocket and `echo` for the wire
  without a model. A `duplex` block excludes `stt` and `tts`, and a cell
  without one is unchanged.

  Audio still terminates in the I/O half. One client frame becomes one frame to
  the provider, unbuffered and unpaced, and no sample becomes a message. There
  is no cell type of its own, no webhook, no second door and no reconnect: a
  session is the conversation, so a failed provider ends the call with
  `duplex_failed` on the `error` lane and `1011` on the socket.

  Three lanes are new and exist only here. `spoken` mirrors `partial` for the
  assistant side and takes up a name the wire protocol had reserved;
  `delegation` leaves when the model hands work to the backend; `in_advise`
  brings a fact, a piece of context or a correction into a running
  conversation, chosen by `hop.section`. The `error_code` list of the cell
  grows by `duplex_failed`, `duplex_warning`, `bad_section` and `wrong_engine`,
  and the `hello` frame grows by `duplex`. Turns are cut on the model's
  timeline rather than by the cell, `speak_end` comes out of quiet rather than
  out of the end of a synthesis, and an interjection under
  `backchannel_max_ms` is a backchannel instead of a barge-in.

  Every duplex emission stamps `hop.engine: 'duplex'`; a cascade stamps
  nothing, so a colony wired before this version sees exactly the hops it
  always saw. One format serves both directions, `16000` or `24000`, and the
  cell still never resamples.

- **A duplex session keeps a clock of its own, and a delegation nobody answers
  is closed by the cell** (`voice@2.2.0`, GH #798, GH #793). Turns in a duplex
  session are cut on the model's timeline, so a caller who stops talking closes
  a turn only when time passes -- and on a line where nobody talks, no fragment
  arrives to say that it did. The provider's running meter
  (`session.usage.updated`) carried that time until now, and it does not hold:
  on a session that hears only silence the meter arrived at no point inside 45 s
  over four measured runs, which is exactly the line it was relied on for. The
  new `tick_ms` (`1000`) is the interval that carries it, and the meter is read
  for `usage_ratio` and nothing else. A watchdog timer is not polling; it is a
  timeout timer, and the event-driven rule is untouched by it. The idle deadline
  `provider_idle_timeout_ms` counts against the last session frame of any kind,
  audio included, as it always did -- only the comment claiming it rested on the
  meter was wrong.

  What that deadline could not tell apart until now is a caller who paused from
  a wire that had died: session frames alone do not say which of the two a
  silence is, and thirty seconds of it on the line ended the call. The session
  now asks its own socket: every `keepalive_ms` (`8000`) the adapter sends a
  WebSocket ping, and only the pong carrying its own payload resets the
  deadline. Eight seconds is a third ping at 24 s, so two pongs may go missing
  before a call is given up. A ping somebody else sends still counts for nothing, so a proxy
  pinging a dead upstream cannot hold a call open.

  On the same tick sits a deadline for delegations. One left open longer than
  `delegation_grace_ms` (`12000`) is closed with a single `Commentary` append on
  its own `delegation_id` carrying `delegation_fallback` (`"Das kann ich gerade
  nicht nachsehen."`), and then leaves the open list, so the sentence falls once
  rather than once per tick. Measured, an unanswered delegation left the caller
  with one holding sentence and then 55 to 58 s of silence, with no timeout from
  the provider's side inside a minute. The prompt is still the first line -- an
  assistant answers every delegation, empty-handed if need be -- and the
  fallback is an instruction rather than a script, because a live model
  paraphrases what it is given.

- **The telephone refs the `voice` template that keeps that clock**
  (`freeswitch@2.1.1`, GH #798, GH #793). Only the ref pin moves, to
  `voice@2.2.0`: no cell, no lane and no declaration of the hive changes.

- **The telephone can put a live model behind its media half**
  (`freeswitch@2.1.0`, GH #787). The hive refs `voice@2.1.0`, accepts
  `in_advise` at its rim on an edge to the media half alone, and emits `spoken`
  and `delegation` beside the lanes it had. The medium does not move: the
  switch still does the SIP and forks the audio to the `phone` mount,
  `fork_sample_rate` is still `16000`, and the ringing and the answering are
  the dialplan's. `speak_end` is still the beat a hang-up waits for; behind a
  live model it comes out of the quiet after the model stopped speaking, since
  such a model announces no end of speech.

- **A member wires the channel whose model answers on its own timeline**
  (`member@1.9.0`, GH #787). Two edges, sixty-four to sixty-six:
  `./channels -> ./assistants` carries a `delegation` straight across as
  `in_delegation` -- round the firewall, whose exit stamps `in_turn` and would
  make a turn out of a handover -- and `./assistants -> ./channels` carries the
  three advice sections (`fact`, `context`, `correction`) back down as
  `in_advise`, guarded on `context.channel_node`. The `sidecar` edge into
  `./apps` is untouched, so an app that offered one of those sections still
  gets it. One existing edge is widened: the one into the firewall promotes
  `context.engine` out of `hop.engine` beside `context.channel`, and a channel
  that stamps nothing leaves the key empty. Nothing is taken away and the rim
  lists do not move, so a parent wired at `1.8.0` is still wired correctly.
  Both new edges stop at a container, which is where this level's authority
  ends: the last leg into the generation and into the channel is drawn by the
  mutation that grew the child, and `builder@1.12.0` renders it (GH #803).

- **A generation takes the errand its voice model hands out mid-call**
  (`assistant@2.8.0`, GH #784). One new accepted lane, `in_delegation`. In a
  duplex call the model on the line keeps talking to the caller and hands the
  backend an errand of its own accord, so what arrives here is no turn of the
  conversation: the member's channel hands it in directly rather than through
  the firewall, whose exit stamps `in_turn`. `context.delegation_id` is the
  correlation the answer travels back under, and `context.engine` is what puts
  the surface into advise mode. The lane is drawn twice, once around each
  keeper -- `./talky-chat` when `context.channel_node` is `chat`, `./talky`
  otherwise -- because every rim edge of one keeper has a twin around the
  other. Derived from the accepts list of `talky@5.2.0`, which both keeper refs
  now name. One new lane and its two edges are the second digit
  (`docs/development-rules.md` § 4/4a).

  The `./cogny` ref moves on to `cogny@5.0.2` (GH #794) without a second bump:
  `2.8.0` has not shipped, and an unreleased entry is amended in place rather
  than superseded.

- **The collector assembles for a brain that answers nobody**
  (`collector@4.2.0`, GH #784). In a duplex call the model on the line speaks to
  the caller itself and the brain behind it advises that model, so three pieces
  move. `system.instructions.mode` carries the charter of that role: it reads
  `advise` when `context.engine` is `duplex` and is empty otherwise, and it is
  written on every assembly rather than only when it applies, because `system.*`
  is upserted per slot path in the brain -- a slot that is only ever set would
  keep advising for a lifetime after a single duplex call. It hangs off
  `context.engine` rather than off the channel, since one `apps/voice` carries
  half-duplex and duplex turns alike.

  `in_delegation` is the third turn-opening lane: role `delegation`,
  `consult_id` from `context.delegation_id`, a fresh `turn_id`, `iter = 0`, and a
  frame on the wire that says what the line is. It never reaches `SAID`, so it
  becomes neither an episode nor a row in the daily batch -- the errand belongs
  to the model on the line, and a memory fed from it would hand the caller its
  own words back as a recollection (the defect of GH #282).

  And an `in_turn` carrying `messages[user, assistant]` is written as a pair:
  two rows under one `turn_id`, before the window read of the same bundle, the
  answer's id derived from the question's (`<id>-a`). `turns.id` is the time
  order of that table, and two random ids drawn in the same microsecond would
  swap the pair inside the window as often as not.

- **The keeper takes that errand in as a lane of its own** (`talky@5.2.0`,
  GH #784). `in_delegation` joins the accepts list, with the `context` fields it
  needs declared required, and joins the collecting edge `. -> ./collector`,
  because a door and the lane behind it are one statement and the sub-unit that
  assembles a delegation sits at the end of that edge. Nothing else moves; the
  purpose text now also names the `schemas` cell the keeper gained in the same
  wave.

### Changed

- **The dead-letter read answers newest first** (GH #794). `GET
  /colony/dead_letters` without `?since=` now orders by `id DESC`, so a capped
  read shows what is happening now instead of the colony's first day forever;
  given a mark it still walks forward from it, which is the order a watcher
  wants. Nothing about what enters the queue changes -- in an event-driven
  design an event with no target is the normal case and the queue is where it
  belongs, so the queue is meant to grow large and only reading it had to.

### Fixed

- **`scripts/test-tier.sh` builds where `scripts/gate.sh` builds** (GH #802).
  Every worktree shares one cargo target directory, but only the gate knew the
  rule, so a targeted test run in a linked worktree built a private `target/`
  there — four worktrees of one wave took a build volume from 161 G to 29 G,
  and each of them left the shared ghost-binary stamp naming somebody else.
  The rule, the tree sync and the stamp now live once in
  `scripts/cargo-target.sh` and both scripts read them from there. In the same
  family: `scripts/strand.sh gate` archived by strand name alone, so a second
  run of a strand overwrote the first one's summary, run log and station logs;
  every run now gets a directory of its own with a `latest` pointer beside it.
  `scripts/wave_retro.py` reads both archive shapes, so the retro of a wave
  that straddles the change still counts its red runs.

- **Three claims in the present tense described a tree that had moved**
  (GH #794). `docs/development-rules.md` named `assistant@2.5.1` docking `tool`
  and `schemas` at two brains where the tree has `2.8.0` and three; the
  `grow_level` recipe and `templates/builder/README.md` counted twenty and
  twenty-five lanes where the assistant declares twenty-one at the rim and
  twenty-eight in all. Counted against the tree rather than copied from the
  report that found them. (`in_delegation` was written up here as a lane the
  table deliberately skips; GH #803 found that reading wrong and the entry
  above is what replaced it.)

- **A grown child never heard the two lanes its level had just learned**
  (`builder@1.12.0`, `meclaw-os@1.8.11`, GH #803). `member@1.9.0` re-stamps a
  channel's `delegation` onto `./assistants` and an assistant's advice sections
  onto `./channels`, and both of those edges end at a CONTAINER. A container is
  not a pass-through here -- `Edge.to` is a static path, so a lane addressed
  inward costs one more edge, named for the child -- and `grow_level` drew
  neither. Measured on a built colony: the container carried seven lanes into
  its generation and not `in_delegation`, so every delegation and every advice
  section died there as `hive_no_route`, and the one manifest that wired a
  duplex channel drew both hops by hand beside the table. The recipe now
  renders them: `. -> ./<generation>` on `in_delegation` under the permissive
  `context.assistant` guard `in_tool` and `in_menu` already use, and
  `. -> ./<channel>` on `in_advise` under the `context.channel_node` guard the
  answer edge already uses. An assistant level costs twenty-four edges and a
  channel four; a screen still costs three, because nothing advises a display.
  A generation or a channel grown before this version is missing its last leg
  and needs the one edge drawn.

- **`cogny@5.0.2`** (GH #794): the collector pin follows the sub-unit to
  `collector@4.2.0`. A reference resolves exactly, so a pin left behind is a
  template that stops instantiating; what the collector grew in between is the
  `in_delegation` lane and the `advise` assembly mode, neither of which this
  core's rim declares, so the core behaves exactly as `5.0.1` did.

- **The `since` parameter of the dead-letter read was documented as a no-op**
  in three places (GH #794), although it has filtered since phase-16 W2
  (ruling A2) put `created_at` on every entry.

- **`replace_nodes` no longer writes an environment placeholder out resolved**
  (GH #796): a lift's `with.params` reach the lifted cell's `config.json`, so
  they now take the same disk-facing substitution pass as
  `add_nodes[].override_params` and `swap_nodes[].with.params` -- `${VAR}`
  stays a token on disk and binds at every read, as it always did for the other
  two operations.

- **A sidecar section may be the sentence itself** (`talky@5.2.1`,
  `collector@4.2.0`, GH #799). The splitter cut the block by top-level key and
  dropped every section whose body was not an object, and the offers those
  sections are written from describe each one as a string -- so a model that
  followed the section heading wrote `{"fact": "Der Termin ist Dienstag."}` and
  the advice never left the composite. In a duplex call that is a caller who
  hears a holding sentence and then silence, the same silence GH #797 closed at
  the other end of the seam.

  A bare, non-empty string is a section body now. It travels wrapped --
  `{"payload": "<string>"}` -- because the body slot the lane declares is an
  object slot, and the string inside it is the one the model wrote, byte for
  byte: the cell still repairs nothing. An object travels exactly as before, so
  a producer writing the nested form is unaffected. What still cannot travel is
  named rather than guessed at: a list, a number, an empty or blank string
  leaves as `hop.sidecar_dropped` on the answer half. The blank string is
  refused at the producer because the consumer refuses it too -- a section
  without words ends as a named refusal, never as an advice.

  The contract asks for one form, and it is the section's own. The frame the
  collector writes in front of the block printed `{"fact": {...}}` for every
  section, whatever shape the offer declared, so a model reading it was told to
  write an object where the offer asks for a sentence. It prints the form each
  offer actually takes now -- braces where the body is an object, quotes where
  it is a string -- which is the same repair read from the other end of the
  seam. `assistant@2.8.0` re-points its two refs at `talky@5.2.1` and keeps its
  number: an unreleased version is extended, never superseded.

## [0.40.1] — 2026-09-20

A patch release: the workspace compiles on Windows again. Nothing that ships
moved, the release is a static musl build for Linux x86_64, and no topology
needs anything done to it.

### Fixed

- **The browser fixture is Unix-only, and now says so to the compiler**
  (GH #776). `cdp_browser_fixture` is a test double that speaks CDP over the
  pair of file descriptors a browser inherits, so it needs Unix descriptors and
  user namespaces. Its entry point was already Unix-only; its imports and its
  helpers were not, and on a platform without `FromRawFd`, `is_fifo` and
  `is_char_device` the workspace therefore did not build at all. Every helper
  now carries the same condition as the entry point, and a non-Unix build gets
  a `main` that says why the double cannot exist there.

## [0.40.0] — 2026-09-20

A minor release: a browser is a cell. `browser@1.0.0` runs one Chromium-based
browser per member over a pair of file descriptors, a context per identity and a
page per card, and puts each page's picture on a `page:` topic of a display's own
socket; pointers, wheels, keys and text come back on the same link. The browser
itself is a prerequisite out of the machine's package system, `params.chromium_path`
is required and has no default, the cell adds no sandbox flag and refuses to start a
browser whose own sandbox does not hold, and the ceiling it puts on the process is a
cgroup cap asked of whoever owns the cgroup the packaged browser re-homed itself into.
`web@2.1.0` reads a table of topic kinds where it carried one prefix, and
`display@2.6.0` gains a twenty-seventh content component, `display-browser`: a page is
content in an ordinary window, not a fifth kind of window, so it takes part in
everything a window takes part in. The curator's own writing got a fix in the same
wave (GH #765): a patch leaves only after the state row has landed, where before, 190
of 401 measured state writes were refused and their drawing sent anyway.

### Added

- **A browser is a cell** (`browser@1.0.0`, GH #766, ADR-0044). One
  Chromium-based browser per member, a browser context per identity and a page
  per card, with each page's picture on a `page:<page>` topic of a display's own
  socket and pointers, wheels, keys and text coming back on the same link. A
  frame never becomes a message; what the topology hears about are pages —
  `page` on every state change, `error` on a failure, `receipt` per message.

  The browser itself is a **prerequisite out of the machine's package system**,
  not something this project ships: `params.chromium_path` is required, has no
  default and no search path, and the cell adds no sandbox flag, refuses
  `--no-sandbox` and does not start a browser whose own sandbox does not hold —
  measured after the spawn, through the pid it remembers. What `params.sandbox`
  does here is the cell's own ceiling, a cgroup cap and nothing else
  (ADR-0043).

  That cap follows the browser, because a packaged one does not stay where it
  was started: its packaging re-homes the process into a scope of its own
  within 41–83 ms, long before the cell's sandbox verdict at 159–194 ms, and
  the sub-cgroup the substrate had made for it is then empty. So the cell reads
  where its child actually sits and caps *that* — not by writing the three
  files, which held for the life of the browser and was gone at the next
  `daemon-reload`, but by asking whoever owns the cgroup (`set-property
  --runtime` on the unit that carries it) and reading the values back. The
  refusals are fail-closed and named: a cgroup that is no unit, a cgroup that
  is the colony's own or an ancestor of it — capping that would cap the colony
  — a manager that does not answer, and a read-back that disagrees. The escape
  hatch is the same one as everywhere, an explicit `{"trust": "trusted"}`, and
  every refusal says so. Measured cost: 7–18 ms against 1–3 ms for the write,
  inside a window the spawn needs anyway. An OOM kill in a scope that is not
  ours is still named -- the cell remembers the count of the cgroup above
  before it caps, and reports `oom_kill=<n>` from the difference when its own
  disappears with its last process.

  CDP travels over `--remote-debugging-pipe` on a pair of file descriptors, so
  no port is opened and no dependency is added. A context is ephemeral by
  design, so a restart is a logged-out browser; the page rows are not, so a
  restart reopens what the last life was holding.

  `params.sandbox` is **required** for this one cell type, and a `restricted`
  profile must carry `limits`. Everywhere else an absent block means the
  unenforced historical behaviour; here it would mean a process tree with a
  renderer per site and no cap at all, and only the shipped `config.json` made
  that look safe. An operator who wants no ceiling writes `{"trust":
  "trusted"}` and has said so.

  A page comes back out of `suspended` two ways, `in_navigate` and a join on
  its topic, and the viewport is applied to the page rather than only reported
  about it -- a pointer's coordinates are CSS pixels of the page viewport. A
  page displaced by `max_pages` emits `page {state:"closed"}`, so the
  application that owned its card hears about it and the row goes with it. The
  wait between the cell's two halves and a `page:` join both stand under
  `params.external_timeout_ms` like every other wait in this cell: a
  long-running cell has no message-timeout behind it.

- **A second kind of topic on a display's socket** (`web@2.1.0`, GH #766). The
  socket carried one foreign topic prefix, `voice:`, and three
  `starts_with("voice:")` guards to do it. It now reads a table of kinds:
  `page:<page>` reaches the mount `browser` when a join names none, carries
  binary out of the cell as the event `image`, accepts no binary from the
  client at all, and is capped at eight live links per socket -- against
  `voice:`'s mount `voice`, event `audio` both ways and cap of four. The count
  is per kind, so a screen already holding four calls can still open a window.

  The join payload is flat, and what the door does not read now travels: a
  `LinkRequest` carries `params`, the join payload minus the `mount` that chose
  the door, one level deep. That is how a `page:` join brings its viewport to a
  cell without the socket learning what a viewport is.

### Changed

- **The ref pins moved with it** (`canvy@2.3.2`, GH #766). A ref names a
  version, and a version that no longer exists is `template_missing` at
  instantiation — so every template that refs the `web` cell moved to
  `web@2.1.0` in the same commit as the bump. Nothing else about `canvy`
  changed.

- **A window may hold a page** (`display@2.6.0`, GH #767). The catalogue gains
  a twenty-seventh content component, `display-browser`: a frame with an address
  under it, which a browser cell of the member's fills with a picture of a web
  page. A page is content in an ordinary window and not a fifth kind of window,
  so it takes part in everything a window takes part in -- one view, one window,
  one tile, one rung -- and adds nothing to the screen's state.

  The screen joins the page's topic while its window carries a level of 1 or
  more and gives it back at 0. Joining by the ELEMENT instead would hold a
  stream open for every page ever shown until the tab is closed, because a
  window that has been put away is still in the page. The level is one for the
  whole screen, so every output joins or none does.

  Which cell answers is the new setting `browser_mount`, shipped as `browser`.
  The screen writes it onto every page it draws, the way it already writes the
  window a typed line belongs to: the name is the operator's arrangement of the
  member's colony, and an application cannot know it.

  What a person does inside a page reaches that cell and nothing else. The
  pointer, the wheel and the keyboard travel to the browser as frames; the
  curator hears none of it, and the tap of the tile beside the page still
  reaches it.

- **A patch leaves only after the state row has landed** (`display@2.6.0`,
  GH #765). The curator sent the calls that redraw the browsers in the same
  message as the write of the screen state row -- so a write the store's
  compare-and-set refused had already drawn a state that never became the
  screen's. On a test colony 190 of 401 measured state writes were refused, and
  what a person saw was a window that flashed open and shut: the drawing of the
  refused pass, taken back by the next pass that landed.

  The calls now ride the write's own request and are sent from the reply to it.
  `rows_affected 1` draws them; `rows_affected 0` draws nothing at all and runs
  the pass again on the row the store now holds, up to `STATE_RETRY_MAX` times
  as before. Measured on a test colony over 30 events: every event is drawn
  exactly once, in 125 ms at the median and 244 ms at the slowest -- where
  before, 18 of 60 events had their drawing sent between two and nine times,
  the last of them as late as 4.2 s after the touch.

  The client-side hold that GH #744 added stays, and the same measurement says
  why: way A stops a REFUSED pass from drawing, and what is left is a pass that
  LANDED. In a six-run series without the hold, one of the twenty-two taps that
  reached the screen was answered first by the pass of the clock's own stroke,
  which had started before the finger and wrote its row -- the state before the
  tap, drawn for 90 ms, and no ordering of the writes reaches that case.

  This release carries `display@2.5.1` as well, listed under Fixed below: that
  version was cut inside this wave and never shipped on its own.

### Fixed

- **The curator runs one message at a time** (`display@2.5.1`, GH #765). The
  compose cell named no `params.max_concurrency`, so the stateless dispatcher's
  default of four applied and up to four workers read, computed and wrote the
  one screen state row in the same instant. The cell now declares
  `max_concurrency: 1`: its messages are handled one after another, in the order
  they arrived.

  What this does not do is remove the contention the state write's
  compare-and-set is there for. A pass is TWO messages of that cell with a store
  round trip between them, so two events that arrive inside one trip are still
  handed the same row, and the write that loses is still repeated on the row
  that now stands. Measured on a test colony -- six runs of ten taps in 800 ms
  each, three with four workers and three with one -- the refused writes are the
  same on both sides (1, 18 and 30 of 14, 31 and 44 writes against 1, 7 and 18
  of 14, 24 and 31), and every tap reached the state in all six.

## [0.39.0] — 2026-09-19

A minor release: the screen is built to its description, and a typed sentence is
a turn of its own channel. `display@2.5.0` is the first template whose behaviour
is a copy of one document rather than a reading of it, the curator's twelve
steps are byte-identical with that document's reference model, its scenarios
travel with the template and run against the cell in seconds, and the whole
screen state is one row in the store instead of props carried from pass to pass
through whatever a browser was holding. One screen now serves many exits from a
switch at the root, each rendering what it can carry. `chat-channel@1.0.0` is a
new template: one `code` cell a chat application puts its input line on, minting
the `turn_id` the turn, its answer and every window built from that answer carry,
and putting it on the lane every turn of every channel travels. `assistant@2.7.0`
gives that channel a talky of its own, `member@1.8.0` lets an application take a
view back down, and `builder@1.11.0` renders the edge that carries it. That id
now survives the whole road (`firewall@2.3.1`, `session-keeper@2.2.1`,
`collector@4.1.1`, GH #724), and a curator pass reaches the browser as one frame
per output (`web@2.0.4`, GH #723).

The screen's vocabulary narrows with it, and that is the one breaking change:
the two events a screen sends are `tap` and `hold`, and `canvas_slots`,
`params.dock_max`, `plane`, `modal` and a curator-written `state` are gone.
**Migration: rename the two events in any client that binds them, and drop
`canvas_slots` from the profiles.** An application that only sends views has
nothing to change.

### Breaking

- **The screen's vocabulary is the one its description uses** (`display@2.5.0`,
  GH #707), and that narrows the template DSL. The two events a screen
  sends are `tap` and `hold`; the names `tile` and `touch` are gone, and a hold
  carries no field at all. The window props `plane` and `modal` are gone with
  them, and so is `state` as a word the curator writes -- a window stands on a
  `level` and carries a `rung`, which is what that spelling stood for.
  `canvas_slots` and the root `params.dock_max` are no longer read: how many
  windows an exit draws follows from its kind, and every
  entry of `screens` names its own `dock_max`. **Migration: rename the two events
  in any client that binds them, and drop `canvas_slots` from the profiles.**
  An application that only sends views has nothing to change -- `state: "urgent"`
  and `state: "hidden"` are still what an application says about its own window,
  and `layer` says what `modal: true` used to say. One thing a view no longer
  decides: the `ord` of a view row is not read for the order of the windows any
  more. `canvas_order` (§ 6.3) orders the open canvas windows -- the leading one
  first, then by score -- and `dock_order` (§ 4.28) orders the tiles. A window
  that is only a tile has no place on the canvas to be ordered in.

### Added

- **The dock comes and goes in one short movement** (`display@2.5.0`, GH #741).
  It switched hard on a monitor in both directions and flew in over 360 ms on a
  phone with nothing at all on the way out. Both directions are now the same
  140 ms of opacity and a few pixels from the edge it lives on, on every output.
  A closed dock stays what it was -- a box that was never made, `display: none`
  -- because the movement is declared with `transition-behavior: allow-discrete`
  and a starting style; an engine without those keeps the hard cut it had, which
  is no step back. Under `prefers-reduced-motion` nothing moves.

- **The weather window sets its unit under the number** (`display@2.5.0`,
  GH #741). The value is the figure and the unit its label, the way the dock
  tile has always drawn it; the unit used to stand beside the number and raised
  behind it. Value and unit travel apart all the way from the application, so
  nothing outside the sheet changes.

- **A chat line says where it came from and when it arrived** (`display@2.5.0`).
  Beside the channel word every line now carries its clock time. The line
  carries it raw -- `at`, the epoch milliseconds of the line -- and the browser
  writes HH:MM into it in the time zone of whoever is looking, because one
  screen state is shared by every exit and has no time zone of its own while
  each device has one. A line of another day carries its date short in front of
  the time; a line without `at` renders exactly as before, with no empty box.
  Tiles are unchanged: a tile is a face, not a transcript, and only what is
  about a time -- a timer, an appointment -- carries one there.
- **The screen is built to its description** (`display@2.5.0`, GH #707). The
  curator's pass -- the twelve steps that turn hints, a verdict, a gesture and
  the time into rungs, levels, tiles and the next moment -- is byte-identical
  with the reference model of the one document that describes this template,
  and a lock fails when the two differ. The scenarios of that document travel
  with the template and run against the cell in seconds, three numbers per
  run: the model against itself, the copy inside the cell, and the cell
  through its own door and store. A rule now changes in the document first and
  reaches the code by copy, never the other way round.
- **One screen, many exits, and a switch at the root** (`display@2.5.0`,
  GH #707). Every named exit is its own copy of the tree under `<name>.`, and
  `/<mount>/` is the switch that leads a visitor to one of them: the page it
  serves is the `default_screen`, and a narrow touch device is sent to the
  `phone` exit where the screen has one. A profile says what its exit is (`display_type`, now
  required) and how many tiles it carries; a television is given no inputs
  whatever it asks for, because nobody touches one.
- **Two events, and neither of them describes itself** (`display@2.5.0`,
  GH #707). A finger on a tile is a `tap`, naming the window it belongs to; the
  OS mark held down is a `hold`, and it carries nothing -- what a hold means is
  the screen's to decide, not the browser's to assert. An empty seat is a
  component of its own now (`display-seat`), so the place a tile will come back
  to stays where it is.
- **A typed sentence is a turn of its own channel** (`chat-channel@1.0.0`, GH #709). The
  new template is one `code` cell a person's chat app puts its input line on. It mints the
  `turn_id` the turn, its answer and every window built from that answer carry, it stamps
  the member who typed it, and it puts the turn on the lane every turn of every channel
  travels -- firewall, session, talky. The answer comes back to it and stops: this channel
  has no loudspeaker, and the app that shows the conversation hears the answer on the
  member's own lane. Two edges wire it, the same two every channel costs.
- **One talky per channel** (`assistant@2.7.0`, GH #709). The level holds a second
  conversation surface, `./talky-chat`, for the channel `chat`: the same ref onto the same
  template with the same overrides, because the model, the tool list and the memory tier are
  the level's decisions and a chat answering in another voice would be another assistant to
  the same person. `in_turn` and the advice a consult produced are split on
  `context.channel_node`, the tool round on `context.tool_caller`, and every sweep, prune
  and mutation receipt fans out to both, because each keeper has its own sessions to tidy.
  Every connect point names both rims, so whoever wires a chat channel draws that rim's
  v-lanes a second time. The four TRANSFER lanes are the exception and stay on the spoken
  keeper: both keepers are a `session-keeper`, and an export names a directory and an import
  an `import_hive` by the name of the HIVE rather than of the node, so two of them would
  claim one export directory and answer one import address (GH #712). Two keepers are two
  session windows: what was spoken reaches the typed keeper through the member's memory,
  never out of its own window.
- **An app takes its view back down** (`member@1.8.0`, GH #709). The edge that carries
  what an app drew out of `./apps` carries `withdraw` beside `view`, in the same edge
  rather than in a twin, so a view that is over reaches the screen as `in_withdraw`
  instead of waiting out a `ttl_ms`. `display` has accepted the lane since it shipped and
  nothing carried the asking; it became load-bearing with several views per app, where a
  timer that has rung and a card that has been replaced have to say they are over rather
  than stand there fading. The down-edge onto the screen stays the instantiating
  mutation's and re-stamps the pair with one ternary, and `builder@1.11.0` renders it that
  way (`meclaw-os@1.8.10` moves the pin). A screen grown before that needs the one edge
  redrawn, or a withdrawal reaches the channels container and stops there as
  `hive_no_route`.
- **Four levels and two ladders on the screen** (`display@2.4.0`, GH #702). An application
  declares which ladder a window competes on -- `layer: "canvas"` (the default) or
  `layer: "modal"` -- and each ladder has its own focus, so a chat over a document does not
  take the document's rung. What is drawn where is the new window prop `plane`: the loudest
  urgent window in front, one active modal over the canvas, the canvas focus and as many
  `relevant` windows beside it as the exit carries. Three more hints: `seat` and `seat_ord`
  put a tile in the seated band at the bottom of the dock (the clock lowest, the weather above
  it, an empty seat stays empty, and a seated tile is never cut), and `linger` says how long
  this window keeps its full weight after a touch, clamped to `linger_ms + fade_ms`.
- **A tap on a tile is a touch on its window** (`display@2.4.0`). The screen answers it
  itself, without asking the judge: the window's `since` becomes now, the verdict on it is
  dropped, and it takes the focus of its ladder -- on every exit at once. A second tap on the
  window that is LEADING its ladder puts it back into its tile (`dismissed_at`), where it fades
  as usual; a tap on any other window, an urgent one that merely rings in its tile included,
  is a touch and brings it forward. A tap on the canvas closes the standing modal, and holding
  the OS mark touches whatever the screen shows about the subject `chat`. Neither gesture is
  remembered anywhere: `since` and `dismissed_at` are the whole statement, and a later touch
  lifts the mark by itself. An alarm an application raises after the finger put a window away
  is not answered by that gesture: such a window leads its ladder again, because the finger
  puts away what it sees.
- **One line a person types into** (`display@2.4.0`): the content component `display-input`,
  bound on Enter, carrying the object id of the window it stands in -- the screen fills that
  id, as it does on a tile, so an application never names the index chain. What was typed
  leaves the hive as an `event` of the application that put the window up -- addressed by
  the id the screen wrote and never by the sentence, which a person could otherwise make
  look like the id of somebody else's view. The catalogue is twenty-nine components now.
  The window it stands in is never taller than the screen: the leading window is capped
  against the visible height minus the safe area, what does not fit scrolls inside it, and
  the input line keeps the foot -- eight chat lines used to push the field 528px under the
  bottom edge of a phone. And the conversation in it shows its newest line: a window that
  opens stands at the foot, a line that arrives is followed, and a window somebody has
  scrolled up in stays where they left it until they scroll back.
- **Each exit renders what it can carry** (`display@2.4.0`). An entry of `screens` gets three
  dials of its own: `canvas_slots` (how many windows stand large here; tv 2, monitor 3, phone
  1), `dock_max` (its own tile cut) and `dock_default` (`shown` or `hidden`). The state is one
  -- the same windows, the same rungs, the same focus -- and a tile that does not fit is
  absent from that exit rather than dimmed. The mark carries `unseen`: how many present
  windows are asking for attention from a tile on THIS exit, counting neither a window the
  finger put away nor one this dock's cut dropped, and the screen's clock orders the moment
  that number falls.
- **The screen has four planes, and they are four numbers** (`display@2.4.0`,
  GH #703). A window carries the plane it stands on, the sheet turns it into a
  z-index, and the blur follows the plane: a modal takes the canvas back a
  little, an urgent window takes both back further. A modal also stands centred
  over the canvas instead of in its column, so a sentence somebody says does
  not relayout the screen. Until 2.3.3 the sheet carried nine single numbers
  and the blur was triggered by a boolean an application set about itself -- so
  the OS mark standing above an urgent window was document order rather than a
  statement.
- **A phone starts with its dock closed, and a tap opens it**
  (`display@2.4.0`, GH #703). The ground state comes from the profile and is in
  the first paint; the opening is the client's and lives on `<html>`, where no
  render reaches it. It is not remembered: a reload puts the screen back into
  its profile default. A closed dock is `display: none`, never `opacity`.
  While it is closed, the OS mark carries a dot when something urgent or
  freshly touched is present and not shown.
- **A press under 250 ms is a tap, over it a hold** (`display@2.4.0`,
  GH #703). A hold reaches both halves of the system: the voice cell hears the
  frame, the screen hears a touch on the chat. Nothing is lost to the threshold
  -- from the first touch the microphone's frames are kept in a two-second ring
  and sent behind the hold that frames them. A click ends the gesture it
  belongs to, so a press whose `pointerup` never arrived cannot speak
  afterwards.
- **A window may carry a line to type into** (`display@2.4.0`, GH #703). One
  field, no form; the screen empties it after Enter, in a macrotask of its own
  so that what was typed is read before it goes. Its type has a 16 px floor,
  which is what keeps a phone from zooming into the field and staying there.
- **The proof runs in WebKit too**
  (`workshop/tools/display-webkit-browser.mjs`, GH #703). An iPhone viewport in
  the engine an iPhone actually is.

### Changed

- **The state is one row in the store** (`display@2.5.0`, GH #707). All of it --
  every window with its verdict and its curator values, the bar, the weights,
  the chat, `unseen`, the strokes and the order of the dock -- stands under
  `(owner "display", view_id "screen-state")`, and the object tree is a rendering
  of it. Before this the curator's values lived as props on the display's own
  objects and travelled from pass to pass through whatever the browser was
  holding, and the tree was copied per exit and recomputed in each copy, so a
  screen with three exits had three answers to the same question. `unseen` is one
  number for the whole screen for the same reason.
- **The finger stands above the score, the chat closes itself, and the dock cut
  takes no window** (`display@2.5.0`, GH #707). A touched window keeps the front
  of its ladder for a while (`led_until`) instead of for exactly one pass, so an
  arriving score cannot take the screen out from under the hand that just
  reached for it. A chat window carries the `turn_id` of the turn it belongs to
  and closes when the answer to that turn has been read, rather than waiting for
  its relevance to decay. And a dock that is too short for what is present drops
  tiles in stages -- the unseated first, the seats last -- and never touches the
  window a dropped tile belongs to: the cut is rendering, and the state is one.
- **The judge sees no geometry** (`display@2.4.0`). `screen` and `dock_overflow` are gone from
  the situation, and the sentence about the dock being full is gone from the instructions: the
  judge decides one state for every exit at once, and how much of it a screen shows is that
  screen's profile, cut after the verdict. `params.dock_max` is now the FALLBACK for an entry
  of `screens` that names no `dock_max` of its own; when nothing says anything the kind of the
  exit decides, tv 7, monitor 8, phone 5. The package ships that dial as `0` -- a number here
  is ONE number for every exit, so the shipped `7` gave a phone the television's dock.
- **The dock reads the rung, not the compatibility word** (`display@2.4.0`). A window that is
  not drawn wears `state: "hidden"`, so a dock ordered by `state` dropped the second urgent
  window out of the tiles at a narrow `dock_max`. Rank and cut read `rung` now, and a tile that
  rings keeps its place.
- **The canvas is ordered by what it means** (`display@2.4.0`): plane, then the youngest touch,
  then identity. A drawn window is lifted above every declared `ord`, so the order of the
  windows the canvas draws no longer mixes with the seats of the ones it does not. The seat of a
  standing view is untouched -- the moment a view was last written is still not a sort key
  (GH #609).
- **`modal` is deprecated** (`display@2.4.0`). It said what `layer: "modal"` says; a window
  that names `modal: true` and no `layer` is read as modal for one release, `data-modal` is
  still written, and the README lists the hint as deprecated. Nothing to migrate today.
- **A tile that answers a finger says so before the colony does**
  (`display@2.4.0`, GH #703). The press is a ring in the same frame the finger
  lands; what the tap MEANS is still the server's word, one pass later.
- **A chat tile reads as a chat, and a value keeps its unit**
  (`display@2.4.0`, GH #703). The bubble leads, the last line gets two lines
  instead of one cut, an unread answer shows a dot, and a unit is set beside
  the number rather than swallowed by its ellipsis. The weather window leads
  with the value, the glyph carries the state and the place is a label. Frames
  and windows are unchanged.

### Fixed

- **The leading window keeps its left and its top edge** (`display@2.5.0`,
  GH #739). The canvas region carries `overflow-y: auto`, which makes it a
  scroll container on both axes, and a scroll container clips at its padding box
  on all four sides -- also on the sides where it does not scroll. Its padding
  was zero, the air stood on the column around it, and so the cut ran along the
  outer edge of the first grid cell. A window on the `focus` rung is drawn
  outside its box, two per cent wider and four pixels higher, with a one-pixel
  ring beyond that; `canvas_order` puts the leading window in that first cell
  and the leading window is the one that wears `focus`. Measured on a monitor it
  lost 6.8 px on the left and 15.2 px on the top, on a television 25.7 px on the
  top, and on a phone three edges plus four pixels of sideways scroll that
  nothing could scroll to. The gutter now sits inside the scroller instead of
  around it: the window stands where it stood, and the cut falls outside it.

- **A television paints at sixty frames a second again** (`display@2.5.0`,
  GH #740). The ground drifted for ever -- a keyframe moving the
  `background-position` of three gradients, which is not a compositable
  property -- and a grain layer was blended into every one of those repaints.
  At 3840x2160 that is a full repaint of 8.3 megapixels per frame, and a
  television measured 5 frames a second while nothing at all was happening;
  building a window up or taking it down simply made it visible. Both stop on
  that one output now. Its glass, its plane blur, its transitions and its window
  keyframes stay exactly as they are: measured, they cost nothing there.

- **The refusal names what is missing and which page said so** (`display@2.5.0`,
  GH #741). The caption read `microphone needs https or localhost`, and
  `localhost` is an address the reader is not on and cannot go to; the address
  that would work lives in a proxy the colony knows nothing about and may not be
  invented. It now says `the microphone needs https` followed by the page's own
  origin -- the two things the browser knows for certain.

- **The shipped sheet travels without its comments; the source keeps them**
  (`display@2.5.0`, GH #735). `display-dna.css` argues for every rule it has,
  and every screen was loading those arguments: about 47 kB of reasoning that
  only a reader of the file ever needed. The copies a screen gets now carry the
  rules and the sheet's head comment, which says what it is and where the
  source is; the file itself is unchanged and keeps every word. Nothing about
  the design language moved -- the CSS a browser parses is the same -- and the
  page went from 97 kB to about 50.
- **A window its application has just written is judged anew**
  (`display@2.5.0`, GH #742). A verdict of the judge used to stand until the
  judge itself spoke again or a finger touched the window -- an application
  writing to the window changed nothing about it. A window the judge had
  closed while it slept therefore stayed closed after its application woke it,
  for as long as the judge needed to run once or twice: the answer to a
  question about the weather arrived, the conversation stepped back, and the
  screen stood empty for several seconds until the weather window was allowed
  to open. An application touch -- a changed prop of the window, or a raised
  `touched` -- now clears that window's `judged_relevance` and
  `judged_hidden`, so the application's own `relevance` counts until the judge
  speaks again, exactly as it does after a tap. That next run is not
  guaranteed: a call that falls inside the judge's minimum interval is
  discarded rather than made up, and the window then stands on what its
  application says about it until the next touch after the interval -- which
  is the guideline's own order, a window judged anew at every real event. The
  bar stays as it is -- it is the screen's, not one window's -- and so does
  the map of context weights; but the cleared window is no longer weighted by
  it. Until the judge speaks again that window counts with the unnamed weight
  0.5, in its score and in its rank in the dock, because the weights are the
  same standing judgement as the two values and were written for the situation
  the touch has just ended. Keeping that half was the second half of the bug:
  a weather window woken with a relevance of 0.8 stayed shut at the context
  weight of 0.3 the judge had written for the timer round before it -- 0.24
  against a bar of 0.3, seven seconds with nothing open. A window that first
  appears carries no verdict to clear, and the topic touch -- another
  application's window on the same subject -- leaves the standing window's
  verdict alone, because the judge may have closed that window on purpose.
- **Two taps inside one round trip no longer lose one of them** (`display@2.5.0`,
  GH #744). Every event on the screen starts a read pass of its own, and between
  the pass's read and the write that follows it lies a full message round trip.
  The curator kept its whole memory in one row and replaced that row blind, so
  any event that started its own pass inside that window computed on the row the
  first pass had read and then overwrote what the first pass concluded. Measured
  with real mouse clicks: two taps 36 ms apart both opened the same window, so
  ten taps on one tile ended with it open where an even count ends put away; and
  three application writes 18-46 ms after a tap carried the state from before the
  tap. The row now names the version it was read at, and a write whose row has
  moved on is turned away -- the same pass then runs again on what the store
  holds, at most three times, and says so out loud if it still cannot land. The
  row that carries the state is created the way a view row always was, one
  message that removes the name and writes it again, because the table has no
  primary key to lean on.
- **A patch from an older pass no longer takes the tap back** (`display@2.5.0`,
  GH #744). The browser draws the effect of a tap at once and the next pass
  confirms it. A pass that was already running when the finger landed renders
  the state from before the tap, and it arrived first: the window flashed open
  and shut again, which from the hand is indistinguishable from a tile that does
  nothing -- five taps on the chat tile in four seconds, every one of them
  processed by the screen. Each window now carries the moment it was last touched
  or put away, and the browser keeps its own drawing until a patch carries a
  bigger one; a window a tap closed alongside is held by its own moment, so a
  conversation opened by a hold, or by a finger on another exit, is never shut
  again by a drawing made seconds ago. The hold ends by itself after a few
  seconds, because a tap the screen absorbs never moves a moment at all.

- **The refusal the mark says is one a person can read** (`display@2.5.0`,
  GH #722). § 5.4 asks a refused voice channel to be said "in its live region",
  and the live region is `clip-path: inset(50%)` -- so the whole of saying it
  reached a screen reader and nobody else. On a laptop holding the mark on a LAN
  page over plain HTTP, the measurement reads `microphone needs https or
  localhost` in the DOM, `data-phase="error"` on the mark and the mark at half
  opacity, and the person sees a mark go faint with no reason given. While the
  mark stands in that phase, the same line it already writes is now drawn as one
  small caption above the mark, flush with its trailing edge, inside the safe
  area and with nothing behind it, in the mark's own dimming. Every other phase
  keeps the live region it had. Nothing was added to the screen's state and no
  clock is involved: the caption lasts exactly as long as the phase, and a
  gesture or a microphone that opens after all is what ends it.

- **A curator pass is one frame per output** (`web@2.0.4`, GH #723). GH #718
  made a patch bundle one push per route for the root and the structural arm
  and left the plain slot arm at one push per slot, so a pass that moves a
  window -- main, aside and dock of the same output, always all three -- still
  reached the browser as several frames a few milliseconds apart. A browser
  re-renders the whole page container out of its cached tree whenever a diff
  arrives and patches only the keys that diff names, so a frame naming one slot
  re-draws the others from the values the cache held BEFORE the pass and writes
  them back over the DOM: whatever the client had drawn optimistically is gone
  until the next frame restores it. Measured on the fresh instance (18.09.2026,
  Chromium, a real click closing the chat window): two diffs, `{"2"}` at
  +167 ms and `{"0"}` at +172 ms, and `data-level` `2 -> 0` at +30 ms
  (optimistic) `-> 2` at +172 `-> 0` at +176. The window blinked, which is what
  the human acceptance reported as "opened and closed twice, on every tile".
  The push now carries every slot the pass touched in one diff,
  `{"0": ..., "1": ..., "2": ...}` -- the shape the client already reads, since
  the structural arm has always sent all four slots and the statics in one
  frame. A pass over two outputs stays two frames, one each; a single call
  stays one slot.
- **The answer carries the turn's own id** (`firewall@2.3.1`,
  `session-keeper@2.2.1`, `collector@4.1.1`, GH #724). A channel assigns a
  `turn_id` when it accepts a turn, and the answer -- and every window that
  arises from it -- is supposed to carry it on. Measured on two live colonies
  and on both shipped channels: it never got past the screening. All three
  exits of `firewall/screen` rebuilt the header from a fixed key set the id was
  not in, `session-keeper/stamp` built the stamped turn's header from scratch
  and copied only the body into it, and `collector/assemble` then minted a
  fresh `uuid4` for every round. So the answer that reached the chat, the
  ambient view and the stage named a turn nobody had spoken, the chat window
  carried no id at all, and the rule that closes a chat once a canvas window
  answers its last turn had two ids to compare and never fired. The id now
  travels the whole road: it is on `pass`, `reject` and `hold` alike, it rides
  the two store round-trips in the context the way the parked body already did,
  and the collector adopts it instead of minting -- `uuid4` stays the fallback
  for an ingress with no channel in front of it, and an advisor's return keeps
  minting, because the id on ITS hop belongs to another session. A turn a
  person releases from a hold re-enters under the id it was spoken under too:
  `./warden` stores it with the parked row, because the release lane carries a
  hold id and a decision and nothing else. An adopted id is a foreign string,
  so it is capped in length, loses any `|` -- the separator the collector
  builds its own composite round ids on -- and is refused outright in the
  reserved `close-<session>` shape, which is a bookkeeping row's key.
- **The judge is told what its own numbers do** (`display@2.5.0`, GH #726). A
  window stands large when `weight x relevance x decay` reaches the bar, and the
  prompt the judge was handed said nothing about that product -- only that a high
  bar means a concentrated screen. Measured against `openai/gpt-5.6-luna` on a
  live screen and its twin: every one of forty verdicts came back with `bar` at
  0.9 and nearly every window hidden at `judged_relevance` 0.05. At that bar only
  a context weighing 1.0 with a relevance of 0.9 opens anything, so the window
  that answered the person's question stayed a tile while the conversation kept
  the screen, and verdicts reversed within seconds with no event between them.
  The prompt now carries the arithmetic with a worked example, the span a usable
  bar lives in (0.2 to 0.5, and 0.3 unless the person wants quiet), that a weight
  is not an off switch and the conversation gets no bonus either, that the window
  showing the answer is what matters now while the chat steps back, and that a
  verdict does not move without a new event. `workshop/tools/judge_eval.py`
  measures a prompt against a real model on recorded screens, at both edges --
  every expectation that asks for a window to stand is paired with one that asks
  the screen to stay quiet, so a prompt that simply opened everything would fail
  it. Of twenty expectations the old prompt missed nine and the new one three.

- **The mark keeps saying what it refused** (`display@2.5.0`, GH #720). The
  refusal of § 5.4 lasted about a tenth of a second: `data-phase` is an
  attribute of the rendered mark, the client wrote it, and the next patch --
  the pass that the very same hold had started -- put the server's empty value
  back. Measured on the fresh instance over plain HTTP on a LAN address: dimmed
  at t=460 ms, bright again at t=562 ms, and the live region wiped with it.
  The mark's phase and its spoken line now live in the client's own hook, which
  writes them back after every patch, so the dim and the words stand until a
  new press or a microphone that opens after all takes them off. Nothing was
  added to the screen's state: what the mark is doing is true about one page's
  voice channel and about no other output of the same member, and the colony
  has no opinion about a device it cannot see.

- **A long press with a refused microphone is still a hold** (`display@2.5.0`,
  GH #719). On an output whose profile carries `audio`, a press that could not
  open the microphone -- a page served over plain HTTP, a device that is not
  there, a person who said no -- ended in silence: `openMic()` returned `false`
  and `down()` returned with it, so nothing reached the screen, the dock did
  not answer either (§ 6.4 reads the PROFILE, not the device), and the mark
  neither dimmed nor counted. Measured in both engines: a 900 ms press, every
  counter 0, `data-phase` empty. The hold is the event and the audio is best
  effort, so the press now sends its `hold` whatever the microphone said -- on
  an output with a keyboard the chat's own input line carries the conversation
  -- and the mark says the refusal and dims (§ 5.4). The refusal is not
  latched: a microphone granted a minute later needs no reload.

- **A held turn keeps its last words** (`voice@2.0.3`, GH #717). An endpointing
  provider ends a turn after a stretch of silence IN the audio stream
  (Deepgram Flux: `eot_threshold`, 0.7 s), and a client stops sending about
  120 ms after `release`. So nothing reached the provider after the key came
  up, no end of turn ever came, and the boundary was cut at `release_grace_ms`
  with the interim transcript -- which lags the audio, so the end of the
  sentence was gone. Measured on the fresh instance (18.09.2026) with a fixture
  released 20 ms after the last word: cut at +1501 ms, transcript empty; the
  same take with silence still streaming closed 137 ms after the release with
  the whole sentence. The cell now feeds the provider 20 ms frames of digital
  silence from the `release` until the turn ends or the grace runs out, at the
  rate the connection negotiated. The answer arrives complete and sooner. The
  client is untouched, the cap stays as the backstop, and `release_grace_ms: 0`
  still cuts on the frame.

- **Two hives follow their children's pins** (`freeswitch@2.0.5`,
  `canvy@2.3.1`). The telephone's media half refs `voice@2.0.3` and the
  canvas's display refs `web@2.0.4`. Only the pins move -- no cell, no lane and
  no declaration of either hive.

- **A patch bundle reaches the browser as one picture** (`web@2.0.3`, GH #718).
  A curator pass arrives at a display as ONE message with several
  `object.update` legs, and the cell used to publish and push after every one
  of them. Five of a tap's eight legs are root updates, and a root update
  re-sends the whole packed tree as it stands at that moment -- so a browser was
  handed four intermediate pictures, each with the window it had just opened
  closed again, before the last leg opened it for good. Measured on the fresh
  instance (18.09.2026): `data-level` `1 -> 0 -> 1` within 255 ms on one tap,
  and a second browser watching the same state saw only a blink. A bundle now
  accumulates what it touched and pushes once per route, after the last leg, so
  what goes out is the end state; a single call pushes where it always did. A
  leg that moves a root object keeps its broadcast -- a display lays down one
  page per output, and a pass that writes one output's root and another's window
  has to reach both. Seven frames and ~774 KB per tap become one frame and
  ~154 KB.

- **An application's hover state no longer reaches an exit with no finger**
  (`colony-view@1.1.2`, GH #715). `display-hive.md` § 6.4: "without `pointer`
  and without `touch` no `:hover` rules". The screen's own sheet has asked its
  root that question since `display@2.5.0`; the sheet an application ships goes
  into the same page and had never been asked it, so on a television -- an
  output device, whose profile carries no inputs -- a stray kiosk cursor lit
  three controls of the colony view in the accent colour. All three rules now
  stand behind the screen's `data-inputs` gate, and a lock holds the sheet to
  it in both the file and the copy the runtime is handed.

- **A changed template now runs every lock that reads it** (GH #713). A test that
  reaches into `templates/<name>/` through a helper -- resolving the catalogue root
  once and joining the rest of the path onto it -- is selected for that template's
  diff like one that spells the path out, and a test that walks the whole catalogue
  is selected for every template, however it spells the root. `scripts/gate.sh`
  prints the base its diff was taken against before it plans.

- **No window outgrows its exit, whatever its rung** (`display@2.5.0`, GH #710).
  The height cap of a window read the content box, so the padding and the border
  around it fell outside the number and a window at the cap stood taller than the
  room it was given. It is the border box now. The two loudest rungs also lift the
  window they mark; the lift is a scale, it multiplies the height, and the cap did
  not know about it. Both the lift and the cap now spend the same token, so a
  window that carries a rung is capped at what it will actually measure.

- **On a phone the leading window stays the largest** (`display@2.5.0`, GH #710).
  A phone stacks its windows and the leading one is meant to be the tall one; its
  siblings had a floor but no ceiling, so a sibling with more content than the
  leader grew past it and the stack stopped saying which window leads. Siblings
  now carry a ceiling of their own, and what does not fit scrolls inside the
  window the way § 5.9 asks.

- **A refused mark dims, it does not only change colour** (`display@2.5.0`,
  GH #710). A voice channel that refuses puts the OS mark into its error phase,
  and the phase changed the mark's colour alone. On a television across a room
  and for anybody who reads colour differently that is not a difference. The mark
  loses opacity in that phase now, by a token, the way every other unavailable
  control on this screen does.

- **The dock toggle no longer hangs on the voice channel** (`display@2.4.0`,
  GH #704). The OS mark carries two gestures: a short press toggles the dock, a
  long one speaks. A refused channel used to set the `disabled` property on the
  button, and a disabled control dispatches no pointer events at all -- so the
  dock went with the voice cell, although the toggle is presentation and pushes
  nothing. It showed nowhere while a voice cell was up and appeared after one
  was restarted: the mark stayed dead until the page was reloaded, and on a
  phone the dock is the only way to the tiles. The refusal is the hook's own
  state now; only the hold reads it, and it refuses in the open -- the mark
  shows its error phase and the state line says what the channel said, and it
  is not announced as `aria-disabled`, because it still answers a finger. A
  page with no socket at all is answered the same way rather than by leaving
  the mount, which used to take every listener below it along.
- **A page can reach the edge of a phone again** (`web@2.0.2`, GH #703). The
  shell declared `width=device-width` and nothing else, so a browser on a
  device with a notch or a home indicator laid the page out inside the safe
  area and every `env(safe-area-inset-*)` a stylesheet asked for read 0. The
  viewport now says `viewport-fit=cover` as well, which is what makes those
  numbers real. Nothing else about the head moved.

- **A held take keeps its ending without being held on to** (`voice@2.0.4`,
  GH #743). `release_grace_ms` is 2500 ms rather than 1500. The silence tail of
  `voice@2.0.3` reaches the recognition provider, but the grace it was paid for
  was set by a figure that means something else: a model quotes the latency it
  spends AFTER its endpointing threshold has fired (Deepgram Flux, 400-700 ms),
  while the number this cap has to cover is the time from the last spoken word.
  Measured end to end over three machine-timed holds, that is about 1795 ms --
  so every hold released by hand was cut on the cap with the last interim, and
  the end of the sentence was missing unless the person held the key down until
  the transcript had caught up. The cap is a backstop again rather than the
  usual exit, and it costs the extra second only where a provider has gone
  quiet. `0` still cuts on the release frame.

- **The telephone's media half follows the grace** (`freeswitch@2.0.6`,
  GH #743). A ref pin and nothing else: the hive's media half refs
  `voice@2.0.4` instead of `voice@2.0.3`, so a colony grown from this template
  gets the longer grace with it. Nothing inside the hive changes, and the media
  half runs `auto`, where no hold and no grace exist -- the pin is there so the
  tree does not ship a hive that names a version it no longer carries.

### Migration from `display@2.3.x`

An application that only sends views has nothing to change: one that says nothing new competes
on the canvas ladder, lingers for the screen's `linger_ms`, sits nowhere in particular, and
renders as it did, and `state: "urgent"` and `state: "hidden"` are still what it says about its
own window. What does change is named under **Breaking** above -- the two event names, the
window props `plane` and `modal`, `state` as a word the curator writes, and the two dials
`canvas_slots` and the root `dock_max`. A client that binds an event of its own called `tile`
or `touch` renames it; a profile that carries `canvas_slots` drops it.

## [0.38.1] — 2026-09-14

A patch release: a held recording now arrives whole, on both sides of the
socket. In the cell, a provider debt is only recorded while the recognition
provider is actually inside a take, an end of turn without a transcript keeps
the interim, a session that ends quietly between two holds writes its debt off,
and every hold and every closed boundary leaves a line in the log, without a
transcript (`voice@2.0.2`, `freeswitch@2.0.4` for the pin, GH #697). In the
browser, `release` drains before it lets go, and the window is measured per take
(`display@2.3.3`, GH #698). The wedged-client negative control of the voice
service test waits for a happens-before it used to assume (GH #699). Nothing in
the contract moved, no migration is needed from 0.38.0.

### Fixed

- **A take is no longer eaten by a debt nobody owed** (`voice@2.0.2`,
  `freeswitch@2.0.4` for the pin of its media half, GH #697). When a boundary
  in `hold` mode was closed by something other than the recognition provider —
  the key again, the cap, a mode switch — the session recorded an end of turn
  it was still owed, even when the provider had already delivered one before
  the key came up. That debt was paid by the end of the NEXT take, which
  therefore never became a turn: the boundary closed on its cap with nothing in
  it, the turn number moved, and the lane saw nothing. The same debt was left
  behind by every key pressed without a word said — a take with nothing in it
  ate the one after it. A debt is now only
  recorded while the provider is actually inside a take; an end of turn that
  carries no transcript keeps the interim heard so far instead of dropping it;
  and a recognition session that ends quietly between two holds says so, so
  the debt it was owed is written off. The cell also says what it does: one
  line per hold with what it pushed and how long it was open, one per boundary
  that closed, and a boundary that closes empty is a warning. No transcript is
  logged.
- **The end of a take leaves the browser** (`display@2.3.3`, GH #698). Letting
  the key go used to lower the audio gate first, so every frame still in the
  worklet's accumulator or in the message port's queue was dropped along with
  the capture chain's own latency — measured on a live screen as the last 300
  to 600 ms of every take. `release` now drains before it lets go: the worklet
  flushes what it holds, whatever arrives for one more window still goes out,
  and the take is closed at the end of it. The window is measured rather than
  chosen — the capture latency the browser reports for the track or the
  context, one worklet block and the longest delivery gap of this take —
  floored at 120 ms and capped at 600.

## [0.38.0] — 2026-09-13

A minor release: the screen gets a dock, and the colony loop stops waiting on
reads of its own log. `display@2.3.2` draws one canvas, a dock of tiles of one
size at the right edge and the OS mark that is the hold-to-talk button;
presence and focus are two axes, an application brings its own tile and names
its topic, and the renderer knows which screen it is drawn on. `GET
/colony/messages` answers from a task of its own (GH #683, ADR-0041), so a
page of the message browser no longer silences the heartbeat. Nothing on the
wire breaks: a screen grown from `display@2.2.x` is lifted with
`replace_nodes`, `aside` and every hint of 2.2.x are still accepted, and the
one component that goes, `display-mic`, is replaced by `display-os`, the
screen's own object that no view ever named. A judgement decides what is large
and never what exists, so the clock keeps its tile while the weather is being
read, and a hook on the root runs the seconds of a countdown, chimes when a
window arrives urgent, and draws the zoom between a tile and its window.

### Added

- **The screen has a dock** (`display@2.3.0`, GH #694, #695). Presence and
  focus are two axes now: a window is *present* while it stands in the store
  and has not decayed, and it is on the *canvas* while it holds the focus or
  urgent rung. Every present window has a tile of one size in a dock at the
  right edge, ordered by relevance, and it keeps that tile while the same
  window is large on the canvas. A judgement decides what is large and never
  what exists, so the clock does not leave the screen when somebody asks about
  the weather. An application hands the screen its own tile as a child under
  the key `tile` (`display-tile`: a glyph, a line, a value, a topic); without
  one the screen falls back to the window's context and title. Two new hints:
  `topic`, so a standing window on the same subject takes a fresh answer from
  another application instead of standing beside it, and `modal`, which is now
  the only thing that may blur the canvas -- a change of focus is not a modal
  moment, and the rule that made it one is gone. New settings: `dock_max`,
  `screens`, `default_screen`.
- **An OS mark instead of a microphone capsule** (`display@2.3.0`, GH #695).
  The mark at the bottom right is the button: press and hold to speak, release
  to send. It has no card behind it and it does not say what was heard -- that
  belongs to the conversation. `display-mic` is replaced by `display-os`; the
  hook keeps its name, the gesture is unchanged, and the phases (`listening`,
  `sending`, `speaking`, `error`) are light on the mark and a line for a
  screen reader.
- **The renderer knows its display** (`display@2.3.0`, GH #694). A `screens`
  setting gives each output a type, a viewing distance, a physical size and
  its inputs; every output is a page of its own at `/<mount>/<screen>`, and
  the root carries `data-profile`, `data-inputs` and one `--scale` that the
  dock, the mark and the type derive from.
- **The screen brings its own motion** (`display@2.3.0`, GH #695). A hook on
  the root runs the seconds of every countdown in the browser, stamps the
  bar's `--now` once so a page loaded late is not off by however long it was
  open, plays a two-tone chime built from oscillators when a window arrives
  urgent (no asset ships for it), and moves a window into its tile and back --
  a FLIP, measured before and after the patch, and nothing at all under
  `prefers-reduced-motion`. `display-timer` carries `data-end-at` and a `now`
  prop for it.
- **The components that carry a script are a named list** (GH #696). The lock
  that counted them to one reads `SCRIPTED` now: the OS mark and the shell,
  and nothing else may.

### Changed

- `aside` is accepted and drawn as canvas (`display@2.3.0`): the screen has one
  centred column and a dock beside it, and a window in focus is compact rather
  than a strip across the width. A view that names `aside` is unaffected.
- `urgent` no longer locks the focus (`display@2.3.0`): every urgent window
  rings, and the focus stands beside them.
- The OS mark reads as »OS« (`display@2.3.2`): the ring opens to the right
  and the S sits on its rim.

### Fixed

- **A read of the message log no longer holds the colony loop** (GH #683,
  ADR-0041). `ColonyMsg::ReadTrace`, `ReadLedger` and `ReadMessages` need one
  thing from the loop's state — the path of the database file — and each opens a
  read-only connection of its own, so awaiting them in the loop bought no
  ordering and cost the heartbeat: on a large `colony.db` a single page of the
  message browser was hundreds of milliseconds during which the loop could not
  reach its interval arm, and a supervisor reads a silent loop as one that
  stopped answering. The three reads now answer from a task of their own, at
  most four at once, and the loop marks the hand-over in its beat stream under
  the endpoint's name. `GET /colony/messages?resolve_blob=true`
  additionally resolves at most 25 blob bodies per page and says so with a new
  `blob_resolution_truncated` field; the rows beyond it keep their
  `body_payload` uuid, and a caller that wants the rest pages for it. And a
  sidecar is found by name rather than by scanning the blob directory.

## [0.37.0] — 2026-09-13

A minor release: the diff format grows by one operation and the screen learns
to curate. `replace_nodes` lifts a standing hive, or a leaf, to a new version
of its template in place: same path, outer edges untouched, every child judged
kept, replaced, added or left, the hive's own declaration renewed, and the
committed receipt names what happened to each child. Nothing that stood before
is deleted: a replaced child is parked beside its successor as
`<name>~<old-version>`, which is why `~` is now reserved in new node names.
The screen (`display@2.2.3`) scores every window on every pass, keeps its own
time with one order on a clock, and asks a minimal judge on content changes;
a grown screen hears its channels' failures as notices (`builder@1.10.0`).
Additive throughout: the door's reply, the manifest reply and the mutation
receipt carry a sixth key `changes` (`[]` for every other operation), the
`timer` cell acknowledges a repeated order instead of refusing it, and no
migration is needed from 0.36.x, a screen grown from an older `display` is
lifted with `replace_nodes`, and the way back is the same act with the old
version.


### Added

- **The screen curates what it shows** (`display@2.2.0`, GH #679, ADR-0037,
  ADR-0038). Focus is a number on the screen (`focus`, 0–1, with a weight per
  context), every window carries `context` and `relevance` as hints, and the
  compose cell scores each window on every pass: what falls below the bar is
  hidden, exactly one window in `main` holds the focus rung, the rest stand on
  `ambient`, `relevant` or `urgent`, and presence follows the rung — the title
  is a caption at `ambient`, the big title at `focus`, the big title in the
  accent, breathing, at `urgent`. A window leaves over two frames, so the sheet
  plays its leave keyframe before the row is deleted. The screen keeps its own
  time: the compose cell predicts the next moment anything changes — a score
  crossing a rung, a `relevant_until`, a `ttl_ms` — and orders exactly one
  strike for it from a `timer` cell inside the hive, none when nothing is due;
  `ttl_ms` is a promise about *when* after all. Beside it stands a minimal
  judge: an `llm` cell that re-judges the whole screen on every content change
  and never on a tick, answering with the bar, the weights and a per-window
  `hidden` or `relevance`; a verdict has a lifetime, and a screen with no model
  configured is judged by the floor alone. A new port `in_notice` takes a
  classified message (`system_error`, `error`, `warning`, `important_note`,
  `note`) and wraps it into a prose window of its sender; a notice the cell
  refuses comes back as a `receipt` carrying `invalid_notice` (a new
  `error_code`, additive); a bare `hop.error_code` is translated into a
  sentence by a table that lives in the template. Two more words an
  application may say — `tone` (`accent`, `muted`) and `pinned` (freezes the
  decay) — and one the operator says on the root, `ground` (`day`, `night`). The knobs — `linger_ms`, `fade_ms`,
  `focus_default`, `judge`, `judge_min_interval_ms`, `notice_defaults`,
  `ground` — are the member's dials. All of it is additive: the port set grows
  by one, the sheet's default does not move, and `main|aside` are what they
  were.
- **A grown screen hears its channels' failures** (`builder@1.10.0`, GH #680,
  ADR-0039). The `grow_level` recipe draws a third edge for a screen — every
  `error` of the `channels` container down onto the screen, re-stamped
  `in_notice` — so a microphone that caught nothing is a system notice on the
  person's screen and not only a line in the operator's journal; the member's
  exit edge stays. The edge lives in the recipe and not in the `member`
  template because the container knows its screen only through the mutation
  that grows it. `examples/organism/grow-screen.json` carries the three edges
  byte for byte; `meclaw-os` moves its pin and nothing else.
- **A standing hive is lifted to a new version of its template in place**
  (`replace_nodes`, GH #682, ADR-0040). The node keeps its path and its outer
  edges; children the new version leaves unchanged keep their `cell.db`,
  changed ones are replaced under their own name with the old one renamed
  beside them, new ones are grown, and a child the new version no longer names
  stays, disconnected. The committed receipt names every child. The hive's own
  declaration -- its lanes and inner edges -- is the new template's from the
  next mutation on. The old one is renamed `<name>~<old-version>`, so `~` is
  reserved: a new node name carrying it is refused with `schema`.

### Changed

- Applications that write on the screen hand it their ages and spans and say
  `context`, `relevance`, `class` and `pinned` instead — hints, not decisions.

### Fixed

- **A parked child's death is the parked child's** (GH #688). After a
  `replace_nodes` the old child's entry stands parked beside its successor
  with the task it was born with, and a task that died by panic or by the
  `message_timeout` backstop inside the lift's stop window reported its
  death under the birth path -- against the new child's row, which was
  restarted, doubled, and later removed in RAM. The colony now settles such a
  death on the entry whose mailbox is closed: the parked one is parked, the
  new child is not touched, and the old mailbox's remainder is dead-lettered
  instead of reaching the new child.
- **A repeated clock order is one order, and a removed one is revived**
  (`display@2.2.3` and the `timer` cell, GH #690). A read pass that computed
  the moment already ordered sent `remove <id>` and `add <id>` in one pass;
  the timer marked the row `removed`, the `add` collided with it
  (`schedule_id_exists`), and the moment never struck, so an expired view
  stayed up until somebody wrote. Two halves: the compose cell no longer
  removes the order it is about to place again, and the `timer` cell takes an
  `add` on an active row with the same moment as the same order (acknowledged,
  nothing changes) and an `add` on a `removed` row of the same id as a revival
  (the row is active again, in place); only a different order under the same
  id is `schedule_id_exists`.
- **An application says a window is touched, and the screen believes it**
  (`display@2.2.2`, GH #689). The screen compares a window's own props to
  find a touch, and an answer that lives in a child component (`display-text`
  under a speech pane) never moved `since`, the focus or the judge; the four
  windows now declare the hint `touched` (an epoch, as text), a changed
  `touched` is a touch, and a prose view may say it in its content. Same
  version: the hold-to-talk button is a fixed point on the screen and its
  transcript and state lines float above it, out of its flow, so a line that
  grows no longer moves the button.
- **The hold-to-talk button no longer releases itself** (`display@2.2.1`,
  GH #684). On a fresh screen the state line under the button was empty until
  the join had answered; the first press wrote into it, the block grew upward,
  the button slid out from under the pointer and `pointerleave` let go. The
  press captures the pointer now and releases on `pointerup`, `pointercancel`
  or a lost capture, the line has a height before it speaks, and a page that
  loses focus or goes hidden releases the hold.
- **The root redirect follows the prefix a path proxy sent** (GH #685).
  `GET /` answers `307 Location: <prefix>/ui/`, where `<prefix>` is the
  `X-Forwarded-Prefix` read with the same grammar the web cell uses — a path,
  no trailing slash, no `//`, no `..`, bounded, plain characters — or empty. A
  header that fails the grammar is ignored, not repaired.
- **A daemon under a transient system user finds its delegated cgroup**
  (GH #686). A unit with `Delegate=yes` hands its directory over with the
  controllers offered but none enabled; the daemon now enables `cpu`,
  `memory` and `pids` there itself, moving into a sub-group of its own first
  when it is the only process in the directory. `--sandbox-probe` answers
  `limits yes` under `DynamicUser=yes` + `Delegate=yes`, with and without
  `DelegateSubgroup=`; `ProtectControlGroups=yes` still mounts the cgroup
  tree read-only and stays off in a unit that wants caps.
- **Concurrent compose passes leave one order on the clock** (`display@2.2.1`,
  GH #681). An order's `schedule_id` is derived from the moment it is due
  (uuid5 over the second the timer is told, rounded up like `at`) instead of
  drawn at random, so two passes that read the same stale `due` and land on
  the same second order the same id; the second `add` is acknowledged as the
  order the clock already holds (since GH #690 below -- in 2.2.1 it answered
  `schedule_id_exists`, an `in_tick_error` the cell swallows), and one order
  results instead of two standing side by side.

## [0.36.1] — 2026-09-12

A patch release: the repairs the first days on 0.36.0 turned up, and the gap the
0.36.0 release itself exposed. A cell that a swap reactivated can be swapped away
again without a restart, `code` and the five stateless cells now hand their stop
wiring back the way every other factory does. The FreeSWITCH dialplan contract
resolves a call by the pair (dialled number, caller number), so one switch can
serve many colonies on many numbers with no change to any cell. The `web` README's
proxy example strips the prefix it announces and carries the socket. The export
audit refuses a test that reaches into a directory the export never carries, the
class that made the 0.36.0 public CI red while every private gate was green. And
the design of the one listener writes down the origin of a mounted `web` cell.
Nothing in the contract moved, no migration is needed from 0.36.0.

### Fixed

- **The web README's nginx example strips the prefix it announces**
  (`web@2.0.1`, GH #677). The block's `proxy_pass` had no trailing slash, so the
  prefix reached the listener and its first segment was no mount; the example
  now ends in `/`, names a neutral prefix, and says in words that the proxy has
  to strip what `X-Forwarded-Prefix` announces. A drift lock reads the block out
  of the README and proves both halves against a real cell.
- **The dialplan contract resolves a call by the pair (dialled number, caller
  number)** (`freeswitch@2.0.3`). The `meclaw_line` block in the template's
  README keyed the switch's line table by the caller alone —
  `db(select/meclaw_lines/${caller_id_number})` — and checked the dialled number
  only for being a number, so one caller number reached one colony whichever
  number they had dialled. The block now reads
  `db(select/meclaw_lines_${destination_number}/${caller_id_number})`: one
  `mod_db` realm per dialled number, the caller as the key, the PIN row in the
  same realm. A colony behind a shared switch names the realm of the line it is
  reached on in `params.db_realm` (`meclaw_lines_<dialled number>`), so
  `add_number`, `set_pin` and `disable_pin` write the realm the dialplan reads
  for that number; the caller-only form stays as the one-number case and the
  shipped default. One caller reaching different colonies on different numbers
  and one number serving many callers on many colonies both fall out of it, and
  no cell changes. The README also says, against `mod_db.c`, that a realm is
  persistent: the rows are in the switch's database, a module load touches
  `db_data` only to create it where it is missing, and a row ends by a delete or
  by an insert on the same key (GH #667).
- **A `code` cell reactivated by a swap can be swapped away again** (GH #673).
  A `code` leaf swapped away by name and swung back with the existing-node form
  came back active, and a second swing forward was refused with
  `stop_wiring_unavailable` until the colony restarted. The `code` factory's
  respawn closure built its dispatcher without the colony inbox and dropped the
  fresh stop pair, so the reactivated node had nothing a later disconnect could
  stop it with; the stateful and long-running factories hand the pair back
  through `renotify_stop_wiring`, and `code` now does the same (the five other
  stateless factories carried the same closure; see GH #676 below). A test swings a `code`
  leaf forward, back and forward again in one colony lifetime. Substrate-internal,
  no contract surface moves.
- **The five stateless cells `bash`, `edit`, `file`, `web_fetch` and `web_search`
  keep their stop wiring after a swap reactivates them** (GH #676). Same class
  as #673: each factory's respawn closure built its dispatcher without the
  colony inbox and dropped the fresh stop pair, so a leaf swung back by a swap
  refused the next swing forward with `stop_wiring_unavailable` until the
  colony restarted. The five closures now hand the pair back through
  `renotify_stop_wiring`, in the form the #673 fix gave `code`. A test swings
  each of the five forward, back and forward again in one colony lifetime.
  Substrate-internal, no contract surface moves.
- **The export audit refuses a test that reaches into a directory the export
  never carries** ([#675](https://github.com/mmeyerlein/meclaw/issues/675)).
  The 0.36.0 release passed every private gate and the dry audit, and the
  public CI was red on one test that ran a generator under `workshop/` —
  a root that never ships, and one the audit had no rule for: R2b judges
  `templates/`, R2c judged `plans/` alone, R2d judges `docs/`. R2c now covers
  every root the export refuses outright (`plans/`, `ideas/`, `archive/`,
  `workshop/`), matching the path form — a literal that starts with the root,
  a `../../<root>` chain, or `repo("<root>")` — and not a mention in an
  assertion message. A hit is green only with both a presence guard in the
  test (an existence probe on the same path, which the test returns early
  on) and an entry in the audit's exception list that says why; a guard
  without the entry is red, and so is an entry without the guard, and so is
  an `include_str!` of such a path, which no guard can save. The probe has
  to name what it protects: the root itself, or an identifier the file binds
  from the root reference. Measured over the tree: five exported tests read
  `workshop/` behind a guard, each now listed; the pre-fix shape of the
  0.36.0 test is red under the rule, with or without a stray `.exists()`
  elsewhere in the file. The synthetic self-test carries the outcomes.

## [0.36.0] — 2026-09-12

The screen brings its own design language. `display@2.1.0` ships the token
sheet as a file of its own and carries it inside the shell, together with a
vocabulary of twenty-six components that an application names in its tree
without defining; a compose cell swapped for a newer one recognises a page
whose vocabulary is older than its own and redefines it in the same bundle; and
the two faces the sheet names are an operator asset, declared by one directory
or not at all. Under the screen, the builder and the substrate close what the
first colonies on 0.35.0 found: a template can say which of its params have no
usable default, and the door refuses a node grown without them; a class may
carry more than one version in the library; a colony writes to stderr and keeps
`log.jsonl` beside it; a member wish counts nothing any more; the arrival turn
of a telephone call says the caller is on the line; a mistyped key in a swap is
refused instead of committed; and an inspector's sheet no longer paints the
page it stands on.

### Breaking

- **The builder no longer counts members** (`builder@1.9.0`, `meclaw-os@1.8.9`
  for the pin). A caller that subscribed to `hop.error_code = count_unavailable`
  on the builder's `error` lane no longer receives it; a member wish that would
  have raised it is now rendered. Nothing shipped consumed the code —
  `meclaw-os` routes the `error` lane whole (GH #663).

- **`add_templates` refuses a `version` no reference can name.** An entry whose
  `template.json` declares a `version` that is not `major.minor.patch` is
  rejected with `schema`, pre-destructive — where it used to register. Such an
  entry then answered to nothing: `@<that string>` is not a reference the
  resolver parses, and the bare name found neither a readable version nor an
  unversioned entry, so the class sat in the catalogue and no `add_nodes`
  reached it. **Migration: give the template a three-digit version.** Nothing in
  the shipped library is affected — every `template.json` under `templates/`
  already declares `major.minor.patch` — and a template that declares no
  `version` at all stays legal. This is the one narrowing of the operation;
  everything else about it is additive (GH #664).

### Added

- **A colony writes to stderr, and keeps `log.jsonl` beside it.** Only the file
  was written before, so under an init system `journalctl -u` showed the service
  lifecycle and nothing of the colony's own life, while the file it wrote
  instead was rotated by nobody. Without a flag both sinks are on now: stderr
  carries one compact line per event, where the init system owns the ring buffer
  and the rotation, and the file stays JSON and stays `jq`-able,
  so every script that reads it keeps working. `--log-stderr <auto|off>` and
  `--log-file <auto|off>` switch a sink off, `--log` still names the file's path
  and `--log-level`/`--log-filter` still set its filter. `RUST_LOG` now has an
  effect and steers the stderr half alone, so a variable in a runner's
  environment cannot redirect what a script reads out of the file. A substituted
  `${VAR}` value reaches neither sink, pinned for both (GH #662).
- **A param that has no usable default says so, and the door enforces it**
  (`operator_set` in `contract.settings`, `error_code`
  `operator_param_unset`). A template's `params` are defaults, and a wish that
  leaves them alone is legal — but some are not defaults in any useful sense: a
  URL pointing at localhost, an empty identity, an empty allowlist. Measured on
  0.35.0: a wish that described the wiring of a telephone channel grew its
  signal half with every shipped value, committed green, and left a colony with
  a channel that would never have reached any switch. A `SettingSpec` can now
  carry `operator_set: true` — *this value has no meaning until an operator sets
  it; the shipped `default` is a shape, not a working value* — and a mutation
  that grows a node from that template without setting it is refused
  pre-destructively, at both doors that grow one (`add_nodes` and the
  instantiate form of `swap_nodes[].with`), whoever submitted it. What is
  checked is the act and not the value: setting the param to exactly the shipped
  default passes. Unset params are collected, so three are named in one refusal.
  The design lane reads the declaration a round earlier: the briefing states the
  rule and the catalogue marks the params with `[operator-set]`, so a wish that
  does not name the value comes back as `wish_incomplete` with the question.
  Additive — a template that says nothing behaves exactly as it did (GH #661,
  ADR-0032).

- **A class may have more than one version, and `add_templates` takes the new
  one.** The library is keyed by name AND version: a new version of a registered
  class registers beside the old one, which stays where it is and stays
  resolvable. The write goes to `{templates_root}/local/<name>@<version>/` — the
  colony builds that path from the clamped name and the version the entry's own
  `template.json` declares, and `local/<name>/` when it declares none. Two
  refusals remain, both under the existing `error_code` `template_name_taken`:
  the identical `name@version`, and an entry nobody can pin — one without a
  version while versioned entries of that name exist, or the mirror case. The
  scan follows: two `template.json` abort it when they share a name AND a
  version (`ScannerError::DuplicateVersion`), two versions of one name are two
  entries of one class. A bare reference still has exactly one answer, given by
  the resolver (highest version) rather than by an aborted scan. Directories
  already lying under `local/<name>/` are untouched and stay resolvable; there is
  no migration and no `remove_templates` (GH #664, ADR-0033).

- **The screen brings its own design language** (`display@2.1.0`). The only
  sheet a display page linked was the `web` template's `/vision.css`, the base
  language of every surface a colony can have, and every application that
  wanted the screen to look like one thing sent that look as `client_css` on its
  own view — four applications, four copies of one sheet. The `display`
  template now ships the sheet as a file of its own,
  `compose/display-dna.css`, and carries it inside the `display-shell`
  definition: one `<style>` block, the screen's layout first and the sheet
  last, so it re-tokenises `/vision.css` at equal specificity. With the sheet
  comes the vocabulary it is written against: a catalogue of twenty-six
  components — four windows (`display-pane`, `display-panel`,
  `display-overlay`, `display-ornament`, the only glass in the catalogue) and
  twenty-two pieces of content, from `display-value` to `display-progress` —
  that an application names in its tree without defining, bringing in
  `components[]` only what is its own. `display-view-prose` wears the catalogue
  (`display-pane` outside, `display-kicker` and `display-text` inside); the
  custom wrapper stays a bare content shell, so an application may hang its own
  pane in it. Every class the thirty-one templates write has a rule in what the
  shell ships — the custom wrapper no longer writes the class `view` and
  `display-document` no longer writes `display-document-page`, neither of which
  any sheet ever had a rule for — and the shell at the shipped defaults stays
  under 80,000 bytes raw. `web` does not move: `/vision.css` remains the base
  language, loaded first. Two applications outside this repository move to the
  screen's vocabulary in the same wave (GH #669, ADR-0034).
- **A screen recognises its own outdated vocabulary** (`display@2.1.0`). The
  compose cell defined its components on the bootstrap pass only — the page has
  no `/`, or a root that is not this cell's — so a compose cell swapped for a
  newer version found a page that was already its own and kept sending views
  against the vocabulary the old code had defined. The root now carries `vocab`,
  a twelve-character fingerprint of the definitions as JSON, computed once at
  import. A read pass whose root holds another fingerprint sends every
  `component.define` again and brings the root up to date in the same bundle;
  the next pass sees the two agree and sends nothing. Once per change of
  vocabulary, never per tick — the same economy an application's components
  already had. The price is the one a redefinition always has: every route of
  the `web` cell re-renders, once (GH #670, ADR-0035).
- **The faces are an operator asset** (`display@2.1.0`, `params.font_base`).
  The sheet names Inter and Fraunces at the head of two fallback stacks and
  declares neither. `font_base` is the directory the two files are served from,
  relative to the page's own base, and the compose cell builds the two
  `@font-face` rules from it as a raw prop of the shell. Empty, the shipped
  default, declares nothing: the fallback stacks carry the type and the page
  makes no request. The file names are fixed, `inter.woff2` and
  `fraunces.woff2`, so what an operator provides is one directory. No font file
  ships in this repository, and a test walks `templates/` to keep it so
  (GH #672).

### Changed

- **A member wish reads nothing off the tree** (`builder@1.9.0`,
  `meclaw-os@1.8.9` for the pin). The fast lane counted the members an
  organisation already carried so the screen every member gets could be given a
  port nobody else held. Since `display@2.0.0` a screen owns no port and answers
  under a name made from the member's own, so the count was still measured — one
  `/colony/graph` round trip and two store operations per member wish — and
  spent on nothing. The counting cell, the six edges it stood on, the second
  route out of the switch that fed it and the refusal that named an unreadable
  number are gone; what refuses a member wish now is a name that renders no
  mount, and nothing else. A member wish costs no colony round trip and no store
  operation (GH #663).

### Fixed

- **The inspector paints its own view and not the page** (`colony-view@1.1.1`,
  GH #671). A view's `client_css` goes into the page raw, and `colony-view`'s
  sheet wrote `html,body{ margin:0; height:100%; background:… }` and two
  document-level `@media (prefers-color-scheme)` blocks on `body` — so one
  application set the ground of the page for every other view standing beside
  it on the same screen, and for the screen itself, on every re-emit of the
  view. A hand-run
  bridge had attributed the block to the host page; measured, it came from the
  application. The three document rules are gone, and the view's root carries
  the height it needs inside a slot (`position:relative; height:62vh;
  min-height:22rem`) instead of `position:absolute; inset:0`. The palette and
  the depth tints stay on `.colony-view`. The rule behind it is named in the
  `web` template's README as one the cell does not check — `html`, `body`,
  `:root` and a document-level colour-scheme query belong to the screen, and a
  CSS parser in the cell would be a second language in the substrate — and it is
  pinned on the shipped sheet instead: a test refuses any selector whose subject
  is the document, and a second one asserts that no ops path has grown a
  selector check behind the sentence (ADR 0036). `web` stays `2.0.0`.
- **The scan refuses a `version` no reference can name** (GH #668). The walk over
  `templates/` read a `template.json`'s `version` verbatim, so a descriptor
  placed by hand with `"version": "1.0"` was registered and then unreachable:
  `resolve` parses a reference with `parse_simple_version`, so neither the bare
  name nor `name@1.0` ever answered. Since GH #664 the registration door refuses
  exactly that with `schema`; the scan gives the same answer now. It SKIPS the
  entry rather than aborting — a library is a directory anyone may write into,
  and one descriptor must not cost a colony its boot — and the skip is named,
  with the path, the version and the reason. Every shipped template carries
  three digits, so nothing in the tree moves.
- **The arrival turn of an inbound call says what is true** (`freeswitch@2.0.2`).
  The dialplan answers an inbound leg at once and streams it, and the template's
  README has said since `1.0.1` that `call_incoming` is the one turn of such a
  call — the moment a person is on the line. The turn itself read `Incoming call
  from <number>.` with `call_state: incoming`, and named only the number.
  Measured on a real call: the assistant read it as a notice to the owner and
  answered *"Incoming call from …. Shall I pick up?"* — into the line, to a
  caller who was already connected, who replied that they were already on. Three
  turns lost before the conversation began. The turn now reads `The caller is on
  the line. Greet them.` and carries `call_state: live`, at both arrival places:
  the free line and the caller put through from the queue. No question, because
  a model answers what it is handed; nobody named in it, because the number and
  the member id travel in `hop`, not in the text, so nothing reads an id out
  loud; `live`, because that is the state the row is booked into in the same
  second (GH #665).
- **`freeswitch` declares the three params an operator has to set**
  (`freeswitch@2.0.2`). `voice_ws_url`, `line_user_id` and `callers` ship a
  value that is a shape and not a working one — a URL pointing at localhost, an
  empty identity, an empty allowlist — so they now carry `operator_set` and a
  channel grown without them is refused at the door instead of committing
  something that reaches no switch. `dial_prefix` is deliberately not among
  them: the gateway it names works (GH #661).
- **A mistyped key in a swap no longer commits in silence.**
  `add_nodes[].override_params` runs through the override-key check of GH #294:
  a key the template does not declare is refused pre-destructively. Its twin on
  the swap side did not, although staging maps `swap_nodes[].with.params` onto
  the same contract before it writes the new node's overlay — so a typo in a
  swap committed, the node ran with the shipped default, and nothing said a
  word. Both doors ask the same question now, with the same `error_code` and the
  same receipt shape (GH #666).
- **In `hold` mode the recognition session lives per hold** (`voice@2.0.1`,
  `freeswitch@2.0.1` for the pin of its media half). The session used to open
  with the connection, so a push-to-talk page that nobody spoke into ran into
  the provider's own idle deadline (`provider_idle_timeout_ms`, 30 s by
  default): an `stt_failed` frame, one retry, the same ending again, and a
  `1011` that closed the socket — a page left alone for a minute had a dead
  button until it was reloaded. Now the session opens with the first `hold`,
  and one that ends with the key up is dropped in silence; the next `hold`
  opens a new one and the audio behind the frame waits in the queue. `auto`
  mode, which telephony runs, is unchanged: there the session is the call
  (GH #657).
- **The screen's hold-to-talk button says what it is waiting for**
  (`display@2.0.1`). Three moments used to pass in silence: while the browser
  was asking for the microphone the page said nothing, so a press that was
  waiting on a permission prompt looked like a press that did nothing; a key let
  go before that answer arrived vanished without a word; and a closed call
  disabled the button for the life of the page. Now the line under the button
  reads `asking for the microphone…`, then `press again` when the permission
  came too late for that press, and a close leaves the button alive — the next
  press joins a new call. A refused join stays final, because that answer is
  about the screen and not about one call (GH #658).
- **The design lane's briefing states the modifier grammar** (`builder@1.8.1`,
  `meclaw-os@1.8.8` for the pin). The briefing described an `add_edges` entry as
  two endpoints "plus an optional `condition` and `modifier`" and stopped there:
  `set_context`, `set_hop`, `delete_context`, `delete_hop` and `lane` appeared
  in it zero times, so the one compartment that decides what an edge does to a
  message was named and never explained. Measured on a colony built entirely
  through that lane: four build requests, all four committed, and 13 of 25
  contract edges drawn wrong — bare identifiers where a CEL value needs its own
  quotes, promotions written into `set_hop` where the context is what survives
  the next hop, and no v-lane carrying its `lane` field. The briefing now names
  the five keys, which compartment each one writes, that a failed modifier skips
  the whole edge, and that a lane is a field rather than a condition (GH #599;
  the validation half of that issue stays open).
- **A `voice` mount in its teardown window answers `503 surface busy`** instead
  of swallowing the connection. The handoff channel used to fall only when the
  I/O half returned, which is after the mount registration goes: in the window
  between the two a late connection was handed into a queue nobody reads any
  more and got no answer at all, neither a status nor a close. The channel is
  now closed before the registration, so the listener gives the answer it
  already promises for a mount whose reader is gone. An upgraded socket ends
  with the cell in the same window: every connection closes itself with `1001`
  when the half ends, and the next life registers the mount again and answers a
  new socket. Both promises now have a test; the listener's peek constants
  turned crate-private and the drop order in `web` and `voice` carries its
  reason as a comment (GH #660).

## [0.35.0] — 2026-09-11

A surface cell has no port. A colony has one listener, and everything on it
that is not the HTTP API is reached under a name: a display at
`http://<listener>/<mount>/`, a voice door at `ws://<listener>/<mount>/ws`.
`params.port` and `params.bind` leave the `web` and the `voice` cell type
altogether, and a document that still carries one is refused at parse with the
migration in the message. The OS hands out a mount per member instead of a
port. In front of a telephone the switch is the proxy: it maps a number and a
PIN to a colony's listener and the mount of that colony's telephone half, and
stamps who is calling into the event it posts. This release carries what the
one-listener wave built and what the ruling after it changed, in one Breaking
section.

### Breaking

- **`web` and `voice` have no port, not even as an option.** `params.port` and
  `params.bind` are removed from both cell types, and `params.mount` is
  required. A params document that still names one of the two is refused at
  parse, before every other check, with the migration in the message:

  ```
  port: removed in web 2.0.0 — the cell is reached at /<mount>/ on the colony's
  listener; drop the key and name a mount
  ```

  `bind:` is told the same sentence, and a `voice` document reads
  `voice 2.0.0` where this one reads `web 2.0.0`. It is a refusal rather than
  an ignored key because a display whose port was dropped in silence would come
  up at an address nobody asked for.

  **Migration.** Drop the two keys, name a mount, and reach the cell on the
  colony's listener: a display at `http://<listener>/<mount>/`, a voice socket
  at `ws://<listener>/<mount>/ws`, its declaration at `/<mount>/info`. What
  stood in front of a cell's own port — a reverse-proxy rule, a firewall line —
  stands in front of the one listener, and one rule covers the whole colony.
  `web@2.0.0`, `voice@2.0.0`, `display@2.0.0`, `canvy@2.3.0`. ADR-0014 is
  superseded by ADR-0031 (GH #654, umbrella #653; GH #645, umbrella #639).
- **An upgraded cell's `params` table has to lose its `port` and `bind` rows.**
  A param update is persisted in the `params` table of the cell's `cell.db` and
  replayed over the birth params on every wake and every respawn, so a `port`
  or a `bind` once set on a single cell outlives the upgrade of its template and
  `2.0.0` refuses the merged document. Delete those rows before the cell comes
  up under the new version; editing `config.json` is not enough. An
  `override_params` merge cannot remove a key either, which is why a `port` set
  to `null` is refused with the same sentence rather than read as unset.
- **The OS hands out a mount instead of a port.** The `builder` recipe knob
  `screen_port_base` (a number, `7900`) is replaced by `screen_mount` (a string
  with one placeholder, default `{member}-display`), and the member grow recipe
  writes `override_params.web.mount` from it. Two organisations with a
  same-named member render the same mount: the second registration answers
  `MountFailed`, that cell stays up and is not reachable under the name. The two
  ways out are a member name that is distinct colony-wide, or a hand-written
  manifest carrying its own `override_params.web.mount`. `builder@1.8.0`,
  `meclaw-os@1.8.7` (GH #655).
- **`freeswitch@2.0.0`: the switch is the proxy.** `voice_ws_url` defaults to
  `ws://127.0.0.1:7777/phone/ws`; the form is `ws://<listener>/<mount>/ws`, and
  the media half mounts as `phone`. The switch's own table is the register: one
  row per line in the realm `meclaw_lines` (`params.db_realm`), keyed by the
  caller's number, with the value `<line_user_id>|<voice_ws_url>` — who the
  caller is put through as, and where the audio is streamed — and the PIN in a
  row of its own under `pin.<user>`. `call_incoming` trusts `hop.user_id` as the
  identity the switch verified; `params.callers` stays the fallback for a
  dialplan that stamps none, and a call with neither is refused `unknown_caller`
  as before (GH #616).
- **`GET /colony/surfaces` rows carry `mount` and `kind`.** The `own_addr`
  column is gone: no surface cell binds an address of its own, so the second
  address slot in the row had nothing left to carry. The `?format=traefik`
  document is unchanged.

### Added

- **A surface mounts by name.** A `voice` cell takes `params.mount` and is
  reached under that name on the colony's one listener: the socket at
  `/<mount>/ws`, the declaration at `/<mount>/info`, the test page at
  `/<mount>/`. The name lives in a process-level registry the cell's I/O half
  writes on every life, and a name another cell holds is refused out loud — the
  cell stays up and is simply not reachable under it. The grammar is narrower
  than a path segment (`[a-z0-9-]{1,64}`, and never a first segment the API
  owns), so a name that needs escaping is refused before the cell spawns.
  `mount` is mutable, and a mount that moved takes effect on the next life of
  the cell. A member with a browser voice channel and a telephone holds two
  mounts, `voice` for the screen's half and `phone` for the switch's. Decision:
  ADR-0031, which supersedes ADR-0014 (GH #642, umbrella #639; GH #654,
  umbrella #653).
- **One listener for what is not a browser.** `--api` reads a connection's
  first request line and hands the stream, unread, to the cell that mounted the
  first path segment; a mount is reached under `/<mount>/…` on that port and
  serves its own routes there. Everything else on the same socket is the HTTP
  API, as before. A mount that cannot take another connection answers `503` and
  closes, and a connection is decided once, on that first line. `GET
  /colony/surfaces` publishes the mount table: the listener's address and one
  row per mount with its name and its cell type. `?format=traefik` renders the
  same table as a Traefik HTTP-provider document, one router per mount and one
  service for the listener, with the request's `Host` header as the service
  URL; any other `format` is a `400 bad_query` (GH #644 and #645, umbrella
  #639).
- **The listener core is a colony function.** The peek-and-hand-off loop, the
  accept error classes and the drain live in
  `meclaw_colony::surfaces::listener` as
  `serve(listener, registry, fallback, shutdown)`, with a `Fallback` trait for
  the connection whose first segment is no mount; the CLI wraps its API router
  in one and keeps nothing else. `meclaw_testing::surface_listener(registry)` is
  that same core with a `404` fallback, so a fixture holding a display and a
  voice cell binds one port for both instead of one each (GH #654).
- **The shell is prefix-aware.** A `web` cell writes every link of its page
  from `X-Forwarded-Prefix` plus its own mount, so a proxy may put a display on
  a domain path or on a subdomain root and say which. The header is read only
  when it is a path (`^/[A-Za-z0-9._~/-]{0,200}$`, no trailing slash) and never
  when it leads with `//` or carries a `..` segment; anything else is ignored
  rather than trusted. The socket URL, the asset bundles and the page's
  `<base href>` follow that base, and the LiveView join's URL is stripped of it
  again, so a `page.set` route stays what it was (GH #645).
- **`identity_header`.** A new `web` param, empty by default. With a request
  header named, the value that header carried at the socket upgrade travels as
  `hop.user_id` on every semantic browser event of that connection. Nothing is
  stamped while the param is empty, because a header a client can set without a
  proxy in front of it is not an identity. A `hop` is single-hop, so the edge out
  of the screen owes the stamp a promotion into `context.user_id`: the member
  grow recipe renders it (`builder@1.8.0`), and
  `examples/organism/grow-screen.json` carries it. Separating members — auth,
  cookies, storage — is the proxy's job, and a page that needs browser storage
  keys it by mount (GH #645).
- **Audio in the display window.** `display@2.0.0` carries a hold-to-talk
  button. Pressing it joins a `voice:<call>` topic on the LiveView socket the
  page is already holding, and the `web` cell hands those frames to the `voice`
  cell mounted under the name the display was given (`voice_mount`, default
  `voice`). There is no second port and no second connection. Three events carry
  it: `frame` for a text frame either way, `audio` for binary either way,
  `close` for the code. Everything the wire protocol says stays true on the
  topic: both modes, `4409`, `client_too_slow`, and a refusal is the same
  sentence the socket door answers with, carried as the join's error reason,
  because both doors run one admission. One socket holds at most four live
  `voice:` topics; the fifth join is refused `too many voice topics on this
  socket`. The page cuts 20 ms PCM16 frames in an `AudioWorklet` and plays the
  answer through an `AudioContext` at the rate `hello` declared; a microphone
  needs a secure context, and the button says so where there is none. Two
  proofs travel with it: a Rust client that speaks through the display's socket
  and reads the turn on the topic, and headless Chromium driven over CDP that
  holds the real button with a real microphone. `voice` gains `mount`-side
  documentation in
  [docs/voice-wire-protocol.md](docs/voice-wire-protocol.md) (GH #643, umbrella
  #639).
- **A socket ends with the cell.** Every handed connection of a `web` and of a
  `voice` cell is held by its I/O half rather than detached, so a respawn takes
  its sockets with it instead of leaving a browser or a call talking to a cell
  that is gone (GH #654, #645).
- **A member name that renders no mount refuses the whole wish.** The builder
  checks the mount grammar before it renders anything: the length, the alphabet
  and the reserved first segments stand in the recipe as they stand in
  `meclaw_colony::surfaces`, a name that would render an invalid mount comes
  back as `wish_incomplete` naming the grammar and the field, and no manifest is
  written — a manifest rolls forward and has no rollback, so half a member is
  worse than none. A test holds the recipe's copy of the reserved segments
  against the substrate's, in both directions (GH #655).
- **Three tools write the switch's line table.** Beside `call` and `hangup` the
  `freeswitch` channel offers `add_number(number)`, `set_pin(pin)` and
  `disable_pin()`. Each one is a single GET at the switch's `/webapi/db`
  (`mod_db`: `db insert/<realm>/<key>/<value>`, `db delete/<realm>/<key>`), and
  the answer is a receipt. Two `error_code` strings are new: a switch that
  refuses the write says `line_write_failed`, and a hive without `line_user_id`
  says `line_unconfigured` and writes nothing. A PIN the dialplan could not read
  back — anything but 4 to 8 digits — is `invalid_arguments`, the code the
  channel has carried since `freeswitch@1.0.0`, with nothing sent.
  The PIN rests where the switch keeps it, in that host's own database, guarded
  by that host's access control (GH #616).
- **The dialplan contract for a switch in front of several colonies.**
  `templates/freeswitch/README.md` § *What the dialplan owes* carries it as
  copyable XML: the `db(select/…)` lookup of the calling number, the PIN prompt
  (`play_and_get_digits 4 8 3 8000 … ^\d{4,8}$`), the compare, one generic
  connect block that streams the call to the URL the row named, the
  `call_incoming` post with `user_id` on it, and the announcement a refused line
  hears. Adding a colony to such a switch is one row in its table (GH #616).

## [0.34.0] — 2026-09-09

The documentation has one shape now, and the quick start is one line that
asks for the key. The README is a hub of four paragraphs and links; a flat
concept layer sits between it and the reference files, one page per
question; the why pages, the glossary and the folder READMEs were rewritten
to that shape, the reference files got a head each and lost the prose other
pages carry. `start.sh` asks for the provider key on a terminal, writes it
to one file and grows the assistant; `MECLAW_EXAMPLE=organism` boots the
shell so the five steps of getting started run end to end and `meclaw ask`
reaches the agent grown last. Two defects found on the way are repaired: a
`voice` client that stops reading is dropped out loud with an `error_code`,
and `ask` no longer reads a sibling hop's dead letter as the verdict on its
own turn. Nothing in the contract breaks; one `error_code` is added.

### Added

- **The quickstart is one line.** `scripts/start.sh` installs the binary
  through `install.sh`, fetches the templates and examples that match it,
  picks a free port, starts a daemon in the background and grows a colony into
  it. Without `OPENROUTER_API_KEY` in the environment it grows
  `examples/hard-shell`, sends the cloud-metadata fetch and prints the refusal
  out of `GET /colony/trace`; with the key it grows `examples/meclaw-os`, sends
  one `meclaw ask` and prints the answer. It stops at the first failing step
  and names it, never prints the key, and writes it to one file only, the
  colony's `.env` (GH #627). The long form, every step and every knob, is
  [docs/installation.md](docs/installation.md).
- **`start.sh` asks for the key.** Without `OPENROUTER_API_KEY` in the
  environment and with a terminal to ask on, the run asks for the key before it
  installs anything. The answer is read from `/dev/tty` rather than stdin,
  because in the one-liner stdin is the pipe the script arrives on; it is not
  echoed while it is typed, and echo is restored however the run ends. With the
  key in the environment nothing is asked. With no key and no terminal -- a
  pipeline, a CI job, a cron line -- the run says how to set it and boots the
  keyless colony as before (GH #630).
- **The five steps of getting started run end to end.**
  [docs/getting-started.md](docs/getting-started.md) is new: install, start the
  `meclaw-os` shell, then grow an organisation, a member and an agent of your
  own into the running colony and talk to it. `start.sh` grew the flat
  assistant and nothing else, so the three declarations of
  `examples/organism` had no shell to land in; `MECLAW_EXAMPLE` now picks the
  colony, and `organism` boots `examples/organism/seed-ref`, whose root tree
  declares the shell and grows it on the first boot. The colony's `.env` gets
  every model token the shipped declarations read, not just `MODEL_BRAIN`
  (GH #631).
- **`examples/organism/grow-door.json`.** A `door` and a `terminal` at the
  colony root, four edges. The door puts an inbound turn on the `in_turn` lane
  and stamps `context.assistant`, which is what a channel does for the person
  using it, so `meclaw ask --target /door` reaches the grown agent. The
  terminal takes `answer`, `write` and `turn_write` where they leave the shell,
  which used to dead-letter; `error` and `reject` stay undrained (GH #631).
- **Both scripts are release assets.** The release workflow uploads
  `install.sh`, `start.sh` and a second archive
  `meclaw-<version>-templates.tar.gz` (the `templates/` and `examples/`
  directories of the tag, with a SHA-256 sum) next to the binary. The
  canonical URLs are
  `https://github.com/mmeyerlein/meclaw/releases/latest/download/install.sh`
  and `.../start.sh`; `https://meclaw.ai/install.sh` and `/start.sh` redirect
  there. `start.sh` falls back to the tag tarball for releases that predate
  the templates asset.

### Changed

- **The documentation has one shape.** `README.md` is a hub: four
  paragraphs, a one-line quick start, links into `docs/`. `docs/README.md` is
  the start page, and a flat concept layer sits between it and the reference
  files: `getting-started.md`, `model.md`, `meclaw.md`, `cells.md`,
  `meclaw-os.md`, `security.md` with `security/secrets.md`,
  `templates-and-apps.md`, `status.md`, and an index for `why/`. Every page
  answers one question in the same order: what it is, why it exists, how to
  use it, where to read on. The eight `why/` pages, `glossary.md`,
  `stability.md`, `costs.md`, `examples/README.md`, `templates/README.md` and
  `CONTRIBUTING.md` were rewritten to that shape; paths under `why/` did not
  move. The reference files (`meclaw-overview.md`, `cell-types.md`,
  `config.md`, `rewiring.md`, `store-backed-tool-loop.md`,
  `voice-wire-protocol.md`, `installation.md`) keep their structure and got a
  head of ten lines each, lost the prose another page carries, the comparison
  table with other systems, and the internal phase markers (GH #632, #633,
  #634, #635, #636).
- **`SECURITY.md`** names the private reporting path; private vulnerability
  reporting is enabled on the repository.

- **`voice@1.4.1`: a client that stops taking frames is dropped out loud.** The
  cell gives up on a WebSocket client once 64 commands are queued behind an
  already full connection channel, and that verdict used to reach a colony as a
  `Disconnected` with no reason attached. It now emits one message on the
  `error` lane first, with `hop.error_code` `client_too_slow`, the call under
  `hop.session_id` and `hop.call_id`, and `hop.dropped_frames` naming how many
  queued commands went with the session. Nothing changes on the topology side,
  which was and stays backpressure: a listener that falls behind stalls the
  sender and loses nothing. `templates/voice/README.md` says both halves now
  (GH #601).

### Fixed

- `meclaw ask` no longer reads a sibling's dead letter as the fate of the turn
  it sent. A trace holds everything one turn sets off, so a lane that emits on
  the side and finds nobody to consume it lands in the queue under that trace
  while the answer is still being worked on, and the command ended with `1` and
  a `no_route` message seconds before the answer arrived. The queue is now
  matched by `message_id`, so only an entry that killed the posted message ends
  the wait; a mistyped `--target`, which is what the queue is read for, still
  ends it at once (GH #640).
- The example cell counts agree with the tests that pin them: `meclaw-os`
  17, `never-forgets` 16, `organism` 92 cells and 565 edges, and
  `display-colony-view` 8, now measured by a test of its own (GH #638).
- `rewiring.md` instantiated two templates that do not exist; the diffs now
  name `fetcher`. Two references to closed issues (#390, #83) are gone.

## [0.33.0] — 2026-09-08

`meclaw ask` sends one turn to a running colony and prints the answer; the
quickstart used to need two `curl` calls and a `jq` pipeline for that. The README
and the `docs/why/` pages were rewritten for a first-time reader, and the three
pages that had answered 404 since 0.32.1 are back. The telephone path learned
what a second call is and what sample rate a call actually carries. Three
substrate defects are repaired: a hive contract that read an edge without looking
at its direction, two timer schedules sharing a second where only one fired, and
endpoint validation that resolved a bare short name against a foreign scope. One
change is breaking, and it is the first item below.

### Breaking

- **A sealed hive's interior is no longer addressable from outside.** A message
  that enters the colony from outside — the HTTP ingress, and every other source
  message — and names a CELL inside a hive that declares `params.ports` is
  refused with the new `error_code` `hive_boundary` instead of being delivered
  past every door the hive put in front of it. This is the rule
  `docs/meclaw-overview.md` § The hive boundary has stated since 2026-08-18
  ("`<hive>/<cell>` is not an address … including where the substrate still
  resolves it today for want of a declaration"); GH #133 enforced it for
  `add_edges`, and until now nothing enforced it for a message.

  **Migration.** Address the hive and name the lane: `{"target": "<hive path>",
  "hop": {"route": "<lane>"}}` (GH #175). Where the hive declared the interior
  address itself — an entry in `params.ports`, or a `params.contract`
  `accepts[].at` connect point — that address still answers, and the connect
  point answers on its own lane only. A LEVEL inside a sealed hive is refused
  exactly like a cell: five shipped templates carry a nested hive through a `ref`
  marker, and treating a nested rim as an address would have made it the way
  around the boundary. The hive path itself is of course still an address.
  Internal traffic is unaffected — inside the hive the graph is the hive's own
  business — and a hive that declares no ports is untouched, exactly as under
  GH #133's opt-in.

### Added

- `voice@1.4.0`: every emission carries `hop.call_id` beside `hop.session_id` —
  the same value under the name a channel addresses the connection by — and an
  `in_speak` selects its connection with `context.call_id`. The `hello` frame
  and the emission tables in `docs/cell-types.md` and
  `docs/voice-wire-protocol.md` name it. A phone hive with two calls in flight
  can now attribute a `turn`, a `partial`, a `speak_end` and an `error` to the
  call it belongs to (GH #620).

- `freeswitch@1.1.0`: `params.second_call` (`busy`, `queue`, `parallel`) and
  `params.capacity` decide what an inbound call gets while another one is
  running, and every outcome leaves a row in the `calls` table AND a receipt on
  its own lane — `call_accepted`, `call_queued`, `call_refused`,
  `call_abandoned`, each carrying `hop.call_id`, `hop.policy` and
  `hop.capacity`. A queued caller is taken off the recogniser with
  `uuid_audio_stream <uuid> pause` (never `stop`, which would close the
  websocket and end the media half's session) and hears
  `params.queue_hold_media` (the switch's own hold media by default, so a queue
  costs nothing at a vendor); the queue is emptied by the end of a call and by
  nothing else — no timer, no poll. `hangup` takes an optional `call_id`
  (GH #620).
- **`meclaw ask`: one turn to a running colony, the answer on stdout
  ([#623](https://github.com/mmeyerlein/meclaw/issues/623)).** Reading an answer
  used to take two `curl` calls and a four-line `jq` pipeline over
  `/colony/trace`. It is now one command:
  `meclaw ask --api 127.0.0.1:7777 --target /door "Say hello in one short sentence."`
  It POSTs the turn the API already accepts, follows the trace of the message it
  sent, and prints the first hop that travels on route `answer` or `error`.
  `--channel` names the conversation (default: a fresh `ask-<uuid>`), `--timeout`
  bounds the wait (default 120 s), `--json` prints the hop unchanged for scripts.
  Exit codes: `0` on `answer`, `1` on `error` and on a transport failure, `2` on
  timeout. A mistyped `--target` is acknowledged with a 202 and then dies in the
  router, so the command watches `/colony/dead_letters` as well and reports the
  refusal with its `error_code` instead of waiting out the timeout. No new HTTP
  route and no change to an existing one — the command is a client of the three
  routes the quickstart and the operator UI already used, so the contract does
  not move.

- `voice@1.4.0`: **the cell negotiates its sample rate per connection** (GH #619).
  A client names what it sends with
  `ws://…/ws?session=<id>&sample_rate=8000`, and the recognition session runs at
  that rate instead of at whatever the template configured. The two directions
  are answered separately: inbound is binding — a rate the recogniser does not
  serve refuses the connection with a `400` naming the rates it does — while
  outbound falls back to the synthesis provider's own rate, which
  `hello.audio_out` then declares. `?encoding=` is accepted for symmetry and
  takes the one value this protocol version has, `pcm_s16le`; any other name is
  a `400`. `GET /info` gained `audio_in_rates` and `audio_out_rates`, so a
  client reads the negotiable sets instead of provoking a refusal to learn them.
  A client that sends neither parameter is served exactly as before, and the
  cell still never resamples.
- `hive_boundary` joins the canonical `error_code` vocabulary (README §
  Stability). It is distinct from `unresolved_path` (the path exists) and from
  `hive_no_route` (the hive was never asked to forward).
- A dead letter can carry a `detail`: the one reason-specific fact its six
  locating fields cannot. For `hive_boundary` it is the absolute path of the hive
  that refused the address — the receipt names the boundary, which is what the
  report asked for and what `resolved_target` alone could not say. Additive
  everywhere: a NULL-able `dead_letters.detail` column behind a guarded `v9→v10`
  migration (an existing row keeps every value it had), and a
  `/colony/dead_letters` field that is omitted when there is no such fact, so a
  reader that never asks for it sees the JSON it always saw (GH #612).

### Changed

- `voice@1.4.0`: `consumes.context.session_id` is no longer declared `required`,
  so a message that names only the call reaches the cell; it is still read
  wherever `context.call_id` is absent. **The ownership of `context.session_id`
  is unchanged** — it stays the member's `session-keeper`'s, and this release
  only ADDS the call key beside it. A colony wired against `voice@1.3.0` keeps
  working unchanged. `contract.ingress.context` does not move either: its list
  is the standard header convention and `call_id` is not one of them, so the key
  reaches context through the channel's own ingress edge.
- `voice@1.4.0` retracts the `voice_session` workaround of GH #603 § 3. The
  installing manifest in `templates/voice/README.md` loses the second context
  key and the restamp on the way down; an answer is addressed with
  `context.call_id` and nothing rewrites it in transit.
- **`freeswitch@1.1.0` changes what a `1.0.1` colony does with a second call.**
  The shipped defaults are `second_call: "busy"` and `capacity: 1`, so after a
  bare `swap_nodes` a second inbound call that `1.0.1` would have answered is
  ended at the switch with `USER_BUSY` and leaves a `call_refused` receipt. That
  is the point of the release, and it is still a behaviour change nobody asked
  for: the way back to the old behaviour is one knob,
  `override_params: {"signal": {"second_call": "parallel", "capacity": <n>}}`,
  which lets `n` calls run at once — with the caveat that two calls then share
  one `context.channel` unless the manifest's channel line is changed too
  (`templates/freeswitch/README.md` § *What a second call gets*).
- `freeswitch@1.1.0`: `hangup` refuses with `ambiguous_call` and lists the live
  calls instead of ending the newest one when the model did not say which line
  it meant. Ending the wrong line cannot be undone. Both halves of the channel
  stamp `hop.call_id`, and the media half is pinned at `voice@1.4.0`.
  `templates/freeswitch/README.md` retracts its own paragraph *A second call at
  a time* and names what is still missing: a queue nothing caps, no clock on a
  waiting caller, and an arrival count with the same one-round-trip window the
  `speaking` count has.
- `freeswitch@1.1.0` migration from `1.0.1`: `swap_nodes`, then three edits in
  the installing manifest — promote `call_id` instead of `voice_session`, drop
  the `set_context` on the answer edge, and add the two edges that drain the
  receipt lanes. The first two are optional for one version; the third is not,
  or every inbound call dead-letters one receipt.
- `docs/README.md` is a list of self-describing links instead of a three-column
  table: every document is named for what it is, with half a sentence where the
  name alone does not carry it, and no row was dropped. The section about the
  documentation's own production process is gone. One sentence now says that the
  published documents are written in English, and the rule that the overview
  wins on conflict stands once instead of twice. The first screen of
  `docs/meclaw-overview.md` reads as English rather than as translated German;
  its comparison table keeps all six rows and
  the headings are unchanged. The pointer to the glossary said fifteen words and
  now says sixteen, which is what the glossary carries
  ([#625](https://github.com/mmeyerlein/meclaw/issues/625)).
- `docs/why/ontology.md` is back, with the subject it lacked when it was cut:
  the typed catalogue a colony is built out of — cell types, templates and the
  contracts they publish — and what the word does not mean here. The URL had
  been answering 404 since 0.32.1 and is linked from outside the repository.
  `docs/why/self-modification.md` and `docs/why/names.md` are three-line
  pointers now instead of pages of their own: the first to `rsi.md`, the second
  to the glossary section that took the role names
  ([#625](https://github.com/mmeyerlein/meclaw/issues/625)).
- `docs/why/rsi.md` and `docs/why/you-talk-it-shows.md` are back as pages, and
  both had been 404 since 0.32.1. `rsi.md` says the two-part thing the page was
  written for — the primitives for rebuilding a running colony are here and
  tested, the loop that would close on them is open, and leaving it open is a
  position rather than a gap in the schedule — and it names the lock that keeps
  the builder off the mutation door. It carries the text that shipped as
  `self-modification.md` in 0.32.1, so that name is now the pointer and `rsi.md`
  the page: the older URL is the one links in the wild use.
  `you-talk-it-shows.md` says what the `voice` cell and a screen are aimed at
  together, with the sidecar path from 0.32.0 that carries an answer's offers to
  an app. Its earlier claim that this repository had no voice was false from
  0.31.0 on and is gone ([#625](https://github.com/mmeyerlein/meclaw/issues/625)).
- The CLI's "no subcommands" rule now says what it always meant: the **colony**
  has no subcommands, and it is still driven by flags alone, nginx-style. `ask`
  is the one client command in the same binary, because it operates no colony but
  addresses one; as a flag, `--api` would have carried both the bind address of
  one's own server and the address of somebody else's. The withdrawal is written
  out in `docs/meclaw-overview.md` § CLI.
- `GET /colony/trace` now orders a shared second by insertion (`ORDER BY
  created_at ASC, rowid ASC`). `created_at` is whole seconds and a lane the
  colony walks in milliseconds carries one timestamp across all its hops, so the
  order inside that second used to be the query plan's to choose — and it
  chooses differently with a `trace_id` filter than without. A reader that takes
  "the first answering hop of this trace" now gets the same hop every time.
- `freeswitch@1.1.0`: **8 kHz is the native path for telephony now** (GH #619).
  A telephone call is 8 kHz, and the template used to have the switch upsample
  it to 16 kHz — twice the bytes on the socket, and a recogniser handed
  interpolated samples carrying no more bandwidth than the original. Deepgram
  Flux serves 8000 natively, and so do Cartesia and ElevenLabs on the synthesis
  side. `params.fork_sample_rate` therefore defaults to `8000` instead of
  `16000`, and it is written into both halves of the `uuid_audio_stream` line —
  the module's rate and the fork URL's `?sample_rate=` — so the two places that
  have to agree are one number. Measured on the same recordings both ways — on
  synthesised recordings, not on a real trunk — the word error rate is identical
  and the first token arrives at the same time, so what the change buys is the
  byte count and one conversion fewer (`workshop/voice-smoke/README.md`
  § *Measured*). An instance that wants the
  old rate sets `fork_sample_rate` back to `16000`.
- `README.md` uses one word for the thing it is made of, `cell`, from the first
  sentence on, which is the word every other document uses. The second paragraph
  is gone: it repeated the first, and the argument it made is one link away in
  `docs/why/everything-is-a-file.md`. A sentence on why the project exists
  stands after the first paragraph. Three rows of the comparison table from the
  overview, Erlang/OTP, LangGraph and Temporal, moved up under a heading of
  their own, because that is the first question a reader asks. A screenshot of
  the colony's browser view sits above the quickstart
  (`docs/assets/colony-dashboard.png`, carried by the export map like any other
  file). The quickstart is four steps instead of five, install, start, grow and
  ask, and reads the answer with `meclaw ask` rather than a `jq` pipeline; the
  browser view stayed as a sentence. The example model in the `.env` line is
  `openai/gpt-5.6-luna`, which is the small model `docs/costs.md` measures,
  where it used to be `openai/gpt-4o-mini`. Below the quickstart, "What just
  happened" says what the four commands did, and "Why it is built this way"
  gives each of the ten ideas one sentence and the link to its page under
  `docs/why/`. The page is laid out like a README a person would write: a
  centred name, one italic line saying what it is, three badges, then prose.
  The documentation table at the end is a list of links that say where they go
  ([#624](https://github.com/mmeyerlein/meclaw/issues/624)).
- The five contract surfaces left `README.md` for `docs/stability.md`, and the
  README keeps two sentences and the link. Nothing is retracted: the surfaces,
  the additive rule on `0.x`, the Breaking rule for `CHANGELOG.md` and the
  absence of a SemVer guarantee under `crates/` are all in the new document,
  which is where "README § Stability" now points
  ([#624](https://github.com/mmeyerlein/meclaw/issues/624)).
- `examples/meclaw-os/README.md` says seventeen cells where it said fourteen, in
  the summary, in the step that reloads the registry and in the closing note.
  Seventeen is what the table in that same file adds up to (`door` 1, `firewall`
  4, `talky` 11, `sink` 1) and what the registry of a colony grown from
  `grow.json` reports
  ([#624](https://github.com/mmeyerlein/meclaw/issues/624)).

### Fixed

- **A hive contract reads an edge by direction.** An `add_edges` entry that ends
  on a hive's own path was always measured against that hive's `accepts` list,
  even when it started at a node inside the hive — where the message LEAVES
  through the rim rather than entering it. The catch-all the `member` template
  ships (`./channels -> .`, stamping `hop.route = 'error'`) is exactly that
  shape, and `error` is a lane the member emits, so the shipped edge was refused
  as a live mutation while the boot instantiated it with a warning at most. The
  check now classifies the edge: `from` outside is an entry and is held against
  `accepts` as before, `from` strictly inside is an exit and is held against
  `emits`, and the refusal says which — and an exit stamping a lane the hive
  only accepts is now refused where it used to commit. The boot judges both
  halves and still warns rather than refuses. No new `error_code` — this is
  `hive_contract` answering the question it always meant to answer.
  ([#602](https://github.com/mmeyerlein/meclaw/issues/602))
- `timer`: when two schedules of one cell come due at the same second, both now
  fire, once each, in schedule order. The I/O loop used to pick a single winner
  per instant and re-plan the others strictly after that second, so a schedule
  sharing its second with another one skipped a whole period, silently and
  without a log line. Measured on a live colony with `*/20 * * * * *` next to
  `0 */15 * * * *`: the quarter-hour schedule never fired
  ([#613](https://github.com/mmeyerlein/meclaw/issues/613)).
- `timer`: a cron firing lands on its second and stays there. The I/O loop
  recomputed the next occurrence from the instant it woke — the firing time plus
  the wake latency — and the cron parser carries the sub-second part of the
  instant it is asked about into its answer, so that fraction became the anchor
  for the next tick and grew with every one of them. The search is now anchored
  on the whole second; one-shots, which carry an absolute instant, are untouched
  ([#626](https://github.com/mmeyerlein/meclaw/issues/626)).
- A message addressed at a cell behind a hive boundary is no longer delivered
  silently. It reaches the address the hive declared, or it is refused with a
  receipt that names the hive and the address — never a third, quiet thing
  (GH #612). The check is a pre-check at the call site in front of the routing
  corridor, like the `cell_inactive` one, so a refused message spends no TTL and
  the frozen corridor is untouched.
- `add_edges` endpoint validation resolves a bare short name the way the apply
  resolves it — against the mutation's own scope. A name that existed only as a
  hive in a FOREIGN scope used to pass the check and then commit an edge onto an
  address nothing occupies; validation and apply had named two different nodes
  and nothing said so. It is now refused `edge_schema`, and the refusal names the
  scope the name was resolved against. A cross-scope reference keeps the spelling
  it always had, a relative path (GH #612).

### Note

- A boundary refusal happens before the routing corridor, so it writes no
  `message_log` row: it lives in the dead-letter queue and never in
  `/colony/trace`. The entry carries the `trace_id` and `message_id` of the
  posted message, so a caller polling `/colony/dead_letters` can still attribute
  it (GH #612).


## [0.32.1] — 2026-09-08

A patch release without a code change. The README and the public documentation
were rewritten so that a person can read them: shorter sentences, one thought per
sentence, no slogans, every number with a source in the tree. Nothing in the
contract moved, and no migration is needed from 0.32.0.

### Changed

- `README.md` went from 234 to 91 lines. The first paragraph and the five
  quickstart commands are unchanged. The nine "why" sections left the README;
  they live in `docs/why/`. The stability section keeps all five contract
  surfaces, the additive rule on `0.x`, and the Breaking rule for this file.
- `docs/why/` has six essays instead of nine. `names` became the section "Names
  of the shipped roles" in the glossary. `ontology` had no subject of its own;
  the mechanism it described (`add_templates`) is in `everything-is-a-file` and
  `rewiring`. `you-talk-it-shows` claimed there was no voice in this repository,
  which has been false since 0.31.0; its one durable paragraph moved into
  `an-os-for-agents`. `rsi` is now `self-modification`, same thesis, no slogan.
- `docs/meclaw-overview.md` lost about a quarter of its words. Every rule,
  every error code, every table and every code block is still there; the 54
  spec-claim anchors and the heading structure are unchanged. What fell was
  repetition between sections, defect narratives from past audits, and
  justification prose.
- `docs/cell-types.md`, `docs/config.md`, `docs/rewiring.md`,
  `docs/voice-wire-protocol.md`, `docs/store-backed-tool-loop.md`,
  `docs/costs.md` and `docs/glossary.md` were rewritten paragraph by paragraph
  with their facts, tables and examples intact. The German editions of the paired
  documents follow the English ones in the same commit.
- `templates/README.md` describes each of the 40 templates in a few sentences.
  The version history that used to sit in the catalogue lives in each template's
  own README.
- The private assistant name that served as an example in `docs/cell-types.md`,
  `docs/rewiring.md`, `templates/voice/README.md` and the `talky` and `voice`
  descriptions is now the neutral example name Sam.
- Corrected numbers on the public surface: 16 built-in cell types plus `hive`
  (the text said 15), 40 templates (the text said 38), 15 cells in
  `memory-hive` (the text said thirteen), and three cell types that receive a
  default sandbox profile (the text said four; `mcp` reads the same profile
  but must declare it).
- `CONTRIBUTING.md`, `ROADMAP.md`, `examples/README.md` and `docs/README.md`
  were rewritten for tone only; every issue and register anchor in the roadmap
  is unchanged.

### Migration

None. No API route, no `template.json` or `config.json` key, no port, no
`error_code` and no `web` route changed.

## [0.32.0] — 2026-09-07

A minor release, and what it adds is a **typed offer**. Until now the fenced
block a front model appended to its answer meant exactly one thing, remember
this, and anything else it might want to say had to be a tool call, one more
round through the brain, measured at 44 % adoption at best against 12/12 for a
fence at the end of the answer. The fence now opens with ```` ```sidecar ````,
holds ONE JSON object, and every top-level key is a **section**: the splitter
inside the generation cuts it up and emits one message per section. `memory` is
the first section and carries byte for byte what the old `extraction` lane
carried; every other section is an OFFER, sorted by the member into the apps
that asked for it, on an edge the installing mutation draws rather than the
template. The screen that shows the result grew a second column and an order a
rewrite cannot move, and the app rim, the fan-out edges a member draws to what
listens, is what turns a spoken turn into something on a screen.

Around that, speech grew a channel of its own. `freeswitch@1.0.0` puts a
telephone in front of the `voice` cell: one call is one session, the signalling
is a book with a row per call, and the audio still never becomes a message. The
`voice` cell learned to say when a sentence is over (`speak_end`), to speak what
a reader would read rather than what a writer wrote (`speak_plain`), to be told
which names to expect, to leave in 20 ms frames, and to synthesise through
ElevenLabs as a third provider. A hive may now insist on a lane at birth
(`required`), and two repairs close the release: a registered template keeps its
placeholders instead of materialising them on disk, and an inbound call raises
one turn instead of two.

This section is also where **0.31.0** reaches the public for the first time. It
was cut on 2026-09-05, never exported and never tagged; its section stands
unchanged below and there is no `v0.31.0` tag, `v0.32.0` is the one tag that
carries both.

### Added

#### `member@1.7.0`, `assistant@2.6.0`: the block after the answer carries sections, and the member sorts them ([#607](https://github.com/mmeyerlein/meclaw/issues/607))

Until now a front model appended exactly one fenced block to its answer and that
block meant exactly one thing: remember this. A screen hint, an app that wants to
be written to, anything else at all had to be a tool call — one extra round
through the brain, measured at 44 % adoption at best, against 12/12 for a fence
at the end of the answer.

So the fence grew a dimension. It opens with ```` ```sidecar ````, holds ONE JSON
object, and each top-level key is a **section**; the splitter inside the
generation cuts it up and emits one message per section on route `sidecar` with
`hop.section` naming the key. `memory` is the first section and it is byte for
byte the annotation the `extraction` lane has always carried, one level deeper
inside the fence.

**`assistant@2.6.0`** emits the new lane and does nothing else with it: one edge
`./talky -> .`, no section read anywhere. It cannot read one — a section is an
OFFER, and an offer may be made by an app of the person standing outside the
generation entirely.

**`member@1.7.0`** is what sorts it, and the split falls where knowledge does.
`hop.section == 'memory'` goes up into the memory hive on the same `in_remember`
door with the same three promoted keys `extraction` uses; everything else goes
into `./apps` on ONE section-blind edge, and the mutation that installs an app
draws `./apps -> ./apps/<app>` on the section that app offered. The rim cannot
know the sections — a section is named by whoever offered it and an app is
installed long after the template was written. The lane is declared with
`at: ["./apps"]`, so no rim lane moved: a parent wired at `1.6.3` is still wired
correctly.

That edge into `./apps` is the level's rather than the installing mutation's,
which makes it the second exception to *whoever listens orders it* — and it rests
on the same measurement as the two restamp edges beside it: a section is only
ever written because it was OFFERED, so on a member with no app the model is
never asked for anything but `memory` and nothing arrives.

**`extraction` is gone from both levels**, because `talky@5.1.0` renamed the
port and no generation can raise the lane any more — an edge for it would be an
edge nothing travels. **What stayed two-phased is the SHAPE, not the lane:**
`memory-hive`'s `extract-glue` reads BOTH — the section out of `body.payload`,
and the block-in-a-turn it read before — and decides per message rather than per
version, so a replayed message, an operator probe and a model that has not been
re-instructed all still write. A section addressed to `memory` that is not one is
refused write-free, in its own words, so an operator can tell a mis-drawn edge
from a model that wrote nonsense.

`grow_level` renders the exit under its new name for a grown generation
(`examples/organism/grow-assistant.json`, twenty-three edges).
#### `voice@1.3.0`: a lane that says when a sentence is over (`speak_end`)

A browser never needs to know when the assistant has finished speaking — it can
hear it. A telephone does, twice over, and neither answer was reachable from the
topology: the cell told its own client on the socket and told nobody else.

So there is a fourth lane. `speak_end` is one source emission per `in_speak` the
cell accepted, when that synthesis is over, carrying `session_id`, `speak_id`
and `reason` (`done`, `cancelled`, `failed`) and no words at all — whoever waits
for the sentence to finish already had the sentence. It leaves whether or not
the connection is still held, because a synthesis that ended *because* the
client went away is exactly the case a waiter must not hang on; the cell's own
invariant of exactly one end per accepted speak is what makes waiting for it
safe.

The lane ships **off** (`emit_speak_end`, default `false`), the rule `partial`
already runs under (R-V8'): the emission goes to the cell's own path and the
out-edges decide, so a lane nobody drew an edge for would dead-letter once per
sentence. Whoever listens orders it, in the same breath as the edge that drains
it. `emit_speak_end` is on the runtime params surface; the client's own
`speak_end` frame is a different path and is untouched. Nothing about the wire
protocol changed. Documented in `docs/cell-types.md` § `voice` and
`templates/voice/README.md`.

**The promise the lane rests on is now kept in every arm.** "Exactly one
`SpeakEnded` per `Speak` the handler issued" was the connection task's stated
invariant and it had two holes, both reachable when the client goes away
mid-sentence: a `Speak` whose very first frame could not be written returned
without reporting anything (the id had already left the command channel, so the
post-loop block found nothing), and a synthesis whose last frame could not be
written broke out after `speaking` was already empty. Both now emit the verdict
on the lane before they leave — no `speak_end` frame, since there is nobody to
read it. Nothing about this is new API; it is the difference between a waiting
telephony hive that hangs up and one that waits for ever.

The caller it was built for is `freeswitch@1.0.0`, below.

The template's binding manifest also grows the `voice_session` pair the
telephone found (GH #603 § 3), because the defect is not telephony's: the
member's `session-keeper` mints and stamps `context.session_id` for its own
bookkeeping, and this cell selects a connection by that same key — so on any
member that holds a keeper, the spoken answer came back naming a session no
connection held. The ingress edge promotes the connection's id into
`voice_session` as well and the answer edge puts it back. It is written down as
a workaround: which of the two owns `context.session_id` is a ruling nobody has
made.

#### `display@1.1.0`: a second column, and an order a rewrite cannot move (GH #609)

Found while building an ambient application -- a clock, a weather tile and a
countdown, standing on a screen beside a conversation. `display@1.0.2` knew
exactly one region and sorted the views in it newest-first, so a widget
rewritten every twenty seconds took the top slot on **every tick**: not because
it was important, but because it was recent. That is the right answer for a card
and the wrong one for anything standing, and the two readings cannot share a
screen.

**A screen now has two columns.** `main` is the wide one and the **default**, so
every view written before this version lands exactly where it landed before.
`aside` is the narrow one beside it, at `clamp(15rem, 22%, 24rem)`, and it takes
no width at all while it is empty -- a screen that has never heard of a region
looks the same as it did. Under 60rem the two stack. The rule travels in the
`display-shell` template as one `<style>` block rather than as a line in
`/vision.css`: the token sheet belongs to the `web` template and describes a
design language, while *main is wide and aside is narrow* is a statement about
this screen. Anything that is not one of the two is still `invalid_view` on
`receipt`, with nothing written.

**The order inside a column is three keys, and the interesting one is the key
that is gone.** First the `ord` the view declared -- optional, `0` by default,
signed, a band rather than a slot, so a widget asks for `-10` instead of asking
every other sender to move down. Then **first appearance**: a new view sorts
behind everything already standing and keeps the seat it is given until
something above it goes away. Then `(owner, view_id)`. **The moment a view was
last written is not among them**; `updated_at` is the `ttl_ms` clock and nothing
else now.

First appearance is remembered **by the screen**, not by a column of the table:
the seat of a view is the `ord` the display is already holding it at, which the
compose cell reads back on pass 3 anyway -- so what the display holds is an
input to the layout rather than only something to diff against. Two consequences
worth knowing: a page that has to be bootstrapped has no seats, and every view
on it is new together; and a view that changes region is new in the region it
arrives in.

**The trap between the two changes** was that both regions used to hang under
the page root at `ord: 0`, and the display documented the root as taking exactly
one child. That constraint had already been lifted, in the `web` cell, by
[#394](https://github.com/mmeyerlein/meclaw/issues/394) -- a materialised page
carries n+1 statics for n slots, and a one-child root is "a composition CHOICE
now rather than a constraint" -- and nobody had come back to the display to say
so. Both regions are direct children of the root now, at `ord` `0` and `10`, in
declaration order.

The `views` table gains one column, `ord`, and `region` gains a second legal
value. **Migration: none.** A view that names neither is the view it was, and a
`store` adds a column its declaration gained with `ALTER TABLE ADD COLUMN` on
the next spawn -- a screen that was already up keeps its rows.

#### `builder@1.7.4`, `meclaw-os@1.8.5`: the pin nachzug of the two-column screen

`builder`'s `member_screen_template` is what a member's screen is instantiated
from, and it named `display@1.0.2`. It names `display@1.1.0`; `meclaw-os` refs
the builder by version and follows. No recipe, no lane and no parameter moved
around either of them.

#### `voice`: ElevenLabs as a third text-to-speech provider (GH #591)

`params.tts.provider` takes `"elevenlabs"`. The claim the `voice` cell shipped
with — *a third adapter is one new file and one match arm* — was worth exactly
what a claim is worth until somebody tried it; this is the trying, and it cost
one file (`providers/elevenlabs.rs`), one arm in the factory, one params struct
and a scripted fake. The cell, the wire protocol and the turn machine are
untouched, and so is the template: `provider` was always a value, never a shape,
so an instance switches with one `override_params` block and no version bump
anywhere.

The adapter speaks the vendor's documented streaming WebSocket
(`/v1/text-to-speech/{voice_id}/stream-input`), one connection per synthesis,
the whole turn as one message followed by the end-of-stream marker, `pcm_<rate>`
audio decoded from Base64 in arrival order. The credential travels in the
`xi-api-key` header — never in the URL and never in a message body, the two
places the documentation also offers and the two places a transport error or a
wire log would carry it away.

Two properties of that protocol are visible on the params surface, because
hiding them would only move the failure later. **The voice id is a path segment
of the endpoint**, not a request field, so an unresolved `${…}` there is not a
wrong voice but a wrong URL — it is refused by name at parse time, as is any
value carrying `/`, `?`, `#`, `&`, `%` or whitespace. **And `sample_rate`
accepts only the rates the vendor serves** (`8000`, `16000`, `22050`, `24000`,
`32000`, `44100`, `48000`), because the cell never resamples: an unserved rate
is a cell that announces one format in `hello` and then never speaks.

**There is no cancel message in this protocol.** Cartesia has one; this endpoint
documents three client messages and none of them retracts audio. So a barge-in
closes the socket, which is the whole vocabulary available — and the adapter is
held to the same eleven cases as the Cartesia one, including that a dropped
cancel *sender* is not a cancel.

#### The conversation guide grades a multi-section sidecar block (GH #608)

`workshop/evals/conversation-guide/run_guide.py` gains the arm
`--annotation sidecar`: ONE ```` ```sidecar ```` block per turn holding one JSON
object with a named section per offer, which is the delivery
[#604](https://github.com/mmeyerlein/meclaw/issues/604) decided on. The `memory`
section is graded by exactly the rules the shipped single-section arm is graded
by — one `annotation_shape`, so the two arms' adoption figures are comparable —
and the optional `display` section is counted apart, against the closed
vocabulary of the ruling (`kind` one of fact/list/table/text/link/chart, `data`
present, `mode` graded only when written) and against a per-turn expectation the
guide records. The block it seeds is
`contracts/sidecar-sections-2026-09.txt`, composed the way
`templates/collector/assemble` will compose it (preamble, required section
first, then alphabetical) with the shipped rules byte-identical from
`DELTA, NOT STATE` down. `guides/g3-a-day-with-a-screen.json` is its guide: G1's
planting, revision, nothing-turn and four final questions, plus six single-turn
sections that ask to be shown something.

The block it seeds is **composed, not typed**: since GH #606 the file is the
output of `templates/collector/assemble`'s own `sidecar_block()`, run over the
two shipped offers and read back through the same `contract_block()` the run
uses, so the harness measures the block a colony would actually carry. A
`display` section is graded against that offer, which requires `kind`, `title`
and `data`. A guide may say `display: "either"` for a turn whose answer shape
the model decides: such a turn is counted and printed and scores neither way,
because grading a card there would measure the guide's guess about an answer it
had not seen.

**Both block arms are now counted at the SOURCE**, on the brain's own
completions out of the central message log, and the report says so in
`adoption.source`. The reason is a property of the shipped tree rather than of
either arm: `talky/splitter` cuts the block out of the answer before the capture
sees it, so a count taken at the delivery reports `absent` on every turn the
model got right. The delivery-side counters stay in the report beside the source
ones (`annotation_blocks` next to `annotation_blocks_at_source`), because
`absent` there and `valid` here is the sentence "the block left the answer".

#### `voice@1.2.0`: what an assistant wrote is not what a provider reads (`speak_plain`)

The first live call this cell ever carried ended with the synthesis provider
saying the punctuation out loud. That is not a provider bug: an assistant writes
for a screen without being asked to — `**emphasis**`, `# headings`, `- lists`,
`[links](https://example.com)`, code fences, table pipes — and a text-to-speech provider reads
what it is handed, character by character.

So the handler now turns a written answer into SPEECH text before it enters the
session's queue, which is the only place it can happen: the queue holds the text
that will be synthesised, so rewriting after an answer was queued would let a
later `params` flip reach answers that were already accepted. The markup goes
and the words stay — a link keeps its text and an image its alt text, a code
fence loses its fence and keeps its code, a heading loses its hashes, a
blockquote its `>`, a list item its marker and its `[ ]` box, an escape loses
its backslash and keeps the character behind it, a table row becomes its cells
joined by commas and a separator row disappears, and a line break becomes a
sentence end (a full stop unless the line already ends in `.`, `!`, `?`, `:`,
`;` or `,`). Nothing else is touched: umlauts, punctuation and digits travel as
they are, and HTML entities and tags are somebody else's escape, left alone on
purpose.

**Two shapes are kept on purpose, because somebody dictated them.** A `*` or `_`
with whitespace on both sides is an arithmetic operator, not an emphasis, and
`3 * 4` stays what it was. And an ordinal at the start of a line loses only its
punctuation, never its digit: `5. September 2026` becomes `5 September 2026`,
because a date and a list item are indistinguishable there — an agent that reads
a date back without its day has lost something, while a list read as "1 Erstens"
has lost nothing but a little grace.

**An answer with no words left in it is not spoken and not refused.** A
horizontal rule, an empty emphasis, an assistant turn that arrived empty:
nothing is queued, the cell logs it at debug level, and the session is untouched.
A refusal would have an agent retry a turn that was fine, and a synthesis of
nothing is a `speak_start`/`speak_end` pair around silence that costs a provider
call. An empty assistant turn used to be synthesised; it now takes the same path.

Three properties are pinned by their own tests: prose with no markup in it comes
out byte for byte as it went in, the rewriting is IDEMPOTENT — checked as a
property over every example the module's tests use, because the inline rules can
uncover a structural marker the structural rules already walked past
(`` `# install` ``, `[- Punkt](https://example.com)`, `**| a | b |**`), so a line is rewritten to
a fixpoint rather than once —, and a page of unclosed brackets is read in one
linear walk rather than a quadratic one.

It is a hand-written rule set (`crates/meclaw-cells/src/voice/speech_text.rs`),
not a markdown parser: a parser is the honest tool for RENDERING markdown, and
this is a short list of shapes a microphone should not hear, one screen long and
one unit test per rule.

The new param `speak_plain` (default `true`) turns it off, and `false` hands the
provider the answer exactly as it arrived — the behaviour this template had
before. It is on the runtime update surface and is in force from the next answer
on, because the handler is the half that rewrites; only its DECLARATION in
`hello` and `GET /info` — a new field on both, additive, at the end — follows on
the next respawn, since that is the I/O half's. Documented in
`docs/cell-types.md` § `voice` and `docs/voice-wire-protocol.md`.

The same template version also learned that **`release` is not where a turn
ends** (`release_grace_ms`). Push-to-talk on the built-in test page lost the last
real line of every take, and the take before it turned up at the front of the
next one. Both are the same event: a recognition provider reports the end of a
turn some hundreds of milliseconds after the audio carrying its last words was
sent — Deepgram Flux takes 400–700 ms — and `release` cut on the frame, so those
words were dropped, and the late end-of-turn then landed inside whichever
boundary happened to be open when it arrived.

So `release` now says what it means: no NEW audio belongs to this turn. The
boundary stays open and **drains** — `partial` frames keep arriving and still
belong to the turn that is closing — and exactly one `turn` leaves at whichever
comes first, the provider's own end-of-turn or the new cap `release_grace_ms`
(default `1500`, `0..=10000`), which cuts with the last interim. Events after
the cut belong to no boundary, so the next `hold` starts empty. A `hold` pressed
mid-drain closes the old take at once with what it has; a `mode` frame does the
same rather than being refused, since a boundary that has been released cannot
be released again and a refusal would be a dead end until the grace ran out.
`0` restores the old behaviour as a value rather than as history.

Whenever a boundary is closed by something other than the provider — the key
again, the cap, a mode switch — the SESSION remembers a provider end it is still
owed, because the provider is still inside the turn the old audio started and
its next end-of-turn carries the take that has just closed. That one event pays
the debt and is thrown away, whether it lands inside the next boundary or
outside every boundary: whether the client presses again before or after the
provider answers is a race, and a debt only one of those orders could collect
would be a leak in the other. It is written off when the recognition session
dies, because a provider that is gone owes nothing and a debt carried over a
reconnect would eat the first real end-of-turn of the next take — the very loss
the drain exists against.

The cap needs a clock the handler does not have, so it is one more round trip
over the seam this cell already has (`ArmReleaseGrace` out,
`ReleaseGraceExpired` back) and a generation counter that makes a timer for a
turn that already closed a no-op instead of a second, empty turn. The
generations are minted per CELL rather than per session, because a session
identity outlives a connection: a caller who redials — or whose second
connection displaces the first with close `4409` — would otherwise get a new
session whose first generations are the numbers the old one's timer is still
asleep on. The param is on the runtime update surface and is read at the next
`release`, never moving a deadline a turn is already waiting on, and it is
declared in `hello` and by `GET /info` — a new field on both, additive, at the
end — so a `hold` client reads the upper bound on its `turn` instead of
guessing it.

And the test page builds its socket address **relative to itself** (`new URL`
against the page's own location, with the path normalised to a directory), so a
reverse proxy that serves the cell under a path prefix no longer sends the
page's WebSocket to `/ws` at the root.

#### `voice@1.2.0`: the recogniser can be told which names to expect (`stt.deepgram.keyterms`)

Flux heard "Ivan" where a live call said "Egon". That is what a general model
does with a name it was never trained on — it returns the nearest name it knows —
and no threshold fixes it, because the recognition was confident and wrong.

`stt.deepgram` therefore takes `keyterms`, a list of words the recogniser should
expect: `"keyterms": ["Egon", "meclaw"]`. Each entry goes out as its own repeated
`keyterm` query parameter, which is the shape the service reads a list in, so an
entry made of several words stays ONE boosted term instead of splitting into two
— the space is percent-encoded like every other query value this adapter writes.
The case is the caller's: a proper noun capitalised, everything else lowercase,
exactly as Deepgram documents it. Blank entries are dropped rather than sent as
an empty parameter, and the list is empty by default — a colony that names no
keyterms sends byte for byte the query it sent before, which has its own test.

It sits inside the `stt` block, so like the rest of that block it is set at
instantiation and never on the runtime params surface. Documented in
`docs/cell-types.md` § `voice` and `templates/voice/README.md`.

#### `voice@1.1.0`: outbound audio leaves in 20 ms frames (`audio_out_frame_ms`)

A synthesis provider picks its own chunk size and Cartesia's are large, and the
cell used to hand each chunk to the client exactly as it came. That is fine for a
browser and fatal for a telephone: FreeSWITCH's `mod_audio_stream` 1.0.3 aborts
the whole call — `SIGABRT`, `free(): corrupted unsorted chunks`, inside its
closed-source playback half — as soon as one outbound binary frame carries more
than about 100 ms of audio. Measured on the real socket: 960 B (20 ms), 4410 B
and 4800 B (100 ms) play; 9600 B, 14400 B and 19200 B kill the call.

So the frame size is the CELL's decision now, not the provider's. The new param
`audio_out_frame_ms` (default `20`, `0` = passthrough, `0..=1000`) cuts every
synthesis chunk into frames of at most that length before they leave — 960 bytes
at 24 kHz PCM16 mono. It is an upper bound rather than a fixed size: a frame is
either exactly that long or the remainder of a provider chunk, never longer, and
an even remainder leaves at once rather than waiting for the next chunk. The cut
never runs through a sample: a part-sample tail is
held and travels with the next chunk, and whatever is still held when the
synthesis ends leaves as one short last frame, so the bytes and their order are
exactly what the provider produced. Nothing is paced and nothing sleeps — a
burst of small frames is what that module expects, and a gap is what it cannot
take. A cancel discards the held tail with the rest of the synthesis, and the
echo provider is never framed at all.

The value is declared, so a client reads it instead of measuring it: `hello` and
`GET /info` both carry `audio_out_frame_ms`, and it is `0` where nothing is ever
framed (no text-to-speech provider, or the echo loopback). It is on the runtime
params surface, with the same reservation both timeouts carry: an update is
persisted at once, but the I/O half frames at the value its life was built with,
so a moved one reaches the wire on the next respawn. Documented in
`docs/cell-types.md` § `voice` and `docs/voice-wire-protocol.md`.

#### `freeswitch@1.0.0`: a telephone as a channel of a person

A new template, and a channel rather than an app: a call that comes in, a call
that is answered and a call that ends are things that **happen**, so each of them
becomes a TURN of the member's conversation. FreeSWITCH stays the media edge in
gateway mode — it does the SIP and `mod_audio_stream` connects to the channel as
a WebSocket client — and the hive holds both halves of one call: the MEDIA half is
a `voice` cell, unchanged, and the SIGNALLING half is one `code` cell that offers
the tools, one that keeps the book, a `web_fetch` cell that talks to the switch
over `mod_xml_rpc`, and a `store` that holds the calls. The two halves live
together because **one id binds them**: FreeSWITCH's channel UUID is the
`?session=` of the audio stream and therefore the `session_id` an answer is
spoken back into.

The channel offers the assistant two tools of its own, `call(number, purpose)` and
`hangup()`. `call` answers immediately, with the session the conversation will run
under — the OUTCOME arrives as a turn (`answered`, `busy`, `no_answer`, `failed`),
because a telephone call is not a value a function returns, and a tool that waited
for one would hold a round open across a ringing telephone. There is no clock in
the template: the ring timeout travels to the switch as its own
`originate_timeout`, and the answer comes back over the same connection.

`params.callers` maps a number to a sender id, which is what turns a caller into
somebody the member knows; a number with no entry is refused with
`unknown_caller` rather than becoming a turn nobody can attribute.

`member@1.6.3` carries the wiring half of it, additively: `tool`, `schemas`,
`tool_result` and `tool_schemas` may now dock at `./channels` as well as at
`./apps`, and two new restamp edges `./channels -> ./assistants` turn a channel's
`tool_result` into `in_tool` and its `tool_schemas` into `in_menu`. It is the app
rim's mechanism at a second rim (ADR-0024), not a second mechanism.

**Three findings came off the first real call and a fourth off the review of
the fix** (GH #603), and they are in this template from the start rather than in a patch release, because
`phone@1.0.0` never left this tree:

- **the stream starts on `api_on_answer`, with `uuid_audio_stream` at `16000`.**
  Starting the stream is an API command, and `execute_on_answer` runs an
  *application*: FreeSWITCH answered `Invalid Application` and hung the freshly
  answered call up. `fork_sample_rate` is written as the number `16000` —
  both spellings reach the module (`mod_audio_stream.c` v1.0.3 lines 170-177
  read `16k`/`8k` by `strcmp` and everything else through `atoi`), and a digit
  string is the form that cannot be mistaken for a unit inside a one-line
  switch command. The whole variable is left out when `answer_app` hands the
  leg to a dialplan extension (`&transfer(...)`), which starts its own.
- **every promotion in the installing manifest is `has()`-guarded.** One was
  not, and a CEL modifier that fails to evaluate skips the whole edge — so the
  channel's tool offer never reached the assistant's menu, silently.
- **the call travels as `context.voice_session` beside `context.session_id`.**
  The member's session keeper owns the second key and rewrites it for its own
  bookkeeping, so the spoken answer came back naming a session no connection
  held. The ingress edge promotes the call's id into `voice_session`, the answer
  edge puts it back on the way into the channel. It is a workaround written down
  as one: which of the two owns `context.session_id` is a ruling nobody has made.
- **a `hangup` in the middle of a sentence waits for the sentence**, and a
  sentence cut short reaches the switch. Both ride the `voice` cell's new
  `speak_end` lane over an edge inside this hive. A `hangup` that finds a
  synthesis running books the intent and answers the model that the line will be
  cut when it has finished; `speaking` is a COUNT, because the media half queues
  what it is given and a flag would let the first `speak_end` cut a second
  sentence off. Both sides of that exchange read the row back after they write
  it, because the hang-up and the `speak_end` it waits for can interleave
  either way round and one read-back would close one order and leave the other
  waiting for ever. The `speak_end` that takes the count to zero fires the
  `uuid_kill`. A `speak_end` with `reason: cancelled` or `failed` fires
  **`uuid_break <uuid> all`** — FreeSWITCH's own command, because
  `mod_audio_stream` dispatches only start/stop/pause/resume/send_text
  (`mod_audio_stream.c` v1.0.3 lines 148-186) and offers no clear, and the half
  sentence the caller is still hearing sits at the switch. That last one is
  reasoned rather than read — the module's playback half is closed source — and
  the README marks it for verification at the switch. There is deliberately
  **no clock** on the waiting hang-up; the reason, and what it would take, are
  in `templates/freeswitch/README.md` § *What is not here*.

#### `member@1.6.2`, `assistant@2.5.1`: the apps rim (rulings 2026-09-04/05)

A member could already hold an app — the `./apps` container has been there since
a member grew a screen — but an app could only ever be **written to**. It had no
way to hear the conversation it was drawing about, and no way to offer the
assistant anything. Both are now ordinary wiring, and an app stays what it was:
a sub-form of the member, a sealed hive with no port, no secret and no channel of
its own. There are exactly three ways to plug in, all of them **at the rim** —
observe, offer, write — and none of them is an interception. Every edge an app
gets is an additional one, so the paths that existed without it fire exactly as
they did before.

The split of ownership is the ruling that shapes the whole thing: **whoever
listens orders it.** `member@1.6.2` DECLARES the observer lanes on its container
— `turn` and `partial` as `emits`, `tool_result` and `tool_schemas` as `accepts`,
all four with `at: ["./apps"]` — and ships only the two restamping edges
`./apps -> ./assistants` that turn an app's answer into `in_tool` and `in_menu`,
which cannot fire while no app is installed. The edges that carry the observation
(`./firewall -> ./apps` on the screened turn, `./assistants -> ./apps` on the
answer, `./channels -> ./apps` on the interim transcript, and the container
binding) are drawn by the **mutation that installs the app**, exactly like the
`emit_partials` switch on the voice channel that feeds them. A member with no
listening app therefore carries no such edge and dead-letters nothing, and the
member's guarded default exit — the one that answers a turn nobody's channel
raised — keeps working, because the observer edges are guarded on the channel and
never fire beside it. A second installation redraws the same member edges and the
commit is idempotent: an edge is the same edge when `from`, `to`, `condition`,
`modifier` and `default` are. `answer` deliberately keeps its rim entry **without**
`at`: it is a rim lane, and an `at` would switch off the exit check that guards it.

Offering runs on v-lanes in both directions of the question and on the ordinary
road for the answer. `assistant@2.5.1` declares `tool` and `schemas` with
`at: ["./talky", "./cogny"]`, so a v-lane may leave the brain's rim and land on
the connect point the app pronounces for itself (`at: ["./show"]`); it carries the
assistant exit's stamps itself, because it bypasses that exit. The result comes
**back** the way the memory's already does — the app emits at its own rim, the
binding edge stamps `context.tool_answerer`, and the member restamps into
`in_tool`/`in_menu` — so nothing on the assistant's rim moves and every growth
recipe keeps the door it draws. The menu needs no new operation either: an app
answers its **whole** offer whatever list was asked for, with an empty `unknown`,
and the collector merges the rows of all answerers as it already did. And an app
may watch tool results go by, as a v-lane fan-out from the assistant's `./tools`
(for which `assistant@2.5.1` declares `tool_result` with `at: ["./tools"]` — not a
rim lane, `development-rules.md` § 8b) and from the member's `./memory-hive`;
there is deliberately no container edge for the latter, which would have delivered
the observed copy into the assistant a second time.

The install manifest, with its placeholders, is in `templates/member/README.md`
§ Installing an app. What is **not** here is the loader: no `app.json` read at
start, no colony-level app listing, no `app install` command, and no builder that
installs by reading `/colony/graph` and drawing only what is missing. Those are
one later errand, and an app today is an ordinary template plus a manifest.
`builder` and `meclaw-os` move by a patch each, as the pin nachzug and nothing
else.

#### `required` lanes: a hive may insist on a lane at birth (`hive_contract`, third shape)

A hive contract could say what a hive accepts and where the lane connects, but not
that it **must** be connected. So a hive whose whole purpose is a lane — an app
that exists to draw what it hears — could be instantiated deaf, and the first
sign of it was silence.

`params.contract.accepts[]` therefore takes `required: true` (absent means
`false`, and every standing template keeps today's behaviour). It means: *whoever
instantiates me must wire this lane to me.* The check runs in the post-state stage
of the mutation, beside the lane doors and the required drains, because it needs
the post-state edge table — and it judges exactly the hives **this diff gives
birth to**, the same list and the same reasoning as the port boundary: the border
judges what the diff DRAWS. A lane with `at` counts as wired when an edge carrying
that lane ends on one of its connect points; a rim lane counts as wired when the
router probe of an inbound edge lands on the hive path, which is the door check
asked from the outside rather than from within. Missing, the mutation is refused
`hive_contract` — the third shape of a code that already exists, no new word —
naming the hive, the lane and its `because`, collecting rather than stopping at
the first find, before the commit and rolled back like its neighbours.

The limits are part of the feature. **Only birth is judged**: a later
`remove_edges` that takes the lane's edge away is not an instantiation and is not
re-judged, and the door check on standing hives stays what it is. Boot does not
even warn, for the same reason it does not warn about the port seal — the birth
topology is authorship. And whether the emitter at the other end actually delivers
the lane, rather than merely being wired to it, is nothing the substrate can know:
a switch like `emit_partials` sitting at its default is a topology decision, and
the rule for it stays *both halves, or neither*.

### Breaking

#### `talky@5.1.0`: the `extraction` port is now `sidecar`, one message per section (GH #605)

The sidecar was one hard-wired thing: a ```` ```memory ```` fence, cut out of an
answer by `talky/splitter` and carried on a port called `extraction` as the raw
block, in the text of a single turn. An application that wants to be written to
inline — a screen, say — had no way in that did not cost a second brain round.

The block is generic now. It opens with ```` ```sidecar ````, carries ONE JSON
object, and **one top-level key per section**. The splitter reads the object and
emits one message per key on route `sidecar`, with `hop.section` naming the
section and the body carrying `{"messages": [], "section": "<key>", "payload":
<the section object>}`. It looks no section up and routes none anywhere: the
edges downstream distribute on `hop.section`, so a section this tree has never
heard of travels without a line of code changing in the cell.

**Migration.** A parent wired on `hop.route == 'extraction'` writes nothing. The
memory annotation is now the section `memory`:

```json
{"from": "./talky", "to": "/front/memory",
 "condition": "has(hop.route) && hop.route == 'sidecar' && has(hop.section) && hop.section == 'memory'",
 "modifier": {"set_hop": {"route": "'in_remember'"}}}
```

**The legacy fence still reads.** A ```` ```memory ```` block — and the
tolerances beside it, a bare ```` ```json ```` fence or a naked trailing object
carrying the payload — becomes the section `memory` with the whole block as its
payload, so a colony whose models have not been re-instructed keeps writing.

**What did not change.** A block that cannot be read, or whose top level is not
an object, is still cut out of the answer and dropped with `hop.sidecar =
"malformed"` and nothing on the lane (GH #534): found decides the cut, readable
decides the lane, and nothing is ever repaired. A round carrying tool calls is
still never taken apart (GH #378). New: a top-level key whose value is not an
object has no body to travel in and is dropped by name, listed in
`hop.sidecar_dropped` on the answer half.

**The levels above follow in the same wave.** `assistant` re-points its `talky`
ref, renames its own `extraction` port to `sidecar` and routes it upward
unchanged; `member` takes the memory half on `hop.section == 'memory'`; and the
memory hive's inline ingress reads the section's `payload` out of the body,
keeping the old turn form readable so a colony can be rewired in two steps
rather than one. The shipped `examples/organism` manifests move with them.

Measured by `crates/meclaw-cells/tests/gh379_the_splitter_cuts_the_sidecar.rs`,
`gh534_an_unreadable_block_still_leaves_the_answer.rs` and `talky_composite.rs`.

#### `phone@1.0.0` is now `freeswitch@1.0.0`

The template was named after the medium where it should have been named after
the machine. What is behind it is a FreeSWITCH, and a colony that later wants a
second telephony edge needs the two to be able to stand beside each other under
names that say which is which (ruling 18, 2026-09-06). The **node** is the
machine — `<member>/channels/freeswitch`, `context.channel_node = 'freeswitch'`
— while `context.channel` stays `'phone'` and `hop.platform` stays `'phone'`,
because that is a kind of room and not a vendor.

It is listed as breaking although it breaks nothing that shipped: `phone@1.0.0`
was cut locally with 0.31.0 and never exported, so no public tree has it. A
colony that grew the node anyway migrates with `swap_nodes`
(`{"match": {"name": "channels/phone"}, "template": "freeswitch@1.0.0"}`, which
leaves the call table where it is) and then rewrites the five edges of the
installing manifest, which name the node. Full migration:
`templates/freeswitch/README.md` § *The migration from `phone@1.0.0`*.

### Changed

#### `collector@4.1.0`, `memory-hive@3.4.0`, `cogny@5.0.1`: the block contract is offered, not typed (GH #606)

A front model has been asked, since `talky@4.1.0`, to append one fenced block to
its answer — the memory hive's per-turn annotation. The words of that block lived
as a literal in `templates/collector/assemble`, which is a cell that enforces none
of its rules, one composite away from the hive that reads them. It was the
arrangement [#552](https://github.com/mmeyerlein/meclaw/issues/552) had already
retired for `memory_recall`'s schema: **whoever is reached declares themselves.**
And it had a second cost that only shows when something else wants a piece of the
same answer — a screen, say. A contract one cell types can describe exactly one
consumer.

**One block, several sections.** There is one fenced block now, ```` ```sidecar ````,
holding ONE JSON object with one top-level key per section. `memory` is one of the
keys.

**The sections are offered.** A `tool_schemas` answer carries `sidecar[]` beside
`schemas[]` — `{section, required, schema, instruction}` per section — and the
collector merges them over the answerers by section name exactly as it merges the
tool menu by tool name ([#529](https://github.com/mmeyerlein/meclaw/issues/529)):
the same rows of its own `menu` table (one column wider), the same order, the same
first-answerer-wins rule. `templates/memory-hive/schemas` is the first offerer, and
its offer travels on every answer it gives — the section is not a response to a
name.

**The collector owns the frame and nothing else.** It COMPOSES a preamble — a
fixed frame (one block after the answer, ONE JSON object, one top-level key per
section), then the whole-object shape of this particular composition, then the
obligation with the section names in it; 494 characters for the pair that ships.
Under it stands one `## <section> (required|optional)` per offer with the
offering template's own words and a compact example rendered from its schema —
required first, alphabetical inside each half, under the new ceiling
`params.sidecar_max_chars` (default 6,000; optional sections fall from the back
with a warn line, a required one never falls). The knob `inline_extraction` is
called `sidecar`.

**The last two preamble lines are repairs the harness bought.** The first
composition of this contract was measured over 52 turns and produced **7
malformed blocks against 0 in the control arm**, in two patterns, and neither was
about a section — both were about the frame. One model **dropped the outer
braces** and wrote the section objects side by side: every heading shows the
INNER shape and nothing showed the outer one, so the preamble now prints the
whole object with the braces the model has to write. Another model let the
**optional section stand instead of the required one** on the turns where both
applied: *a required section is written on every turn* is true, general, and was
read as a rule about sections in the abstract, so the names make it a rule about
THESE two and the clause about the optional one says the failing case out loud.
Both lines are generated from the offers, never typed.

**It travels with the MENU, not with the turn.** The composed contract lands on
`system.instructions.sidecar` beside `system.tools`, on the `menu` message: the
same durability class, one write per change and nothing per turn, re-derived on
every mutation receipt. The property [#525](https://github.com/mmeyerlein/meclaw/issues/525)
needed is untouched — the slot is derived, never seeded, so a brain that GREW
still receives it — and the per-assembly write is retracted rather than left
running beside it, because two writers on one slot path race every round. Nobody
offering anything writes an EMPTY slot rather than falling silent: durable state
is revoked, never merely abandoned.

**The empty-menu guard is read per half.** An answerer that offers a section
without declaring a tool (a screen is the case this was built for) leaves
`system.tools` untouched — a `$replace` over a menu of nothing but the collector's
self-served names is the revocation that guard exists to refuse. A collector that
does not ask for the block ignores every offer of one, silently: `cogny` has no
splitter, and a warn line there would report a correctly wired tree as a defect.

`templates/memory-hive/inline-contract.md` stays the authority; it describes the
section form now and the sentences naming a fence of its own are retracted. New
receipt key `hop.sidecar_sections` on the `menu` message. `cogny` only re-points
its collector ref. Pinned by
`crates/meclaw-cells/tests/gh299_the_contract_asks_for_both_parts.rs` (the rules,
against the ingress) and
`crates/meclaw-cells/tests/gh525_a_grown_brain_carries_the_extraction_contract.rs`
(the delivery, offer to brain).

#### Withdrawn: "the smallest view needs no app"

Shipped prose said an agent's ordinary `answer`, carried into a screen by the
member's own down-edge, *is* a view — so a person could be shown a paragraph
without an app. On a real colony it is not. A view is a body carrying `view_id`,
`kind` and `content`; a talking agent's answer carries `messages[]` and nothing
else, and `display@1.0.2` refuses exactly that body by name: `invalid_view`, with
the reason `"view_id" must match [a-z0-9-]{1,64}`. The claim read the smallest
KIND of view — prose, which needs no component tree — as the smallest WRITER of
one.

The routing half was true and stays true: the down-edge does carry the answer
into the display's own `in_view`. What arrives there is simply not a view. So the
smallest screen shows nothing of the conversation, which is a legitimate state and
the ordinary one for a member that has only just grown a screen; whoever wants an
agent's prose on a screen installs a producer of view bodies beside the agent — an
app at the rim of the member, or any cell built to emit `view`.

The test that pinned the claim
(`crates/meclaw-cells/tests/gh459_a_screen_is_a_member_channel.rs`) pinned a
double whose `answer` already carried a view's three keys, so it measured a
view-shaped body and called it an answer. It now measures the truth against the
shipped `compose.py`: a plain prose answer is refused `invalid_view` for the
reason above, a body with the three keys becomes a store write, and the answer
that reaches the screen through the member arrives there carrying no `view_id`
and no `kind`. Prose withdrawn in `templates/member/README.md`,
`templates/member/apps/config.json`, `templates/README.md` and
`examples/organism/README.md`
([#597](https://github.com/mmeyerlein/meclaw/issues/597)).

### Fixed

#### A registered template keeps its placeholders ([#611](https://github.com/mmeyerlein/meclaw/issues/611))

`add_templates` files a CLASS in the instance-local library, and until now the
declaration travelled through the diff's full substitution pass on the way in.
So the file BODIES were rewritten before they were written: a `config.json` that
referenced the colony's key as `${SECRET_API_KEY}` landed under
`{templates_root}/local/<name>/` with the key in CLEAR TEXT — found on three
colonies built from the same recipe, five files each — and every later
registration of that derived class copied it again. The mirror image of the same
bug refused registrations outright: a README that merely MENTIONS `${…}` in
prose was read as a placeholder and answered `env_var_missing`, for a variable
the registering colony had no reason to own.

`add_templates[].files` is now exempt from BOTH passes. The bytes of a
declaration reach the library exactly as they stood in the body, and every
`${…}` in them binds where it always binds: the instance class (`${ctx.*}`,
`${uuid7:*}`) at instantiation, the environment class (`${VAR}`) at every read
(GH #20). An instance grown from such a class still gets the environment's
value, and no file on disk carries it. The entry's own fields (`name`,
`version`) substitute as before — `files` is the one slot that is somebody
else's bytes. This is what `docs/config.md` § Zugriff has claimed since GH #440;
the code now does it.

Library entries that already carry a materialised value are **not** rewritten —
the rule applies forward, from the next registration on, and a class registered
before this fix has to be registered again (under a new name) or edited by hand.
Measured by
`crates/meclaw-colony/tests/gh611_a_registered_template_keeps_its_placeholders.rs`.

#### `freeswitch@1.0.1`: one turn per call, a silent ending, and a refused caller gets the line back ([#614](https://github.com/mmeyerlein/meclaw/issues/614))

The first real inbound call through the channel, measured on 2026-09-06 at
16:56. Three findings, all in the signalling half, and all of them the same
mistake: *a model answers everything it is handed*
(ADR-0025, `plans/adr/0025-a-receipt-is-feedback-to-a-writer.md`).

**The caller heard two greetings.** The dialplan answers an inbound leg at once,
so `call_incoming` and `call_answered` reached the hive inside the same second
and each raised a turn. The assistant answered both. An inbound call is announced
by `call_incoming` — the moment a person is on the line — and the `call_answered`
behind it now books the state and says nothing. A call this channel *placed* is
unchanged: `call_answered` is its turn, carrying the number and the purpose.

**The assistant said goodbye into a dead line, twice, and hung up a call that
was already gone.** `call_ended` raised a third turn, and the sentence that
argued for it — *an agent that goes on talking into a call that ended is the
failure this lane exists to prevent* — is exactly the failure it caused: a
generation reads a turn by **answering** it, the answer went into a line that no
longer existed (`in_speak` → `no live connection`), and the tool call it came
with was answered *"no call is running"*. `call_ended` now moves the row —
`state` and `cause` — and raises nothing, the treatment `call_ringing` already
had. The end of a call is recorded where the line is recorded: the channel's own
`calls` table. `call_state` therefore no longer takes the value `ended`.

**A refused caller was parked in silence.** A number in no `callers` entry
raises `unknown_caller` and no turn, and that was all it did — while the
dialplan had already *answered* the leg in order to `curl` this hive. Its own
mailbox fallback fires on a `curl` that fails, not on one that comes back
carrying a refusal, so the caller sat in an answered call nobody would ever
speak into. The signalling half now kills that leg itself (`uuid_kill` on the
UUID the dialplan named) in the same breath as the refusal, and what the caller
hears next is the dialplan's business again.

Third place: no lane, no tool name and no dialplan event moved, so a colony on
`1.0.0` migrates by swapping the node onto `freeswitch@1.0.1` and rewires
nothing. Measured by
`crates/meclaw-cells/tests/freeswitch_channel_places_a_call_and_hears_the_line.rs`.

#### `voice@1.2.0`: `language_hint` is legal on one Flux model only

A colony that set `model: "flux-general-en"` next to the template's `language`
never connected: the adapter sent `language_hint` with every model, and Deepgram
allows it on one. "`language_hint` is only supported on `flux-general-multi`.
Sending it to any other model (including `flux-general-en`) returns a `400`
error" — `400 INVALID_PARAMETER`, before a single frame of audio moves
(<https://developers.deepgram.com/docs/flux/language-prompting>). The adapter's
own module header had said as much since it was written; the code had not.

The rule is now the allowlist Deepgram states, not its negation: the hint goes
out for a model name ending in `-multi` and stays home for everything else, and
the omission is logged at `debug`. That way a model name this build has never
heard of loses the language bias — recoverable — rather than the whole request.

#### `member@1.6.3`: a screen takes a `view`, and a receipt is not a turn (GH #598)

Two colonies burned model calls in a closed loop between an agent and a screen —
44 brain calls in six minutes on one, 68 on the other — and both halves of the
loop were shipped recipes.

**A screen was bound on `answer` as well as `view`.** GH #459 drew the display's
down-edge on the claim that an agent's prose answer is the smallest view there
is. It is not: `display@1.0.2` reads a view out of the BODY (`view_id`, `kind`,
`content`) and an `answer` carries `messages[]` and none of them, so every such
answer came back refused with `error_code: invalid_view`. No shipped recipe binds
a screen on `answer` any more — `templates/builder/recipes` (`_screen_level`),
`examples/organism/grow-screen.json`. What reaches a screen is what a producer of
views emits.

**And the refusal came back as a turn.** The member re-stamped a `receipt` owned
by one of its generations onto `in_turn` with `hop.kind = 'receipt'`, because the
assistant level accepts no receipt lane. A generation reads a turn by ANSWERING
it, so the answer went to the screen, the screen refused it, and the loop ran at
the speed of the model. That edge is gone: every `receipt` no **app** of the
member owns leaves the level on `error` with the original lane on `hop.kind` —
the branch the level already had for an owner it could not place, widened by one
clause, and the treatment `pack_ack` had documented all along. An app keeps its
receipts: it declares the lane, it produced the view that was refused, and it
answers a refusal with code rather than with prose.

`assistant` and `display` are untouched — the loop was never inside either of
them — and no lane, no `error_code` and no cell moved, which is why `member` is a
third digit. Decision and reasoning: `plans/adr/0025-a-receipt-is-feedback-to-a-writer.md`.
Measured by `crates/meclaw-cells/tests/gh598_a_screen_receipt_is_not_a_turn.rs`.

- **The gate's tree sync now full-touches a foreign worktree** (GH #595). Its
  targeted path — `git diff <stamp-sha> HEAD` plus both dirty lists — was
  applied to a foreign path as well, and that under-touches by construction:
  two worktrees at the same commit hold byte-identical sources, so a test binary
  compiled from the other copy is fresh by mtime, links, runs, and carries the
  other tree's `CARGO_MANIFEST_DIR`. Measured as 17 red file tests reading
  another worktree's fixtures. A foreign stamp path is now always the full
  touch; the narrow one stays for the same tree at another commit.
- **`corpus-committed`: a new gate station that reads the seed corpus before
  anything regenerates it** (GH #596). The `corpus` station regenerates first
  and checks second, so it graded the file it had just written and a committed
  `docs.jsonl` that no longer described its sources was green in every mode.
  The new station runs the same `--check` against the tree as it is: `NOTE` in
  `strand` (a strand gates before its commit — the line is the reminder to
  `git add` the regenerated seed), `RED` in `integration` and `release`, and
  never in `ci`, where `workshop/` does not exist.

## [0.31.0] — 2026-09-05

A minor release: speech becomes a channel. The new `voice` cell type terminates
audio at its own WebSocket listener and hands the colony text turns, one `turn`
per end of turn, interim `partial`s on request, and speaks the assistant's
answer back on the same socket. Speech-to-text and text-to-speech are traits
with two providers each (Deepgram Flux and an OpenAI-compatible realtime
transcription; Cartesia and an OpenAI-compatible streaming synthesis), an echo
provider calibrates the wire, a built-in test page speaks the protocol from a
browser, and `voice@1.0.0` is the channel template a member instantiates. Also in
this release: the memory-hive porter walks the substrate slot (GH #261, breaking
for `in_import` callers), and the two repairs the voice wave found in its own
review (GH #592, #593). Nothing else in the contract moved.

### Breaking

#### `memory-hive@3.3.0`, `member@1.6.1`, `org@1.4.1`, `meclaw-os@1.8.3`: the porter is a walk over the substrate slot (GH #261)

The transfer lane's import leg is **one message and the whole document**, and the
five mechanisms the porter carried beside the substrate's are gone. Until 3.2.0
`in_import` took ONE part of a memory document as the body of the message; a
document was a sequence of sixteen such messages, and the caller was responsible
for posting them in the walk's order and for reading the `final` marker off the
last one. Since 3.3.0 `in_import` takes a DIRECTORY: `hop.import_from` names the
run the export wrote, nothing travels in the body, and the substrate's `transfer`
slot applies every `seed/<table>.jsonl` of it in one call.

What came out of `templates/memory-hive/porter`, and what answers it now:

| what the porter carried | what the slot does instead |
|---|---|
| a hand-maintained Python `SCHEMA` mirror, policed by a drift test | `export` reports the schema from the database itself |
| a four-round-trip `scratch` probe for idempotence | one message, one transaction over the whole document |
| a provenance name-list, checked by name | column-set equality in both directions, or the part is refused |
| per-row inserts that could fail halfway into `import_write_failed` | a table applies whole or is refused whole |
| a document format (`meclaw-memory-export/1`), a part sequence and a `final` marker | `seed/<table>.jsonl` plus `export_final.json`, written last by one rename |

What stays is the **walk**, because nobody else can know it: the order of the
sixteen tables, the key that makes a row the same row in each of them, the three
machine tables that are lane state rather than memory, and the closing
`canonicalize` per identity dimension. That is what `hop.import_from` buys — and
it buys one property the message form could not have: **a document with one broken
file writes nothing at all**, not even the tables ahead of the broken one, because
every file is parsed before the first row is applied. Sixteen calls would have
applied ten tables before finding out about the eleventh.

The refusal vocabulary of the lane shrinks with the mechanisms that produced it.
`hop.reject_reason` is now `export_write_failed` or `import_failed`, and the
substrate's own transfer code rides beside it on `hop.store_error`
(`transfer_seed_malformed`, `transfer_io_error`, `transfer_path_out_of_bounds`,
`import_schema_drift`, …) with `hop.store_operation` naming the operation — the
same shape this hive already used for a refused store (GH #343), and for the same
reason: that code list is OPEN, and a reason enum that had to grow with it would
turn the next new code into a failed emit. Gone from the enum:
`missing_audience`, `missing_channel`, `import_format`, `import_unknown_table`,
`import_schema_drift`, `import_probe_failed`, `import_write_failed`. Gone from the
`dump` hop: `export_part`, because there is no part sequence left to locate a part
in — one receipt names the whole document (`hop.export_of` tables applied,
`hop.rows_written` rows that were new, `hop.export_final` always `'1'`).

The substrate learned two things for this (`crates/meclaw-colony/src/db_transfer.rs`).

**A whole-directory import is ONE transaction**, over every table the call names,
inside one call on the cell's own connection. It used to be one transaction per
table, and the difference was not academic: the column-set gate runs while the
rows are applied rather than in the parse, so an `import_schema_drift` at table
nine committed the eight before it — and the receipt of that refusal says
`rows_affected = 0`, which is the worst shape a partial write can have: a
document that reports it did nothing and a target that carries half of it. Every
SQL failure had the same shape. Now a refusal anywhere rolls the whole walk
back, and `import_document` keeps its own single-part transaction unchanged
(`crates/meclaw-colony/tests/gh261_a_walk_names_its_row_identity.rs`
§ *a_table_refused_late_rolls_back_the_tables_before_it*).

And an `import` that names more than one table may name **`keys`**,
`{"<table>": ["<col>", …]}`. `key` belongs to one table's identity and therefore
only travelled when the call named one table, which left the whole-directory form
unusable for the cells that need it most — a `store` declares its tables in
`params.schema`, that declaration cannot express a `PRIMARY KEY`, and the import
refuses a keyless table by name. Additive: `key` alone behaves exactly as before,
and an absent `keys` falls back to each table's own primary key.

The precondition the issue set for itself was met before any of this was written:
a full round trip over a **real, grown** memory hive — export to a directory,
import into an empty one, compared row for row across all sixteen tables with
`audience_set`, `channel` and `speaker` compared as data and not as presence.
`crates/meclaw-cells/tests/gh261_a_grown_memory_walks_the_slot.rs` is the public
form of that measurement: it grows its own hive through this hive's own lanes and
measures the same equality, and it replaces
`gh243_a_memory_can_leave_a_hive_and_arrive_in_another.rs`, which measured the
mirror rather than the transfer and could be green while nothing moved.
`crates/meclaw-colony/tests/gh261_a_walk_names_its_row_identity.rs` pins the
substrate half.

The public promise moves with it: `docs/cell-types.md` § *Content transfer* and
`docs/meclaw-overview.md` § *Seed concept* said "one transaction per table" and
now say what the code holds, in both language halves.

**Migration.** A caller that posts a memory document part by part has to stop:
send ONE message on `in_import` with `hop.import_from` naming the directory an
export wrote, and drain `dump` for the one receipt and `reject` for a refusal.
Nothing about the FILES changed — a directory written by `memory-hive@3.2.0`'s
export is read by `memory-hive@3.3.0`'s import unchanged, and it is still the seed
set a fresh hive is born from (`examples/memory-import/`, untouched). A caller that
matched on one of the seven retired `reject_reason` values matches
`import_failed` plus `hop.store_error` instead. `member@1.6.1`, `org@1.4.1` and
`meclaw-os@1.8.3` are the pin nachzug and nothing else: no lane of any of the
three moved, and each of them names the version it derives from.

### Added

#### `voice` cell type and `voice@1.0.0` template: speech is a channel, and audio never becomes a message

A colony could be spoken to only by writing the words down first. Every channel
this substrate had — Telegram, Slack, the HTTP door, a display — carries text,
and the shortest path to speech was a transcription somewhere outside that
posted its result like a person typing. That path loses the two things that make
speaking worth it: it cannot answer while the sentence is still arriving, and it
cannot be interrupted.

The `voice` cell is a channel bridge built like `web`: long-running, two tasks,
its own listener on its own `port` (loopback by default), its own `cell.db`. One
WebSocket connection is one client. Raw PCM16 mono goes in, **text turns** come
out on the `partial` and `turn` lanes; an assistant's turn goes in on `in_speak`,
and speech comes back out on the same socket. The full frame reference is
`docs/voice-wire-protocol.md`, and the type is documented in `docs/cell-types.md`
§ `voice`.

**Audio terminates in the I/O half, and no sample ever enters a mailbox.** That
is the load-bearing decision (ADR-0023). A message here is a JSON body that is
logged, routed and possibly persisted, and a stream of samples is none of those
things; so the connection task holds the client socket and the provider socket
and carries the bytes between them, while only the semantics — a partial, a
turn, an error — travel as messages. Which is also why **the cell never
resamples**: the inbound rate is the one the speech-to-text provider demands,
the outbound rate the one the text-to-speech provider was asked for, both are
declared in the `hello` frame, and the client adapts. A binary frame of odd
length is not audio at all and is refused with close code `4400` plus a
`bad_audio_frame` on the error lane, rather than rounded down into noise.

**Two modes.** In `auto` the provider draws the turn boundary; in `hold` the
client does, by holding a key. Both end in exactly one `turn` per boundary — a
provider that withdraws its end-of-turn afterwards does not un-emit, and the
continuation becomes the next turn. A `partial` is a draft: it carries no
`turn_id`, a `turn` carries no `eager`, and `emit_partials: false` switches the
lane off where nothing listens for it.

**Providers are traits with two implementations each**, chosen by
`params.stt.provider` / `params.tts.provider`: Deepgram Flux and an
OpenAI-compatible transcription session for speech-to-text, Cartesia and an
OpenAI-compatible endpoint for text-to-speech. A third adapter is one new file
and one match arm — nothing in the cell, the wire or the turn machine moves.
Model names and thresholds are `params` with defaults rather than constants,
every provider carries its own credential (`${VAR}` only, redacted everywhere)
and its own `base_url`, and the `base_url` is what lets the tests point the real
adapter at a fake server instead of proving that a mock matches a mock.

**`echo` is the first thing to run**: a binary frame comes back byte-identical
and nothing else travels, so the wire — microphone, sample rate, framing,
playback — is calibrated before anybody blames a model for what an audio path
did. Alongside it the cell serves two reads of its own: `GET /info` is the
`hello` declaration as JSON without opening a connection, and `GET /` is a
built-in, self-contained browser test page that speaks the protocol. The
microphone needs a secure context — `localhost`, or a TLS proxy in front, which
is the same reverse proxy that terminates authentication, because this type
grows no auth story of its own.

`error_code` strings on the error lane are closed: `invalid_body`,
`missing_session`, `unknown_session`, `speak_failed`, `stt_failed`,
`bad_audio_frame`, `invalid_input`. The per-connection codes a client gets on its
own socket are a separate list and deliberately do not enter the topology.

The template `voice@1.0.0` is one cell, in the shape of `telegram-connector`, and
its README carries the binding manifest for a member. Two names are reserved and
nothing implements them — the frames and lanes `spoken` and `tool_call` — so a
later speech-to-speech composition has a place to say what it heard and what it
wants called.

## [0.30.1] — 2026-09-05

A patch release: the repairs the first colony built on 0.30.0 turned up. Growing a
member is one submission again rather than two racing ones, so the screen and the
app cannot land in front of the person that holds them; and `meclaw --validate` now
asks a `ref` marker's `override_params` the same two questions the mutation door has
always asked, so a wrong cell path or a wrong param key is named before anything is
grown instead of a boot cycle later. Nothing in the contract moved, no migration is
needed from 0.30.0.

### Fixed

#### `builder@1.7.2`, `meclaw-os@1.8.2`, `tools@1.4.2`: a member wish is ONE submission (GH #585)

A member wish grew the person and then, in a second submission handed to the same
front in the same turn, its screen and its app. The order between the two was
semantics — the devices draw into `<member>/channels` and `<member>/apps`, scopes
only the person creates — and a front has no ordering across two submissions.
Measured on a fresh colony and reproduced twice: the device submission reached the
door first, was refused with `edge_schema` (`to='.' unknown` / `from='.' unknown`)
for a scope that did not exist yet, and the person committed behind it. The result
was a member with **no** screen and **no** app, the wish reported success, and the
only trace was one `rejected` row in `mutation_log`.

`recipes` now renders the person, the screen and the app as **one** manifest, in
dependency order. Nothing at the door changed and no lane moved: a manifest is an
ordered list the colony rolls off entry by entry through the very handling a single
body takes, so an entry is judged against the tree the entries in front of it grew
— that property is now measured
(`crates/meclaw-cells/tests/gh585_a_member_wish_is_one_submission.rs`) instead of
only stated in a doc comment, and `docs/meclaw-overview.md` § *Zweite Body-Form: das
Manifest* says it in both language halves. One manifest is one digest and one caller
row, so a refusal of the wish still finds the door it came from.

`tools@1.4.2` carries the same correction on the surface a model reads: the
`build_topology` schema and the template's own description said `grow_level` answers a
member with more than one manifest. They now say more than one **declaration**, in one
submission, because the devices draw into scopes only the person creates. The builder's
authoring prompt (`templates/builder/brief`) says the same about
`examples/organism/grow-screen.json`, which is unchanged and stays applicable on its own —
it is the tail of the one manifest now rather than a second one.

**Migration: none needed.** The wish, its parameters and the tree it grows are the
same; what changed is how many bodies knock on the door.

#### `--validate` reads the `override_params` a `ref` marker carries (GH #586)

A `cell.type: "ref"` marker in the root tree may carry a top-level
`override_params` block, addressed by the cells of the template it names. The
mutation door has asked both halves of such a block for a long time — a key that
names no cell (GH #140), and a key inside an entry that names no param
(GH #294) — but `meclaw --validate` asked neither: a wrong cell path committed
as a silent no-op, and a wrong param key surfaced a whole boot cycle later at
the spawning cell. `--validate` now resolves the marker's template and asks the
same two questions through the same code, with the same wording and the same
`error_code` (`schema`). It is a hard error without `--validate-strict`, the
same sharpness an unresolvable reference already had: a param the referenced
template does not declare is not a legal topology somebody might have meant.
`--validate` still grows nothing. Every offending key in the block is named
in one run, the way the mutation door names every misspelled key of a diff — a
pre-flight check costs a round trip per finding otherwise.

## [0.30.0] — 2026-09-05

This is the wave in which every open issue was either built or ruled. The poll
timers are gone, the menu and the screen follow the mutation receipt the door
now leaves behind, and `colony.json` carries `mutation_receipts` for it. File
transfer moved into the substrate: the `transfer` slot writes and reads the
directories a store owns, so the holders write their own seed set and the one
cell that did it for them is gone. `memory_recall` is an ordinary tool call
again, answered by the member's own memory hive instead of a private lane. Every
member grows a screen and an app, and the OS hands out the port. The last ~140
environment knobs in the template library are params, and a gate keeps that
surface closed. And the wave that prepared it left one gate process behind
(GH #577): a single entry point whose scope comes from the diff.

### Breaking

#### `talky@5.0.0`, `cogny@5.0.0`, `collector@4.0.0`: the private memory lane is gone (GH #552)

The lane `in_memory_call` and the setting `memory_call_tier` are **removed**. Both
existed for one reason: the collector inside `talky` and `cogny` answered
`memory_recall` itself, out of the recall port it already owned for the ambient
leg. It does not any more — the member's own memory hive declares the tool and
serves the call — so a door into a room nobody is in came out with it, and the
knob that switched it on came out with the door. `templates/collector` loses one
accepted route and one setting; `talky` and `cogny` each lose the accepted route
and the internal edge that fed it; `talky`'s brain seed loses the row that typed
the schema.

**Migration: none needed.** No shipped caller sends `in_memory_call` — the two
composites routed the name into their own collector and nothing else ever did — and
no shipped tree sets `memory_call_tier` beyond the two ref markers that moved with
this change. If your own parent draws a `memory_recall` edge into `./talky` or
`./cogny`, remove it: the call leaves on the ordinary tool exit now, and an edge of
yours pointing at the composite delivers it to a lane that parks. `memory_recall` is
declared on the assistant level's `tools` list
(`templates/assistant/talky/config.json`, `override_params."collector/assemble".tools`),
which is where a name only the level can route belongs — the same place
`consult_cogny` is declared, for the same reason.

#### `member@1.6.0`: `export-sink` is gone — the holders write their own seed set (GH #555)

The one `code` cell the member level owned is **removed**, and with it the six
edges that fed it and drained it. It existed for one reason: a document that is
a FILE by definition travelled as messages, and something had to turn the stack
back into files. Since the substrate half of GH #555 the `transfer` slot writes
directories itself, so every holder's own store does it — `memory-hive`,
`affinity`, `firewall` and, four levels down inside a named generation, the
`session-keeper`. Each one's porter names a directory, asks its store to write,
and raises `export_done` off the slot's own receipt: `hop.export_hive`,
`hop.seed_dir` (relative to that store's fence) and `hop.rows_written`. The
ruling behind it is one sentence: *cells manage their own files, nobody else
does.*

**Migration.** Point each exporting store at a directory with
`params.transfer.base_path` — an absolute path, `override_params` on the
manifest that grows the level: `"memory-hive/store"`, `"affinity/store"`,
`"firewall/rules"`, and `"talky/session-keeper/sessions"` on the manifest that
grows a generation. The env knob `MEMBER_EXPORT_DIR` is gone with the cell that
read it; the fence is a param now, and `hop.export_to` — which the operator's
`in_dump` trigger passes through beside `hop.target` — names the directory of
one run under it. A tree that sets neither keeps the shipped default
(`/tmp/meclaw-member-export`) and lands one directory per holder under it, which
is the layout the sink wrote. **Redirect it.** That default is the sink's, and
what travelled through it now travels through a store's own fence: `/tmp` is
world-readable on an ordinary host, and a member's export is its whole memory,
its curated record and its screening policy. Nothing about the fence is a
secret, and nothing about it is checked — the substrate writes where the
`params` say, which is exactly the point of putting the decision in the
instance.

**What is lost, and it is one thing.** The member-level `export_final.json`
**no longer exists**. It was a composition statement about four hives — which of
them had finished — written by the one cell that saw all four, and the substrate
knows no hives at all: it writes the marker of the CELL that wrote a directory,
`{format, cell, exported_at, tables, rows}`. What replaces it is that every
directory says for itself whether it is whole. `examples/memory-import/build_import.py`
reads exactly that, one `seed/export_final.json` per directory, and refuses a
directory without one; nothing else in the tree read the member-level file.
A reader of your own that waited on it has to wait on the per-hive markers
instead — or on `export_done`, which now arrives once per holder and names the
directory.

**Two lanes moved with it.** `export_done` is raised by the holder that finished
rather than by the level, so the member draws one exit per holder instead of one
per sink. And `dump` — which carried export parts AND import receipts, told apart
by `hop.dump_kind` — carries only the import receipt now, and **leaves the
level**: it used to end inside the member, in the cell that read it and said
nothing about it, which is the one arrangement GH #284 forbids. `member`, `org`
and `meclaw-os` each carry it out, the way they already carry `close_report`.
`hop.dump_kind` is removed from every porter's contract: with one payload per lane
there is nothing left to tell apart. `hop.export_final` stays — it marks the last
part of an incoming document, which is what triggers the re-derivation.

### Added

#### The mutation door leaves a receipt, and the boot is the first one (GH #553, substrate half)

A committed mutation was an event only its own caller could see. The verdict
travels back on `reply_to` — and `POST /colony/mutations` sets none, `meclaw
--apply` sets none, so on the two paths an operator actually uses there was no
channel at all. Everything else in a tree that wanted to know the graph had
moved asked on a timer instead: a menu tick every five minutes, a topology
snapshot every minute, forever, for an answer that changes a few times a day.
That is a poll in an event-driven substrate, and it is paid for in availability —
the ticks are what put the loop under the watchdog's nose on a busy box.

The door now emits the event itself. With `colony.json` carrying
`"mutation_receipts": {"to": "<hive>"}`, every **committed** knock leaves exactly
one terminal message at that hive: sender `/colony`, the mutation's own
`trace_id`, the knocking message as `parent_message_id`, a fresh routing budget,
an empty UBF body, and everything it has to say in the `hop` —
`route: "mutation_committed"`, `mutation_id` (or `mutation_ids` for a manifest),
`outcome`, `scope`, `form`. Five keys and no more: *what* changed is a question
`/colony/graph` already answers, and a receipt that answered it too would be a
second, staler truth. The target is a hive by intent, so the receipt enters as a
hive transit and the hive's own `{"from": "."}` edges decide who hears it — the
substrate hands over an event and never learns a listener's address.

**And the boot leaves the first one.** Right after `InitialApply` puts the edge
table up, one receipt with `form: "boot"` and a nil id goes out. A listener that
only heard about *changes* would sit empty until the first mutation of the day —
on a stable tree that is hours, and a screen that answers `503` for hours is a
screen nobody trusts. It also heals the one case a shutdown could eat: an
`--apply` run whose receipt died with the process is repaired by the next start.

Three properties are structural rather than remembered: a refusal leaves no
receipt (including a manifest that stopped part-way — its own reply already says
where it stopped), the shutdown drain emits none (a receipt there is new work the
drain would then have to wait for, the same rule GH #47 applies to source
emissions), and a receipt is neither an answer nor a mutation, so it can start no
round and beget no second receipt. Opt-in throughout: with no
`mutation_receipts` key nothing is emitted and no mutation behaves differently.

This is the second digit at the release cut: a caller can now do something that
was never promised before.

#### The transfer slot writes the seed it reads (GH #555)

A document used to leave a cell as a message and arrive as a message. The file
half — turning that document into the `seed/<table>.jsonl` a cell is born from —
was done beside the store, by a script cell that had to be told where to put it.
The owner's ruling on that is one sentence: *"cells manage their own files,
nobody else does."* The `transfer` body slot now takes `{"operation": "export", "to":
"<dir>"}` and `{"operation": "import", "from": "<dir>"}`, and the substrate
writes and reads the directory itself, above every cell type with a `cell.db`.
Without a `table` an export writes **every** content table into one directory;
`tables` names the list and its order. The message form is untouched and stays
the default when no path is named.

**The fence is the cell's own, and it is the only place a file may appear.**
`params.transfer.base_path` is an absolute directory the consuming cell declares
for itself (the precedent is `file`'s `base_path`), and `to`/`from` are relative
to it. A cell that declares none has nowhere to write and falls back nowhere.
A path that climbs out — absolute, `../…`, or out through a symlink — is refused
by name, and the lexical pre-check runs before any filesystem call, so the
answer is identical whether anything exists out there or not. The fence is
parsed but never canonicalised and never checked for existence at boot or at
`--validate`: a member whose export directory does not exist yet still boots,
and finds out at the first `to`/`from`. Every file lands whole or not at all
(`.part`, `fsync`, one `rename(2)`), the completion marker
`seed/export_final.json` is written last, and on the way in every part is parsed
before the first one is applied. The colony stays write-free outside `{root}`
instantiation; the one who writes is the cell.

New `error_code`-class strings (README § Stability): `transfer_path_out_of_bounds`,
`transfer_io_error`, `transfer_seed_malformed`.

This is the second digit at the release cut: a caller can now do something that
was never promised before.

### Changed

#### `memory-hive@3.2.0`, `affinity@3.3.0`, `firewall@2.3.0`, `session-keeper@2.2.0`, `operator@1.2.0`: the export leg is one message (GH #555)

Every one of the four holders walked its content out table by table, as one
message per table, and the walk was the porter's whole export leg. It is one
message now: `{"operation": "export", "to": <dir>, "tables": <walk>}` at the
hive's own store, and the substrate writes `<fence>/<dir>/seed/<table>.jsonl`
plus `seed/export_final.json` itself. What is left in each porter is the thing
only that hive knows — the walk ORDER, which is load-bearing on the way back in
— and the completion word. The stores declare the fence
(`params.transfer.base_path`); a table that a store does not have refuses the
whole export by name instead of travelling as an `absent` part, and
`emb_models`, the scratch tables and the FTS shadows stay behind because the
walk names what travels.

`operator/export` passes `hop.export_to` through beside `hop.target` — the
directory of one run, relative to each holder's own fence — and renders
`hop.export_hive` and `hop.seed_dir` into the receipt an operator reads. It
resolves neither and never sees the file: a path this side canonicalised would
be a claim about a filesystem the operator does not stand on.

New `reject_reason` for the four porters: `export_write_failed` replaces
`export_read_failed`, because the read that could fail is the substrate's now
and what a porter can be told is that the write did not happen. A refusal there
means no marker was written, so the directory is not a document.

The second digit for all five: a caller can name a run's directory and get files
where it asked, which was never promised before.

#### `builder@1.7.0`: every member grows a screen and an app, the OS hands out the port (GH #543)

A person in this substrate is the thing that has input and output devices. The
fast lane grew the person and stopped there: the screen it speaks through and the
application that draws on it were a second, hand-written act, and every colony
that wanted one wrote the same two declarations again — with a port somebody
picked by hand, which is fine exactly once. Two members with two hand-picked
ports agree only by luck, and when they do not, the second screen binds nothing
and says so to nobody who is looking.

A `grow_level` wish for a **member** now renders **two manifests** in order. The
first is the level, unchanged to the byte
(`examples/organism/grow-member.json`). The second is
`examples/organism/grow-screen.json`, cut to the two declarations that grow the
devices: a `display` into the member's `channels` and a `colony-view` into its
`apps`. There is no parameter that turns them off — `classify` drops a falsy key,
so *set to false* and *never mentioned* would be the same wish, and the ruling has
no third state anyway. What fills them is the builder's own configuration:
`member_screen_template`, `member_app_template` and `screen_port_base` are `params`
of the `recipes` cell, overridable per instance, and the recipe reads them off its
own stdin. It is still **told** which template to instantiate; only the teller
changed.

The port is `screen_port_base + <the member's index in its organisation>`, and the
index is **measured**: a new cell, `templates/builder/tally`, parks the wish in the
builder's round table, asks `/colony/graph` for the members container, counts its
direct children and stamps `hop.member_index` on the wish before the renderer runs.
The renderer keeps `network: deny` and its own promise — *no inference, no
retrieval* — literally true. A colony carries many organisations and one OS, and
the OS is what allocates what is system-near; the builder stands at the OS level,
so this allocation is that responsibility in its first form rather than a right an
organisation lent it (ADR-0022).

Two manifests are two submissions with no rollback between them, and the order is
semantics: the second draws into `<member>/channels`, a scope only the first
creates. That is measured rather than assumed — applied the other way round the
screen manifest applies nothing — and each manifest carries its own digest and its
own caller row, so a refusal of either finds the door it came from.

One edge moved with this. The way back from a screen into a generation —
`<member>/assistants → ./<generation>` on `in_turn` guarded by `hop.owner` — used
to exist only in the hand-written example, and no built colony had it: the
ordinary `in_turn` door is guarded on `context.assistant`, which a view event never
carries, so a screen event reached the container and stopped. It is part of the
**assistant level** now, which is the only place that knows the generation's name,
and it is an exact complement of that door (`!has(context.assistant)`) rather than
a second edge beside it — `apply_edges` fans out to every matching regular edge,
and a display *receipt* carries both the owner and the context the answer
travelled on, so two overlapping edges would hand the generation the same turn
twice. An assistant costs twenty-one transit edges instead of twenty.

New `error_code`-class strings (README § Stability): `count_unavailable`, spoken
by `templates/builder/tally` when the count could not be taken and by
`templates/builder/recipes` when the index or the port base arrived unreadable.
One code, because it is one event seen one cell apart — the OS was asked for a
port and cannot answer — and one repair. It is never quietly rounded down to the
base port: a screen given a socket somebody else holds fails as a page that never
loads. An **absent** index is not that case and still means zero, because nobody
counting is a statement about a first member rather than a guess.

Second digit, because a caller can now do something that was never promised: one
wish grows a person and the devices that person speaks through.

#### `builder-librarian@2.2.0`: a level row gets a retrieval window of its own (GH #543)

`retrieve` cut every row to 1200 characters unless it was a catalogue row. A LEVEL
row is the complete transit edge set a level gives a new child, and a set published
half is a set that will be drawn half — the promise
`a_level_row_survives_the_retriever_whole` makes. It had been kept by thrift: the
assistant row measured 1186 of 1200, and the header of `_level_body` had been
trimmed four times to keep it there. The way back from a screen moved into that
level and the row measured 1286.

So the window moved rather than the set. `kind == "level"` is the third window
(`BUILDER_LIBRARIAN_LEVEL_CHARS`, 1600), beside the catalogue's 4000 and the
ordinary 1200, and `workshop/tools/build_librarian_seed.py` mirrors it — the
generator still refuses a row it cannot publish whole rather than silently
following whatever the retriever currently says. Third digit: an existing promise
goes on holding.

#### `tools@1.4.1`: the `build_topology` schema says what the builder does (GH #543)

Two sentences the model reads were describing a builder that no longer exists.
`credential` has ridden in the level's own declaration since `builder@1.6.1`
(GH #567) and the schema still called it *a second declaration beside the level*;
and a member wish now answers with two manifests, which the schema did not mention
at all. Both repaired, plus one sentence saying what a member wish does **not**
take: neither a screen flag nor a port, because a member always gets both and what
fills them is the builder's own configuration. Third digit: prose repair.

#### `meclaw-os@1.8.0`, `org@1.4.0`, `member@1.6.0`, `assistant@2.5.0`, `talky@5.0.0`, `cogny@5.0.0`, `collector@4.0.0`, `colony-view@1.1.0`: the menu and the screen follow the mutation receipt, the two poll timers are gone (GH #553)

Two cells in this library asked a question that already had an answer.
`collector/menu-clock` woke every five minutes to ask a tools hive for
declarations that only change when somebody mutates the graph;
`colony-view/refresh` woke every minute to ask `/colony/graph` for a topology that
only changes for the same reason. On a real colony with two collectors that is
roughly two thousand ticks a day, each one a full pass through the colony loop, and
each answer identical to the last one in all but a handful of cases. A poll in an
event-driven substrate is availability spent on a question that has already been
answered — permitted, until this wave, only as a bridge until the event existed.

The event exists. The mutation door leaves one terminal receipt per committed knock
at the hive `colony.json` names (`mutation_receipts.to`), on the lane
`mutation_committed` — and **the boot is the first receipt** (ruling O-0904-2), so a
colony that has just started has already said "the graph moved" once before anybody
touches it. There is no lazy first fill and no store round trip: a restarted colony
fills its menus and draws its screens out of the boot receipt, which is why the
`503` window of a display no longer depends on somebody sending an `in_refresh`.

What is new here is the road from that receipt to the two consumers, and it is drawn
**one level at a time** — because a level that declares a lane takes part in it and
may not be skipped (ADR-0020). `meclaw-os` accepts `mutation_committed` and carries
it into `./orgs`; `org` into `./members`; `member` into `./assistants` **and**
`./apps`, because an agent's tool menu and a person's screen are the same fact read
twice; `assistant` into `./talky` and `./cogny`, and each generation into its own
`./collector`. **The lane keeps ONE name the whole way down**, which is what lets
every level in between declare it — a level that declares a lane is a mandatory hop
for it, and a level that renamed it could not be one. The collector turns
`mutation_committed` into its own internal `in_menu_tick`, exactly the shape
`./menu-clock -> ./assemble` had; the internal lane stays internal, and nothing
outside can name it. `colony-view` turns the receipt into its own `in_refresh`,
which was always the lane its `refresh` timer fired on.

`templates/collector/menu-clock/` and `templates/colony-view/refresh/` are **deleted**,
with the edges that named them. `MENU_CRON` and `COLONY_VIEW_REFRESH_CRON` are gone
with them; no other knob moves. `in_refresh` stays, as the operator gesture it always
was: ask for the picture again without changing anything.

**A shipped example opts in wherever it has a consumer.** `examples/organism` and
`examples/display-colony-view` set `mutation_receipts` in their own `colony.json`,
because each of them grows something that listens — an agent's collector, an app's
picture — and since `colony-view@1.1.0` the app has no other producer at all. The one
that stays out is `examples/meclaw-os`: it grows a `talky` and a `firewall` and no
consumer of the lane, so an opt-in there would be a key with nothing behind it. What
that costs while a colony is still growing is a **`no_route` dead letter at the empty
container** — the boot receipt of an empty seed has nowhere below it to go yet, and that
row is the documented, expected one: it names its sender and its trace, which is what
GH #284 keeps the dead-letter queue for.

**Version digits.** Second digit throughout, and for one reason on every level: a
caller can now do something that was never promised before — send `mutation_committed`
at `meclaw-os`, `org`, `member`, `assistant`, `colony-view`, `talky`, `cogny` and the
collector inside them. `meclaw-os` 1.7.0 → **1.8.0**,
`org` 1.3.0 → **1.4.0**, `colony-view` 1.0.2 → **1.1.0**. `member@1.6.0`,
`assistant@2.5.0`, `talky@5.0.0`, `cogny@5.0.0` and `collector@4.0.0` are this wave's
target versions already and carry the change without a second bump — a version moves
once per wave, not once per strand. `clock` 1.0.0 → **1.0.1** is prose only:
its README listed both timers this issue removed. `tools@1.4.1` carries a sentence of
this strand too — its README named `MENU_CRON` — and moved for GH #543 in the same
wave, so it is not bumped twice either.

#### `memory-hive@3.2.0`: the memory answers `memory_recall` itself (GH #552)

Whoever is reached declares themselves. Until this version the name was answered by
a COLLECTOR one composite away, and the schema the model read was a hand-typed
projection of *this* hive's own `in_query` contract: seven context keys, three of
them the model's, retyped into a template that answers no recall — and a second time
into a brain's birth seed, and a third time as README prose. Three artefacts for one
contract, each free to drift, held together by one test whose own header said it was
an interim. The half-open-window rule is the case in point: `read_window` here
REFUSES a window with one bound, and the copy said both bounds were independently
optional, so a model that named one bought a refusal it could not see coming.

The hive that enforces the rules now writes them down. Two cells, and neither
retrieves anything: `schemas` answers the hive's new `in_schemas` lane with one
`{name, description, parameters}` — the `tools` hive's own cell, byte for byte in
lane pair and answer shape (GH #548) — and `tool` translates, one way turning a
`tool_call` into the ask `./recall` already understands, the other turning the
bundle, or the refusal, into one `tool_result` under the original call id. The tier
stays configuration (`tool` params.tier): a model that could choose its own depth
could ask for one the instance was tuned away from. A refusal is an ANSWER —
`missing_audience`, `missing_channel`, `half_open_window` and `store_refused` reach
the caller as `hop.error_code` on the result — because a call nobody answers stalls
the asking round until its idle window runs out.

The road is four template edges at the member rim (`member@1.6.0`), six inside the
generation (`assistant@2.5.0`) and four rendered per generation by
`builder@1.7.0`'s level recipe. Template edges rather than v-lanes on purpose: the
member is a mandatory hop anyway, because it is the level that stamps the round a
recall is asked in, and an edge on disk is one an audit can read. Two silent traps
came with it and both are pinned: `./cogny -> ./tools` had to become a `default`
edge in the same act, or a named `memory_recall` edge beside it delivers every core
tool call twice; and the hive's two ways out of `./recall` had to be narrowed
explicitly on `context.memory_call_id`, because this hive already has two default
edges and a third piece of default semantics is a debugging cost nobody can afford.

New `error_code`-class strings on `memory-hive`'s `tool_result` lane:
`malformed_tool_call`, `memory_not_configured`, and the recall cell's own four
refusal reasons handed on verbatim. The `schemas` cell answers with the tools hive's
own two, `tool_unknown` and `tools_missing`.

Second version digit for the hive: two accepted lanes and two emitted ones are new
and nothing existing changed behaviour. First digit for the three templates that
gave the name back, second for the two that gained the edges. See
[#552](https://github.com/mmeyerlein/meclaw/issues/552).

#### `memory-hive@3.2.0`: every knob is a param, the tick is called `clock` (GH #138, #551)

Forty-nine behaviour knobs of this hive were `${MEMORY_*}` substitution tokens:
colony-global by construction, so two members of one colony could not have their
memories tuned apart, and every knob NAME was a public configuration contract
that could only be renamed by breaking a running colony. Worse, none of them
could be tuned per instance AT ALL: an `override_params` entry may only name a
key the addressed cell carries under `params`
([#294](https://github.com/mmeyerlein/meclaw/issues/294)), and a knob that exists
only as a token inside a script literal is not such a key. The configuration
surface of the biggest template in the library was one shared `.env` or nothing.

All forty-nine are params now, defaults bit-identical, in the form
`collector@1.2.0` set ([#136](https://github.com/mmeyerlein/meclaw/issues/136)):
the value lives under `params.<knob>`, is declared in `contract.settings.<knob>`,
and the script reads it off the stdin document with the same number as its own
fallback literal. Thirty-one belong to `./recall`, eleven to `./dream-glue`, two
to `./close-glue`; `./closer`, `./dreamer`, `./dialectic` and `./judge` carry
their deliberation budget as a literal inside `params.provider_extra`, and the
nightly schedule is a literal inside `params.schedules[0].cron`. `./recall` gained
thirty declarations it never had: thirty-one knobs stood beside ONE
`contract.settings` entry, so the contract said almost nothing about a cell that
is almost entirely configuration. The environment route is GONE rather than deprecated:
the only `${...}` left anywhere under `templates/memory-hive/` is the provider
lane, twelve names, each of them a credential, an endpoint or a model id.
`MEMORY_EMBED_DIM` is on that list one by one and not by class: it is one
statement with `MEMORY_EMBED_MODEL` and the shipped `store/seed/emb_models.jsonl`,
the seed is not variable-substituted, and a `dim` that disagrees empties the
semantic leg silently instead of raising — so the three stay on one surface or
they stop matching.

The tick moved in the same commit: `templates/memory-hive/cron` is
`templates/memory-hive/clock` (ruling R-0904-5), which is the third and last of
the three renames [#551](https://github.com/mmeyerlein/meclaw/issues/551) asked
for. The dated exception list in
`gh401_shipped_timer_schedules_deserialize.rs` is empty again, so the sweep over
the tree is the whole rule.

What this cost elsewhere, and why it is stated: six tests and three eval harnesses
used to push the nightly consolidation out of a run's way with a
`MEMORY_DREAM_CRON=` line in a `.env`, and such a line is read by nothing now —
silently, which is the failure mode this kind of migration has. They say it with
`override_params` instead, and a test asserts that the schedule an override names
is the one the real timer parser plans on, not merely that the runs stayed green
(`the_clock_ticks_on_the_schedule_its_params_carry`). The scenario suite and the
LongMemEval harness grew a `params` lane beside their `env` lane for the same
reason. The three copies of every value are pinned against each other by
`crates/meclaw-cells/tests/gh138_memory_hive_params.rs`, and
`scripts/check_tree_rules.py` R6 stops parking this template: a new `${KNOB}` in
it is a hard finding now.

The version digit does not move again — 3.2.0 already carries `memory_recall`
(#552) and the one-message export leg (#555), and this is the same release of the
same template. Had it stood alone it would still have been the second digit: a
caller can now do something that was never possible before, which is the rule's
own words. See [#138](https://github.com/mmeyerlein/meclaw/issues/138) and
[#551](https://github.com/mmeyerlein/meclaw/issues/551).

#### `session-keeper@2.2.0`, `summarizer@2.1.0`, `dispatcher@1.2.0`: knobs are params (GH #138)

Ten behaviour knobs of these three templates were `${KEEPER_*}` /
`${SUMMARIZER_*}` / `${DISPATCHER_*}` substitution tokens, and the dispatcher is
the case that shows why that was too narrow: this template is instantiated MORE
THAN ONCE inside a single agent — once in `talky`, once in `cogny`, once in
`builder` — and one colony-global key made the asking surface and the answering
core share a tool-class declaration they must not share. A `consult_cogny`
handoff belongs on the asking side and nowhere else, and the environment form
could not say so at all.

All ten are params now, defaults bit-identical, in the form `collector@1.2.0`
set ([#136](https://github.com/mmeyerlein/meclaw/issues/136)): the value lives
under `params.<knob>`, is declared in `contract.settings.<knob>`, and the script
reads it off the stdin document with the same value as its own fallback literal.
`./close` carries `idle_ms` and `close_limit`; `./prep` carries `recent_turns`,
`phaseout_chars`, `tool_chars` and `round_lines`; the dispatcher carries
`max_calls`, `async_tools` and `handoff_tools` beside the `interim` it already
had, and the two name lists read a JSON array or one comma-separated string. The
keeper's nightly cron is a literal inside `params.schedules[0].cron`, because a
`timer` has no top-level `cron` param and `TimerParams::parse` would ignore one
in silence — an override names `schedules`, the key that exists. What is left in
`.env` under these three templates is one name, `OPENROUTER_API_KEY` on the
summarizer's writer.

What this cost elsewhere, and why it is stated: **thirty-nine test setups and one
eval harness wrote these names into a `.env`** to steer a run — eighteen of them
a `KEEPER_NIGHT_CRON` line that pushed the nightly close sweep past the run,
seventeen a `KEEPER_IDLE_MS` line that made every open generation a candidate for
a forced sweep. Such a line is read by nothing now, SILENTLY: the shipped night
fires for a few hours a day, so a run that lost its line behaves differently
depending on the hour it started, and a lost idle window leaves a test waiting
for a close that cannot come. Every one of them moved in the same commit — to an
`override_params` entry addressed by cell path
([#140](https://github.com/mmeyerlein/meclaw/issues/140)) where a mutation grows
the tree, and to the same key written into the same `config.json` where the tree
is booted from disk, which is what the mutation door does to a staged config.
`crates/meclaw-cells/tests/gh138_keeper_summarizer_dispatcher_params.rs` measures
each knob class twice — once with no override, where the shipped default must
stand, and once with one, where the override must be what the cell acts on — so
the claim is the behaviour and not the green.

`meclaw-os@1.8.1` moves with them and is the third digit: its `requires.env`
rollup named the dispatcher's three keys, and a rollup that names a key nothing
reads sends an operator to set a value that goes nowhere. The three are gone from
it and from `examples/meclaw-os/seed-ref/.env.example`; nothing else about the
shell changed. `session-keeper` stays at 2.2.0 — the same release of the same
template already carries the one-message export leg (#555) — and `summarizer` and
`dispatcher` take the second digit, for the reason the rule gives in its own
words: a caller can now name a key in `override_params` that was refused before.
`talky@5.0.0` and `cogny@5.0.0` do not move; they reference the two sub-units,
and a ref pin round is not a composite change. See
[#138](https://github.com/mmeyerlein/meclaw/issues/138).

#### `affinity@3.3.0`, `firewall@2.3.0`: knobs are params (GH #138)

The two hives that stand around a person — the one that says who somebody is and
the one that decides whether a stranger reaches them at all — carried ten
behaviour knobs as `${AFFINITY_*}` / `${FIREWALL_*}` substitution tokens. That
made every one of them colony-wide: two members of one colony could not keep
their records at two cadences, and the screens in front of two channels could not
be given two budgets. Worse, the knob could not be addressed at all — the mutation
door only accepts an `override_params` key the cell actually carries under
`params` (#294), and a value that exists only inside a `script_inline` literal
carries nothing.

All ten are params now, in the `collector@1.2.0` form (#136): a value under
`params`, a declaration in `contract.settings`, and — for the nine a script reads
— a literal beside the accessor, three copies of one number that a test compares
against each other. `./brief` declares `disclosure_rows`, `traverse_depth` and
`traverse_nodes`; `./push` declares `subscriber_rows`; `./screen` declares
`firewall_max_chars`, `firewall_rate_max` and `firewall_rate_window_ms`;
`./warden` declares `firewall_hold_ttl_ms` and `firewall_hold_max`. The tenth is
`affinity/clock`'s cadence, which stays a value INSIDE `params.schedules` because
that is the only key a timer's parser reads — a top-level `cron` would be ignored
in silence. **Every default is bit-identical**, and neither hive reads a `.env`
line any more: both are deterministic, neither asks a provider anything, so the
allowed set of substitution tokens in these two subtrees is now empty and a test
sweeps both trees to say so.

The safety clamps survive the move and are pinned: `firewall_max_chars` still
cannot be set above the hardline body ceiling of 262144, `firewall_hold_max` still
cannot lift the hold pile above 1024, and `firewall_hold_ttl_ms` still has no
value that switches the expiry off — `0` is floored at 1 ms. A knob that is
absent, `null` or blank is "not configured" and is therefore the shipped default,
which for the TTL means 3600000 rather than the floor; a number typed as a string
is read as a number.

**The silent half is the one that mattered.** Eighteen test files pushed one of
these knobs out of their way with a line in a `.env`: fifteen wrote
`AFFINITY_PUSH_CRON=` to keep the push tick out of a run, three wrote one of the
`FIREWALL_*` names to set the screening arithmetic. Those lines are read by nothing
now and would have said nothing about it — a lane ticking into a green run is a
flake, not a red assert. Each one is an `override_params` entry or an instance
config today, and `crates/meclaw-cells/tests/gh138_affinity_firewall_params.rs`
proves the new form both ways round: without an override the shipped literal is
what the real timer parser plans on, with one it is the overridden schedule.
`scripts/check_tree_rules.py` R6 stops parking both templates: a new `${KNOB}` in
either is a hard finding now.

Neither version digit moves again — `affinity@3.3.0` and `firewall@2.3.0` already
carry the one-message export leg (#555), and this is the same release of the same
templates. Had either stood alone it would still have been the second digit: a
caller can now do something that was never promised before. See
[#138](https://github.com/mmeyerlein/meclaw/issues/138).

#### `argus@1.1.0`, `access@2.5.0`, `receptionist@2.1.0`, `meclaw-os@1.8.1`: knobs are params (GH #138)

Twenty-two behaviour knobs of these three templates were `${...}` substitution
tokens: colony-global by construction, and — worse — not tunable per instance AT
ALL. An `override_params` entry may only name a key the addressed cell carries
under `params` ([#294](https://github.com/mmeyerlein/meclaw/issues/294)), and a
knob that exists only as a token inside a script literal is not such a key. Two
control loops in one colony had to share one measurement window; two brokers had
to share one TTL ceiling; two receptions had to build the same composite onto the
same lanes. The configuration surface of a security boundary and of a colony's
front door was one shared `.env` or nothing.

All twenty-two are params now, defaults bit-identical, in the form
`collector@1.2.0` set ([#136](https://github.com/mmeyerlein/meclaw/issues/136)):
the value lives under `params.<knob>`, is declared in `contract.settings.<knob>`,
and the script reads it off the stdin document with the same value as its own
fallback literal. Seven belong to `argus` (`./meter`, `./mutator`, `./probe`, and
the cycle schedule as a literal inside `./clock`'s `params.schedules[0].cron`),
eight to `access` (`./invoke`, `./policy`, `./sweep`, the sweep schedule in
`./clock`, and the vault's `key_source` and `credential_name`), seven to
`receptionist`'s `./greet`. The environment route is GONE rather than deprecated:
the only `${...}` left anywhere under the three subtrees is the provider lane —
the judge's provider, endpoint, model and key, and the reception's model id.
`argus`'s radius knob changed shape with the move: `numeric_param_keys` is a JSON
array rather than a comma string, because a params key can be typed and a
comma-separated list was a workaround for an environment that has none. A typed
comma string is still read the same way, since an override reaches that knob as a
config line somebody writes.

The vault's two knobs moved with the rest and nothing secret moved with them.
`key_source` names a SOURCE (`auto`, `prompt`, `systemd-cred`, `plainfile`) and
`credential_name` names a file under `$CREDENTIALS_DIRECTORY`; the passphrase
itself is still an environment variable the hive only NAMES (`params.unlock_env`,
shipped `null`), because a woken vault is locked until an operator opens it.

**`meclaw-os@1.8.1`** followed, and this is the half that would have rotted in
silence: the shell's `template.json` § `requires.env` is the roll-up of every
`${VAR}` its refs substitute, derived by test rather than transcribed, and fifteen
of its twenty-nine entries had just stopped binding anything — eighteen counting
the three the authoring dispatcher took with it in the same wave, which leaves the
block at eleven. A declared key that
binds nothing asks an operator for a value that goes nowhere, so the fifteen are
out, and `examples/meclaw-os/seed-ref/.env.example` — the copy-ready roll-up of
that block — lost the same lines. Removing a declaration nothing reads is a
repair, so the third digit.

What this cost elsewhere, and why it is stated: five test setups pushed a sweep or
a cycle out of their way with an `ACCESS_SWEEP_CRON` or `ARGUS_CYCLE_CRON` line in
a `.env`, and two wired a reception through `RECEPTIONIST_REPLY_TO` and its
siblings. Such a line is read by nothing now — silently, which is the failure mode
this kind of migration has, and which no assert would have caught. They say it
with the param instead, where a mutation's `override_params` would have merged it.
The three copies of every value are pinned against each other by
`crates/meclaw-cells/tests/gh138_argus_access_receptionist_params.rs`, which also
proves the point of the move once per accessor class: two probes tuned to
different windows, two receptions building different composites, two mutators with
different radii, and both clocks planning on the schedule their own params carry —
checked against the real `TimerParams` parser, not against a fixture.
`scripts/check_tree_rules.py` R6 stops parking these three: a new `${KNOB}` in any
of them is a hard finding now.

Second digit for all three: a caller can do something that was never possible
before, which is the rule's own words
(`docs/development-rules.md` § 4). See
[#138](https://github.com/mmeyerlein/meclaw/issues/138).

#### the long tail: `builder-librarian@2.2.0`, `llm-registry@2.1.0`, `archive-bridge@1.1.0`, `daily-digest@2.1.0`, `canvy@2.2.0`, `clock@1.0.1`, `tools@1.4.1`, `vault@1.3.0`, `coder-pipeline@2.1.0`, `talky@5.0.0`, `cogny@5.0.0` — their knobs are params (GH #138)

Twenty-two behaviour knobs across eleven templates, defaults bit-identical, in
the form `collector@1.2.0` set
([#136](https://github.com/mmeyerlein/meclaw/issues/136)). These are the
templates with one, two or four knobs each — the tail after the hives — plus the
grey zone ruling R-0904-6 settled by hand: a **grant id** and a **sandbox root**
are references, not material, so they are behaviour and they are params, while a
bearer token, an endpoint and a model id are the provider lane and stay in `.env`.

The tail has three shapes and each needed a different move, which is why a
migration that only knew the scripted one would have left two thirds of it behind.
**Scripted** (`builder-librarian/retrieve` 4, `llm-registry/select` 2,
`llm-registry/hand` 1, `archive-bridge` 1, `coder-pipeline/coderprep` and
`/dispatch` 1 each): the knob becomes a `params` key, a `contract.settings` entry
and the literal its own accessor falls back to — three copies, one value.
**Typed** (`tools/file` and `tools/edit` `base_path`, `coder-pipeline/fs`
`base_path`, `vault` `key_source` and `credential_name`, `talky/brain` and
`cogny/brain` `credential_grant_id`): the key was always a param of a substrate
cell type, with a token sitting inside it, so the migration is the TOKEN leaving.
**Scheduled** (`clock` `cron` and `schedule_id`, `canvy/clock` `cron`,
`daily-digest/clock` the digest URL and the chat id): the value lives inside
`params.schedules`, one key holding the whole list, and an override replaces the
list.

What that buys is the point of the issue, and in this group it is unusually blunt.
`clock` is a template whose entire purpose is to stand several times under
different names — and `CLOCK_CRON` gave every clock in a colony the same cadence
and every one of them the same schedule key. `tools` is one assistant's file
surface, and `TOOLS_FILE_ROOT` gave every assistant in a colony the same
directory to reach. `coder-pipeline` states its workspace in three places that
must agree, and one environment variable made agreement look automatic while
nothing checked it. `builder-librarian` briefs a builder, and two builders in one
colony could not brief at different widths. None of this was reachable by
`override_params` before: that mechanism may only name a key the addressed cell
carries under `params` ([#294](https://github.com/mmeyerlein/meclaw/issues/294)),
and a knob inside a `${VAR}` in a script literal is not such a key.

**One behaviour change is worth reading twice.** `daily-digest`'s
`TELEGRAM_DIGEST_CHAT_ID` had no default, so a colony that did not set it refused
to boot and named the variable. A params default cannot refuse; the chat id
therefore ships as the empty string. What replaces the refusal is the template's
own shape — the hive instantiates inactive, so nothing fires until a crossing
edge is drawn, and the mutation that draws it is the one that names the chat. An
empty chat id delivers nowhere rather than somewhere wrong, and the README says
so where the old table stood.

The `.env` lane that stays in these eleven is four names, all credentials or
endpoints: `OPENROUTER_API_KEY`, `TELEGRAM_BOT_TOKEN`, `SEARCH_ENDPOINT`,
`SEARCH_API_KEY`. `tools`'s `requires.env` and `meclaw-os`'s rollup drop the keys
they declared that nothing reads any more (`TOOLS_FILE_ROOT`, the four
`BUILDER_LIBRARIAN_*`), because a rollup that names a dead key sends an operator
to set a value that goes nowhere — `meclaw-os@1.8.1` and `builder@1.7.1` move a
third digit for that and for the librarian knob table `templates/builder/README.md`
published. Three test setups pushed one of these knobs out of their way with an
`.env` line and now say it as `params` or as the instance config on disk; such a
line is read by nothing after a migration and says nothing about it.
`crates/meclaw-cells/tests/gh138_long_tail_params.rs` measures every knob class
twice — once with nothing overridden, where the shipped literal must stand, and
once with an override, where that is what the cell acts on: the real
`TimerParams::parse` for the three clocks, `VaultParams::parse` for the vault,
`LlmParams::parse` for the two brains, the `file`/`edit` factories for the sandbox
roots, and the shipped scripts themselves for the scripted ten.
`scripts/check_tree_rules.py` R6 parks none of the eleven any more.

`llm-registry`, `archive-bridge`, `vault` and `coder-pipeline` take the second
digit — a caller can now do something that was never promised. The other seven
already carry this wave's target version and are not bumped twice for it.

#### the env-knob migration is finished (GH #138)

With this wave the shipped template catalogue has no behaviour knob left in
the environment. One hundred and thirteen of them moved across twenty
templates — the memory hive's forty-nine, argus/access/receptionist's
twenty-two, the long tail's twenty-two,
keeper/summarizer/dispatcher's ten and affinity/firewall's ten — every one of
them onto the `params` surface of the cell that reads it, with defaults
bit-identical and `collector@1.2.0` as the form, whose own twenty-six went
first (#136). That is about a hundred and forty across twenty-one templates in
all; `meclaw-os@1.8.1` and `builder@1.7.1` moved beside them, because a
`requires.env` rollup that names a key nothing reads asks an operator for a
value that goes nowhere. What remains in
`.env` is the provider lane: credentials, endpoints and model ids, read off the
NAME by `scripts/check_tree_rules.py` R6. The one template that did not migrate is
`steward`, deprecated since [#462](https://github.com/mmeyerlein/meclaw/issues/462):
it ships one more release and takes no further work, so its seven knobs stay
parked in the gate's `TRANSITIONAL` table with that reason written down rather
than being paid for.

**One knob is deliberately outside all of this, and it is named here rather than
rounded away.** R6 scans the catalogue, and `scan()` skips every `_`-prefixed
root — the same `_`-prefix rule R1/R3/R4/R5 have always run under — so the
minimal copy-from shapes under `templates/_cell-types/` are not in the scan.
`templates/_cell-types/edit-min/config.json` carries
`${EDIT_BASE_PATH:-/tmp/meclaw-edit}`, a sandbox-root knob of exactly the class
this wave migrated everywhere else. The exclusion is scope, not principle:
R-0904-6 ruled `templates/`, and `_cell-types` was never in the inventory.
Whoever brings those roots into the scan brings that knob with them. So the
count above is the catalogue's, `steward` is parked, and `EDIT_BASE_PATH` is the
one behaviour knob left under `templates/` on purpose.

#### docs: the terminal recording is archived (GH #43)

`docs/` carried a terminal recording — `demo.sh`, the `demo.cast` it was recorded
into, and the `demo.svg` rendered from it — and the export shipped all three with
the published tree. The README stopped pointing at them on 2026-09-01, with the
rewrite that gave the page its question sections, and that was deliberate: the
quickstart a stranger walks and the shape of a colony in text are what the page
needs, and a repository of this class does fine without a cast. The
three files now live in `docs/archive/`, under an archive header that names when
and why, and their `DOCS_MAP` entries are gone, so they no longer travel with the
published tree. Nothing else referenced them; `docs/README.md` says where they went.

#### templates: the env-knob surface is closed by a gate (GH #138, gate)

A template's tunables were `${KNOB}` substitutions read out of `.env`, and the
README called that an experimental surface that would migrate onto `params` "over
the `0.x` line". It was narrower than it looked: `override_params` may only name a
key the addressed cell actually carries under `params`
([#294](https://github.com/mmeyerlein/meclaw/issues/294)), so a knob that exists
only as a `${VAR}` inside a `script_inline` cannot be tuned per instance at all —
it is colony-wide or it is nothing. Meanwhile nothing stopped the next one from
being written, and 24 templates had grown about 120 of them.

`scripts/check_tree_rules.py` gains **R6**: every `${VAR}` a template hands a cell
— under `params`, `script_inline` included, and under `override_params` — is a
finding unless its NAME says provider lane. The class is read off the name on
purpose (`_API_KEY`, `_TOKEN`, `_BEARER`, `_BASE_URL`, `_ENDPOINT`, `_MODEL`,
`_PROVIDER`, the prefix `MODEL_`, plus `OPENROUTER_HTTP_REFERER`,
`OPENROUTER_X_TITLE` and `MEMORY_EMBED_DIM` one by one): a checker that guessed
from the value would pass the day somebody writes a timeout into a variable called
`..._BASE_URL`. `${uuid7:…}` and `${ctx.…}` are the instance class and are
skipped. Every template that has not migrated yet is parked by name in the gate's
`TRANSITIONAL` table with the strand that migrates it, so the gate is green from
the day it lands and the remaining work is readable from one place; a migration
that forgets its row gets a STALE line naming the row to delete.

`gh204_declared_defaults_match_the_inline` now compares a knob in whichever form
its template has — the environment twin, or the `collector@1.2.0` form of
`params.<k>` against `contract.settings.<k>.default` against the script's own
fallback literal ([#136](https://github.com/mmeyerlein/meclaw/issues/136)). Its
measured floor is unchanged, and the count it guards went from 62 to 97, because
a migrated template now converts its comparisons instead of losing them. No
template moved in this change and no default changed; the migration itself
follows. See [#138](https://github.com/mmeyerlein/meclaw/issues/138).

#### One gate chain, and it reads the diff (GH #577)

There used to be three gate chains — a strand gate, a pre-push gate and the CI
workflow — each with its own hard-coded list of steps, and each paying for work
the diff had not asked for: a Python-only change still compiled the workspace.
There is one entry point now, `scripts/gate.sh`, which asks one resolver,
`scripts/gate_plan.py`, which stations a diff needs, runs each of them once, and
reports every station on a single `GATE …` line plus a receipt under
`target/gate/<tree>/` — namespaced by the worktree it ran in, so several
worktrees sharing one `target/` at the same revision cannot overwrite each
other's receipt and station logs. CI runs the repository's own
gate scripts (`scripts/gate.sh ci`, `scripts/test-tier.sh t2`) rather than a
second copy of the steps, so a push that changes only documentation no test
reads skips the Rust test shards; the dev and test profiles build with
`debug = 0`,
which takes a test binary from up to 200 MB to a mean of 33 MB. Measured on the
v0.28.0 → v0.29.0 diff, the old chain ran the suite three times and the anchor
gates four; the stations and their triggers live in the resolver's docstring
and nowhere else. See
[#577](https://github.com/mmeyerlein/meclaw/issues/577).

#### `canvy@2.2.0`, `daily-digest@2.1.0`: a timer that only ticks is called `clock` (GH #551)

The library shipped six names for the same cell. A `timer` whose whole job is to
fire on a schedule was called `cron` in one template, `refresh` in another,
`night`, `menu-clock`, `tick` or `clock` elsewhere, so a reader could not tell a
cell's job from its name and a manifest author had to open the template to find
out what the thing in front of them did. Two of the three are settled here:
`templates/canvy/refresh` is now `templates/canvy/clock`, and
`templates/daily-digest/cron` is now `templates/daily-digest/clock`; the third,
`templates/memory-hive/cron`, moves in the `memory-hive@3.2.0` entry above and
empties the dated exception list. Nothing else moved — the same schedules, the same defaults, the same single out-edge
each, the same lanes at the rim. A tick whose NAME carries the semantics keeps
it (`night` closes a session, `menu-clock` asks the tools hive), and the payload
a `daily-digest` firing carries rides on its schedule rather than on the cell's
name, which is why `cron` had nothing to say for itself.

**A standing instance keeps the name it was grown with.** Instantiation copies
the subtree, so a colony already running either template is untouched by this;
a path IS a cell's identity and `move_nodes` is the only operation that changes
one. Both hives are sealed (`params.ports` is empty), so no caller could ever
name the renamed cell — the same reason `cogny@4.5.0` moved its second digit for
the same shape of change, and the reason this one does too. The rule is no
longer a warning either: the timer family left the unruled table in
`scripts/check_tree_rules.py`, the ruling is written down in
`docs/development-rules.md` § 8a R4, and what holds it is a sweep over the tree
rather than a checker that guesses
(`every_pure_tick_is_called_clock`). See
[#551](https://github.com/mmeyerlein/meclaw/issues/551).

#### The mutation door refuses a single-cell hive template by name (GH #572, #573, #574)

A hive is a scope marker, not an actor, so no cell factory serves the type
`hive` — and every door that stages a SINGLE cell used to discover that at the
wrong end. A template whose root is a hive with nothing under it is not a
subtree by either instantiating door's measure, so it took the single-cell path,
validated clean, staged a directory, and was answered from the apply arm with
`spawn: factory missing for hive`: an unnamed refusal from the half of the
mutation that is past deciding. It is now refused at stage 4, pre-destructively,
by name, at **both** doors — `add_nodes[].template` and the instantiate form of
`swap_nodes[].with` — through one predicate, because it is one question. The
refusal names the shape that works: grow the unit with an `add_nodes` entry,
which stages the subtree and registers its hive scope, and let a generation
change name it in `swap_nodes[].with: {"name": …}`. ADR-0021.

Beside it, a second reader of one list is gone. The contracts a mutation gives
birth to are deduplicated against what already stands — a path that stands keeps
the contract it was born with — but a `swap_nodes` successor's contract was
pulled out of the raw template read instead, behind the dedup's back. Naming a
richer template in a RESUME of a sleeping hive was therefore enough to anchor a
v-lane on a connect point that no `config.json` anywhere declares: the port
boundary said no and the re-anchor verdict said yes, about the same path in the
same mutation. Both now read the deduplicated list. The generation-change form is
untouched — a successor an earlier `add_nodes` entry of the same diff
contributed still counts — and a test holds each side. Underneath both, the five
routing terms of an `add_edges` entry are read by one function
(`add_entry_match_view`) and compared by one predicate
(`edge_identity_equal_views`) that the apply-time dedup now shares: the two
stage-6 lane checks and `EdgeTable::contains_equal` used to spell that reading
out separately, which is why GH #283 had to add the routing phase to each of
them by hand. The header-locality mirror (`mutation/header_views.rs`) still
reads the same five terms itself, because it keeps the modifier as a typed
`ModifierSpec` rather than the stored JSON; folding it in is a separate
question.

New `error_code`-class strings (README § Stability): `hive_template_single_cell`.

All three repair an existing promise — validation is pre-destructive and refuses
by name — so this is the third digit. See
[#572](https://github.com/mmeyerlein/meclaw/issues/572),
[#573](https://github.com/mmeyerlein/meclaw/issues/573) and
[#574](https://github.com/mmeyerlein/meclaw/issues/574).

### Fixed

#### A `/colony` read no longer walks the templates library (GH #571)

The colony's watchdog was ending the process at the top of every minute. The
minute is not a coincidence: it is a display's refresh tick taking a
`/colony/graph` read, and `docs/meclaw-overview` promises that read is answered
"out of colony's in-memory registry, without touching a database". The read kept
that promise. The dispatcher around it did not: **before** every `/colony/*`
dispatch — whatever the endpoint — it read the whole `templates` table out of
`colony.db`, cloned it into a snapshot, and built the rescan future, whose
synchronous prologue walks the entire templates library with `read_dir` and reads
every `template.json`. All of it ran on the colony task before the first `.await`,
and all of it was discarded again for every endpoint but the three that actually
read templates. Measured on a 48-template library: 98 read syscalls and 5.2 ms
per `/colony/graph` read; on a real library with cold caches that is the
half-second the watchdog saw. It is now 0 syscalls and 124 µs — the endpoint
decides what the prologue costs, and a read costs nothing.

**A read that is slow now has a name.** The work pulse GH #439 gave the mutation
path reached the reads too, but was never ticked for them: a long read was
silence, and the supervisor has exactly one reading for silence with nothing
declared — a parked loop that stopped answering, which is the fatal verdict.
Every `/colony/*` dispatch now declares itself as `colony-read <endpoint>`, so a
slow read is a `slow_work_item` on the work-item budget and the trip line says
which endpoint the loop was inside. `/colony/mutations` is unaffected: its own
label follows and wins, as it always did.

**And the heartbeat now survives a burst.** The colony→supervisor channel held 8
beats. The loop emits three per handled event and the supervisor drains once per
period, so a handful of events filled it in milliseconds — and what a full
channel keeps is the OLDEST word, not the newest. A loop that had just said
`Working` was read as one whose last word was `Parked`, which is
`in_flight_work=false`, which is `starved=colony_loop`, which ends the process. The
capacity is a named constant now (`HEARTBEAT_CAPACITY = 256`) and carries a
sixty-event burst without losing the loop's last word.

Repair of shipped behaviour, no contract moved: third digit.

#### The single-declaration door refuses a manifest body by name (GH #581)

The colony has two mutation doors: one takes a single declaration, one takes a
manifest — an ordered list of such declarations in one body. Handed a manifest
body, the SINGLE door answered `committed` and applied nothing: no node, no edge,
no log row for any of the entries it was given. A caller that took the wrong door
got a green receipt and a colony that had not moved.

The mechanism was one `unwrap_or`. The single door reads its work from the body's
`diff` key and treats an absent key as the *empty* diff — a reasonable reading for
a no-op declaration, and a silent one for a body whose whole content sits under
`manifest`. Every step below it then had nothing to do, found nothing wrong, and
committed. The vocabulary check that already refuses a key nobody reads looks
INSIDE the diff, so it never saw the top-level key that made the body a different
form. A body carrying `manifest` *beside* `diff` was worse still: the `diff` half
landed, the other half was dropped without a word.

The door now names the refusal, on the same discriminator the manifest door uses —
the presence of a top-level `manifest`, and nothing else. Any other unknown
top-level key stays ignored exactly as it was. The refusal is spurless by
position: it runs before the mutation id is minted, so it opens no mutation-log
row, exactly like the drain refusal. `error_code` is the existing `schema` and no
new string joins the documented set — a body form a door will not apply is what
`schema` has always meant, and the manifest door says the same from its side.

"Committed" for "did nothing" is the one answer a door must never give. Repair of
shipped behaviour, no contract moved: third digit.

## [0.29.0] — 2026-09-02

### Changed

#### README: the quickstart is the path a stranger actually walks

The README quickstart is now the path a stranger actually walks (PATH, tag-pinned
clone, key hint, a jq step that reports an error hop); the retracted idle-cost
figure is gone. Five steps, and the fourth already shows the point: the answer is
not a return value, it is a message on the record. The retraction itself is in
`docs/costs` § *Retraction* (`memory-hive@3.0.0`).

#### `colony-view@1.0.2`: a frame says which level it is (GH #549)

A hive frame was labelled with its directory name alone, and that word is not
unique in a grown tree. A member is a person and an assistant is a generation of
that person's agent, so the obvious name for the first generation is the
person's own name: `members/alex` holding `assistants/alex` drew two nested
frames reading one word, and the only way to tell the person from the agent was
to count rectangles. A frame now reads `alex · member` and `alex · assistant`.

**The level is read off the path, and nothing new travels for it.** The
composition levels address their children through fixed containers, so a frame
whose parent segment is `orgs`, `members`, `assistants`, `channels` or `apps` is
an `org`, a `member`, an `assistant`, a `channel` or an `app`; anything else is
a plain `hive`. `/colony/graph` is unchanged and carries no kind: its `nodes[]`
entries have exactly two keys, and hives are not nodes at all — they exist only
as prefixes of cell paths, which is why the view derives every frame in the
first place. Putting a level on that wire would be a public API round
(README § *Stability*) for a single consumer; five string comparisons in the
layout cost no contract, and a tree that does not use the composition levels
reads `hive` everywhere, which is the honest answer rather than a guess.

The label is single-sourced in the layout. The browser half places it — it takes
the FIRST `<text>` of a frame and moves it with the rectangle — and never writes
its content, so the component keeps exactly one text element and the new `level`
prop decides what it says beside the existing `name`.

#### `display@1.0.2`: a key collision is refused by name (GH #568)

A component-tree node may name its own `key`, and the object it becomes is named
`<parent>/<key>` instead of `<parent>/<index>` (`display@1.0.1`, GH #544). Two
siblings naming the SAME key minted the same id, and the second overwrote the
first in the tree the compose cell builds: the view was accepted, the receipt
said nothing, and one node was simply not on the screen. The published contract
— *a key is any non-empty string without a `/`* — promised more than it
delivered.

**It is a refusal now, and the tree is rejected as a whole**, the way a bad
`keep` list or an over-long `view_id` already is. `check_node` collects the keys
minted per parent and returns `invalid_view` with a `detail` naming the key and
the parent it collided under, so the refusal travels the `receipt` lane every
other validation failure uses.

**A numeric key is refused with it.** `ord` still comes from the index, so keyed
and unkeyed siblings coexist by design — and a key like `"3"` names exactly the
object the unkeyed fourth child beside it names. Refusing it is the smaller
contract, and no shipped view needs it: a view **kind** — `prose` or `component`
— names no keys at all, because keys live in the tree a sender draws; and the one
shipped sender that draws any, `colony-view@1.0.2`, derives every key from a
colony path behind an `n.`/`h.` prefix, unique by construction and never able to
look like an index. Both rules are read on the CHILDREN of a node: the root of a
view's tree is an only child under a wrapper named for its owner and its
`view_id`, so its own key can collide with nothing.
Third digit: the contract already promised what the door now enforces.

#### `submit@2.3.1`: the gate checks its own anchor, and two comments learn the new address (GH #558, GH #566)

**The created-node branch verifies the name it trusts.** `submit`'s form check
accepts an `in_pack` edge on two branches, and the second one — the parent
drawing the door of the child it is growing — is only ever as narrow as the set
of names it reads out of `add_nodes`. That set was taken as written. It is now
taken only from entries that this declaration can actually bring into the world:
the entry must **instantiate** — a `template`, or an `adopt` block — and the name
must resolve **under the declaration's own scope**, segment-wise and never as a
string prefix. The two instantiating shapes are the mutation door's own grammar
and are mutually exclusive: `template` builds the node from the library, `adopt`
builds it from a cell already on disk and mints a fresh cell id, which is an
instantiation and not a relocation. An entry carrying neither brings no addressee
into the world; a name that escapes the scope addresses somebody else's tree.
Neither anchors a door, so the edge is refused on the spot with
`subscribe_target_not_self` — the same form refusal as before, earlier and named
in the receipt's `detail`, and the broker is still never asked.

**No new `error_code`, and no new permission.** The contract already promised
that the target is the requester's own hive *or a node the same declaration
creates*; a node outside the scope, or one no entry instantiates, was never
created by that declaration, so the refusal it earns is the one that was always
its. No hole is closed here: the out-of-scope endpoint is refused at the mutation
door a stage later (`scope_out_of_bounds`), and every submission is still put to
the broker with the requester's identity. But this branch is a **security
default**, and a security default that leans on a later stage for the shape of
its own anchor is one refactor away from being wrong.

**And the two comments that still named the old road** (#558): the gate's script
said in two places that "the edge says `/os/submit`". Since #556 the submitter is
an occupant of the front door and the edge says `/os/operator/submit`; the README
had the corrected sentence, the script did not. Comment-only, and the old address
stays visible as the since-half of the sentence.

Third digit on both counts: the promise is unchanged, the check is the one being
repaired, and a comment repairs nothing but a reader.

#### `collector@3.5.0` (+ `talky@4.6.1` / `cogny@4.6.1` / `assistant@2.4.1` pins): a capped round is a named partial answer (GH #570)

**A round that spends its iteration budget now ends on a sentence, not on a
payload.** Until `3.4.0` the seam left on the `answer` lane with the assembled
projection exactly as it stood — window turns, then the tool round — and the last
turn was whatever the last tool had returned. **The last turn is what a consumer
reads**: the shipped surfaces take the last text of an answer and put it in front
of a person. Measured on the e18 rebuild (finding K-2): a `cogny` capped
mid-search, its `answer` body ended with a raw `web_search` `tool_result`, the
`assistant` level relabelled that message to `in_advice` with no guard on it, and
the surface wrote the search JSON into the conversation as the advice turn. The
better the errand was going, the more certainly a tool got quoted at the reader.

On the `spent` branch and only there, `brain_message` now appends one turn of its
own:

```json
{"origin": "assistant", "type": "text",
 "text": "The round hit its iteration cap (max_iter=8) before an answer was written. Collected so far: 5 tool call(s) -- web_search, fetch_url. The last result began: …"}
```

It is assembled in the cell and never asked of a model — a round that could not
finish is the one moment another provider call is the wrong answer, and a
sentence that changes with the weather is not a marker a reader can learn. Tool
names come from the round's `tool_call` turns in call order, deduplicated; the
head of the last result is whitespace-collapsed and cut to 200 characters.
**Nothing is lost**: the raw round stays in the `round` table and `thread_recall`
still reaches it. What changed is the last *word*.

**`hop.partial` is the marker `round_capped` could never be.** That key means two
things at once — the round ended early, or `round_bytes` / `tool_chars` trimmed
some bytes off a round that is still going and still on the `brain` lane — and
only the first is an answer somebody is looking at. `partial` is the first alone.
Like every key this seam writes it is **always present** (`"1"` / `"0"`), because
a CEL modifier that reads a missing key fails and a failed modifier skips the
edge. `round_capped` is untouched, and so are the byte caps: they stamp
`partial=0`.

No consumer had to move. `any_text` — the rule every asker on the advice lane
reads its turn with — picks up the digest by construction, because it takes the
last turn that carries text at all.

- **`talky@4.6.1`, `cogny@4.6.1`** — the `collector` ref pin only; no lane, no
  port, no param of either composite moved.
- **`assistant@2.4.1`** — both occupant ref pins only.
- Docs: `templates/collector/README.md` gains § *A capped round is a partial
  answer*; `templates/cogny/README.md` § `max_iter` and `templates/talky/README.md`
  § *The reply lane carries three sorts* say what the exit now carries;
  `docs/config.md` / `.en.md` name the second hop key beside `round_capped`, and
  the two reply-edge examples in `docs/rewiring.md` / `.en.md` get **comments**
  naming it — their conditions are unchanged and were already right.
- **One repair beside the feature, in `templates/talky/README.md`.** From
  `talky@4.5.1` up to `4.6.0` that README recommended guarding the reply edge with
  `has(hop.round_capped) && hop.round_capped == '0' && !has(hop.degraded)`, on
  the stated ground that the assembler stamps `round_capped` on every answer. It
  does not. Both keys are written by the **seam**, and a real answer never comes
  through the seam: it arrives on `in_answer` and leaves carrying **neither**
  `round_capped` nor `partial`. So the recommended guard matched nothing, while
  the `!has(hop.round_capped) && !has(hop.degraded)` that the shipped composites
  actually wire — `crates/meclaw-cells/tests/talky_composite.rs`,
  `gh273_a_swept_close_reaches_the_memory.rs`, and both `docs/rewiring` versions
  — is the correct test. The README now states the measured truth and recommends
  that form (the released `[0.28.0]` entry below still shows the guard it took
  over from; that entry is the one this paragraph retracts).

#### A newborn's contracts are read below the root, and a successor's too (GH #567)

**A v-lane may end on a hive the same mutation instantiates** — that is what a grow
recipe is, one `add_nodes` and the level's edges beside it — and until the node is
staged the declaration it needs lives only in the template. GH #562 taught stage 6 to
read it there, out of ONE file: the template root's `config.json`. For a composite that
is the wrong file. The rim that pronounces the connect point is an occupant reached
through a `ref` marker — `talky@4.6.1` declares `credential_request` at `./brain`, the
`assistant` that holds it declares nothing — and a marker directory carries no contract
of its own, because the declaration lives in a different template altogether. So the
connect point of a newborn's nested rim was invisible, and a recipe that drew the lane
in the same breath earned `v_lane_no_connect_point` for a connect point that had been
declared all along.

The read is now the ref-aware walk the staging itself uses
(`hive_contract::contracts_from_template_subtree`, built on `subtree::parse_subtree`):
every hive of the staged subtree contributes, addressed at the newborn's path joined
with its path relative to the template root. The old single-file answer is the first
element of the new list rather than a case beside it.

**And a `swap_nodes` successor now reaches the same list.** A generation change is ONE
mutation (GH #256): `add_nodes` grows the successor, `swap_nodes` swings the old node's
edges onto it, and the level's own wiring — the v-lane included — is drawn beside both.
That wiring was refused for the same root-only reason, because a successor is a composite
too. The successor's contract WAS read a second time further down, for the re-anchor
verdict (`v_lane_reanchor_verdict`), and thrown away: two readers of one declaration, free
to disagree about it. That read is hoisted to where the birth contracts are built, feeds
them, and is handed on to the swap block — one reader for both questions.

What is deliberately unchanged: the birth contracts are **appended, never substituted**
— a path that already stands keeps the contract it was born with, so a diff cannot talk
a live hive into a connect point by naming a template — and they still reach the port
boundary and nothing else, which is the scoping the M1 review of #567's first half put
in place.

That invariant now asks the right question. It used to be enforced against the CONTRACT
list, and `collect_hive_contracts` only emits a hive that declared one — so a standing
hive declaring nothing was invisible to the guard. An `add_nodes` at an occupied path
whose cells are stopped is a legal **resume**: it passes the naming check, stages nothing
and rewrites no `config.json`. Naming a template with connect points on such a resume
therefore handed the port boundary a declaration for a path that stands without it, and
the v-lane onto a connect point that exists nowhere on disk **committed**. The guard is
now the colony's own hive table — what STANDS, not what declared — which is also exact
for a partial subtree resume: a nested hive the resume really does stage still brings its
birth contract. Pinned by `a_resume_cannot_re_declare_a_standing_hives_connect_point`.

Both language versions of `docs/meclaw-overview.md` and `docs/config.md` say
where a newborn's contract is read; ADR-0020 carries it as a consequence.

#### `builder@1.6.1`: a generation and its credential road are one act (GH #567)

The `grow_level` recipe rendered a wish carrying a `credential` block as **two**
declarations, and said so in its own comment: the level first, the four v-lanes
and their grants one declaration later. That was not a preference. The lane ends
on `<generation>/talky/brain`, two levels inside the node the first declaration
gives birth to, and the connect point that makes it legal is declared by
`talky`/`cogny` — so while a mutation read a newborn's contract from the template
**root** alone, the same four edges drawn beside the `add_nodes` that creates
their target's grandparent earned `v_lane_no_connect_point`.

Stage 6 reads the whole staged subtree now (see *A newborn's contracts are read
below the root* above), so the recipe renders **one** declaration: the node, the
level's own transit edges, the four credential v-lanes behind them, and the
grants in `seed_rows` of the same diff. One digest, one gate decision, one
`mutation_log` row for what the author always thought of as one act.

**It stands at the member**, one storey above the container an assistant
otherwise declares itself in (GH #503) — the wide form `subscribe` already takes
for the identity door, and for the same reason: an edge lives in the graph of the
lowest common ancestor of its endpoints, and a brain and the member's broker
share only that one. The child is therefore named `assistants/<generation>`. Ask
for both switches at once and it is still one declaration, one node, one scope,
with every edge of both roads behind the level's own.

The order argument the old comment carried retires with it. Two acts meant a
refusal on the second could leave a generation whose brains hold an empty
`api_key` and name a grant nobody wired — loudly, but standing. There is no
roll-forward between two acts any more.

`examples/organism/grow-credentials.json` is unchanged and stays the byte truth
of these four edges: applied on its own it wires a generation that already
**stands**, which is a legitimate operation of its own, and the six shipped
declarations of `grow.manifest.json` keep their order. The pin test compares the
rendered declaration's last four edges against that file byte for byte, and its
grants too — save the two timestamps the renderer stamps as it draws and the
event id, which the shipped file numbers the way a hand-written file does
(`gh466_grow_level_renders_the_level.rs`). One word of a grant event moved with
that comparison: the seeded reason now reads *seeded with the level*, which is
also what it is.

`gh567_the_credentialled_wish_is_one_act.rs` is the second opinion the rendered
string needs: the same declaration handed to a real mutation door on a colony
grown from the shipped templates comes back `Committed`, with the four v-lanes
carrying their lane names in the graph the colony itself publishes. The builder
scenario `I5` says the same over HTTP, end to end, and counts one mutation row
where it counted two. Third digit: the recipe repairs its own promise of *one
declaration, not two*.

#### `operator@1.1.0` / `meclaw-os@1.7.0` / `access@2.4.3`: the submitter moves into the front door (GH #556)

**One front, one place a submission lives.** `submit` stops being a hive of the
OS shell and becomes an occupant of `operator` — `/os/operator/submit`, gate and
store. Until now a submission crossed two shell stations, `./operator` and then
`./submit`, for what is one job, and the road was only readable by reading two
templates.

**ADR-0015's guardrail is precised rather than broken**, and the amendment of
2026-08-31 says which half of it was ever load-bearing: the protection is the
edge that is *missing between the drafter and the submitter*, and a missing edge
is missing at every address. After the move the two are not even siblings, and
the one edge they ever shared — the `registers_class` nudge of #504 — now runs
`./operator -> ./builder`, so builder and submitter share no edge in either
direction. `mutate` still reaches `/colony/mutations` over one edge that lives in
the birth topology and cannot be drawn by any mutation at any scope.

- **`operator@1.1.0`** — the `submit` ref moves in beside the cell that lends a
  submission its sender, which is called `intake` now (a ref inside a template is
  named after the template it references, `docs/development-rules.md` § 8a, R1).
  `apply` and `in_receipt` stop being rim lanes and become the interior edges
  `./intake -> ./submit` and `./submit -> ./intake`. Four lanes cross the rim in
  their place, and none of them is the front door's own: `ask` out and
  `in_verdict` back to the broker beside it, `mutate` out, and `sub_receipt` —
  the submitter's own receipt, let out only when it carries `hop.error_code` or
  `hop.registers_class`, because a submission answered twice on `receipt` is a
  submission whose caller cannot tell which answer is the outcome.
- **`meclaw-os@1.7.0`** — four refs where there were five, and 47 edges where
  there were 49. The ask, the grant, the mutate and both corpus nudges are drawn
  from `./operator` with the same guards; the two edges that carried a manifest
  out to `./submit` and a receipt back are gone from this level entirely. What
  the shell *declares* is unchanged — thirteen lanes in, fifteen out — because
  the union rule cancels exactly what moved inside.
- **`access@2.4.3`** — the four rows the submitter asks over name
  `/os/operator/submit` where they named `/os/submit`. R-AC-1 rules the node
  whose reach the rule is about: a row naming `/os/operator` would grant an
  export cell and a lifecycle composer the same thing. Re-pointing a shipped row
  at the node that moved grants nothing new, so it is the third digit.
- **`submit@2.3.0` is unchanged** — it is re-homed, not rewritten.
- **A running colony cannot adopt this by mutation.** The `mutate` lane lives in
  the birth topology, so it lands with the next instance build; running instances
  keep their grown form (no-delete).

#### `tools@1.4.0`: the build pair is named as a pair (GH #554)

The two occupants that carry a build were called `build` and `apply`, and the
tree said nothing about the fact that they are two halves of ONE round: one
fetches a draft, the other submits it. A reader of the graph had to know the
story before the two names looked related at all, and alphabetically they sat
apart, one between `bash` and `edit` and the other beside it by accident.

They are `build-draft/` and `build-apply/` now. **`draft` names what the first
half delivers** — the sentence *"This is a DRAFT"* is verbatim in its
`tool_result` — and the shared `build-` prefix is what makes the pair visible
where a reader actually meets it: in the nine edges of `params.graph` and in a
colony's registry listing, both of which now sort the two neighbours together.

**The tool names the model sees did not move**, and that is the whole boundary
of this change: `build_topology` and `apply_manifest` are the contract surface a
brain's `system.tools` names and a door dispatches on (`hop.tool_name`), and a
cell name is not a tool name. The `schemas` occupant's table, the two doors'
conditions and every caller's prompt are byte-identical across this version.

What moved is the hive's own address space, which is why it is the second digit:
a mutation or an `override_params` path that named `<assistant>/tools/build`
names `<assistant>/tools/build-draft` from 1.4.0 on. A generation already grown
from 1.3.0 keeps the names it was grown under — the template library is not on
the running path of a booted colony — and the pair lands with the next instance
built.

#### `tools@1.3.0`: a cell type is not a tool (GH #547)

`templates/tools` shipped eleven occupants and two of them were reachable from
nothing. `mcp/` and `vault/` stood in the hive as directories no edge touched;
the README printed the recipe a reader would have to apply to wire the `mcp`
themselves, and `requires.env` carried `MCP_ENDPOINT` and `TOOLS_VAULT_BROKER`
as keys "unwired until somebody answers this". A shipped template is a worked
example. An occupant nobody can reach is not a worked example: it is a
placeholder with documentation attached, and the documentation had grown longer
than the thing it documented.

Both directories are gone, and the sentence that carried them is **retracted**
in `templates/tools/README.md` rather than silently deleted
(`docs/development-rules.md` § 3):

> **A cell type is not a tool.** The tools hive holds the cells this agent's
> tool calls reach. Every shipped cell type does not belong here by virtue of
> being a cell type: a `vault` answers its broker and stands in the broker's
> hive, and an `mcp` bridges one named server and is wired when somebody names
> it. Neither is wired here any more, because an occupant nobody can reach
> teaches nothing.

`vault` was the sharper of the two, because it taught the opposite of the
arrangement the library already ships: a vault answers exactly ONE sender, its
broker, and `templates/access` stands one in the capability broker's own hive
beside the `invoke` cell that may spend it. `mcp` is the same shape for a
different reason — it bridges one named server, nobody had named it, and a
long-running cell wired at a loopback placeholder reconnects to nothing forever.

**Neither capability is lost.** An assistant that talks to a named MCP server
adds an `mcp` cell the way any tool is added: one occupant directory, two edges,
one row in the `schemas` cell's table, all three in one diff.
`workshop/corpus/13-mcp-lane/` is the worked example, and it is a whole colony
rather than a directory nobody reaches. An assistant that needs a vault gets one
where a vault belongs.

The declared blast radius moves with them and is narrower for it: `sandbox_union`
is a union over the occupants that actually answer, and `reentrancy` carries one
entry per reachable occupant. The gate that asserted the two islands is inverted
rather than deleted — `the_two_unwired_occupants_are_islands_by_construction`
becomes `the_tools_hive_has_no_unwired_occupant`, and it now fails on ANY
occupant directory no edge reaches, so the exemption cannot come back unnoticed.

#### `cogny@4.5.0`: one lane, one cell name (GH #548, first half)

Two hives in the library declared `in_schemas` and answered it with a cell doing
the same job under two different names — `templates/tools/schemas` and
`templates/cogny/declare`. The two were not merely similar: both `code`/`python3`,
both reading `body.tools`, both emitting the same
`{header, schemas[], unknown[], messages[]}` shape with the same `tools_missing`
/ `tool_unknown` branches and the same restricted sandbox.
`templates/cogny/declare/config.json` said so in its own header comment — *"The
lane pair and the answer shape are the tools hive's, byte for byte."* The scripts
differ in their `SCHEMAS` table and almost nowhere else.

The caller paid for the divergence: an `override_params` path, a mutation that
wants to add a schema, a reader following the declaration round of GH #464 —
each had to know which of the two words this particular hive chose, and the next
hive would have picked a third. `templates/cogny/declare` is now
`templates/cogny/schemas`. `schemas` is the survivor because it names what comes
back and it matches the lane (`in_schemas`). Nothing else moved: the lane pair,
the request body, the answer shape and the `required_drains` entry are what they
were, and the composite is still sealed, so no caller could ever name the cell.

**Still open in [#548](https://github.com/mmeyerlein/meclaw/issues/548):** whether
`memory-hive` gets a declaration lane of its own. It has no `schemas` cell and no
`in_schemas` lane, so the two tool names an agent uses to reach a memory are
declared in the collector instead, and `memory_recall`'s schema is a hand-typed
projection of a contract living one template away — three arguments offered where
the contract wants seven promoted context keys. That is a contract change on a
shipped hive plus a design question about who owns the name, and it gets its own
round.

Both bumps carry their pins: `templates/assistant`'s two ref markers and the
derivation notes of its lane contract name the new versions, and
`templates/README.md`'s catalogue rows move with them.

#### `builder@1.5.2` + `meclaw-os@1.6.1`: a ref is named after its template (GH #545)

An instance is named after the template it is an instance of, and a `ref` onto
template `X` is a directory called `X`. Two occupants of the baumeister were
named after the ROLE they play instead: `dispatch`, a ref on `dispatcher@1.1.2`,
and `librarian`, a ref on `builder-librarian@2.1.1`. Nineteen of the library's
twenty-two ref markers already carried their template's name --
`templates/cogny/dispatcher/` and `templates/talky/dispatcher/` among them --
which made `dispatch` a lone second spelling of a word that is spelled correctly
twice elsewhere. Where the two names differ, the tree says one thing and the
address says another, and every reader has to learn the translation before they
can follow an edge.

**`builder@1.5.2`** -- the two directories are `dispatcher` and
`builder-librarian`, and the eleven edges that named them move with them. No
lane, no guard, no cell and no prose about behaviour changed: this is the tree
spelling what the address already spells. A mutation that named
`<builder>/dispatch` or `<builder>/librarian` in an `override_params` path stops
resolving, and that is the whole of the migration -- the level's own address
space is what moved, which is why a rename is a version event at all.

**`meclaw-os@1.6.1`** -- the baumeister ref re-pinned to `builder@1.5.2`.
Nothing else at that level moved.

The third and expensive one, the assistant's conversation surface, is the same
rule on a level whose name travels through twenty test files; it lands
separately.

#### `assistant@2.4.0`: the identity pack rides a v-lane (GH #561)

`in_pack` no longer travels as a per-level chain. The push leaves a member's own `affinity`
and ends directly at the two brain rims of the generation it is meant for --
`<generation>/talky` and `<generation>/cogny` -- as two v-lanes carrying `"lane": "in_pack"`,
and each receipt rides one back on `"lane": "pack_ack"`. `assistant@2.4.0` stops carrying the
lane and starts vouching for it: the four pass-through edges are gone (thirty-seven edges
become thirty-three) and what remains in the contract is the connect point,
`"at": ["./talky", "./cogny"]`, on both lanes. A level that declares a lane has said it takes
part in it, and this one stamped nothing, filtered nothing and guarded nothing (GH #559 rule 2,
ADR-0020). The fan-out is unchanged in everything that matters -- one subscription row naming
the generation, one pack, two durable writes, two receipts -- it is simply two edges the sender
draws rather than two the level draws, and the pairing that obliges the receipt is now read at
the rim, where `talky` and `cogny` declare it themselves. `grow_level`'s opt-in identity door
renders those four edges, and the submitter's form check accepts a door that ends inside a node
the same declaration creates, not only at the node itself. Bumping the level is the second digit
because an address moved: a caller that wired `in_pack` at the level's own path redraws the edge
at the two rims the corridor names.

#### `assistant@2.4.0` / `member@1.5.0`: the recall road rides a v-lane, and the member still stamps (GH #562)

A brain's `memory_recall` used to cross three levels to reach the person's memory: the
generation's own rim, the `assistants` container, and then the member's door — and only the
last of the three did anything, turning `recall` into `in_query` and stamping `recall_as_of`,
`audience_now` and `channel`, the keys the hive refuses a question without. The first two are
what ADR-0020 built the v-lane for. The assistant stops forwarding the lane and starts
**vouching** for it: `recall` and `in_bundle` keep their contract entries and gain
`at: ["./talky", "./cogny"]`, the four pass-through edges are struck, and the level goes from
thirty-three edges to twenty-nine — the same subtraction GH #561 made one lane earlier, on the
same two occupants. What delivers instead is one deep edge per asker per direction, drawn by
the mutation that instantiates the generation and naming the lane it carries; the reply-to
token, the core's guarded door and the surface's default all move up one level unchanged. The
member declares the same two lanes with `at: ["./assistants"]` and no connect point below it:
under ADR-0020 that makes the person's level a **mandatory hop**, so a v-lane that would carry
a generation's recall past the door that stamps the round is refused with
`v_lane_mandatory_hop` at mutation time instead of arriving as `missing_audience` at runtime.
The member's stamping edges themselves are untouched, and the road is one hop shorter in both
directions. Both templates carry the change in their unreleased number, the same reading
`docs/development-rules.md` § 4 gave #454, #459 and #471.

Three substrate rules follow the declaration, all three the same sentence read from a
different side. A lane that names connect points docks BELOW the rim, so its hive owes it no
door out of its own path (`hive_contract::check_lane_doors` skips it) and the level above does
not carry it in the union of its occupants' lanes. The rim is **closed** for that lane in the
other direction too: an `add_edges` entry that delivers it AT the hive path is refused
`hive_contract` and told which connect point to end on — without that half the declaration
would wave through exactly the edge whose door the migration struck, and the delivery would
become a dead letter nobody is told about. And a v-lane may now end on a hive the same
mutation gives birth to — every grow recipe draws a generation's edges beside its `add_nodes`,
and the target's contract is read from the template directory until the node is staged,
exactly as a `swap_nodes` reads its successor's.

### Added

#### The hand-typed `memory_recall` schema is pinned to the hive's contract (GH #552, interim)

`memory_recall` is served by the collector out of its own recall port (GH #512),
and the schema the model sees is typed by hand in `self_tool_menu()` — a manual
projection of `memory-hive`'s `in_query` contract, with nothing holding the two
together. That missing pin is the drift GH #464 was opened for.

`crates/meclaw-cells/tests/gh552_the_recall_schema_is_pinned_to_the_contract.rs`
is that pin until the rebuild lands. It carries the mapping table in its own
header and measures both halves rather than asserting them: the three parameters
the MODEL fills (`query`, `window_from`, `window_to`) must reach the recall port
as `hop.recall_query` / `hop.recall_window_from` / `hop.recall_window_to`, driven
through the shipped assembler on a real `in_memory_call`; the four the model is
deliberately NOT asked for must be filled beside it — `memory_tier` off the
collector's own `memory_call_tier` knob, and `recall_as_of`, `audience_now` and
`channel` off every `in_query` edge the `member` template draws. A key that is
neither is a recall arriving empty, and on `audience_now` / `channel` empty is a
refusal (`missing_audience`, `missing_channel`), not a wider answer. The second
hand-typed copy is pinned too: `talky`'s brain seed row, what a fresh agent's
`system.tools` holds before the first menu tick, is compared against the same
declaration.

**No version digit.** The schema itself is unchanged — no parameter the recall
path evaluates is missing from it — and the collector gains one doc comment. One
thing the pin records rather than repairs: the two window bounds are declared as
independently optional while the recall cell refuses a half-open window
(`half_open_window`), so a model that names one bound buys a refusal it cannot
see coming. The sentence belongs in the hive that enforces it, and GH #552 is
where it will be written once.

#### `scripts/check_tree_rules.py`: the tree's naming and wiring rules are a gate (GH #550)

Three rules about the shape of the template tree were enforced by reading the
tree, which means they were enforced when somebody remembered to look. A sweep
of all 42 template roots and every example found that each of them had a live
violation, and that every one of those violations had entered the tree past a
green gate.

The gate runs from `scripts/strand-gate.sh` in the *catalogue / versions /
anchors / claims* step, beside the roadmap, ADR and claims gates. It reads
`templates/` and `examples/` only — no network, no cargo, no travelling copy —
so it runs unchanged in the private tree and in the published one.

- **R1 — a ref is named after its template.** A `config.json` with
  `cell.type: "ref"` inside a template sits in a directory named after the
  template it references. The rule stops at the tree's edge, and the boundary is
  the point: a `ref` marker inside a template is named after the template it
  references, an instance grown by a manifest is named by whoever grows it. An
  example that grows an `org` called `acme` with a `member` called `alex` is
  teaching exactly that, so `examples/` is reported and never failed.
- **R3 — no unwired cell in a shipped template.** Every cell directory appears
  as `from` or `to` of at least one edge in that template's
  `params.graph.edges[]`. A shipped template is a worked example, and an
  occupant nobody can reach is a placeholder with documentation attached. One
  that is deliberately unreachable declares `"unwired": true` in its **own**
  `config.json`: the declaration is visible in the diff that adds it, a list of
  exempt paths inside the checker is visible to nobody.
- **R4 — one lane, one cell name.** A small table maps a lane to the name of the
  cell that answers it (`in_schemas` → `schemas`). It grows by ruling and not by
  inference — a checker that guessed which cells do the same thing would be
  wrong more often than the tree is — so the name families nobody has ruled on
  are warnings rather than failures.

`--selftest` is the gate's own pin: a fixture tree with one violation of each
rule plus a declared island and a clean template, each rule required to fire
exactly once and both controls to stay silent. Violations a strand is fixing
right now stand in a dated `TRANSITIONAL` table with their issue numbers, and
the gate names a row that no longer matches anything rather than letting the
exemption silt up.

#### v-lanes: an edge may declare the lane it carries (GH #559)

- **A deep edge may name its lane.** A deep edge — one that ends inside a hive
  rather than on its rim — can now name its lane (`add_edges[].lane`), and a hive
  can name where that lane docks (`params.contract.accepts[].at` /
  `.emits[].at`). Stage 6 of the mutation validation reads the two together: a
  crossed level that declares the lane and permits the endpoint waives its port
  boundary for that one edge, a level that declares the lane and permits nothing
  may not be skipped (`v_lane_mandatory_hop`), and the target hive owes a connect
  point (`v_lane_no_connect_point`). A `swap_nodes` re-anchors a v-lane that ends
  inside the subtree it replaces onto the successor by relative form, or refuses
  the whole swap (`v_lane_unanchored`) — it is never silently dropped. An edge
  without `lane` behaves exactly as before, and the bootstrap enforces none of
  this, as it enforces no port boundary. `/colony/graph` reports an edge's lane;
  `colony.db` schema v9 adds the nullable `edges.lane` column.
- **Tree gate: R5, a v-lane is a declared deep edge.**
  `scripts/check_tree_rules.py` now runs the static half of the v-lane rule
  table: an edge whose endpoint is more than one segment deep must name its
  `lane`, and one level between the ancestor and the endpoint must declare the
  connect point with `at`. A skipped level that declares the lane without a
  matching `at` is a `v_lane_mandatory_hop`, an unopened seal keeps its
  `hive_port_boundary`, and a walk that reaches the endpoint without a
  declaration is a `v_lane_no_connect_point` — all failures, not warnings. `ref`
  levels are resolved into `templates/<name>/`; a level this tree does not hold
  is a note and left to Stage 6.
- **A v-lane is visible where the topology is read.** `/colony/graph` answers with
  the lane an edge was declared on — the reply body itself, not only the
  DTO behind it, now carries `lane` on a declared v-lane and omits the key
  entirely on an ordinary edge, so "this edge declares no lane" and "this server
  does not report lanes" stay distinguishable. `colony-view` draws it: the layout
  hands the lane down to `colony-view-edge` as the `vlane` prop, and the browser
  half renders such an edge dashed with the lane's name in its tooltip. Rendering
  only — the component keeps `editable: []`, nothing is written back, and an edge
  without a lane looks exactly as it did before.

#### v-lanes: the rule set, and ADR-0020 (GH #559)

- **The spec describes a v-lane now.** `docs/meclaw-overview.md` § *v-lanes*
  (both language versions) defines it as what it is — an ordinary deep edge in
  the graph of the two endpoints' lowest common ancestor, which names its lane
  — and not as a new kind of routing: delivery was always flat, and what was
  missing was the declaration. The section carries the validation rule table
  in full, names the three contract-surface error codes
  `v_lane_no_connect_point`, `v_lane_mandatory_hop` and `v_lane_unanchored`,
  and states that an edge without a `lane` field keeps exactly today's
  behaviour. Two sentences are there to stop a wrong reading: the rim of a
  skipped level no longer describes all of its occupants' traffic — deliberately
  — and a v-lane changes nothing about secrets, which still travel as a sealed
  box over ordinary edges, pulled per request against an ephemeral recipient key
  and never pushed.
- **`docs/config.md`** documents both halves of the declaration: `lane` beside
  `condition`/`modifier`/`default` on an edge, and `at` beside
  `route`/`context`/`because` on a hive contract's `accepts`/`emits`, with a
  worked example — a contract that opens `in_pack` at `./talky` and `./cogny`
  and at nothing else. `ports: []` stays literally true: the one exception is
  pronounced by the target template, never taken by the caller.
- **`docs/development-rules.md` § 8b** records the one sanctioned exception to
  the union rule, and the duty that comes with it: per lane migrated to a
  v-lane, the chain **and** its pass-through declarations are struck in the same
  change, because a declaration that merely forwarded traffic would now read as
  a claim to influence it and refuse the very edge replacing it.
- **ADR-0020 — *a v-lane is a contract-declared corridor, not a new kind of
  edge*.** It records the three rejected alternatives with their reasons: a deep
  port (a port with a path in it is an interior cell name in a different field),
  an LCA override (the level that grants the reach is not the level that knows
  what is inside, and the grant would be invisible in the target's contract),
  and an owner field on the edge (a second bookkeeping of what the edge table
  already carries in its paths).

#### `member@1.5.0` / `talky@4.6.0` / `cogny@4.6.0` / `assistant@2.4.0`: a member carries its own broker, and its brains ask it over a v-lane (GH #560)

Authorisation and key ownership stopped sharing one hive. `member@1.5.0` grows an
`access` occupant of its own — the shell's broker keeps answering the submitter's
policy questions and keeps its vault for OS-level keys, while the provider
credentials a person's agents burn now live with the person, which is the same
argument that put the memory hive at that level. The two edges that carry a
credential are **v-lanes** (GH #559), drawn by the manifest that grows a
generation: from the brain that spends the grant straight to `./access`, past
three levels, one of which is sealed. `talky@4.6.0` and `cogny@4.6.0` are what
make that legal — they declare both halves of the lane in their own contract and
name `./brain` as its connect point (`emits.credential_request` and
`accepts.in_sealed`, both carrying `at`), so `params.ports: []` stays literally
true and the opening is one the template pronounces about itself. The two levels
in between declare nothing about the lane and are therefore transparent.
`assistant@2.4.0` carries the two pins. Both brain templates ship
`params.credential_grant_id` resolving to the empty string, which is no grant at
all: a generation nobody wires this way behaves exactly as before and spends its
`api_key`. `examples/organism/grow-credentials.json` is the runnable form — a
sixth declaration that instantiates nothing, draws four v-lanes and seeds two
grants through the mutation door — and `templates/member/README.md` § *The
credential v-lanes* is the recipe, including the two gestures that are not
topology (the vault's passphrase at birth, and the credential itself from stdin
with no colony running). Measured end to end in
`crates/meclaw-cells/tests/gh560_a_members_brain_gets_its_sealed_key.rs`: one
turn, answered, with the bearer the member's vault held — the message log
carries the ciphertext and never the value.

#### `builder@1.6.0`: a generation can be grown with no key of its own (GH #560)

The credential v-lanes stopped being something a person writes by hand. `grow_level`
takes a `credential` block on an assistant — the second opt-in of that level after the
identity door — and renders the form `templates/member/README.md` § *The credential
v-lanes* publishes: two edges per brain, each naming the lane it carries, from the rim
that spends the key straight to the member's own `access`; the grants that answer them,
through `seed_rows` and therefore through the mutation door that keeps a row; and, on
both brains, a grant handle **and** an empty `api_key`, which is the switch and not
tidiness — a brain asks for a credential only while it holds none, and both params are
immutable, so both are set where the generation is grown or the repair is a new
generation. The four rendered edges are compared byte for byte against
`examples/organism/grow-credentials.json`, the same discipline the six level edge sets
run under.

**It renders a SECOND declaration, and that is a substrate fact rather than a
preference.** *(Superseded later in this same unreleased wave: since
`builder@1.6.1` and GH #567 the credential road rides in the level's own
declaration — see that paragraph.)* The lane ends on
`<generation>/talky/brain`, two levels inside the node the first declaration
gives birth to, and the connect point that makes it legal is
declared by `talky`/`cogny` rather than by the generation — while a mutation reads the
contract of a hive it gives birth to from the template ROOT only (GH #562). Drawn in one
breath the same four edges are refused `v_lane_no_connect_point`; drawn one declaration
later the level stands and its contracts are read off the disk. Order is semantics, as
everywhere else: a refusal on the second declaration leaves a generation whose brains
hold an empty key and name a grant nobody wired, and that fails LOUDLY — the first turn
emits `credential_request` onto no edge and dead letters — which is what makes two acts
acceptable where seeding the grants first would leave permission rows for a generation
that may never exist. Measured end to end in
`workshop/evals/builder-scenarios/cases/I5-a-generation-grows-with-no-key-of-its-own.json`:
a wish with no model reachable at all comes back as a draft — two declarations at
this version; the case file reads ONE since `builder@1.6.1` — is quoted back
verbatim through the front door, and the four deep edges are read out of the
graph the colony itself publishes.

**The second digit, and not the third** (ruling of 2026-09-01). The recipe draws a
credential lane a caller could not draw before, which is one capability more at the
composer's front door rather than a repair of one that was there — the same reading
`member@1.5.0` and `talky@4.6.0`/`cogny@4.6.0` got in this very wave. A caller that
names no `credential` block gets byte-identical output from 1.6.0, so nothing has to
move for the bump; what moved is what may be asked for.

One comment repair rides in the same number (GH #558, second half): the cell a grown
door is attributed to is `/os/operator/intake` since the submitter moved into the front
door (#556), where the recipe still said `/os/operator/submit`.


### Fixed

#### `llm`: provider citation markers never reach a person (GH #569)

A live turn ended in `… Am Abend kann es regnen. citeturn0search0` — an
OpenAI-style inline citation marker: the Private-Use-Area codepoints `U+E200`,
`U+E202` and `U+E201` wrapping `cite` and `turn0search0`, a reference to the
provider's own tool-round numbering. The `llm` cell passed the answer text
through verbatim, so a chat surface showed the PUA characters as boxes or
nothing and the enclosed tokens as literal junk.

The response translation now strips these spans before the text enters the
assistant turn, in **both** wire dialects: a span from `U+E200` up to and
including the next `U+E201` falls with its content, a `U+E200` whose closing
codepoint never arrives (a truncated response) takes the rest of the text with
it — behind an opened marker there is marker content only — and any other
Private-Use-Area codepoint falls on its own. Everything outside a marker is
returned byte-identical: no trimming, no whitespace normalisation, so the space
in front of a dropped marker stays. In the Responses dialect the stripping runs
**after** the `output_text` parts are joined, so a marker split across two parts
falls as well. Crate-internal change, no template and no wire-request change.

#### `member@1.5.1`: the export markers land whole (GH #563)

`templates/member/export-sink` has filed every table part through a neighbour
file and a rename since it was written, and wrote both of its completeness
markers — `<hive>/seed/export_final.json` and the member-level
`export_final.json` — with a plain `open(path, "w")`. That truncates the name a
reader watches to zero bytes before it is filled, so a reader arriving in the
gap gets an empty file where a JSON document is promised. **Measured rather
than suspected:** with a sleep inserted between the truncate and the write, a
reader of the member-level marker got `EOF while parsing a value, line 1,
column 0` every time — the panic that turned CI run 33494173120 red. Both
markers now go through the same `write_whole()` helper the parts use, and so do
the parts: one rule, one place in the script to read it.

**The quieter half is the marker's own honesty.** `hive_marker()` swallowed
`ValueError` alongside `OSError` and answered `None` for both, so a peer hive's
marker caught mid-write dropped out of `hives[]` without a word and the
member-level document claimed an incompleteness the directory did not have.
They are different facts and are told apart now, and the line is drawn where it
belongs: **exactly one thing means "that hive has not finished" — the marker is
not there** (`FileNotFoundError`). The hives of a member walk on their own
clock, and an absent marker is that ordinary state.

Everything else is a marker the cell could not read, and none of it says
anything about whether the hive finished: a directory standing where the marker
belongs, a permission the export process does not have, an I/O error on the
block, or a file that is there and is not a JSON object (which, after the
rename, can no longer be a write caught halfway). Each of those names its
directory in a new `unreadable` array on the member-level marker, beside a line
on `stderr`; `hives[]` stays the readable ones. No new `error_code`: the sink is
a transitional cell that GH #555 dissolves, and a refusal that belongs to one
document is named *in* that document.

The bump is the third digit because both halves repair a promise the marker
already made — *"the list is the honest answer at every point in between"*, in
the script's own words. The shipped contract, the lanes and the sandbox
boundary are unchanged.

#### The `web` shell says when the socket is gone — on every page (no template change)

The vendored LiveView client has always published its connection state as
classes on the `data-phx-main` container the shell writes: `phx-loading`,
`phx-error`, `phx-client-error`, `phx-server-error`, set after a short delay so
a blink flashes nothing. Exactly one place in the tree ever read them —
`templates/colony-view`'s own stylesheet, which travels inside that view's
markup. Every other page of every display therefore kept drawing its last
picture for as long as the tab stayed open, and a picture drawn a minute ago and
a live one are pixel-identical.

The shell's `<head>` now carries a handful of CSS lines for those four states:
one fixed banner, *connection lost — this page may be out of date*, on the
container the shell emits itself. This does not retract "the cell type does not
decide what a display looks like" (`templates/web/README.md`, § The Vision token
sheet): the shell still links no stylesheet and still says nothing about the
page inside the container. What it styles is its own element in the states of
the runtime it itself ships — a runtime that publishes a state nothing can see
is half delivered.

A page overrides the default by declaring the same rules. The block is in
`<head>`, a page's own `<style>` arrives in the body, and at equal specificity
the later rule wins; `colony-view` keeps its own banner, and there is never a
second one, because `::after` is one box per element. The default deliberately
does **not** dim: `opacity` on the container would fade the banner with it, and
`opacity` on the container's children multiplies with a dim the page already
applies further down (`colony-view` halves its picture's opacity — half of a
half is unreadable). No template changed, so no template version moved.

#### `member@1.4.0` (docs only): a screen costs two edges, not three

The member README and the template's own description both described a third
instantiating edge for a display channel — `./channels/display-<s> ->
./channels` on `has(hop.error_code)`, re-stamped to `error` — by analogy with a
chat channel. A connector earns that edge because it emits a failure of its own;
a display does not. Its contract declares exactly two emissions, `event` and
`receipt`, its graph carries exactly two edges out of the hive, and a receipt's
`error_code` travels in the body rather than on the hop, so nothing a screen
puts on the wire could ever match such an edge. No shipped wiring changed — the
description did. The third edge in `gh459_a_screen_is_a_member_channel.rs` stays
as the test double's witness lane and is now marked as such.

#### `examples/vault-pilot` (docs only): the pilot is scriptable as written (GH #557)

Two gaps found by running the example end to end. **Step 3 could not be scripted
as printed**: the credential goes in on stdin, so the passphrase cannot, and with
the default `--vault-key-source auto` and no terminal the CLI asks for a prompt it
has nowhere to put. The step now carries the unattended form beside the
interactive one — `--vault-key-source plainfile --vault-key-file <PATH>`, with the
`0600`-and-non-empty rule the reader will otherwise meet as a refusal, and
`systemd-cred` named as the third source.

**And the README never showed a turn on the wire.** It draws `you → /brain` in the
flow picture and left the reader to guess the body, and the shape most readers try
first — `{"role": "user"}` — is refused with `422 invalid_ubf_body` at the HTTP
boundary, before any cell sees it. A fifth gesture now prints the `curl`, in the
UBF turn form the substrate actually validates (`origin` and `type` required,
`role` unknown), and says why `/brain` is the address on the wire for a cell that
lives at `main/brain` on disk. No behaviour changed; nothing shipped moved.

#### `colony-view@1.0.1` + `display@1.0.1`: the picture is the layout's again, and a pin is a marker (GH #544)

`colony-view@1.0.0` drew one rectangle per hive and, on any colony that had been
running for a while, those rectangles stopped being frames: they covered each
other, they covered cells belonging to other hives, and they ran two orders of
magnitude larger than the boxes they held. Measured on a live colony of 104
cells over 26 hives, read out of the display's own object store:

```text
overlapping unrelated hive pairs   208 / 215
worst frame / cell area            299x   (a hive of three cells)
(hive, foreign cell) intersections 1133
boxes standing where the flow put them  1 / 104
```

The picture called those 103 boxes *hand placed*. Nobody had touched them — and
nobody could have, because the `data-oid` every box carried named an object one
tree level away from the one the display mints, so a drag wrote to an id that
does not exist. Two things were wrong under it, and both are identity:

- **`keep` was attached to a slot.** An object id was the child index chain, and
  a picture writes its hives, then its edges, then its cells — so one edge more
  shifted every box's index by one, and the props the view had asked to keep
  were handed to whichever cell inherited the slot. Three cells held two objects
  each. Every box therefore wore the coordinate the flow had computed, at some
  earlier tick, for somebody else: a collage of a dozen incompatible layouts.
- **A coordinate was read as a pin.** `canvy@2.1.8` had already replaced that
  with a marker of its own; the re-cut of GH #455 lost the marker and kept
  `x`/`y` instead, so a box froze at the tick its object happened to be created
  and the layout was consulted for nothing that already existed.

**`display@1.0.1`** — a component-tree node may declare its own `key`, and the
object is then named `<parent>/<key>` rather than `<parent>/<index>` while `ord`
still comes from the index. An index is a slot; a `keep` prop has to follow the
thing. A view that names no keys is unchanged, and both shipped view kinds name
none.

**`colony-view@1.0.1`** — the flow owns `x`/`y` and rewrites them on every tick,
so the flow's guarantees are what a viewer sees. A hand owns `hand` — one prop,
spelled `"dx,dy"` — and a `pinned` marker beside it, and those two are what `keep`
covers. One prop because a browser writes a prop at a time and the display diffs
a prop at a time: as two props, one drag reached the page as two pictures and the
frames were derived once from the half-moved one — three rectangles painted for a
single drag, the middle 971 wide and still 92 high. What the browser wrote also
stands until a diff says it, because the first diff after a drag can still carry
the value from before it. The offset
is against the cell's **own** spot, so it travels with its hive instead of being
left behind by the next re-rank — which is what GH #170 removed, and why this is
not that: the delta is per box, never against a hive anchor. Each box carries
the frame it grows: a hive's rectangle is derived from the boxes it holds, and
the browser — the only half that can see where a hand put them — writes the
derived rectangle back, so the store says what the screen shows instead of the
third, wrong geometry the issue measured. The detail panel says which of the two
placed a box, how far it is from where the layout wanted it, and offers the way
back.

A bound was tried first and withdrawn: trimming an offset to its own hive's
frame made every count hold for any arrangement, and measured on a real screen
as 85 x 15 pixels of travel before a wall, which reads as a broken gesture. What
is no longer promised is written down rather than hoped away — an arrangement
can put two frames over each other, because somebody asked for it; the durable
answer is the app remembering the arrangement so the flow can pack around it,
which is a build and not a repair.

Two defects found in a browser while proving it, both shipped fixed. A drag that
wrote nothing left the box with the single provisional translate, after which the
client read the absolute position as the flow's and the offset as zero — one
blocked drag made a box permanently unmovable. And a tab open across a template
change keeps running the browser half it loaded with, because a `<script>`
arriving inside a LiveView morph is not executed; the shell now carries a digest
of the client that wrote the picture, and a stale tab says so instead of quietly
refusing every gesture.

Two gestures were wrong under the same heading and go with it. A frame with no
parent frame is the canvas, not a group: on a colony that is 96 % empty space
almost every press that misses a box lands inside the outermost frame and inside
nothing else, and one such press dragged all 108 cells of a live colony and
marked every one of them hand-placed. A root frame now pans. And a group drag no
longer writes for a group that did not move: one delta is applied to every
member, and when that delta rounds to nothing nobody is marked hand-placed and
nothing is written at all.

**The default picture also stops drawing the cells that take part in no edge at
all.** `remove_nodes` and `swap_nodes` disconnect and never delete, so a live
colony collects leftovers -- 31 of 123 cells on the one this was built against,
13 of them in a single hive -- and a frame drawn around boxes nobody is looking
at is a frame around nothing. They are hidden by default and the legend carries
a `<n> unwired` button that brings them back; the hive frames and the `viewBox`
are computed over the cells the picture shows, and flipping the toggle re-derives
the FRAMES in the browser, from whatever is left visible. The `viewBox` is
deliberately not re-derived with them: a session keeps the frame it mounted with,
so revealing the leftovers never re-centres the picture under the person looking
at it. The toggle is a class on the container: no round
trip, no stored preference, the same local fact as where you are looking.

Pinned by `crates/meclaw-cells/tests/gh544_the_flow_reaches_the_screen.rs`, which
runs the five counts of the issue over a colony nested five levels deep, once
untouched and once with every box arranged hard against a corner of the canvas,
and mints the ids with the display's own `add_tree` rather than a copy of it.

#### `assistant@2.3.0`: the conversation surface is called after its template (GH #545)

`talky` has been called `talky` since it shipped, and the name travels: it is in
test names, in scenario prose, in the guide's own topology tables. Since
[#454](https://github.com/mmeyerlein/meclaw/issues/454) the assistant level
holds exactly one of them -- and it held it under a ROLE name, so a reader of
`templates/assistant/` saw `./surface`, `./cogny` and `./tools`, two of the
three named after their templates and one not.

The node is `./talky`. **Twenty-six of that level's thirty-seven edges** carry
the name, and two stamped tokens are renamed with it, because a discriminator
that outlives the node it is named after is the one word in a file that has to
be read historically:

* `context.recall_caller` reads `'talky'` where it read `'surface'`
  ([#532](https://github.com/mmeyerlein/meclaw/issues/532)). Nothing outside the
  level compares against the value -- `member`, `memory-hive` and `org` carry it
  through and test only `== 'outside'` -- so neither of those three moves and
  neither takes a version.
* `context.tool_caller` likewise ([#464](https://github.com/mmeyerlein/meclaw/issues/464),
  [#529](https://github.com/mmeyerlein/meclaw/issues/529)); the return edges test
  `!= 'cogny'` and never resolve the other value against the tree. The README
  already ruled that this token is renamed with the node it names -- it said
  `'channels'` until 2.0.0 -- so this is that rule applied, not a new one.

**`ctx.model_surface` is deliberately NOT renamed.** It names the ROLE the model
plays for the level, which is what a level's own ctx key should name: a `cogny`
has a brain too, and `model_talky` would say nothing about why the key exists.
It is also a public contract surface of its own -- `grow-assistant.json`,
`grow.manifest.json`, the librarian corpus and the builder's README all carry
it -- and renaming a ref is no reason to move a key.

**Second digit, and the migration is one line.** An `override_params` path or a
mutation that named `<assistant>/surface/...` names `<assistant>/talky/...` from
2.3.0 on; the level's own address space is what moved. A generation already
grown from `2.2.0` keeps the node name it was born with -- the template library
is not on the runtime path of a booted colony, and a grown level is renamed, if
at all, by a mutation.

`templates/cogny/template.json` carries one corrected sentence with it: its
DECLARATION PORT paragraph described the pair as `./surface -> ./cogny`. Prose
about the caller, no version event of its own.

#### A lane cannot vanish inside one diff either (GH #564)

Stage 6 refused a lane disagreement between an `add_edges` entry and a **standing**
edge, and that check compares against the pre-state only — so two entries of the
**same** diff never met. The apply arm did meet them: it deduplicates each candidate
against the **growing** edge table, so `add_edges: [{…, "lane": "a"}, {…, "lane": "b"}]`
with five equal routing terms inserted `a`, found `b` content-equal, skipped it, and
reported `Committed` for a lane that was never laid.

Both entries are now compared against each other, pre-destructively, before anything
is applied: same `edge_schema`, same stage, and the refusal names **both** entry
indices and **both** lanes, because a caller cannot see from the outside which two of
their lines collapsed into one. Two entries declaring the **same** lane stay idempotent
— a re-applied complete diff must remain a no-op.

**The ruling behind it:** `lane` does **not** become a sixth identity term. Two edges
differing only in the lane name would both be routed, and a double delivery is a worse
answer than a refusal. The second face named in the issue — a template-internal edge at
instantiation — stays a documented comment at the site: an instantiation lays edges onto
paths that did not exist a moment ago, so there is no standing edge to disagree with.

#### Documentation debts of the v-lanes wave (GH #564)

- **`lane` is not an identity term, and a lane deviation is still refused.**
  `docs/meclaw-overview.md` and its English twin now say both halves where edge identity
  is defined: two edges differing only in the lane name are one edge as far as
  deduplication goes, which is exactly why a second `add_edges` entry with the same
  `from`/`to`/`condition`/`modifier`/`default` and a different `lane` is refused rather
  than swallowed.
- **The rule table reads `accepts` and `emits` undirected**, deliberately: a level that
  carries the lane in its contract takes part in it, and direction is a statement about
  traffic rather than about jurisdiction. The price — a level cannot be a mandatory hop
  in only one direction — is named in the overview and in ADR-0020.
- **The mandatory hop does not protect INSIDE the level that draws.** The levels judged
  are the ones strictly between the two endpoints' lowest common ancestor and the
  endpoint, so an edge a level draws in its own graph passes every gate even where that
  level declares the lane. That is the level's sovereignty and not a hole, and it is now
  written down as a consequence rather than discovered by the next reader.
- **The rim closure is asymmetric**, because the danger is: an edge delivering an `at`
  lane at the hive path is refused, while an `at` lane out of the rim has no sender there
  and never fires.
- `assistant@2.4.0`'s contract prose counts its ten inbound and ten outbound lanes and
  tells rim lanes from corridor lanes, where it still said seven and eight.
- The three v-lane error codes are pinned in the spec-claims registry.

#### An `at` entry is always a path below the declaring hive (GH #567)

`at: ["."]` was documented as *"the lane ends at my own rim"* and matches nothing.
Stage 6 compares an `at` entry against the endpoint's path relative to the level, and
that form is always `./…` and never `.` — the connect point is owed by the endpoint's
PARENT hive, one level lower than the spelling suggested. A contract written the
documented way therefore bought the opposite of what it asked for: a
`v_lane_no_connect_point` for the lane, plus a raised claim to a rim door
(`docks_below_the_rim` reads only "some `at` is declared"), which is a refused rim
delivery with no corridor to take its place. Five places say the rule now — both
language versions of `docs/meclaw-overview.md` and `docs/config.md`, and ADR-0020: an
`at` entry is a `./…` path genuinely below the declaring hive, and a lane meant to end
at some hive's rim is declared one level HIGHER, as `./<hive>`.

The gate stopped normalising the mistake away. `scripts/check_tree_rules.py` no longer
folds `"."` and bare names into a valid relative path; either spelling is an R5 finding
that names the offending entry, in a new edge-independent half (`malformed_connect_points`)
— a contract can be wrong on its own, without an edge to prove it. With the endpoint's
`wanted = "."` gone, R5 also stops treating anchoring as walk-global: exactly one level
owes the connect point, the endpoint's parent, which is what Stage 6 always meant.
`--selftest` grows four fixtures for it, and `templates/` holds at zero R5 findings —
no shipped template ever used `"."` or a bare name.

Two smaller repairs ride along. The newborn contracts an `add_nodes` stages are no longer
appended to `hive_contracts` for every reader: they travel in a list of their own, handed
to the one call that is about the port boundary, because the other three readers
(`collect_inbound_lanes`, `lane_requirements`, `collect_lane_doors`) judge what STANDS
rather than what the diff draws — and `collect_lane_doors`'s own comment, *"a hive added by
THIS diff has no contract to break yet"*, had quietly become false. What is deliberately
NOT repaired is filed instead:
[#567](https://github.com/mmeyerlein/meclaw/issues/567) records that a newborn hive's
contract is read at the template ROOT only — not in a nested occupant hive, and not on
`swap_nodes.with`.

#### Three ledger readers stop reading a keeper before it has its tables (GH #565)

`gh471_a_keeper_carries_its_sessions` went red once under load with
`no such table: sessions`. The window is test-side, not product-side: a store is
`Dormant` until the routing pre-send wakes it, and that wake runs synchronously
inside the colony task — it opens `cell.db`, applies the schema DDL and only then
spawns the cell task, so no message is ever answered before the tables exist. The
three test helpers that read a `cell.db` from outside were gated on `is_file()`
alone, and the file exists from `Connection::open` on, a moment before
`CREATE TABLE "sessions"` commits; a poll loop that hit that moment panicked in
`prepare` instead of looping once more. `gh471_a_keeper_carries_its_sessions`,
`gh471_a_member_carries_all_of_itself` and `gh467_a_member_is_born_with_its_history`
now carry the same hardened `rows()` the three sibling files already had: a missing
file or a table that is not there yet is an empty ledger, not a defect.

That the store really does pass through "file, no table" was measured, not assumed:
instrumenting the wake shows the keeper's `cell.db` going from `exists=false` to
`exists=true` at the store's own `Connection::open`, ahead of its DDL — the run that
produced that edge held the DDL back with an injected sleep to make the ordering
readable, but the file is the store's own and its open genuinely precedes the schema.
The repair is proven by pre-creating an empty `cell.db` before the keeper's first
wake — the precise state the poll loop can meet — and reading it through the helper:
the unhardened reader panics with the issue's own
`prepare: … Some("no such table: sessions")`, the hardened one reads an empty ledger
and the test goes on. Delaying the DDL with a sleep does not reproduce it, because
the blocking wake also stalls the runtime's timers and the test cannot look into the
window it opens.

## Older releases

Everything up to and including 0.28.0 is in
[CHANGELOG-ARCHIVE.md](CHANGELOG-ARCHIVE.md).
