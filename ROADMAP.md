# Roadmap

The [issue tracker](https://github.com/mmeyerlein/meclaw/issues) holds everything
actionable. This file only orders it: what comes next, what comes after, and why.

The horizons are relative to the work, not to a calendar. Now is the wave that
is running. Next is the wave after the next instance rebuild. Later has no
date. Alongside rides with whatever wave touches it.

Four rules keep the file from silting up:

- A stream names open issues only. Released work appears once, as one line,
  under [§ Shipped](#shipped).
- Nothing lives here twice. The issue carries the detail, this file carries the
  order and the reason.
- Closed issues get no eulogy. Their record is the issue.
- Every entry carries an anchor: an open issue, or `(register: <id>)` for
  something deliberately not built, with a trigger that would make it due. The
  register itself is internal, so the marker is all this file gives you.

A gate resolves both kinds on every push. A line pointing at a closed issue is
a red build.

Release detail is in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

When Now is empty, the tree is between two waves: what the last one built is in
v0.76.6, under [§ Shipped](#shipped), and the open findings wait in the tracker
for the next wave to give them a horizon. A horizon holds bullets only — a
sentence like this one stands up here, above the first heading, where the gate
does not read it.

What orders the other three: Next collects the findings from running the thing.
Later is ordered by a 50-question LongMemEval run against the memory hive,
public since 0.9.0, which put the bottleneck in the synthesis rather than in the
retrieval: in nineteen of twenty-one wrong answers the retrieval had already
delivered the gold session. Alongside holds surfaces, docs, and the way a colony
is operated; the template surface stays open there and needs no entry to stay
that way, a template being a directory, a README and a `template.json`, with the
worked examples listed in [`templates/README.md`](templates/README.md), where
§ *The hive boundary* says what a hive template has to satisfy.

## Now

## Next

- Re-measuring the builder's acceptance quota, once the acceptance cases stop
  moving under it. The last run measured four cases, one of them ordering a
  build no template could deliver, so a quota read off that is a reading about
  the cases. *(register: builder-acceptance-quota)*
- Metering what a subscription plan carries until it resets. The trigger fires
  when a recurring lane wants the subscription path.
  *(register: subscription-budget)*
- The message-header size watch. Headers carry no cap by design, so the watch
  is the instrument: it fires on drift past ~100 KB on a single hop. Last
  reading was 5.4 KB max. *(register: header-size)*
- Telling submissions apart by the door they came in at. Every question the
  broker is asked carries the same requester and the same subject whichever
  front raised it, so a rule that opens the shell to the operator and holds it
  shut against an agent cannot be written today. The shell-scoped rule ships
  switched off instead. *(register: policy-by-requester-origin)*

## Later

- Re-running the answer half of that measurement, directed and stratified. It
  waits until the memory chain (collector, recall, curator, memory hive) stops
  moving between builds, because a measurement of a surface in motion is stale
  by the next build. *(register: memory-answer-half)*
- Renaming, despite append-only. Nothing in the tree is ever deleted and a path
  is a cell's identity, which is what makes the record auditable. Naming a
  thing and changing that name later is a human act, and today the only answer
  is `move_nodes`, an identity-level operation for a presentation problem. The
  likely shape is a description layer: mutable display names in a database,
  never an address, with the tree untouched underneath. It gets a design round
  before it gets an issue. *(register: display-names)*
- Erasure, despite append-only. A log only grows and a blob file is never
  unlinked, so removing every trace of one person is not an operation today.
  The traces sit in a memory hive, in the last input of every brain that
  answered, in the colony's own books, and in blobs nothing attributes to
  anybody. The likely shape is one mutation that plans per store, forgets
  through the same slot an export reads from, redacts the log's payloads while
  every other row stays byte-identical, and proves itself with an export that
  comes back empty. It has a design and no build. The question that decides its
  size is whether a person's name may be a path segment at all.
  [#618](https://github.com/mmeyerlein/meclaw/issues/618)

## Alongside

- Voice-to-graph. The channel exists: a `voice` cell takes speech in and puts
  turns into the tree. A spoken intent still arrives as a sentence somebody
  else has to act on, never as a node and an edge. What is missing is the
  operating form: which utterances count as build intents, what a speaker hears
  while the graph grows, and what taking something back looks like when nobody
  pressed Enter. It waits on a `voice` instance being used in earnest for more
  than a day, because that is what tells a real intent from an invented one.
  Dictation through the ordinary text path stays designed and explicitly
  secondary. *(register: voice-to-graph)*

## Shipped

One line per release. Details in [CHANGELOG.md](CHANGELOG.md) and the
[GitHub releases](https://github.com/mmeyerlein/meclaw/releases).

- v0.76.6: a quick call without an answer goes to the brain, so the person is answered.
- v0.76.5: a runaway tool round stops at the first call over the dispatcher's budget.
- v0.76.4: a quoted short id never reaches the person; every tool the quick scope allows is on the menu; the
  quick window keeps the running round whole.
- v0.76.3: a late credential receipt reaches its caller; a note is answered by the voice that shows it; the
  ambient memory question asks with the person's own words.
- v0.76.2: the lines of an earlier session stand in the curator's window as past, behind a visible session
  boundary; the handover note reports only what happened and what was said.
- v0.76.1: the curator's handover note reports the past session and never orders the next one; the ambient
  recall stands before the person's words, framed as memory.
- v0.76.0: a channel template brings no access of its own; `freeswitch@3.0.0` gets its credentials by
  grant from an access of the colony, and one grant id serves several consumers, each with its own requester.
- v0.75.0: `--validate --apply` replays the params of every cell a manifest grows, and an app under `./apps`
  reaches a connect point below a sealed hive's rim.
- v0.74.0: a catalogue row carries price steps by prompt length, the memory hive takes a correction from the turn
  it is said in or from a third cell's report on `in_correction`, a turn narrows its own loop cap, and an app that
  embeds `simple-agent@2` seeds the keys its cells spend.
- v0.73.0: builder -> submit grows an embedded `simple-agent@2`, embeddings name their app, a seed order to an
  inactive store answers at once, the timer takes a lead time `after_ms`, and an inner turn crosses the firewall
  to reach talky on its own channel.
- v0.72.0: `figure@2` stands on `simple-agent@2`; the builder grows the agent one level deeper and
  `install_app` draws its lanes under one `agent_id`, receipts name scopes and paths, apps hand answers back
  as later turns, and the curator's history answers `context_turns` again.
- v0.71.0: `simple-agent@2` is the agent of a colony and enrols at its affinity; the speaking rims carry notes,
  the stamp's own questions, conversation events and the sources of an answer check; tools open by enablement,
  and the builder grows the agent in one block.
- v0.70.6: every shipped `llm` cell declares `reasoning_effort` (default `medium`), and the Anthropic lane
  sends off in the form each model accepts.
- v0.70.5: a brain's tool round keeps the system its turn began with -- the curator holds each way's window
  apart, and the Anthropic lane drops a replayed thinking block with a changed prefix instead of the call.
- v0.70.4: security patch -- `rustls-webpki` and `webpki-roots` move to versions that drop the distrusted
  emSign intermediates (RUSTSEC-2026-0381, RUSTSEC-2026-0382).
- v0.70.3: a lighter member colony -- the rarely called code cells of the shipped templates run cold, so a
  colony boots far fewer python children; the kit finds a wave without a date and reads an empty issue
  listing as an unreachable tracker.
- v0.70.2: follow-up fixes from the figure milestone runs -- a turn may require its first tool call, a figure's
  world turn that only narrated gets one follow-up turn that must act, a second answer of the same phone round
  no longer spends a reply, and the memory reads a verdict wrapped in a code fence.
- v0.70.1: the first published cut of the 0.70 line -- 0.70.0 below plus two fixes from its milestone runs: a
  figure's capped handset answer reaches the world, and a simple-agent runs its rarely called code cells cold.
- v0.70.0: a plain agent in one sealed hive -- a turn door, its own tools, client tools a caller adds per
  turn, an identity seeded into its own affinity, behind a curator and a memory; `npc` becomes `figure`, a world
  adapter and a handset around that agent (breaking); the curator's day relay orders a rebuild at a role's
  window target and at the day boundary; every code cell runs warm unless its written reason says otherwise,
  and an idle runner child stops after its idle timeout
- v0.69.1: a model without prefill support no longer answers 400 -- a request never ends on an assistant
  message, a consult's window ends on its request, and a provider's refusal keeps its own sentence; a member
  grown from a derived class takes its credential like `member` itself
- v0.69.0: an llm cell switches model and provider at run time on a registry message -- the registry names
  model, provider, host and credential, the cell asks the vault for that credential over its one grant, and
  the access hive delivers only credentials of the class the grant names; a switch away from the birth provider
  needs a credential of its own and a host the cell allows; a pending consult no longer runs twice
- v0.68.0: every turn takes its agreed way -- talk goes to the conversation model, simple to the fast path with
  its web and weather tools, complex straight to cogny with a bridge sentence, and research comes back later
  through the curator; the quick tier runs on `gpt-oss-120b` pinned to its fastest measured host; a sixth tool
  gives the weather without a key; `llm` cells stay awake 30 minutes, so a pause costs no credential round;
  a folded tool family is called by its ops on every path; a tool call leaked as raw harmony tokens becomes
  the call it is
- v0.67.0: a new `npc` template for a non-player character in a shared world, with one identity, one brain behind
  a curator, a memory hive, the world's tools as its moves, a handset and a night that closes the day; the memory
  hive's night runs its entity and recurrence passes again and writes in the language it read; a parked node no
  longer blocks the boot; the curator's bridge never refuses the request it covers; a brain born on a catalogue
  model gets its package before its first call; the kit registers satellite repositories from host configuration
- v0.66.0: a talky turn leaves with a route stamp, and the curator hands a talk or simple turn to a quick model cell
  behind it, gives a complex one a bridge sentence and names the wait of a research one while the talk goes on; an
  `llm` cell speaks the native Anthropic Messages wire (images, effort and thinking, cache breakpoints) and the
  registry carries measured response times, a quick tier and a `claude-haiku-5-5` row; tool menus fold into one tool
  per family with `help`; memory-hive turns recurring questions into prewarm candidates; a `bash`, `code` or
  `harness` cell without a sandbox runs under the default-deny profile, a secret param takes no `${VAR}`, and
  workspaces and transfer fences are relative to the cell's own hive (breaking); seed rows reach a running store as a
  message; a model swap no longer opens the original's `cell.db` or reads its `config.json`, and a model twin takes
  its seed from the library package; a shutdown no longer loses a timer strike or a Telegram message; every shipped
  LLM cell and example names its app; the answer talky sends reaches the checker the composite declares.
- v0.65.0: a cell can no longer read the message log through `/colony/trace`, mutates only inside its own hive
  subtree unless an edge to the mutation door is drawn, and the boot reads no cell's `cell.db` any more (breaking);
  an `llm` cell hands its state to a successor by message, and a twin parks its turns until the handover is
  imported; the peer and webhook proxies take their secrets as sealed vault grants; the memory-hive night, the
  argus cycle and the session-keeper night are one-shot watchdogs armed by the work instead of fixed schedules; the
  curator's handover, writer, history and push never read a refused store leg as empty and write in one step; a
  muted topic taken up again comes back in the same session, and everyday phrasings put a topic aside; the
  dispatcher expands family calls and answers `help`; a routing error to `reply_to` carries `unresolved_path`; nine
  shipped LLM cells name their app; in the kit, a lane lease no longer ends under a live lane run, release strands
  go ahead of the rest, shell entry points parse whole before they run, every lane probe runs under its own bound,
  and the merge candidate gate plans what the strand gate plans for the same diff.
- v0.64.3: the curator ages a replayed conversation by its own turns, cuts an overflow inside a segment instead of
  refusing the call, rebuilds without a model package and checks every sidecar fact at the source it names; its
  policy and intake write in one step, never read a refused store leg as empty, and the pin set sent last stands;
  memory-hive files a fact under exactly the source it names; a `query_timeout` from the clock or the store no
  longer leaves a watchdog-driven cell without its watchdog; in the kit, `pre` runs only the harness modules a
  diff names, `strand.sh test --unittest` runs the Python self-tests from the root of a tree, `token who` tells a
  series without a passing run apart, and the required-pins check proves that each pin selects its test; four
  lifted tests judge what they claim; the gate triage blocks only on a flake red at a lane about 30 % busy.
- v0.64.2: `rustls` 0.23.46 closes four advisories (lock only); a row that must exist once is written in one
  step, and the display, the presenter and the collector keep a row when its rewrite fails; a one-shot `--apply`
  waits out the shutdown drain and exits non-zero when it does not end cleanly; `affected_scope` no longer rescans
  the registry per path; a clock order the timer refuses no longer leaves the display standing still; the `voice`
  silence tail keeps its 20 ms raster under load; in the kit, a long run stops on its own once its target is lost,
  the gate runs a pinned test on every touch of its file, the merge point raises a version digit once per release
  and rewrites the hash goldens its own cascade made stale, and an idle connection no longer ends a lane hold.
- v0.64.0: the gate runs on a pipeline engine with a build station, a source-closure cache and lane leases, and
  the integration tests are folded into harness binaries; the workspace builds one `meclaw` binary; `file-space` and
  `projection` run warm; an orderly shutdown no longer reports a fatal watchdog trip, and the watchdog no longer
  charges the host's own silence to the colony; a message sent into a dying cell's mailbox is no longer lost, a
  restart no longer hands an overflow row out twice, and the bootstrap DDL runs in one transaction; the mutation
  scans and the vault's key derivation leave the runtime worker.
- v0.63.1: the public test suite passes again after a unit test read a template the export does not ship; the release
  gate's export-tree station now runs the library unit tests that read template files.
- v0.63.0: no clock ticks over unchanged data: the access sweep, the affinity push and canvy run on events, and the
  watchdog contract is written down once; every shipped model cell derives its timeouts from the catalogue row it is
  born on; one token estimate for every template, and the memory names what it cuts; the colony's time bounds follow
  what they guard; a phone line's duplex key comes from the line's own broker.
- v0.62.2: asking the memory to forget a topic mutes it instead of deleting it, so it stays unmentioned until the
  member asks for it; every cell that holds a vault grant can ask for its key in a colony that validates emissions;
  a lift renews the hives nested in what it lifts; a parked node no longer keeps a colony from booting; the release
  gate runs the catalogue tests in the export tree.
- v0.62.1: content to and from a model is no longer cut by numbers typed into code: numbers come from the
  model's catalogue row, the curator alone decides what fits the window and marks what it leaves out, and the
  carrier refuses with a number instead of cutting; the memory's entity graph has a producer again; an
  assistant's brains take a reasoning effort; a colony with no vault deposit boots without dead letters.
- v0.62.0: every cell that holds a secret takes it as a sealed vault grant, and no template carries a
  secret placeholder any more (a literal key in an instance still works through 0.62.x); a quote in the sidecar is
  checked word for word against what was said; a ringing call finds its live model ready; a store write replayed
  after a crash answers what it answered the first time.
- v0.61.7: output budgets follow the model (a cut answer is continued, never kept half), every catalogue row
  states its input bounds and prompt prices, and the llm cell refuses past the hard bound; the curator rebuilds by
  those bounds and by what a cached window costs, and the recall is sized by the model that reads it; a peer is
  known by name and a request to forget is honoured; a delegation's memory block in a phone call is kept.
- v0.61.6: WebSocket frames above 4 KiB are compressed (permessage-deflate on an in-tree codec, axum 0.8); a timer
  fires a schedule by name and can narrow the ops it accepts, so the memory hive's night runs on request; a lab
  channel can replay turns at their own time; a foreign round's turn opens no window on a member's screen.
- v0.61.4: a `web` page travels in parts, keyed down to the root children, encoded once per route and served
  with validators and gzip; joins arrive in pieces, viewers report their screen and backlog, and a bundle can answer
  with its refused legs only; peer delivery through a `meclaw` proxy is at-least-once with an idempotent receive
  and new `deferred`/`expired` receipts (breaking for contracts that list peer events).
- v0.55.0: a strand gate can run on a remote build host; the memory's close pass and its stored facts keep the
  audience of the turns they come from, and an episode can carry an affect mark; the curator reads only the rows
  a round may see and can send a short identity; apps can observe tool calls without displacing a default.
- v0.54.0: a round reads a curator's ledger row only if it was present for it (round ⊆ the row's audience), and
  the ledger counts itself on request; the control loop gets the lanes to measure goals per role and to change a
  text only behind an evaluation, shipped disabled and wired to nothing yet; TTL restores sit at named seams.
- v0.53.0: a proxy takes verified webhooks as one message each; a one-shot timer fires once after a stop
  that made it miss its moment.
- v0.52.0: a store filters, sorts and indexes on JSON paths; an installed app hears every brain, closed
  sessions and pins its own blocks; a turn that names its audience is answered again.
- v0.51.0: a workspace is laid out on disk for a program and adopts what it wrote, travels through git,
  and the file tools reach the models; a Telegram document lands in the member's file space by reference.
- v0.50.0: a file-space hive keeps a knowledge space's files as blocks and versions; every write carries its
  base, a syntax hook guards what lands, workspaces commit all or nothing, and a file knows itself from birth.
- v0.49.0: the curator shapes each window per role, lets the model read its own conversation back,
  asks memory with context and hands a new session a note on the last one; the reasoning core can ask
  back, and a duplex call outlives its provider's session limit.
- v0.48.0: a curator in front of each conversational model owns its window and keeps a ledger of
  every call, the collector hands the round on uncut, and the `llm` cell says when the provider's
  cache goes cold.
- v0.47.2: the tree is held against a committed persona measurement by two free gate stations that
  ask the owner instead of turning silently red, the recall harness runs on a local model through
  today's memory pipeline, and every `llm` cell declares the model it was served.
- v0.47.1: a member reborn from an exported memory is born with its identity again, a conversation
  brain keeps writing its annotation block once it sees its own earlier answers, and the memory
  hive's night closes no fact across an audience.
- v0.47.0: a surface runs behind a strict Content-Security-Policy and what an app writes to it
  stays text; a model push works only in the cell it names, and a refusal reaches the registry; a
  long-running colony no longer strands on a runtime worker that waits for the disk; the affinity
  brief carries each persona once; the strand kit meters the cargo queue.
- v0.46.0: a turn always ends with a reason, the other side has a role and a name, one busy cell
  never ends a colony, the boundary sends only where it is told, and a model is operated in one
  place. `origin: "peer"` with `speaker`/`speaker_ref` and a roster legend; memory keeps who said
  what (`facts.source`); a full mailbox spills in order instead of ending the colony; `params.egress`
  on a peer cell; and `llm-registry@2.3.0` chooses each cell's model from what the cell says it needs,
  in prose, once per change (`params.requirement`, an `in_model` door in every composite).
- v0.45.0: agents behind one member, each answering for itself. An accepted proposal about a
  disclosure becomes the disclosure row and a trust level says no on purpose (`affinity@3.5.0`); a turn
  on a channel with many counterparts briefs the member's record about the counterpart (a third turn
  leg, context key `counterpart`); a member grows a door that takes every turn nobody addressed
  (`builder@1.13.0`); and a mount believes an identity header only from the proxies it trusts,
  loopback by default (`params.trusted_proxies`, breaking for a proxy on another host).
- v0.44.0: a colony lifts, exports and installs the way it promises. It keeps its own copy of
  every template version it instantiated, an app is installed from what it declares, a late answer
  carries the turn that asked, `meclaw --env-report` names the keys nothing binds, and a `proxy` on
  platform `meclaw` carries a credential on its outgoing POST (a static header or an OAuth 2.0
  client-credentials bearer, new code `auth_unavailable`).
- v0.43.0: the display's curator keeps its state in memory. `display@2.7.0` runs compose
  `resident`, the store keeps the applications' rows and one small row of history, one pass
  sends one `patch` and no `read`, and no header carries the screen plan any more (measured on
  a live colony: 775 to 25.6 MB of log per hour); the colony view stands in the dock and opens
  on a tap (`colony-view@1.1.3`).
- v0.42.0: a colony speaks to another colony over a declared lane. The `proxy` cell gains the
  platform `meclaw` (a peer mount on the one listener, one POST out, the lane contract on both
  sides, a field the lane does not name refused rather than stripped, receipts on both sides);
  an ingress emission carries its trace and budget; `affinity@3.4.0` never auto-accepts a
  directory audience; the peer mount and its frame are public contract.
- v0.41.1: eight small defects, each measured before it was fixed. A browser refused
  after it has already started is ended before the refusal is spoken
  (`browser@1.0.1`); paging the message log by cursor is a range read on
  `(created_at, id)` again, with the query plan pinned by a test; a roadmap
  horizon holds bullets only; a gate receipt carries the stations its run
  planned, so the release audit stops reading `not planned` as `skipped`; the
  page-weight markers of the display lab pick their lid by the target they
  measure; and three test rigs measure the promise instead of the host they run
  on.
- v0.41.0: a voice cell can hold one session in which a model hears the caller
  and answers in it. `voice@2.1.0` adds a duplex provider beside recognition and
  synthesis, with lanes of its own for what the assistant says, for the errand
  the model hands the backend mid-call, and for a fact brought into a running
  conversation; `voice@2.2.0` gives that session its own clock, its own
  keepalive and a deadline for an unanswered errand. The road around it is
  drawn to match, from the telephone through the member and the assistant to
  the collector and the keeper, and a sidecar section may now be the sentence
  itself. Beside that, the dead-letter read answers newest first.
- v0.40.1: the workspace compiles on Windows again. Nothing that ships moved,
  and no topology needs anything done to it.
- v0.40.0: a browser is a cell. `browser@1.0.0` runs one Chromium-based browser
  per member over a pair of file descriptors, a context per identity and a page
  per card, and puts each page's picture on a `page:` topic of a display's own
  socket, with pointers, wheels, keys and text coming back on the same link.
  The browser is a prerequisite out of the machine's package system, and the
  ceiling on the process is a cgroup cap asked of whoever owns the cgroup it
  re-homed itself into. `display@2.6.0` draws a page as ordinary content in an
  ordinary window, `web@2.1.0` reads a table of topic kinds where it carried
  one prefix, and a patch now leaves only after the state row has landed.
  Beside it, a strand costs less: the strand kit, a form station in front of
  the cargo lock, a two-stage copy lock, a measurement library, a retry tied to
  its issue, and a retrospective after every wave.
- v0.39.0: the screen is built from its description, and typed input is a
  channel of its own. One document, one reference model, one state row for the
  screen; one switch at the root for many exits; a channel that mints the turn
  id the answer carries, a talky per channel, and a view an application can
  take back down.
- v0.38.1: a held recording arrives whole. A provider debt only while the
  provider is inside a take, an empty end of turn that keeps the interim, a
  browser that drains before it lets go.
- v0.38.0: the screen has a dock. One canvas, a column of tiles of one size for
  everything present, the OS mark as the hold-to-talk button. Presence and
  focus are two axes, tiles and topics come from the applications, and the
  colony loop no longer waits on reads of its own log.
- v0.37.0: a standing hive is lifted in place. `replace_nodes` brings a running
  hive to a new version of its template under its own path and says in the
  receipt what happened to every child.
- v0.36.1: the repairs the first days on 0.36.0 turned up. A reactivated cell
  can be swapped away again, the dialplan is keyed by the pair, the proxy
  example strips its prefix, and the export audit sees every never-exported
  root.
- v0.36.0: the screen brings its own design language. A token sheet and a
  component vocabulary in `display@2.1.0`, `operator_set` params, more than one
  version per class, and stderr logging.
- v0.35.0: no port for a surface cell. One listener, a mount per surface, a
  prefix-aware shell under `/<mount>/`, and a switch that is the proxy in front
  of several colonies.
- v0.34.0: the documentation has one shape and the quick start asks for the
  key. README as a hub, a concept layer in `docs/`, and `start.sh` booting the
  shell so getting started runs end to end.
- v0.33.0: first contact and the telephone findings. `meclaw ask` prints the
  answer to one turn, a sealed hive's interior stops being addressable from
  outside, and a call names itself and runs at 8 kHz.
- v0.32.1: the README and the public docs rewritten in plain language. No code
  change.
- v0.32.0: the answer carries typed offers. One ```` ```sidecar ```` block with
  sections, a member that sorts them into its apps, and a telephone as a
  channel of a person.
- v0.31.0: speech is a channel. The `voice` cell type and `voice@1.0.0`, two
  STT and two TTS providers behind traits, an echo provider, a test page.
- v0.30.1: the repairs a first colony on 0.30.0 turned up. A member wish is one
  submission, and `--validate` reads a `ref` marker's `override_params`.
- v0.30.0: every open issue is built or ruled. The poll timers are gone, a
  store writes its own files, memory answers an ordinary tool call, and one
  gate process reads the diff.
- v0.29.0: the tracker is clean. The front door is one place, the deep edge is
  the declared form, and a generation grows with its keys in one act.
- v0.28.0: the organism grows its own surfaces. Four composition levels from
  one catalogue, a builder that submits through one front, and one memory hive
  for several askers.
- v0.27.0: the builder stops guessing and starts looking. The intake is a
  bounded, typed tool loop with four eyes and no hand.
- v0.26.0: a template arrives in a running colony (`add_templates`), and the
  shutdown drains.
- v0.25.0: a wish in the chat becomes a manifest somebody else submits.
- v0.24.0: the vault delivers without giving anything away.
- v0.23.0: a display moves without being rebuilt.
- v0.22.5: the toolchain moves when somebody moves it.
- v0.22.4: the registry has no unpinned bucket left.
- v0.22.3: a mixed completion answers the turn it spoke for.
- v0.22.2: a seed nobody can load is refused by name.
- v0.22.1: what the first day of a `web` cell in production found.
- v0.22.0: a display is a cell, and it owns its port.
- v0.21.0: the four composition levels, and a connector that is one cell.
- v0.20.1: an emission from the boot window is held instead of lost.
- v0.20.0: identity comes off the edge, and the colony answers counts about its
  own books.
- v0.19.0: the turn annotates itself, and the closed session is read once.
- v0.18.0: a default, a slot, and one message instead of nine.
- v0.17.4: a refusal stops arriving as a result.
- v0.17.3: a template can put another template inside itself.
- v0.17.2: the error paths keep the contract's word.
- v0.17.1: the night the audit was answered.
- v0.17.0: content can leave a cell and enter a running one.
- v0.16.0: a fact remembers who was there.
- v0.15.x: every shipped hive is behind its boundary.
- v0.14.0: a name means one thing.
- v0.13.0 / v0.12.x: the canvas keeps its stylesheet's word, and a surface
  installs into a running colony.
- v0.11.x: a colony serves surfaces over HTTP.
- v0.10.x: the wave before the launch. Vault, audience sets, the steward.
- v0.9.x: sealed hives, open memory.
- v0.8.0: the hard shell.
- v0.5.0-0.7.0: the agent waves. Collector, front door, advisor.
- v0.4.x: the bug-and-substrate wave, the pre-MVP finish line.
- v0.1.x-0.3.x: hardening, memory quality, statement identity.
