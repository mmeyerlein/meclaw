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
v0.44.0, under [§ Shipped](#shipped), and the open findings wait in the tracker
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
- A gate for what the assistant is like. Nothing today guards tone, brevity,
  refusal, what is remembered and when the colony speaks. A model swap, a
  prompt edit or a changed seed passes every gate green, and whoever talks to
  it notices the drift weeks later. The design is in the tree: synthetic
  personas through scripted multi-turn sessions, scored on core invariants,
  memory and timing, with the paid measurement committed as an artefact and two
  free stations holding the tree against it.
  [#621](https://github.com/mmeyerlein/meclaw/issues/621)
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
