# Changelog

All notable changes to MeClaw are documented in this file. One entry per released
package. The format loosely follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows SemVer (0.x: minor/patch bumps for additive features).

## [0.6.0] — 2026-08-14

The front door. Five parallel tracks in one morning, hours after 0.5.0 shipped,
each merged on its own behind full gates. Where 0.5.0 built the agent, this one
builds what stands in front of it: who is allowed in, who gets an agent of their
own, and how an agent asks its own memory a question. Two more templates go
public — nine now under `builder/templates/` — and the tool cells get the
polish that a coding agent leans on.

A minor bump rather than a patch, for the same three reasons 0.5.0 was one: new
templates, a new collector lane, and four sanctioned behaviour changes on the
`file`, `edit` and `web_fetch` cells.

### Added

- **`firewall@1` screens every inbound channel, and never asks a model.**
  Between a surface and an agent sits a hive that measures each incoming turn
  against a rule set of rows and lets it out on exactly one of two lanes: `pass`
  with a byte-identical body, or `reject` carrying both the reason and the rule
  that fired. Six deterministic rule classes in a fixed order — size cap,
  unreadable rule, sender blocklist, sender allowlist, pattern blocklist, rate
  limit — and every verdict is a character count, a literal comparison or a
  clock. The policy is store data, not code: a row is switched off, never
  deleted, and editing the rule set needs no restart and no cell code. It fails
  closed (a rule line that will not parse rejects the turn and names itself) and
  it is deliberately regex-free — a caller-fed regex on the ingress path is a
  ReDoS on exactly the channel being protected. The shipped seed is inert: five
  example rows, all disabled, with only the two arithmetic rules armed. Two
  cells, no Rust (#36).
- **`receptionist@1` gives every channel its own agent.** The first turn from an
  unknown channel makes the reception emit ONE mutation that instantiates a
  fresh `talky@1` for that channel and wires its four ports in the same diff;
  every later turn from that channel takes the edge that mutation drew. The
  ordering is the whole trick and it is measured, not asserted: a
  `/colony/mutations` emission is dispatched inline before the next emission
  leaves the mailbox, so the triggering turn travels behind its own mutation and
  is never lost — a burst on a cold channel is answered twice and forks nothing.
  The channel travels as two keys, a sanitised node name and the raw identity,
  so a channel identity never has to be escaped into a CEL literal. Two cells,
  no Rust (#29).
- **An agent can ask its memory about a time range.** The per-turn recall fires
  before the model has seen the turn, so nothing in an agent could ever *decide*
  to ask for a period. `memory_recall` is now a tool round like any other: the
  dispatcher only names it, and the collector — the memory specialist of the
  hive, which owns the recall port anyway — serves it on a ninth entry lane
  (`in_memory_call`) and correlates the answer through
  `context.memory_call_id`. Empty means the turn's ambient leg, set means the
  `tool_result` of the running round. The recall cell has understood
  `recall_window_from` / `recall_window_to` since P15; this is the first
  producer for them. `COLLECTOR_MEMORY_CALL_TIER` (default 1, empty switches the
  tool off). The dispatcher is untouched, and memory learns no dispatcher
  vocabulary (#78).
- **`edit` can make the match count a precondition.** `find_replace` replaced
  every occurrence and reported the number afterwards, which quietly patches
  places the caller never saw. The optional `expected_matches` turns the count
  into a guard: a deviation leaves the file untouched and answers the new typed
  `unexpected_match_count` naming both numbers. Zero matches keep the more
  specific `pattern_not_found`, and the argument on `insert_at_line` is
  `invalid_input` rather than silently ignored. Without the argument the
  behaviour is byte-identical to before (#105).
- **`file` reads binaries and windows.** `mode: "base64"` hands back the raw
  bytes (RFC 4648 §4, flagged as `header.encoding`), and `offset` / `limit` are
  a BYTE window in both modes, clamped at the end. An offset at or past EOF is
  an empty read rather than an error — that is the paging signal — while
  `limit: 0` is `invalid_input`. The default read contract, including the
  absence of the `encoding` header, is pinned unchanged (#106).

### Changed

- **The `file` and `edit` fence stops being an existence oracle.** The boundary
  check ran after `canonicalize`, so `../missing` answered `not_found` while
  `../exists` answered `path_outside_boundary` — the difference read out the
  existence of files outside the fence. A purely lexical pre-check now runs
  before any filesystem access, on the write path as well as the read paths, so
  every escape attempt gets the same answer and the text does not reveal what
  was looked for. The canonicalize stage stays behind it; only that one catches
  symlink escapes. A path that lexically ascends and would really land inside is
  now refused too — deciding otherwise would mean resolving names outside the
  fence, which is the oracle being closed (#107).
- **`insert_at_line` closes its own line.** Content was spliced verbatim between
  the line slices, so content without a trailing newline fused with the line it
  displaced — silently, with `matches_changed: 1` reported, and the breakage
  surfacing only at the next compile. The cell now appends exactly one newline
  when it is missing. Empty content still inserts nothing (no phantom blank
  line), and a file without a final newline still fuses on append: the cell
  normalises its own argument, never foreign bytes. That limit is pinned as a
  known one (#108).
- **`web_fetch` parses the URL at the gate.** A malformed URL used to fall out
  of the transport layer as `io_error`. The gate now parses it: a syntax error
  is `invalid_input` quoting the URL, and a scheme the cell does not speak —
  everything but `http`/`https`, `file://` included — is the same class, naming
  the scheme. `io_error` goes back to meaning DNS, connect and transport. The
  URL itself travels unchanged; nothing is re-serialised (#110).

### Docs

- **The `store` cell's two error families are named as they are built** (#109).
  The docs sorted `invalid_input` and `query_timeout` into the regular
  `tool_result` family while the code emits them as error messages with
  `finish_reason: "error"`. The code is the intent — that class never reaches
  the database, so there is no result to report on — so the docs move: an
  SQL-level family carrying `header.error_code` on a regular `tool_result`, and
  an args-level family carrying `finish_reason`. Documented alongside, because
  it bites a caller directly: an args-level rejection carries an empty `id` and
  is correlatable only by order, not by `tool_call_id`.

### Tooling

- **The export tool gates DE/EN drift.** `DOCS_MAP` ships the English twin as
  the public file, so a forgotten translation always lands in the public tree
  and never in the internal one — two 0.5.0 commits did exactly that. R6c now
  compares two language-independent signals against the source revision:
  heading-level parity (titles are reported but never compared, and `##` inside
  a code fence is not a heading) and staleness (a German twin committed later
  than its translation). Deliberately not a content diff, which would be
  permanently red and therefore switched off at the first release. Both
  exception lists are single judgements with a reason: a declared structural
  deviation keyed by its exact printed signature, and a translation receipt
  keyed to the full SHA of the German commit it was translated against — the
  next German commit invalidates it automatically. Both are currently empty
  (#113).
- **`talky@1`, `firewall@1` and `receptionist@1` join the export allow-list.**
  `PUBLIC_TEMPLATES` is an allow-list, so a template is private until someone
  enters it; nine are entered now. Each one carries its runtime-reading test
  with it, which is what R2b asks for: no exported test may read what the subset
  lacks (#112, #29, #36).

### Known

- One pre-existing test (`collector_colony`'s idle-window sweep, a deliberately
  tight 300 ms semantic discriminator) went red once under five-track peak load
  and was re-verified green in isolation three times. Tracked as #114; whether
  it wants a harder test or a quieter machine is open.

## [0.5.0] — 2026-08-14

The agent wave. Two waves in one night — the start-Egon wave and night wave 3 —
each built on parallel tracks in isolated worktrees and merged one by one behind
full gates. The theme is the first full agent on top of the substrate: almost
everything below is topology, not Rust. Five new templates ship, the collector
hive becomes public, and the tool cells get the contract batteries that a coding
agent will lean on.

A minor bump rather than a patch: new templates, new knobs, and four sanctioned
behaviour changes (the boot probe's truth table, attachment consumption on the
responses dialect, truncation on `bash`/`web_search`, and a formerly silent
`{text_id}` residue that is now a loud call error).

### Added

- **`--sandbox-probe` answers the sandbox question before the first cell.** A
  flags-only CLI surface for the four host capability probes
  (`filesystem`/`network`/`limits`/`syscalls` — the `params.sandbox` keys); the
  cgroup line names the launch requirement (a systemd user unit vs. an ssh
  session scope) instead of a bare "no". The same report rides along on
  `--validate`, where the spawning probes only run if the tree declares a
  `restricted` profile (#97).
- **The collector hive ships.** `collector@1` — the context orchestrator that
  decides what enters an agent's context window and what leaves it — is now a
  public template instead of a private one. Eight entry lanes, five exit routes,
  one seam, and no model judgement anywhere in the eviction policy (#27).
- **Four more templates, all pure topology.** `session-keeper@1` gives a
  conversation the lifecycle of a phone call: a lazy start per channel, an idle
  clock, a nightly sweep that seals a generation with a guarded update, and no
  LLM anywhere in it. `dispatcher@1` is the fan-out half of a tool loop whose
  fan-in half is the collector — routing, never assembly. `summarizer@1` turns a
  closed generation into one recency-weighted handover summary that an `llm` cell
  consumes as a `system.*` upsert without a provider call, so the next generation
  wakes up with yesterday instead of with nothing (#100). `retry@1` re-emits the
  original call on a failure reply while the *edge* does the counting, and
  `archive-bridge@1` lifts the append-only archive out of the example colony into
  a template (#3, #4).
- **Caps at the brain seam.** The tool round and the memory bundle used to reach
  the model verbatim and unbounded: one large tool result walked past every
  eviction knob there was. `COLLECTOR_TOOL_CHARS` caps a tool result per item,
  `COLLECTOR_ROUND_BYTES` caps the whole round from the newest iteration
  backwards, `COLLECTOR_MEMORY_CHARS` caps the rendered bundle. The cut travels
  on the hop (`round_capped`, `round_dropped`, `memory_capped`) and the full text
  stays readable in the round store — a cap is a preview, not a delete (#91).
- **The seam ends its own loop.** `COLLECTOR_MAX_ITER` (default 8): at the cap
  the assembled context leaves on the answer lane with `hop.round_capped`
  instead of asking the brain again. The round begins at the seam, so the seam
  is what ends it — no dispatcher, no edge condition and no expiring TTL is
  needed to stop a runaway loop, and the turn leaves on a lane the topology can
  react to instead of dying in the DLQ (#77).
- **A close lane and a pruning policy for the window store.** A session close
  hands the whole day out as ONE batch on route `write` (turns in order, tool
  rounds as a raw top-level slot), and writes a ledger row as it does. The prune
  lane then deletes only what has provably left the collector and is older than
  `COLLECTOR_PRUNE_AFTER_MS` (default 7 days), reporting the cut per session on
  the hop. No ledger row, no prune — the store would rather grow than lose
  quietly. There is no built-in schedule; the parent tree wires a timer if it
  wants one (#76).
- **A tool round survives a lost result.** A fan-in that waits for a result that
  never arrives used to park forever. `COLLECTOR_ROUND_IDLE_MS` (default 2 min)
  closes such a round at the next occasion with a synthetic error `tool_result`
  per missing call, then hands back to the existing machinery with
  `hop.round_stale`. A user turn arriving mid-round is parked and stamped
  (`hop.round_deferred`) rather than starting a second assembly: at most one open
  brain call per session (#103).
- **The `llm` cell reads a JSONL seed.** `<cell>/seed/system.jsonl` upserts
  `system` leaves on first open, in one transaction, with the exact semantics of
  `upsert_system_leaf`. A resumed cell is never re-seeded, a `{text_id}` leaf in
  a seed is a loud spawn-time configuration error, and a missing file is not an
  error at all (#99).
- **`bash` and `web_search` learned the size cap `web_fetch` already had.**
  `params.max_bytes` (default 256 KiB on both) trims the emitted text at a UTF-8
  boundary with a visible marker, sets `header.truncated`, and reports the full
  pre-cut size in `header.bytes`. The declared `truncated` header finally has a
  producer everywhere it is declared (#83).

### Changed

- **The boot probe stops guessing from row counts.** An edge-less but perfectly
  healthy workspace booted exactly once: the second boot classified the state the
  first one had written as `Inconsistent` and panicked. The count heuristic is
  replaced by a truth table whose discriminator is whether the initial-apply
  bundle has ever committed (`edges > 0` or `hive_scopes > 0`), which makes
  edge-less single cells and hive-only roots the first-class states the spec
  always said they were. Real corruption stays loud, and gets louder: a COUNT
  that errors on an existing file is now `Inconsistent` instead of a silent
  `FirstBoot` via `unwrap_or(0)`. The two drifting copies of the probe are one
  (#89).
- **Attachments cross the responses wire dialect.** A cell on
  `wire_dialect: "responses"` that declares `consumes.body.attachments` used to
  reject loudly; it now translates `image/*` attachments into `input_image` items
  on the typed `input[]`, folded onto the last user message, with the same data
  URL as the chat-completions path (#94).
- **A pre-#86 `{text_id}` row is loud instead of silent.** A `system` row
  persisted before the delivery boundary resolved pointers would silently drop
  out of the system prompt. Reading one is now a regular cell error naming every
  affected slot path, its origin and the way out — no panic, no restart loop, and
  no provider call with a shortened prompt (#95).

### Fixed

- **`colony.db` "database is locked" under parallel load.** Not a missing
  `busy_timeout` — rusqlite installs one implicitly — but a same-process race:
  boot opened the database twice with an asynchronous writer-thread close in
  between, whose WAL checkpoint took exclusive locks against the re-open. The
  double open is gone and every open path now carries an explicit 30 s budget
  (#98).

### Tooling fitness

- **Contract batteries for every tool cell** (#104). Ten test files covering
  `bash`, `file`, `edit`, `code`, `store`, `timer`, `mcp`, `harness`,
  `web_fetch` and `web_search` against their production factory paths: exit codes
  as data, byte-exact sentinel layout, path-traversal and symlink escapes with
  exact error codes, UTF-8 boundaries under truncation, the `code` cell's stdin
  contract in full, multi-send ordering, and operation timeouts. Plus a workshop
  scenario that drives a real coding task through all ten cells over the
  unmodified `dispatcher@1` and `collector@1` templates — the templates are the
  cells under test, not a copy of them.

### Docs

- **README and roadmap on the current release** (#92), including what the
  meclaw-os stream has actually delivered rather than what it promised.

## [0.4.1] — 2026-08-13

The pre-MVP finish line. The three remaining substrate items of the pre-MVP
stream, built on three parallel tracks and merged behind full gates, hours
after 0.4.0 closed the bug backlog.

### Added

- **Sandbox phase 2: the caps and filters are real.** `params.sandbox.limits`
  (memory, pids, cpu) is enforced through a delegated cgroup-v2 sub-group per
  sandboxed process, held as an RAII scope that tears down on every path —
  including crash and restart — and swept if a dead daemon left one behind.
  `params.sandbox.syscalls` is enforced through an in-tree seccomp-bpf filter
  (raw `sock_filter` programs via libc, no new dependency), closing the gaps
  Landlock does not cover: ptrace, signals to foreign PIDs, raw sockets. The
  harness cell's stdio child runs under the same profile as `code` and `bash`,
  with its process-group and reaping semantics untouched. And a cell freshly
  instantiated from a template without an explicit profile now gets a
  restricted default (network deny, runtime-only filesystem) — a prospective
  cut in the GH #20 tradition: existing topologies on disk keep running
  unchanged, `trust: "trusted"` stays the explicit escape hatch. Enforcement
  tests carry controls and skip visibly where the kernel or the cgroup
  delegation does not cooperate (#85; mcp/subcolony children tracked as #96,
  a host capability probe as #97).
- **The `system` tree resolves its pointers.** A `{text_id}` leaf becomes
  `{"text": "..."}` at the same delivery boundary, under the same depth limit,
  per-path cycle guard and error codes as the `messages[]` class — one shared
  `text_id` document contract (exactly one turn), only the substitution
  differs. Both slots resolve against one working copy per delivery, so a
  failure in either dead-letters the body unchanged. The llm cell's loud
  `BlobUnsupported` rejection is removed; its system-prompt concatenation is
  now infallible (#86; pre-existing persisted rows tracked as #95).
- **A cell can finally read its attachments.** A contract that declares
  `consumes.body.attachments` yields a read-only `AttachmentReader` on the
  colony's blob store — not a new factory parameter, but a function of the
  contract view and the store handle the delivery boundary already holds.
  Every read carries its own operation timeout. The llm cell is the first
  consumer: `image/*` attachments become vision content parts (base64 data
  URLs) of the outbound chat request; a non-image mime and a missing blob are
  named cell errors, and a cell without the declaration behaves byte-identically
  to before (#87; the responses wire dialect rejects loudly and is tracked
  as #94).

## [0.4.0] — 2026-08-13

The bug-and-substrate wave. Every open bug on the tracker and the remaining
pre-MVP substrate items, built on five parallel tracks in isolated worktrees
and merged one by one behind full gates. The wave's sharpest find was not on
the list at all: the provenance schema change would have killed every existing
colony on upgrade, and only a stale pre-v5 database lying around the main tree
could prove it — fresh worktrees, fresh fixtures and green branch gates never
saw it.

### Added

- **Code and bash cells take a sandbox profile.** `params.sandbox` declares
  `trust` (`restricted`|`trusted`), `network` (deny by default) and a mandatory
  `filesystem` allowlist under `restricted`. The filesystem view is enforced
  with Landlock, the network deny with a network namespace — no container
  runtime, no new dependency, and fail-closed: a profile that cannot be applied
  fails the spawn instead of running open. The phase cut is honest and measured:
  on a host with `apparmor_restrict_unprivileged_userns=1` the mount-namespace
  route is closed to an unprivileged daemon, so Landlock carries the filesystem
  view; cgroup resource caps and seccomp filters are schema-visible but rejected
  at config load until they are enforced (#35, phase 2 tracked as #85).
- **The loopback edge may restore `ttl`, as an explicit modifier.** A
  store-backed tool round costs about a dozen routing hops; instead of inflating
  the colony-wide budget, the re-entry edge of a deliberate loop now declares
  `restore_ttl` next to its iteration counter. The declaration is the contract:
  config load rejects a restoring edge that carries no condition, restore never
  exceeds the initial budget, and everything that does not opt in still dies at
  the sharp default of 64. The example colony runs on the default budget again;
  its 160-hop override is gone (#82).
- **Instantiated nodes know where they came from.** `cell.provenance` records
  template, template version and instantiation time in the instance's own
  `config.json` — the copy names its origin even without the colony that made
  it — and the registry indexes the three columns (schema v5), re-derived from
  the files at every boot so a config-only copy re-indexes itself. `template_id`
  is stable across rescans. This is the hook the app-store stream needs: an
  updated template can find its instances (#62).
- **In-message blob pointers resolve at the delivery boundary.** `text_id` and
  `messages_id` inside `messages[]` resolve recursively, bounded by
  `blob_max_recursion_depth` (now actually wired) plus a per-path visited set;
  both violations dead-letter as `blob_recursion_too_deep`, and a failed
  resolution delivers nothing rather than a half-expanded body. The old
  zero-producer prohibition is replaced by the guarantee and its tests;
  `attachments[]` ownership is decided and documented (consuming cell, on
  demand — wiring tracked as #87) (#19).
- **A scheduled lane is a tool lane.** The timer reads its op from a
  `tool_call` turn the way `bash` does, answers every op with a `tool_result`
  carrying the caller's id, and its parse error now distinguishes "no op object
  at the body's top level" from "op object missing a field". The raw-body path
  keeps working byte-identically; a committed fixture drives the remind lane end
  to end from an agent tool call (#81).

### Fixed

- **A batch lane no longer re-extracts what inline extraction already covered.**
  The inline phase marks its episodes as covered in the extraction queue, and an
  empty inline facts block covers its turn — it is the front model's verdict
  that nothing was memorable, not an absence. The batch lane serves its real
  purpose again: catching turns from models that emit no inline block (#52).
- **The inline extraction contract ships with the hive.** It states what the
  batch prompt always knew: the assistant's own answer is not a fact, a question
  is not a fact, restating stored knowledge mints nothing, and deriving validity
  windows from a question's date range is forbidden — the shape that minted
  self-closing period facts out of history answers. Drift-locked in both
  directions by tests (#53).
- **A closure across two spellings proposes the alias instead of hiding it.**
  The nightly identity questions read a bounded set of recently closed rows one
  phase earlier and merge their spellings into the inventory as a pure union —
  exactly the case where a closure just proved two spellings talk about the same
  thing. The C6 scenario that was red on its first live run pins it green, and
  the invariance set inside it holds across the round (#73).
- **A write into a missing parent names the parent.** `parent directory does not
  exist: notes (write does not create directories)` instead of `io error during
  resolve`, with distinguishable texts for permission-denied, not-a-directory
  and read-only causes; the `io_error` taxonomy is unchanged and the wording is
  pinned as a contract (#79).
- **A fan-out that matches nothing is not an alarm.** Every shipped dispatcher
  and collector edge guards its key (`has(hop.tool_name) && ...`), and the
  substrate now tells the two apart: a missing key on a valid expression logs at
  debug, a genuine eval error stays at warn. A sweep test holds every shipped
  config to the guarded form (#80).
- **An existing database migrates before the DDL batch runs.** The v5 schema
  creates an index on a column that only the v4→v5 migration adds, so opening
  any pre-v5 `colony.db` died with "no such column: template" before the
  migration ever ran. Migration now runs first on an existing database, the
  older ALTER steps carry the same table-exists guard, and the rollback pin
  proves its all-or-nothing promise against real step work (#90).

A reliability wave on the substrate. Seven defects, none of them inside a cell
and all of them between cells: a gateway error that arrived as a parser
complaint, a routing budget that ran out five rounds into an agent turn, a fetch
that was paid for again on every round, a scheduled lane that could not be
triggered from outside, a watchdog whose deadline was unreachable and whose trip
said nothing, a test runner that blamed the wrong thing, and an instantiation
that wrote secrets to disk. None of it was found by reading the code.

### Fixed

- **An error inside a 200 body is the error it names.** An OpenAI-compatible
  gateway reports an upstream failure as a regular HTTP 200 whose body carries
  `{"error": {...}}` and no `choices` at all. The translate step reached straight
  for `choices[0]`, missed it, and reported a parse defect, so no failover edge
  on `rate_limit` could ever fire and the provider's own sentence ("wait a
  moment, then it works again") was replaced by a parser complaint. In a practice
  run this killed 3 of 21 turns. The classification now sits at the wire
  boundary, before `choices[0]` is read, and runs through the same table a real
  HTTP status runs through, so an in-body 429 lands on exactly the lane a status
  429 lands on: the same function call decides. The status is read from
  `error.code`, `error.status` or `error.http_status`, then from the typed
  strings, then from the prose, and the provider's message is carried into the
  detail with a visible cap. The discriminator is narrow on purpose: a body with
  `choices` still belongs to the translate step, and a body with neither
  `choices` nor `error` is still a parse error naming `missing choices[0]` (#75).
- **One tool round costs about a dozen hops, so the budget is sized for that.**
  `ttl` is decremented on every routing decision, and one user-visible round of a
  store-backed tool loop is not one hop: the collector's read-modify-write
  conversation with the store is itself routing. Measured on the checked-in
  fixture, six rounds cost 76 hops against a default budget of 64, so the default
  holds five rounds and the sixth dies. The example colony now sizes its budget
  for twelve rounds, the walkthrough carries the hop table per leg and the rule
  of thumb, and the death is no longer silent: exactly one ERROR line per expiry
  names the message, the sender, the target, the trace, the reason, and what has
  to be sized. The frozen corridor keeps its terse warn line unchanged; the loud
  one lives in the wrapper around it (#82).
- **A fetched body has a bound, and a cut is visible.** `web_fetch` returned
  whatever it got, with no cap and no producer for the `truncated` header its own
  contract already declared. In a loop the thread is rebuilt cumulatively, so one
  large result is not paid for once but on every remaining round of the turn
  (measured: 172 KB became roughly 35k prompt tokens twice over). There is a
  `params.max_bytes` now, default 256 KiB, generous enough that an ordinary
  document passes whole and finite enough that a multi-megabyte payload cannot
  enter a loop unbraked. The cut lands on a UTF-8 character boundary and says so
  in the payload, `header.truncated` finally has a producer, and `header.bytes`
  reports the full size the server sent rather than the remainder. The example
  colony sets 32 KiB on its reader, which is the value a loop actually wants
  (#83).
- **A scheduled lane can be triggered once, from outside.** Two halves, and only
  one of them was what the issue suspected. There was never a wall on the
  ingress: an op body carries none of the three central slots, and the validator
  requires one, which is the whole 422. Wrapping the op in `"messages": []` makes
  it valid and it arrives, no new route and no bypass, and the docs now say that
  for op bodies in general because the class is larger than the timer. That alone
  does not make an external run indistinguishable from a scheduled one, so the
  timer got the smallest honest op: `trigger`, carrying nothing but the
  `schedule_id`, because everything else already stands in the row. It enters the
  same frame the scheduler's own tick enters, with the same race check, the same
  iteration counter, the same header set and the same body from the row. An
  unknown or inactive id is refused by name (`schedule_not_found`) instead of
  being silently skipped (#17).
- **The watchdog deadline is reachable, a trip names its evidence, and a trip can
  be survivable.** Three keys in `colony.json` (`watchdog_threshold`,
  `watchdog_period_ms`, `watchdog_on_trip`), in the established idiom of
  `message_default_ttl`, with defaults that are byte for byte the values that
  used to be hard-wired: a missing file changes nothing, and that is pinned. A
  zero and an unknown policy string are boot errors rather than clamps, so an
  operator who mistypes finds out before a daemon runs. The trip line keeps the
  prefix every log search greps for and appends what it used to withhold: which
  side was starved, how long the silence was against the configured window, how
  late the supervisor itself was, how many heartbeats arrived after arming, how
  long it had been armed, whether the colony task is still alive, how many cells
  booted, and which policy applies. It goes to stderr and to the structured log.
  Production keeps `exit` and its non-zero code; `log-only` survives silence but
  never a gone colony task, and it reports every trip it survives (#84).
- **Every scenario case gets a port of its own, and a red line names its
  reason.** The runner rotated eight ports across 46 cases, so a case inherited a
  predecessor's number and whether its sockets had drained was left to chance.
  Each case now binds a port probed immediately beforehand, deliberately without
  `SO_REUSEADDR` so that a draining one is refused rather than inherited, never
  reused within a run, with a retry that takes a fresh number instead of the same
  one again. A failure now carries the daemon's exit state and the tail of its
  log. That diagnosis turned out to be the more valuable half: the failures three
  earlier packages had read as port collisions were watchdog trips, and the
  control run proves it (the unchanged runner on rotated ports produced the same
  numbers) (#74).
- **Instantiation binds secrets late instead of writing them down.** A
  placeholder belongs either to the environment or to the instance, and that
  ownership, not its importance and not its content, decides when it resolves.
  `${VAR}` and `${VAR:-default}` belong to the environment: they stay literal
  tokens in the `config.json` that instantiation writes, and they resolve in
  memory on every read, at boot and at instantiation alike. `${ctx.*}` and
  `${uuid7:*}` are the identity of the node and still resolve once, at
  instantiation, unchanged. The escape form survives the write and is consumed by
  the read. Every surface that instantiates goes through the one writer that now
  produces two views of the same configuration, a disk view and a runtime view,
  so a freshly born cell sees exactly what the same cell sees after a reboot. The
  price is spoken rather than hidden: an environment variable is a permanent
  dependency of the instance now, and a missing one fails the next boot loudly
  with the variable's name instead of letting an empty key through (#20).

### Notes

- **All seven came out of use, not review.** The findings are the ranked output
  of a practice run of the tool cells against a test colony, plus the
  measurements that run provoked. Three of them stopped an agent turn outright,
  and the two that looked like infrastructure noise turned out to be the same
  defect wearing two hats.
- **The watchdog root cause contradicts the assumption the issue was written
  with.** Nothing is being starved. The heartbeat is sent at the head of every
  iteration of the colony loop, which makes the deadline a statement about how
  long a *single* iteration may take. An instantiating mutation runs
  synchronously in that task, because the colony is the only write authority, and
  on a debug build with cold caches it crosses half a second: hand-measured at
  280 ms warm against a 500 ms deadline, a factor of 1.8 that flips with cache
  state. The evidence is in the trip line of all 35 observed trips: the
  supervisor held its own tick to the millisecond, the colony task was alive, and
  the trips happened on an idle machine 0.6 to 0.8 s after arming. A run under
  the survivable policy, with the deadline left at its default, passed every
  assertion of every case that had tripped. The architectural cut that would let
  a watchdog tell a long legitimate iteration from a hung one is bigger than this
  release and stays open on the issue.
- **Secrets bind late going forwards only.** Nothing rewrites a tree that already
  carries a resolved value: no migration, no touching of existing instances. The
  rule applies from the next instantiation on, and cleaning up an older tree is
  an operator's job.
- The public surface this release adds is three watchdog keys in `colony.json`,
  `params.max_bytes` on the `web_fetch` cell, and `trigger` in the timer's op
  field. No new `error_code`, no dependency added, and both frozen routing
  corridors are untouched.
- Two halves are registered rather than built, both on their own issues: letting
  a loopback edge refresh `ttl` would be the structural fix for the round budget,
  but it is a spec change to a closed four-field form with three consequences
  that have to be decided first (#82); and what leaves an assembled context again
  is a policy question rather than a line of code, since an agent that has
  fetched a document and not yet used it loses it with the line (#83).

## [0.3.1] — 2026-08-12

The follow-up wave on 0.3.0. Six defects the statement identity track named as
flanks when it shipped, fixed in one pass, and then a seventh: the paid
measurement that was supposed to close the wave found a live regression the wave
itself had made visible, and the wave stayed open until that was fixed too.
Nothing here is a new mechanism. This is the release where the machinery of
0.3.0 gets the edges filed off that a single measured run could still find.

### Fixed

- **The extraction lane's vocabulary read has an order and a bound.** One
  uncapped `select` over `facts` served two consumers with different needs, and
  its projection had grown from two columns to ten across two releases, so a
  store with 20,000 facts shipped roughly 5.9 MB over the mailbox to answer a
  question about its vocabulary. It is two reads now: the axis hint is a
  store-side deduplicated read of the axes a subject already carries, and the
  replacement window is ordered by recency and limited. Both consumers were
  already budgeted; what was unbounded was only the answer (#68).
- **An end date is not an invalidity.** The nightly supersession pass mirrored
  `valid_until` into `expired_at`, so a plan with a deadline in the *future* fell
  out of the tier 0 foresight leg on the first night after it was written, not on
  the way in. The same rule sat above the attributed closures and could overwrite
  a judgement with arithmetic. The mirror is gone. The two questions the
  foresight leg asks (has anyone closed this, and has the deadline passed) are
  answered by their own two columns again (#65).
- **The books of a night count every model call.** `consolidation_log` wrote
  `llm_calls` as a hardcoded 0 or 1, and the canonicalization judge, the second
  model call of every round that has an identity question to ask, was not
  represented anywhere: no field, no row, no path it could have arrived on. There
  is a receipt per model call now, read from the usage of the hop that answered,
  and the row carries prompt and completion tokens. Verified against an
  independent source: judge 10,389 plus dreamer 40,210 equals the 50,599 the row
  books for that night (#64).
- **A night describes the questions it actually has.** The instruction block of
  the nightly round was rendered whole every night, including on the nights that
  had none of those questions to ask, and it had grown from about 5.1 kB to 8.3
  kB across the statement identity wave. One set derived once now decides three
  things that used to be maintained separately: whether the round calls at all,
  which paragraphs render, and which keys the answer form declares. A call
  without a question and a question without its data section are the same
  impossibility now, not two rules. The round stays one call with all of the
  night's questions, which was always deliberate (#69).
- **The currency question reaches the axes it exists for.** An axis carrying more
  than one page of open statements was skipped rather than truncated, and on the
  benchmark corpus those skipped axes were exactly the bucket axes the whole
  track had been opened for. The skipped rule is right and was not weakened: a
  judge shown six of seventy plans cannot tell that it is looking at a bucket.
  What changed is the question such an axis gets. Over-cap axes are triaged by
  the cheap cardinality question first: a `multi` verdict is terminal (the values
  coexist, the axis is never asked for currency again), a `single` verdict puts
  the axis into a paged currency question across nights, and an undecided
  relation already stands at the head of the next night's cardinality question by
  construction. The page is the recency prefix of the *open* statements, so the
  carry between two pages is the same rule that builds the page: there is no
  cursor table and no page index, and a judgement never compares two statements
  that were not in one prompt together. Pages come out of the nightly axis
  budget, not on top of it (#66).
- **A predicate names the subject matter, not the speech act.** One fact was
  minted as `has_experience` and its own update as `plans_to_beat`, on two
  canonical predicates, so the currency question could never see them in one
  axis entry. The failure was stable across two independent extraction runs,
  which made it a property of the lane rather than a bad model day. The intention
  moved off the predicate and onto the statement, where the marker already
  existed: `fact_kind` has carried `world`/`experience`/`foresight` since the
  tier 0 foresight leg was built, so the speech act in the predicate was never
  the source of the plan/fact distinction, only its duplication, and that
  duplication is what split the axis. No new column, no migration. The two
  readers that used to tell a plan from a fact by its predicate say so
  explicitly now (`(planned)` on a rendered bundle line, `"intent": "planned"` on
  a statement in the night payload), and both markers are absent when they say
  nothing, so a world statement renders byte for byte as it did before. The
  night gained the rule that keeps the shared axis honest: an intention never
  closes something that happened, and something that happened never closes an
  intention. Alongside it, the extractor's replacement window stopped being the
  eight most recently touched axes and became a subject-matter selection over a
  larger recency pool, because recency is a good guess at which axes matter and a
  bad guess at what *this turn* is about. The selection is deterministic and
  model-free (#67).
- **A replacement points forwards in time, never backwards.** The extractor names
  which statement in its window a new fact `replaces`, and nothing checked which
  of the two was younger. On the measurement corpus one of three live extractor
  closures ended a statement three days *newer* than the fact replacing it, and
  it cost the run its only wrong answer. Two guards now, neither of them a model
  call: the write path refuses a replacement whose target was last asserted after
  the replacing fact begins (the fact is still minted, both statements stay open,
  and the refused pair is receipted under the batch key, because a silent drop
  moves no counter), and the night takes back an inverted extractor closure by
  arithmetic before it asks anything, since an end that precedes its own start
  needs no opinion. A judged closure is never touched by the direction check
  (#71).

### Added

- **`distinct` on the store cell's `select`.** A boolean, default `false`, that
  deduplicates *the projection*: two rows agreeing on every requested column are
  one answer, and a `limit` then counts answers instead of rows. It lets a set
  question ("which column combinations does this table carry?") be settled where
  the rows are instead of shipping all of them first. Under `distinct` every
  `order_by` column has to be projected, otherwise `invalid_input`: SQLite
  accepts the other form and sorts by a value the deduplicated rows disagree on,
  and a prefix of an unspecified order is not one. This is the only public
  surface this release adds, and it is what #68 is built on.

### Measured

One paid run at the end of the wave, 1.58 USD against a 4.00 cap, in the same
shape as the track-end run of 0.3.0 so the columns continue that table: the same
eight LongMemEval knowledge-update haystacks, fresh extraction, one consolidation
round per store, the same eight questions again, judged by a model from a
different vendor family than the one answering.

- **The one wrong answer that survived the entire statement identity track is
  right.** The 5K case now sits on one canonical predicate instead of two, the
  new value was minted as a fact rather than as a plan, the night closed the old
  value with attribution, and no fact anywhere in the corpus was ended by an
  intention. The answer went from "your personal best time in the charity 5K run
  was 27:12" to "your personal best time was 25:50, an earlier charity 5K
  personal best was 27:12, which was later superseded". Judged false to true.
- **The speech-act vocabulary collapsed.** Across the eight stores, 307 rows over
  57 distinct `plans_to_*` keys became 9 rows over 2. Those buckets were the
  over-cap axes, which is why the over-cap count fell without anything removing
  an axis.
- **Nothing is silently skipped any more.** Axes carrying more than one open
  statement went 185 to 205, of which the over-cap ones went 72 (38.9 %) to 49
  (23.9 %), and all 47 over-cap axes the eight nights actually saw were triaged:
  24 answered terminally as enumerations in a single night, 23 still owing a
  cardinality verdict and standing at the head of the next night's question. The
  pool bound was never close to binding.
- **The judged slice moved 6 of 8 to 7 of 8**, and it is a different question
  that is wrong: the run found the inverted extractor closure that became #71,
  fixed in the same wave.
- **Every model call of a night is booked with its tokens**, cross-checked
  against the colony message log to the token.
- **The night got slightly more expensive, as designed.** Judge prompt tokens
  over eight nights went 73,659 to 77,133, a 4.7 % increase for the conditional
  sections plus a page marker that was never rendered at all, because no axis on
  this corpus needed one. On a quiet night the instruction block is down to about
  a quarter of what it was. Extraction prompt cost went up 9.9 % per call, which
  is the naming rule of #67 measured, and cheaper than that package's own worst
  case.
- **The honest limits of this run.** Chain fire moved 0.0 % to 1.3 % of
  candidates, which is small but is no longer zero, and it is non-zero *before*
  the round, which is only possible because the extractor closed something.
  Retrieval R@1 stayed at 8 of 8, unchanged. And the paged currency path of #66,
  the expensive half of that fix, is pinned by 16 crate tests and a scenario but
  **has not fired on a live corpus**: not one over-cap axis here is a
  single-valued relation, so `functional = 0` and `paged = 0`. Correct by
  construction and vacuously so. Whether paging works against a live judge is
  still unbought.

### Notes

- The wave opened three flanks of its own and all three are tracked rather than
  swallowed: the extraction lane claims about 1.5 times the turn count under
  sustained ingest, because the new extraction cycle outruns the eval harness's
  stall detector and pays for reclaimed batches twice with no effect on
  correctness (#72); a closure written across two spellings of one relation hides
  the spelling that needs merging, because the night's identity questions read
  open rows only, which predates this wave and surfaced here for the first time
  (#73); and the scenario runner rotates eight ports across 46 cases and
  occasionally collides with TIME_WAIT (#74).
- The memory hive itself, its extraction prompt, its recall script and its dream
  glue live outside the published tree, as they have since 0.1.14. The public
  surface of this release is the store cell's `distinct` flag and nothing else.
- Both frozen routing corridors are untouched, no dependency was added, and no
  new `error_code` was introduced.

## [0.3.0] — 2026-08-12

The statement identity wave. 0.2.0 answered *when are two remembered things the
same thing* on the write side, and left the follow-up question standing: once
both versions of a fact finally sit on one axis, which of them is still true?
Answering it by ordering is what a memory does when it has nothing better, and
it is wrong on every axis that legitimately holds more than one value at a time.
**The value becomes part of the identity, and an interval closes only because
someone said so, with their name on it.** Nothing supersedes by arithmetic any
more.

### Added

- **Statement identity.** The supersession unit moved down from the axis
  `(canonical_subject, canonical_predicate)` to the statement
  `(canonical_subject, canonical_predicate, canonical_claim)`. The claim rides
  the same generic canonical binding 0.2.0 built for subjects and predicates, so
  it costs one declaration row and no new Rust: the written claim is never
  modified, the derived column is filled by the store on every write, and a
  revert is a `delete` on the alias row plus one `canonicalize`. The axis stays
  what retrieval groups on and what the bundle renders. Correctness moved down
  to the statement, recall stayed coarse on the axis (#13).
- **Explicit closures, with attribution.** Ordering arithmetic now supersedes
  only a re-assertion of the *same* statement. Everything else needs a closure,
  and a closure is one attributed `update`: `expired_at`, `superseded_by` and
  `closure_source`, never a rewrite of a written value and never a delete. Two
  producers write them and each signs its work. The nightly judge closes what it
  can argue about (`judge:<run_id>`, reason in the run receipt of the same
  round), and the extractor closes in the turn (`extract:<batch_id>`) when the
  fact it just wrote replaces one it was shown. A wrong closure of either kind
  reverts with one `delete ... where source`, and the values come back untouched
  because none was overwritten on the way in.
- **The replacement window, and a guard rail on it.** Before the extractor mints
  anything it is shown the open statements of the axes its subject already
  carries, ranked by recency, one page per axis and an axis too long skipped
  rather than truncated. An extracted fact may carry `replaces: <id>`, and only
  an id that was provably in that window becomes a closure. The window is parked
  under the batch key and checked again at write time, and a judged closure can
  never be overwritten by the lane. The night validates the other producer in
  turn: recently extract-closed statements travel back into the axis page, and a
  contradiction clears the attribution instead of arguing with it.
- **Cardinality as a judged property of the predicate.** Whether an axis
  enumerates or replaces is now read with a fixed precedence: the seeded core
  list first, a judged verdict second, the learned rule last. Verdicts land in an
  additive table with a mandatory `source`, so *why does this axis enumerate* is
  answerable from data. The round only offers relations neither the seed owns nor
  the store has decided, and a verdict about a seeded relation is discarded
  outright, so the precedence can never become a contradiction in the rows. The
  effect stays presentation: no closure is ever derived from a cardinality.
- **A session guard on the learned rule.** Coexistence evidence only counts
  across different sessions now. The old rule read two facts sharing a
  `valid_from` as proof that an axis enumerates, which on a corpus that stamps
  one instant per conversation is not evidence at all. It shipped first and on
  its own because it addressed the majority of the measured defect by itself.
- **Claim aliases for rewordings.** The same axes the currency question reads are
  read a second time with a different question: do two open statements say the
  same thing? A yes becomes an alias on the claim dimension, the existing
  `canonicalize` pulls the derived column behind it, and the re-assertion
  arithmetic finishes the job, so a rewording becomes history of one statement
  while a real change stays a statement of its own. Numbers and quantities are
  never a rewording (a prompt rule with a scenario trap behind it), refusals are
  persisted and travel back into the next payload as `known_different`, and the
  keyword index deliberately stays on the *written* claim, so recall on the
  original wording survives the merge.
- **A currency marker in the bundle.** A superseded candidate carries
  `superseded by: <claim>` on its rendered line, the inverse of the existing
  `previously:` mechanism. Closed statements stay in the bundle rather than
  dropping out of it: dropping them would destroy history questions and hide the
  store's own uncertainty from the model reading it.
- **A dying cell hands over its mailbox.** Messages queued for a cell that panics
  are no longer lost. A guard owns the mailbox receiver for the lifetime of the
  task; its drop runs on unwind and on task abort, drains what is left into a
  colony message, and the successor receives it before the death is acknowledged.
  The peaceful exits are unchanged and disarm the guard themselves. The frozen
  routing corridors were not touched (#18).

### Changed

- The bundle contract gained the `superseded by:` marker on a rendered candidate
  line and nothing else. No new field, no field changed shape.
- `axis_is_multivalued` is a presentation tie-breaker now, not a decider. The
  error direction of a wrong verdict is therefore the harmless one: it can fail
  to mark an outdated value, it can no longer end a true one.
- `apply_fts_ddl` substitutes each bound column independently instead of all or
  nothing, so an index written before a second identity dimension existed
  migrates in one wake even across a skipped release. Migration stays a wake
  everywhere: the added columns, the cardinality table and the alias and refusal
  tables of the claim dimension all land on the first spawn after the upgrade,
  additively and idempotently, with no tool and no manual step.

### Measured

One paid run at the end of the track, 1.26 USD, in the shape the design ruling
demanded and 0.2.0 never bought: the same eight LongMemEval haystacks, **one
consolidation round first**, then the same eight knowledge-update questions
again, judged by a model from a different vendor family than the one answering.

- **The mechanism works and was seen working on a real judge.** Over the eight
  nights the round produced **two closures, both correct and each carrying its
  reason and its author**, merged 15 rewordings while refusing 21, and answered
  64 cardinality questions. The scenario cases of the track add the shapes a
  haystack does not offer, each on a live model answer: a replacement closed
  with the reason "a person lives in one place at a time", an enumeration trap
  refused in the same call, two wordings merged while the quantity between them
  was refused twice. No row was destroyed, no written value was rewritten, and
  every verdict carries the run that made it.
- **Enumeration now carries a source.** The share of multi-version axes read as
  enumerations went from 20.2 % to 52.5 % over one night, and the composition is
  the point: **40 seeded, 64 judged, 0 learned**. The learned half, which was the
  entire defect a wave ago, contributes nothing after the session guard. That
  counter has stopped measuring a defect and started measuring how much of the
  vocabulary the store has an answer for, which means it should be re-stated as
  *enumerations without a source* versus *with one* before the next wave uses it.
- **Judged answers went from 6 of 8 to 7 of 8** across the round. The honest
  reading of the one that flipped: nothing chained on that store. The round's
  rewrites re-ranked the candidates and moved the current value inside the window
  the answering model actually counted. The right answer, for a reason nobody
  designed.
- **The two flanks, stated as flanks.** Chain fire stayed at 0.0 % of candidates
  before and after, and the run measured why rather than arguing about it. First,
  **72 of 185 axes with more than one open statement carry more than six of them
  and are skipped rather than truncated** by the per-axis page rule, and on this
  corpus those are exactly the bucket axes the track was opened for; the nightly
  budget is the smaller problem (8 axes a night already reach 57 % of the
  reachable ones). That is #66. Second, the one wrong answer of the previous wave
  is **still wrong**, and the round proves the cause is upstream of every
  judgement this wave built: one fact was minted as an experience and its update
  as a plan, on two different predicates, so the currency question can never see
  them in one axis entry. The failure is stable across two independent extraction
  runs, which makes it a property of the extraction lane. That is #67, and it is
  extraction identity, not garbage collection.

### Notes

- **The public surface of this release is again the store cell**: the third
  canonical binding is a declaration over the ops 0.2.0 shipped, plus the wake
  migrations that carry an existing `cell.db` onto the claim dimension and the
  judged cardinality table. The memory hive itself, its extraction prompt, its
  recall script and its dream glue live outside the published tree, as they have
  since 0.1.14.
- Two flanks the track opened on its own account are tracked rather than
  swallowed: the extraction lane's vocabulary read is still uncapped on the store
  side and each row grew threefold (#68), and the night's instruction block has
  tripled to about 8 kB, paid by every judge call including the quiet ones (#69).
- Both frozen routing corridors are untouched, no dependency was added, and no
  new `error_code` was introduced.

## [0.2.0] — 2026-08-12

The memory quality wave: seven issues, one epic, and a single question
underneath all of them. When are two remembered things the same thing? A memory
that answers that question badly does not forget, it *splits*, and a split axis
carries no version chain, so the same memory hands out last year's value next to
this year's and presents both as current. **Identity is decided when a fact is
minted, and repaired afterwards by a judgement, never by a threshold.** That
sentence is the release.

### Added

- **Predicates are keys now, and the extractor knows it.** Canonical predicates
  are English snake_case, seeded by a curated core list of 29 relations (each
  with its cardinality and a gloss) and reinforced by a vocabulary round trip:
  before it mints anything, the extractor is shown the axes the subject already
  carries and has to reuse a spelling that means the same thing. The opposite
  rule protects everything that is not a relation. **Entities are verbatim**:
  subjects, objects, values and proper names are never translated and never
  "corrected when unknown", so a village nobody has heard of reaches the store
  byte-identical (#21, #22).
- **Entity aliasing with an automatic half and a judged half.** The only
  automatic merge is normalization equality (Unicode composition, case fold,
  whitespace collapse), computed in Rust and mirrored as the SQL function
  `meclaw_norm`. Two spellings equal after that are provably one entity, no
  model involved. Everything fuzzier is a trigram Dice score served by the new
  `alias_candidates` op, which reads, scores, sorts, and decides nothing.
  No similarity threshold merges anything, because thresholds lie on short
  names, on sibling names, and on one-letter differences between two real
  places (#23).
- **An alias table plus store-owned canonical columns, feeding all three read
  legs.** `params.canonical` binds `{source, target, aliases, normalize,
  rejected}` per table. The written value is never modified, which is what makes
  the whole mechanism revertible; the derived column is filled by the store on
  every write, so no writer can forget it. The keyword leg indexes that column,
  the axis anchor filters on it, and chain derivation groups by it: one
  alias-aware place instead of three. Reverting a judgement is a `delete` on the
  alias row plus one `canonicalize`, and every fact falls back onto its
  untouched original because none was overwritten on the way in (#24, #22, #23).
- **An index-time stemming tokenizer, German and English in one table.**
  `meclaw_stem` wraps `unicode61` through the FTS5 extension API (no loadable
  extension, no new dependency) and folds each token to a conservative stem: two
  ordered steps, each firing at most once, minimum stem three characters, `-s`
  stripped only after a consonant that actually takes one. FTS5 runs a table's
  tokenizer over the index text *and* over the query text, so the plural
  question meets the singular fact without anyone expanding a query. The defect
  behind the issue was a fact that scored exactly zero for the question it
  answers (#14).
- **The canonicalization dream round.** Once a night, one model call of the top
  tier carries both questions in one payload: which of these relation keys are
  the same relation, and which of these entity spellings are the same entity.
  Accepted pairs become aliases, refusals become rows in a refusal log so the
  same pair is not bought again the next night, and one `canonicalize` pulls the
  derived columns behind them. The round sits in *front* of the supersession
  arithmetic rather than after it, because that arithmetic groups on exactly
  those columns, and a merge landing a night late would leave the materialized
  cache disagreeing with the read path for 24 hours.
- **The invariance gate, split in two.** A consolidation run is now measured
  from both sides: an *invariance set* of uninvolved questions must answer
  byte-identically before and after the round (the 0.1.14 criterion, unchanged),
  and an *improvement set* has to move measurably toward truth. A regression
  anywhere discards the run.
- **Verbatim repeated episodes collapse inside the bundle.** Copies that are
  identical under the same normal form take one slot instead of one each. The
  best-ranked copy keeps rank and score, the newest copy keeps the wording and
  the identity, the legs of the swallowed copies merge into the survivor, and
  the rendered line carries `(seen: N)`. Nothing is deleted and nothing is
  judged: level 0 stays append-only (#15).
- **Tier 0 says so when it ignores a window.** A complete recall window on a
  tier-0 request used to be dropped in silence. It is now marked in all three
  places a consumer might look: `bundle.window_ignored` in the body, a
  `- window_ignored: <from> -> <until>` line in the rendered text that the model
  actually reads, and `hop.window_ignored` for a router. No window, no marker,
  nothing changed (#16).

### Changed

- The bundle contract gained two additive fields: `seen` on every episode
  candidate (always present, `1` when nothing collapsed) and `window_ignored` on
  a tier-0 bundle (present only when a window was sent). No existing field
  changed shape.
- `apply_fts_ddl` learned two drift classes next to the additive one: a
  *canonical* swap (the declared list is the existing one with a binding's
  source substituted by its target) and a *tokenizer* rebuild (the column list
  is identical and only the tokenizer differs, which neither of the other
  classes can see). The two compose, so a store still carrying the 0.1.x shape
  reaches the new one in a single wake.
- Migration is a wake, everywhere. Added columns, alias tables, refusal logs and
  the rebuilt index all happen on the first spawn after the upgrade, additively
  and idempotently, with no tool and no manual step.

### Measured

One paid measurement run, 0.42 USD in total, on eight LongMemEval haystacks
against the documented pre-wave baseline.

- **Axis identity at mint time improved by a factor of two and a half.**
  Distinct predicate keys went from **2472 to 734**, and predicates that are not
  keys at all (capitals, spaces, whole sentences of prose) went from **247 to
  0**. Distinct axes went 2723 to 844, facts per axis 1.18 to 2.91, and the
  share of facts sitting on a multi-version axis went from 22.7 % to 73.2 %.
- **Retrieval improved and answers followed.** R@1 on the knowledge-update slice
  went from **87.5 % to 100 %**, R@5 held at 100 %, and **7 of 8 judged answers
  carry the new version** of the fact.
- **The headline counter did not move, and the run says precisely why.** Chain
  fire stayed where it was (1.9 % against a 2.1 % baseline on the same slice),
  for two measured reasons, neither of them the extractor being sloppy. First,
  **143 of 187 multi-version axes are classified as enumerations**: the
  coexistence heuristic reads two facts sharing a `valid_from` as proof that an
  axis enumerates, and this corpus stamps one instant per *session*, which every
  turn of a conversation inherits. The heuristic is doing exactly what it was
  written to do, on a signal the corpus cannot provide. That is statement
  identity (#13), and it now has data behind it instead of an argument. Second,
  the pairs that still sit on different keys are exactly what the nightly round
  exists to merge, and the benchmark harness disables the dream cron by design
  so that a consolidation run can never interfere with a measurement. The wave's
  answer to that half is therefore structurally absent from this number.

### Notes

- **The public surface of this release is the store cell**: `params.canonical`
  and its bindings, the ops `set_alias`, `canonicalize`, `alias_candidates` and
  `reject_pair`, the alias and refusal tables, the `meclaw_stem` tokenizer and
  the `meclaw_norm` function, and the migration paths that carry an existing
  `cell.db` onto all of it. The memory hive itself, its extraction prompt, its
  recall script and its dream glue, lives outside the published tree, as it has
  since 0.1.14.
- **A price carried on purpose:** the tokenizer lives on the SQLite connection,
  so an external tool opening a `cell.db` directly can no longer read the
  `<table>_fts` index (`no such tokenizer: meclaw_stem`). The base tables are
  untouched, which covers everything an operator normally asks of that file.
- **A judged slice after a dream run is the missing measurement**, and it is the
  cheap one: the same eight colonies, one consolidation round each, the same
  questions again. This release deliberately bought a single judged run.
- Both frozen routing corridors are untouched, no dependency was added, and no
  new `error_code` was introduced.

## [0.1.16] — 2026-08-11

The hardening release: the whole 0.1.x queue, emptied in one sweep. Ten issues
closed, six of them found while fixing the other four. The theme is a single
sentence: **a failure inside the colony task must cost one cell, never the
process** — and its corollary, a failure outside the process (a blackholed
socket, a lost connection, a moved tree) must cost one reconnect, never a cell.

### Fixed

- **A malformed seed file passed validate and killed the colony on first wake.**
  Seed parsing now runs in the validate path (header line, columns against the
  declared schema, every data line a JSON object), guards the spawn, and the
  wake path logs instead of panicking (#56).
- **The watchdog armed during boot and exited 0 on trip.** It now waits for an
  arming signal sent only after a successful bootstrap, discards boot-buffered
  heartbeats, and reports a trip on stderr with a nonzero exit (#6).
- **A silently hung proxy looked like idle.** Long-running cells report a
  last-successful-round-trip mark over the existing heartbeat mechanics, and
  `/health` lists the age of each mark (#7).
- **The Slack socket mode read loop had no idle deadline.** Every read now sits
  under `idle_timeout_ms` (default 120s, four missed pings at Slack's slowest
  documented cadence); an elapsed deadline is a transient connection end
  feeding the reconnect machinery (#50).
- **stderr of a successful code script was dropped.** It lands in the log as
  the promised warn line, capped at 8 KiB on a char boundary (#44).
- **The chat_id promotion edge had no end-to-end pin.** Six tests boot a real
  colony around the shipped bot templates; the silent missing_chat_id death
  turned out to require two mistakes at once, and both shapes are pinned (#49).
- **Embedding calls were missing from token accounting.** The embed lane books
  `tokens_prompt` the way llm cells do; a batch backfill books exactly once (#9).
- **A cancelled DbConn blocking task panicked the process at shutdown** (#11),
  and **a call future dropped by a timeout wrapper lost the connection for
  good** (#59). The first parks safely, the second reconnects lazily from the
  remembered path, replaying the standard cell-db setup plus the store's
  scalar-function hook.
- **The store wake and respawn paths carried eleven expects between them.**
  Both are panic-free now: a database that will not open starts or respawns
  the cell degraded, answering every message with a named `sql_error`; the
  soft failures log loudly and the cell runs without that one feature
  (#57, #63).
- **A restored template index kept pointing at the source machine.** An index
  with any row outside the booted templates root is treated as foreign and
  re-anchored by a full rescan on the first boot (#61).

### Added

- **An import/export matrix**: six move paths crossed with nine artifact
  classes, 23 pins, and a new spec section — *Snapshot versus live-read* — in
  `docs/config.md` stating what boots live, what seeds exactly once, and why
  the restore unit of a colony is the root directory including the WAL
  sidecars (#37, #60).

### Changed

- `/health` returns JSON (`status` plus `io_liveness`) instead of a bare `ok`;
  the status code semantics are unchanged (always 200).
- CI markers for the child-process fixture widened to 120s: a failure marker
  is a detector, not a discriminator (#58).

## [0.1.15] — 2026-08-11

Four defects, none of them found by the test suite. All four came out of a
memory lane running in production, and each one had been invisible in a
different way: an answer that was computed and then not printed, a poll that
hung behind a healthy heartbeat, an extractor that wrote down the conversation
instead of the world, and a retrieval that drowned the answer in copies of the
question. **A timeout that covers only half the operation is not a timeout** —
that is the lesson of the one Rust change in this release.

### Fixed

- **A superseded fact's history reached the JSON but not the rendered text.**
  Chain projection produced the candidate correctly, with `history` attached —
  and the text block handed to the model printed the claim alone. Asked which
  editors it had seen over the last seven days, the agent answered with the
  current one and dropped the switch it was holding. Point mode now appends the
  prior versions (`vscode (previously: Helix until 2026-08-08)`), window mode
  renders the derived span (`[from -> until]`, an open end as `-> open`), and a
  candidate without history or span renders byte-identically to before. The
  scenario runner captures the rendered block from now on, so an assertion can
  pin the **presentation** and not only the computation: this defect was a
  correct bundle with a lossy text.
- **The Telegram long poll was covered by a header-phase timeout only.** In
  production the proxy went quiet for 15 minutes and 42 seconds — connection
  established, updates waiting on the other side, colony heartbeat normal, not
  one log line. The `tokio::time::timeout` wrapped `.send()`, so it expired on
  the response head; the body read behind it ran uncapped, and the HTTP client
  carried no timeout of its own. A peer that sends headers and then falls silent
  (no FIN, no RST) therefore hangs the colony's only outbound connection
  indefinitely, and hangs it *silently* — the watchdog sees a live cell, because
  the cell is alive, merely stuck in an await. The rule-12 deadline now covers
  the **whole** operation, budgeted as `long_poll_request_secs * 1000 +
  long_poll_timeout_ms` so that it can never cut a legitimate long poll and is
  still finite; expiry is transient (the backoff ladder keeps the lane alive)
  and gets its own greppable warning rather than a debug line. Proven by a
  fixture server that writes the response head and never the body — a half-dead
  peer is the only shape that tells a header deadline apart from an operation
  deadline.
- **The extractor minted facts out of the conversation about facts.** It wrote
  an axis from the *question* (`user | asked | …`) and another from the agent's
  own previous *answer* — a self-reference loop, and a fourth spelling of an
  axis that already existed three times. Both flooded the recency-sorted
  temporal leg and pushed the fact that actually answered the question out of
  the bundle. The extraction prompt now states the distinction up front: turns
  are already stored as episodes, so *that* something was asked, answered or
  discussed is never a fact, and a turn carrying no world state correctly yields
  an empty list. An assistant turn is a previous answer of one's own and
  contributes only genuinely new material; a user turn that confirms or corrects
  very much carries world state.
- **The often-asked question drowned its own answer.** In a measured live
  recall, 12 of 20 fused slots were near-verbatim copies of earlier questions,
  while the carrying fact was keyword-invisible (the question in plural, the
  index holding the singular), arrived over the temporal leg alone at rank 13 of
  14, and fell off at the top-k cut — every repetition of the question made it
  worse by one episode. Fusion now cuts by **composition** instead of taking a
  prefix of the order: episodes receive at most a configurable budget of the
  slots (default 6) and keep it against a wall of facts, and whichever side
  cannot fill its share is backfilled by the other, so the bundle never gets
  shorter. This closes the starvation side of the registered one-leg episode cap
  as well. The ordering itself is untouched — this is a membership filter, not a
  re-ranking, and the existing tie-break and attribution rules read exactly the
  list they read before.

### Measured

- **The fusion change is proven paired, on 50 retained evaluation colonies with
  identical extractions:** **0 flips**, and R@1 84.0 · R@5 98.0 · R@10 100.0
  byte-stable across every question class. That it does anything at all is shown
  by the other half of the same comparison: **23 of 27** pairs with an identical
  query vector came out with a different list composition. Composition moves,
  quality does not — which is precisely the claim, since the defect it repairs
  only bites once a question has been asked many times.

### Notes

- **The public surface of this release is the proxy timeout.** Everything else
  is the private `memory-hive` template — its recall script and its extraction
  prompt — which lives outside the published tree, as in 0.1.14. What ships
  publicly is the long-poll deadline, its test, and the new hanging-body mock
  server in `meclaw-testing`.
- Both frozen routing corridors are untouched, no dependency was added, no new
  `error_code` was introduced, and the cell type registry is unchanged.
- Two findings are registered rather than fixed: cell I/O liveness is invisible
  to the heartbeat watchdog (the silent poll above ran for sixteen minutes
  behind a healthy heartbeat), and a dream run cannot be triggered from outside
  a colony — three routes were tried and all three are closed by design.

## [0.1.14] — 2026-08-11

Which version of a remembered fact holds is now decided when the fact is read,
not when a nightly job gets around to it. Two sentences carry the release, and
both are a test now instead of an intention: **dreaming is garbage collection,
not conflict resolution**, and **facts dumped in during the day answer exactly
like facts that have been tidied up — only slower**.

### Added

- **Supersession is derived at read time.** Which version of a
  `(subject, predicate)` axis is current follows from the chain of its
  `valid_from` values at the moment of the query. `expired_at` is demoted from
  truth-bearer to cache: the dream run still writes it, but nothing on the read
  path depends on it having run. The invariance criterion that follows — the
  same question returns the same answer before and after a dream run — is
  enforced by a scenario gate rather than argued in a design document.
- **A superseded hit is annotated, not filtered away.** It projects onto the
  current fact of its axis and attaches to it as `history`. The old guarantee
  survives (no stale claim is presented as current truth) and the question
  "and what was it before?" becomes answerable — out of a single recall,
  without a second round trip.
- **Coexistence is distinguished from replacement, and the distinction is
  learned from the data.** Two facts sharing one `valid_from` do not supersede
  each other; only a strictly later start closes a span. And once an axis has
  demonstrated coexistence, the whole axis is treated as multivalued from then
  on, monotonically: a third value arriving months later does not end the two
  already there. "Has a son" is not "lives in" and the store finds that out by
  itself instead of being told per predicate. The design pass behind this is
  backed by a literature review (OWL functional properties, Wikidata ranking,
  PARIS/AMIE functionality degree, SQL:2011 system-versioning); what shipped is
  its conservative recommendation, with statement identity noted on the roadmap
  as the target picture.
- **Window retrieval with an explicit reject lane.** The temporal leg takes
  `recall_window_from` / `recall_window_to` through port and context in addition
  to the as-of point mode, and cuts a real interval instead of a snapshot. A
  half-open window (exactly one bound set) is rejected as invalid input rather
  than silently completed with a default — a wrong window is worse than a
  refused one. Who derives the window is deliberately the consumer's job: the
  memory lane does not guess a time range from prose.
- **Prefix matching in the keyword leg, at word end only,** so a query term can
  grow but cannot leak into unrelated stems — and **`predicate` in the full-text
  index**, so the relation is searchable, not only the claim.
- **Fusion rules decided from measurements, not from taste.** An exact score tie
  is broken by the temporal leg instead of by whichever UUID happened to sort
  first (identical RRF sums are genuinely reachable with symmetric ranks, and a
  freshly minted UUID is not a tiebreak, it is a coin flip); in window mode the
  temporal order binds. And in point mode the temporal leg no longer votes at
  all — see below.

### Fixed

- **The FTS index can migrate additively instead of refusing to boot.** If the
  declared column list grows at the end and the existing columns are a proper
  prefix of it, the index is dropped and rebuilt from the base table; every
  other drift (a column removed, columns reordered) stays a loud spawn error.
  An FTS index is a rebuildable projection over source text that is never
  deleted, so dropping it destroys nothing — while silently serving a stale
  index shape would.
  **The lesson that nearly shipped a silent defect:** `DROP TABLE "<t>_fts"`
  does **not** remove the three external-content triggers, and the following
  `CREATE TRIGGER IF NOT EXISTS` would then have kept the old column list
  alive. The rebuild would have looked correct — rows written *after* the
  migration would simply never have reached the new column, forever and
  quietly. The triggers are now dropped explicitly, and the test proves it with
  a row inserted after the migration, not with the backfill alone.
- **An as-of query still filtered on the cache it was built to replace.** The
  temporal leg's select carried `expired_at IS NULL`, so after a dream run it
  shrank from two rows to one and the surviving candidate's fusion score moved
  — the same recall, a different answer, depending only on whether the nightly
  job had already run. Exactly what the invariance criterion forbids. The
  window branch a few lines below was already correct; both now state it
  literally: `expired_at` does not appear in a `where` clause.
- **Axis collapse silently dropped the retrieval legs of the hits it
  swallowed.** When two hits of one axis collapse into a single candidate, the
  survivor now carries the **union** of all collapsed hits' legs, deduplicated
  and in canonical order. Score, rank and fusion order remain those of the first
  winner — this is attribution, not re-ranking: "found by leg X" is a statement
  about discovery and belongs to the axis, not to whichever hit happened to
  place first. The defect was pre-existing and had been masked by a weighting
  that made the right hit win by accident.

### Measured

- **LongMemEval, medium stage (50 questions, stratified across question types):
  R@1 84.0 · R@5 98.0 · R@10 100.0** in the shipped configuration. This is a
  measurement of one retrieval lane at one corpus size, not a claim about the
  substrate.
- **The finding that changed the configuration: across 100 runs the temporal leg
  never once found a hit on its own.** Its value is the as-of/window *cut*, not
  its vote in the fusion; what its vote did was displace better candidates. The
  proof is a *paired* comparison on identical extractions — two separate runs
  produce different extractions, so an unpaired number says nothing about which
  leg produced it. Paired, the weightless arm stands at **R@1 84.0 vs 74.0** and
  **R@5 98.0 vs 96.0**, with **11 flips to 1** in its favour (sign test
  p = 0.0063). Hence: no temporal weight in point mode (new knob, default off),
  full weight retained in window mode, where the temporal order is the answer
  rather than an opinion.
- **The benchmark harness grew three capabilities, each born from a failed
  run:** stratified sampling (the dataset is sorted in blocks by question type,
  so a naive first-n slice measured one type fifty times and said nothing about
  the rest), per-run environment overrides with a separate output directory
  (so an ablation cannot overwrite its own baseline, and every override is
  echoed into start log, per-question record and report — a run with changed
  settings and an unchanged-looking report is the one silent mismeasurement this
  harness must not produce), and a boot post-mortem with retry (a daemon that
  dies during boot is now reported dead with its exit code and the tail of its
  log, instead of timing out 120 seconds later as "never came up").

### Known limitations

- **Chain projection starves where the extractor drifts.** In every
  knowledge-update case examined, the old and the new version of a fact sat side
  by side *unchained*, because the extractor had assigned them divergent
  predicates — so the projection never fired, and in a quarter of them the
  outdated version ranked first. The read path is correct; the axis it needs is
  not always produced. Entity and predicate deduplication is registered on the
  roadmap as its own package, now with harder evidence than the anchor-rate
  estimate it previously rested on.
- **The byte-equality promise holds only modulo the remote embedding.** The same
  query text embedded repeatedly yields different vectors and therefore
  different semantic orderings. Everything downstream of the embedding is
  deterministic; the embedding itself is not, and no assurance in this repo
  should be read as covering it.
- **Four substrate findings from the benchmark are registered, not fixed:** the
  heartbeat watchdog exits 0 (an emergency stop is indistinguishable from a
  clean SIGTERM for any supervisor above it), it is armed before boot finishes
  (enough parallel boots and it declares a healthy bootstrap dead), embedding
  calls are missing from token accounting (`hop.tokens_*` only comes into
  existence in the `llm` cell, so an embedding's cost is a list-price estimate
  in a field named `usd_measured`), and episodes structurally reach fewer
  retrieval legs than facts while the fusion rewards leg count.

### Notes

- **The public surface of this release is the FTS drift migration above.** The
  memory lane this package works on is a topology, not substrate: it lives in a
  private template tree and is not part of the published clone, as with the
  builder hive in 0.1.11. What ships publicly is the store-side migration, its
  unit receipts, and the documentation of the new drift semantics. The
  integration tests that pin the template's own scripts run against that private
  tree and are therefore excluded from the export by name, each with its reason
  recorded in the export script — the same treatment the Slack template smokes
  already receive.
- Both frozen routing corridors are untouched, no dependency was added, no new
  `error_code` was introduced, and the cell type registry still holds 13 types.

## [0.1.13] — 2026-08-10

The subscription lane shipped in 0.1.10 and had never completed a single real
call. Fixture-green, live dead. This is the repair.

### Fixed

- **The subscription lane reports the Codex client version.** The backend gates
  model availability on the `version` request header; the cell sent its own
  crate version, so recent models answered HTTP 400 with
  `"The '<model>' model requires a newer version of Codex."` Configurable per
  cell via the new `oauth_client_version` param, because a backend-side bump of
  the floor must be answerable by configuration, not by a release.
- **The subscription lane no longer sends `temperature` and
  `max_output_tokens`.** The ChatGPT backend rejects both outright
  (`"Unsupported parameter: temperature"`). They remain valid on the official
  Responses API, so the cut is on `auth`, not on `wire_dialect` — the metered
  lane keeps its sampling control, and `provider_extra` stays the escape hatch
  for a caller who needs one anyway.
- **Provider rejections carry their text again.** `classify_responses_status`
  only understood the OpenAI `{"error": {...}}` envelope, while the
  subscription backend answers with a flat `{"detail": "..."}`. Every rejection
  collapsed into a bare `HttpStatus(400)` and the one actionable sentence was
  discarded — which is precisely why the two defects above stayed invisible
  through a release. The new `HttpStatusWithDetail` variant carries it into
  `meta.error.detail`; the closed `error_code` enum is unchanged.

### Added

- `llm` param **`oauth_client_version`** (`None` → provider default).
  Immutable like the rest of the auth dimension: it feeds the same backend gate
  as `oauth_originator` and flows into the same `User-Agent`, and a runtime
  overlay in `cell.db` would silently outrank a later `config.json` fix.

### Notes

- Verified against the real backend, not against fixtures:
  `plans/p14-fixtures/live-receipt.md`. The Cloudflare hazard documented in the
  P10 plan (§ 3.2, residual risk R3) did **not** materialise — no 403 in any run.
- A rare shutdown race (`DbConn` panics when the runtime cancels a
  `spawn_blocking` job) was found, diagnosed and **not** fixed here: it did not
  reproduce in 11 targeted runs, and a fix without a reliable reproduction is
  not honest TDD. Registered in `docs/roadmap.md`, diagnosis in
  `plans/p14-fixtures/panic-diagnose.md`.
- Quota behaviour on a real subscription remains unmeasured — the measurement
  deliberately exhausts the operator's plan and needs its own go-ahead.

## [0.1.12] — 2026-08-09

Slack as the proxy cell's second platform — and a lesson from the real API.

### Added

- **Slack Socket Mode support in the `proxy` cell type.** Slack is instance TWO
  of an existing cell type, not a new one: the seam is a single
  `params.platform` discriminator dispatched in the factory, optional with
  default `telegram`. Every pre-0.1.12 configuration parses to exactly the same
  result, and the registry still holds 13 cell types.
- Outbound WebSocket transport (Socket Mode): no public endpoint, no inbound
  HTTP surface. Purely frame-driven — a reconnect is always caused by an event
  (`disconnect` frame, close, error), never by a timer. Backoff damps failures
  and carries a minimum-uptime floor so a peer that accepts and instantly drops
  cannot turn the reconnect path into a hot loop.
- Thread ownership: a mention in the channel root opens a thread on its own
  timestamp; a mention inside a thread stays there; direct messages carry no
  thread; a bot keeps following the thread it opened without needing to be
  mentioned again. Anything else in a channel is ignored.
- Bot-loop guard (default on): own traffic is dropped on `bot_id`, the
  `bot_message` subtype, the sending app id, and optionally the bot's own user
  id. Ignored events are still acknowledged — silence makes Slack redeliver.
- Envelope deduplication and thread-ownership persistence in `cell.db`.
- Hermetic fake Slack (`meclaw-testing::mock_slack`) serving both the Web API
  and the WebSocket on one port, with scripts keyed per app token so a
  multi-bot claim cannot be satisfied by the fake itself.

### Fixed / learned

- **One user message arrives twice.** Verified against the live API: Slack
  delivers a mention to the addressed bot both as `app_mention` and as
  `message`, with the same timestamp but different envelope ids — and
  `message.channels` reaches every app that subscribes to it, so each bot also
  sees mentions addressed to others. Envelope dedup does not cover this (two
  envelopes, two ids). The thread-ownership rule is what keeps a message from
  entering the agent tree twice and keeps bots out of each other's
  conversations; it is a correctness condition, not a politeness rule.
- `api_app_id` on an inbound envelope names the RECEIVING app and therefore
  always equals one's own. A loop guard reading it instead of the sending app
  id discards all traffic and produces a bot that is silent on a healthy
  socket. Pinned by a dedicated negative test.
- Slack timestamps are addresses, not numbers: `ts` carries a dot, `event_ts`
  frequently does not, and a float round-trip destroys the digits that make a
  message addressable. All timestamps are kept as strings.
- **Public-clone test fixtures.** Two tests read a template config from a path
  that is not part of the published tree, so they could not run in a fresh
  clone. The file they read is now committed as a snapshot next to the tests
  (`crates/meclaw-cells/tests/fixtures/memory_hive_store_config.json`) and both
  read it from there. The snapshot does not track its source by design; the
  provenance note lives at both call sites.

### Known limitation

- The Slack variant has **no runtime params overlay**. It builds from birth
  params only, so `base_url`, timeouts and `thread_follow` cannot be changed on
  a running cell — unlike the Telegram variant. Tracked on the roadmap.

### Verified live

Against the real Slack API with two separate apps: both bots connect with
distinct app ids, each receives only its own mention and answers in its own
thread, replies carry the correct per-bot token, `not_in_channel` classifies as
a typed permanent error, and the loop guard drops a real bot post that it
provably received.

## [0.1.11] — 2026-08-09

The builder hive: a colony grows itself, gated and audited.

### Added

- **A builder that turns a request into a running subtree.** A description of
  what to build enters at one end and a deployed, validated subtree comes out
  the other: the draft is written to a staging area, checked by a gate,
  classified by an approval matrix, promoted into the template registry and
  finally deployed through a mutation — each step leaving a receipt, and the
  whole run ending as a receipt file next to the draft it produced. Nothing
  about it is new substrate; it is topology, and that is the point. Extending
  the system is a DSL act, never a recompilation.
- **Self-modification rails that rest on measured behaviour, not on good
  intentions.** Two properties carry the safety frame, and both are pinned by
  scenario cases rather than argued in prose. A cell cannot ADDRESS the mutation
  lane without an edge: every emission targets its reply address or the cell's
  own path, so a script can compose a mutation but not send one. And no mutation
  can CREATE that edge — scope containment rejects a `/colony` endpoint for
  every scope, including the root scope that owns everything else. The
  privileged edge is therefore bootstrap-only: it exists exactly if an operator
  wrote it into a configuration file, and no topology can grant it to itself or
  to anything it builds.
- **An approval matrix that classifies by effect, not by name.** Growing a new
  subtree is auto-approved because an unwired subtree is inert by construction;
  moving an edge between two things that already run is escalated, because that
  is what silently reroutes live traffic. Edges that TARGET the control plane
  are escalated as the privilege-escalation shape they are — while the edge that
  attaches a new subtree to an existing entry point is normal, and required, and
  must not be mistaken for the former.
- **A librarian, lexical by choice.** The builder retrieves patterns instead of
  carrying a corpus in its prompt: the specification, the cookbook, the example
  briefs, the template catalogue and the pinned error codes, cut by section and
  ranked with BM25. No embeddings — a lookup that answers to names does not need
  them, and every build can afford to ask.

### Fixed

- **A start-up race that could destroy a live task record.** Recovery after a
  restart ran concurrently with the first incoming message instead of before it,
  so under load a freshly written `running` row could be swept to `unknown`: a
  task in flight was reported as interrupted, and its real result arrived later
  as a second result under the same id. Recovery now completes before the first
  message is handled. The original hypothesis — a test that failed to correlate
  — was wrong, and acting on it would have hidden real damage behind a test fix.

### Notes

- No Rust was added for the builder. The public surface of this release is the
  race fix above, the specification pass that follows from building on it, and
  a placeholder for a self-hosted model endpoint in the example environment.

## [0.1.10] — 2026-08-09

Subscription auth: the `llm` cell learns a second credential and a second wire.

### Added

- **An `auth` dimension on the `llm` cell (`api_key` | `oauth_subscription`).**
  Model access no longer has to be pay-per-token. A cell can present a rotating
  OAuth token from a token store instead of a static key — no CLI harness
  between the cell and the model, which is the whole point: an agent harness
  that pre-prompts, loops and tools on its own is exactly what an `llm` cell
  must not have in front of it. The seam is vendor-neutral by construction;
  one vendor is implemented, and a second is a set of params rather than a
  rebuild.
- **A second wire dialect: Responses.** Beside chat-completions the translate
  boundary now speaks the Responses shape — typed `input[]` items, a top-level
  `instructions` slot, `max_output_tokens`, flat tool schemas. It is a
  **separate axis from `provider`**: the same vendor with a different wire is
  not a different provider, so the provider constraint stays untouched and
  `auth × wire_dialect` becomes the matrix. The wire is pinned against a
  reference implementation rather than reverse-engineered, and the fixtures are
  the drift detectors.
- **A single-refresher token broker.** The refresh token rotates, so two cells
  refreshing one store concurrently would earn a permanent `refresh_token_reused`
  and force a human back through a login. All cells in a process therefore share
  one broker actor that performs the refresh itself: single-flight by
  construction, no lock, no wait loop. A cell that hit a 401 names the token
  generation it used, so a concurrent refresher wins instead of racing.
- **A two-level error taxonomy.** The spec's `error_code` enum stays closed;
  the discriminator a failover edge actually needs — `quota_exhausted` with its
  reset time, `auth_expired`, `auth_permanent` with `re_login_required` —
  arrives in `meta.error`. Failover itself remains topology: the cell emits a
  typed error and stops. It does not retry, it does not fall back, and it never
  loops.

### Changed

- `api_key` is now optional in `llm` params, because a subscription lane has no
  key. Exactly one credential per cell is enforced at spawn, and the whole auth
  dimension is immutable at runtime — `wire_dialect` and the OAuth overrides
  decide *which endpoint* a credential is presented to, so a mutable one would
  let a message redirect an existing token somewhere new.
- The token store is written as a **patch, not a rewrite**. It is the vendor
  CLI's own credential file that MeClaw is a second writer of; rotation touches
  three token fields and a timestamp and leaves every unknown field alone. A
  naive rewrite would have destroyed an interactive login on the first rotation.

### Notes

- The existing `api_key`/chat-completions path is unchanged down to the byte,
  pinned by a regression test that freezes the serialized request body, the
  path and the exact set of request headers.
- Streaming is a **transport** detail here, not an output feature: the wire
  streams because the subscription backend accepts nothing else, while the cell
  stays atomically-emitting and folds the whole stream into one message.
- Secret hygiene extends the existing key discipline to the token path — no
  token in config, logs, messages, `meta` or error text; redacting `Debug`;
  atomic `0600` writes — and is covered by an explicit audit test rather than
  by convention.

## [0.1.9] — 2026-08-09

MeClaw calls MeClaw: a whole child colony, driven as one cell.

### Added

- **The `subcolony` cell type.** A child colony runs as its own `meclaw`
  process and behaves, from the parent tree's point of view, like a single
  cell: one path, one mailbox, one contract. The child's internal tree is
  invisible and **not addressable** from outside. That is composition, not
  federation — and it is pinned by negative tests rather than merely intended.
  Cross-colony routing is a non-goal, not a deferred feature. The thirteenth
  built-in cell type, long-running and dual-task, built on the P7 stdio-child
  core.
- **A JSON wire for the stdin/stdout bridge (`--stdio-format <text|json>`).**
  A `meclaw` process is now addressable as a structured endpoint, not only as
  a line of text: request and reply frames carry the envelope the text format
  cannot express (`trace_id`, `ttl`, `context`), a `ready` frame announces the
  boot, and unreadable input is answered with a typed error instead of being
  swallowed. **`text` remains the default** and is unchanged, down to the byte.
- **Composition semantics that are tested, not assumed.** The parent's
  `trace_id` is *carried* into the child, so one conversation stays one trace
  across two colonies and two message logs. The TTL is *decremented* crossing
  the boundary — on top of the routing hop — so a sub-colony cycle dies exactly
  like any routing cycle; at zero the crossing is refused rather than made one
  last time. Nothing else crosses unless the facade declares it: `context` only
  through an explicit mapping, `hop` never, in either direction.
- **Secret isolation as a side effect of the process boundary.** The child is
  started with a wiped environment plus an explicit passthrough list, in its
  own process group, so neither the parent's secrets nor the child's process
  tree outlive their scope.

### Notes

- Two failure classes are treated differently on purpose. A **deterministic**
  failure — the child speaks another protocol version, never boots, cannot be
  spawned — does not panic: the cell stays up and refuses every request with
  the reason, because a restart would reproduce the failure exactly and burning
  the restart budget on a certainty only turns one clear error into a process
  storm. A **transient** failure — the child dies mid-conversation — releases
  whoever was waiting with a typed error first and then restarts, because there
  a restart is the cure.
- The protocol version and the release version are separate fields, and only
  the protocol version is asserted. A parent and a sealed child colony are
  expected to run different builds; that is the point of the boundary.
- No task register: not because a request is idempotent (it is not — a request
  can make the child write to its store), but because there is no automatic
  re-fire path. Whoever asked decides whether to ask again.
- No new dependencies.

## [0.1.8] — 2026-08-09

An agent harness — Claude Code in print mode — supervised as a cell.

### Added

- **The `harness` cell type.** A full agent harness runs as a supervised child
  process driven from the topology: a message starts a task, the harness's
  progress streams back as typed emissions, and its outcome arrives as a
  structured result. One child process **per task** — the workspace differs per
  task, and a process boundary is the natural transaction boundary for work that
  changes files. Long-running, dual-task, and the twelfth built-in cell type.
- **A task register that refuses to repeat itself.** Every other cell type is
  idempotent: replay a message, get the same answer. A harness task mutates a
  repository, so replaying it is not the same answer — it is a second run
  against a tree somebody may already be reviewing. `cell.db.harness_tasks` is
  therefore a tombstone register, not a work queue: the row is committed
  **before** the child is spawned, a repeated `task_id` is refused outright, and
  a supervisor restart turns every unfinished row into "unknown outcome, inspect
  the workspace" — never into a new run. There is no code path from the table
  back to a running task.
- **A dead child is normal here.** For `mcp` the child *is* the cell's ability
  to answer, so its death is a panic. For a harness the child is one task, and
  its exit is how a task ends: the cell classifies the outcome, closes the
  tombstone, emits the result, and goes back to waiting. The I/O sub-task
  cycles — idle, spawn, stream, idle — instead of parking.
- **Five typed emissions.** `accepted` answers the requesting message inside its
  trace and hands back the `task_id`; `progress`, `question`, `result` and
  `error` travel the origin lane to `params.emit_to`, correlated by that id. The
  result header carries only what was **observed** — the workspace we assigned,
  the status we decided, and the numbers the harness reported about itself
  (session, model, turns, cost). It deliberately carries no branch or commit:
  the harness's own summary travels as prose, and verifying it is a follow-up
  step in the topology, not a field to be trusted.
- **A stop lever.** `cancel` marks the task as cancelled **before** killing it,
  so whoever reads the table next sees a deliberate cancellation rather than a
  mystery, then tears down the whole process group. Proven against a task that
  never ends on its own, with the kill required to land promptly rather than
  outlast a timeout.
- **A permission channel, wired but off by default.** A `can_use_tool` control
  request becomes a `question` emission; an `answer` message becomes the
  control response. With `approval: "off"` (the default) a question is reported
  **and** refused in the same breath, so a harness is never left waiting for an
  answer nobody will give.
- **Process-group reaping in the stdio-child core.** An agent harness spawns
  process trees — shells, search tools, sub-agents — and `kill_on_drop` reaches
  only the direct child. `ChildSpec.process_group` starts the child as a group
  leader; teardown escalates SIGTERM → grace → SIGKILL across the **group**, and
  a `Drop` guard covers the paths that never reach an explicit teardown (task
  abort, peer panic, colony exit). The test proves both the child and its
  grandchild leave `/proc`, and a control case shows the grandchild surviving
  without the group — so the proof discriminates. `mcp` is unaffected.
- **Environment containment.** `ChildSpec.env_clear` wipes the inherited
  environment before applying an explicit list, so a child sees exactly what it
  was handed. The `harness` cell type uses it with a short passthrough
  allow-list; `mcp` keeps inheriting as before.
- **`serve_child_until_exit`.** The serve loop, but returning the child's fate
  instead of parking on it. `serve_child` is now its parking epilogue, so both
  consumers share one loop.

### Changed

- **The serve loop accepts commands that are not for the child.** Its command
  type is now `TryInto<ChildCommand>`: a consumer may send control messages of
  its own over the same channel, and one that cannot be delivered to the child
  is skipped with a warning rather than read as a shutdown. `mcp` is unchanged —
  an existing `From` impl satisfies the looser bound for free.

### Notes

- **`harness` is not a sandbox.** It runs with the permissions of the colony
  process and brings its own tools. The dependable limits are the environment
  allow-list and the canonicalised workspace clamp; a measured run confirmed
  that the vendor's `--allowedTools` flag **widens** what a harness may do
  rather than bounding it. Treat `harness` the way `bash` is treated: only in
  topologies you trust.

## [0.1.7] — 2026-08-08

A reusable stdio-child core, and the `mcp` cell's second transport riding on it.

### Added

- **`stdio_child`: spawn a child process, speak line-JSON, supervise its life.**
  A new module in `meclaw-cells` that owns the parts every future child-process
  consumer needs and none of the parts any single one of them owns: spawning
  (`ChildSpec`/`StdioChild`), newline-delimited JSON framing tolerant of blank
  lines and non-JSON banners, request/response correlation through an injected
  key extractor, lifecycle events, and killing plus reaping. The I/O sub-task of
  the dual-task pattern owns the child outright — the handler holds no pipe and
  talks to it over the two channels the substrate already provides, so a
  request/response call stays a plain `await` instead of deadlocking against the
  handler's own `select!`.
- **`mcp` speaks stdio.** `params.transport: "stdio"` runs the provider as a
  child process (`command`, `args`, `env`, `cwd`, `kill_grace_ms`) and performs
  the same `initialize` / `tools/list` / `tools/call` protocol over line-JSON.
  `transport` is optional and defaults to `http`: every configuration written
  before this release parses to exactly the same result, and the HTTP path is
  untouched.
- **Post-init liveness for the stdio transport.** The long-running stream read
  carries the signal the HTTP transport never had. When the child dies, the
  in-flight call is answered with a typed `mcp_error` **first**, and only then
  does the cell panic — `one_for_one` restarts it with a fresh child, and after
  the restart limit the registry entry is retained as `failed`. Nothing is lost
  to the panic, because the emit completes before it.
- **Orphan reaping, proven rather than asserted.** `kill_on_drop` plus an
  explicit kill-and-wait; the test reads the child's pid from a file and waits
  for `/proc/<pid>` to disappear, which rules out both a survivor and a zombie
  in one check.

### Fixed

- **A late request after the child died no longer waits for its timeout.** The
  handler's `select!` is biased towards its mailbox over its event channel, so
  it can accept one more message before it has seen the death. The serve loop
  now keeps draining commands after the child is gone and answers each one
  immediately with the child's fate, instead of parking and letting a known
  death surface as a spurious `provider_timeout` a full A-timeout later.

## [0.1.6] — 2026-08-08

The server-rendered operator UI speaks English. This is a small functional
release: the only behaviour that changes is the rendered text.

### Changed

- **Operator UI renders English end to end.** Every string the `/ui/*` pages
  emit — empty states, filter labels, table headers, pivot links, the
  pagination arrow, the dashboard's consistency disclaimer, the header
  compartment captions and the blob-resolution notices — is now English, with
  one term per concept across all seven pages. Route names, query parameters,
  field names and error tokens (`missing_blob_id`, `malformed_blob_id`,
  `blob_unreadable`) are untouched: they are API surface, not copy. No markup,
  layout or logic changed.
- **Tests asserting on rendered text moved with it.** Ten assertions match UI
  copy through `contains()`; each was flipped to the English text first,
  observed red, and only then was the string translated. Two of the ten were
  not in any inventory — they were invisible to the German-text heuristic
  because their literals carry neither an umlaut nor a listed function word.
  The lesson is recorded with them: coupling is found by reading the files,
  not by trusting a scanner's hit set.
- **German test fixtures anglicized.** The `"hallo welt"` fixture (four test
  sites across three crates, eight literals) became `"hello world"`. Each site
  is inside `#[cfg(test)]` or under `tests/`; none has runtime effect. The
  FTS5 tripwire keeps its shape — it indexes two tokens and matches on the
  *second* one, so `MATCH 'welt'` became `MATCH 'world'`, not `'hello'`.

## [0.1.5] — 2026-08-08

The memory hive gets its full read path. No Rust behaviour changed in this release —
everything below lives in the private builder workspace (templates, fixtures, evals);
the only tracked source change is a rename of public test fixtures to generic names.

### Added

- **Recall tier 1 — four retrieval legs, fused, no LLM.** A query fans out into
  keyword (`search` over episodes and facts), semantic (`similar` over binarized
  embeddings), graph (entity anchors → `traverse`, yielding the episodes the edges
  came from) and temporal (an as-of `select`). Each leg returns a ranked id list;
  the lists are merged with **reciprocal rank fusion** (`Σ w/(K+rank)`, K=60) in a
  code cell, hydrated in one round and cut to a token budget. Ties break by best
  rank, then a fixed leg priority, then kind and id — two identical requests
  produce byte-identical candidate lists.
- **Degradation as arithmetic, not as a special case.** An empty leg contributes no
  fusion term, so a dead embedder makes the result mathematically identical to a
  fusion of the remaining three legs. The embedding lane's query mode therefore
  *always* answers — with a vector or with `degraded: true` — because silence would
  hang the fan-in forever.
- **Recall tier 2 (`dialectic`).** An answer synthesised over the tier-1 candidates
  with the source priority beliefs → facts → episodes and a **mandatory gap
  statement**. The gap is enforced by the caller, not hoped for: an answer without
  one is still delivered but carries `gap_missing`, and a provider error downgrades
  to the tier-1 candidates instead of going silent.
- **As-of recall.** Any tier can be evaluated at a past instant, so "what was true in
  May" is a parameter rather than a promise.
- **Historical ingest.** A turn may carry its own event time; the write path keeps
  the caller's `happened_at` and stamps `recorded_at` from its own clock — which is
  exactly the bi-temporal split the schema is built on.
- **Explicit extraction flush.** An operator (or an ingest job) can drain the
  extraction queue immediately instead of waiting for the batch gate's age timeout.
- **Scenario suite as the development gate.** One case per capability — a hand-written
  mini corpus with known gold facts, defined queries and deterministic assertions.
  17 cases, 55 assertions; 13 of them cost nothing because facts enter through the
  inline ingress rather than through a model. Ships in the private builder workspace.

### Fixed

- **Facts inherited the ingest instant as their event time.** An extracted fact whose
  `valid_from` the model did not state fell back to "now", so an as-of query answered
  about the ingest rather than about the conversation. The fallback is now a chain:
  what the extractor claims → when the episode happened → our clock.
- **A superseded fact could still be recalled.** Only the temporal leg filtered
  `expired_at`; the keyword and semantic legs kept ranking invalidated facts. The
  filter now sits at hydration and therefore covers every leg. The raw episode that
  mentioned the old value stays retrievable on purpose — episodes are append-only.
- **A session-boot recall without a query was swallowed.** The echo guard keyed on the
  query being non-empty, which is precisely what the deterministic tier-0 bundle does
  not have. Request detection now keys on what the port edge promotes.
- **The batch claim was unbounded.** The extraction gate claimed every pending row, so
  a bulk ingest turned hundreds of turns into a single model call. Batches are now
  bounded by the token threshold and an item cap.
- **A fenced JSON answer stalled the extraction lane.** Model output wrapped in a code
  fence failed to parse and the batch was requeued forever. Fences are stripped, and
  an answer that stays unparseable is parked for inspection instead of spinning.

### Measured

First eval numbers, on the **smoke stage only — 10 questions, all of them the easiest
category** (`single-session-user`) and therefore no statement about the whole set:
retrieval Recall@5 100 %, Recall@1 100 %, MRR 1.0; judged end-to-end 90 % by a judge
model, 80 % under a strict manual reading. Model identity for every call is taken from
the provider's `response.model`, never from configuration. Details and the honest
caveats live with the project, not in this repo.

## [0.1.4] — 2026-08-08

### Added

- **store: `traverse` operation.** Multi-hop walk over a declared edge table via
  a recursive CTE. The caller names the table plus the column roles (`src`,
  `dst`, optional `kind` and `weight`), the start node(s), an optional `where`
  over the edge rows and an optional projection of further edge columns — every
  identifier is resolved against the SQLite catalog, every value is bound. The
  result is a set of **paths** (end node, depth, the nodes walked through, the
  last edge's attributes and the accumulated weight), so scoring stays with the
  caller instead of being guessed in the store.
- **store: traversal guards.** `max_depth` (default 2, hard cap 5) and
  `max_nodes` (default 200, hard cap 5000) are mandatory by construction; a
  value beyond the cap is rejected, never silently clamped. Cycles are
  eliminated per path, so a walk always terminates and no path visits a node
  twice. Hitting the node cap sets `truncated` in the payload — the result never
  shrinks silently.
- **store: `similar` operation.** Nearest-neighbour ranking over a column of
  binarized embedding vectors, combinable with `where`, `order_by` and `limit`.
  Every row carries a `distance` column (hamming distance, smaller is better);
  without an explicit ordering the result is ranked best-first with `rowid` as
  the tiebreaker. Rows whose vector is NULL — the embedding backfill queue — are
  excluded, because NULL would otherwise sort to the top.
- **store: `hamming(a, b)` scalar function**, registered on every `store`
  connection (wake and respawn alike). Arguments may be base64 text or a blob;
  unequal vector lengths, malformed base64 and non-vector arguments raise a
  regular `sql_error`. Comparing across embedding generations is a caller error
  and now fails loudly instead of producing a plausible, wrong ranking.

With this, all four retrieval legs — temporal, keyword, graph and semantic — are
answerable inside the store.

### Changed

- `rusqlite` gains the `functions` feature (needed for the registered scalar
  function). No new dependency, no lockfile change, and no loadable SQLite
  extension.

## [0.1.3] — 2026-08-08

### Added

- **store: query layer.** `where` accepts comparison operators (`eq`, `neq`, `lt`,
  `lte`, `gt`, `gte`, `in`, `is_null`, `or_null(<op>)`) next to bare equality;
  new `order_by` (multi-column, `asc`/`desc`) and `limit` (integer >= 1, no
  implicit default). Bi-temporal as-of queries, top-k and recency now run in the
  store instead of fetch-all plus filtering in a code cell.
- **store: `search` operation** over SQLite FTS5. Opt in per table via the new
  `params.fts` (`{"<table>": ["<column>", ...]}`); every result row carries a
  `rank` column (bm25, smaller is better). External-content index plus triggers;
  an existing `cell.db` builds its index once on the next spawn, so rows written
  before the declaration become searchable.
- **memory-hive template**: recall legs and the dream lane push their predicates
  into the store; `store` declares full-text indexes on `episodes.content` and
  `facts.claim` (the keyword recall leg itself lands in P5).

### Changed

- **store: identifiers are resolved against the SQLite catalog.** Table and
  column names are matched against `sqlite_master`/`pragma_table_info` and only
  the catalog's own spelling is ever written into a statement; caller text
  reaches SQL exclusively as a bound parameter.
- **store: `select` with an unknown column now reports `unknown_column`** instead
  of the generic `sql_error` (the code was always specified, only the classifier
  missed this path). No new error codes were introduced.

### Security

- **store: identifier syntax gate on the two DDL paths.** `create_table` and
  `params.schema` accept `[A-Za-z_][A-Za-z0-9_]{0,62}` only, reject the `sqlite_`
  prefix and the reserved `_fts` suffix. Both used to format caller strings
  straight into DDL.

## [0.1.2] — 2026-08-07

### Added

- **`memory-hive@1`** — a 9-cell agent-memory topology template (`store`, `writer`, `recall`,
  `extract-glue`, `extractor`, `dream-glue`, `dreamer`, `cron`, `embed`) built entirely from
  existing cell types, with **no substrate changes**:
  - **Bi-temporal facts** — `valid_from`/`valid_until` (event time) alongside
    `recorded_at`/`expired_at` (system time) plus `superseded_by`, so "what is true now",
    "what was true in May" and "what did we believe in May" are all answerable. Nothing is
    ever deleted: supersession stamps an expiry, belief retraction flips a flag.
  - **Batched extraction** — an accumulating gate (~512 tokens or a 30-minute-old item) keeps
    the LLM cost per turn at zero; the synchronous write path stays LLM-free and immediate.
    A second, inline ingress accepts pre-extracted payloads from a front-line model; both go
    through one validator and one `(episode_id, claim_hash)` dedup.
  - **Idempotent nightly consolidation** — the delta window derives from the run log and every
    written value derives from the window end, so a replayed run leaves memory byte-identical
    and a missed timer firing needs no catch-up.
  - **Embedding lane with graceful degradation** — a dead embedder leaves rows queued with
    `NULL` blobs; writes and recall keep working and the hive never hard-fails on it.
  - Recall ships tier 0 only: a deterministic, token-budgeted context bundle. Higher tiers
    (multi-leg retrieval, synthesis) and the store-side query layer they need are next up.

  Ships in the **private builder workspace**; public packaging of the builder core is pending.

### Notes

- The template works against the current equality-only `store` ops by design (no `ORDER BY`,
  `LIMIT`, `LIKE` or `IS NULL`): temporal and freshness filtering happens in its `code` cells
  until the store gains a query layer.
- New roadmap defer: `cell-types.md` § `code` states that a successful script's stderr is
  logged at warn level, while the implementation only sets the `had_stderr` header. Needs a
  ruling (align the code or shorten the spec).

## [0.1.1] — 2026-08-07

### Added

- **Message browser** — the colony's message log is now browsable:
  - `GET /colony/messages`: read-only list endpoint over `message_log` with keyset
    pagination, filters (`to_path` incl. prefix, `from_path`, `trace_id`,
    `correlation_id`, `body_kind`, time range), a two-stage query (indexed predicates
    first, residual filters under an explicit `scan_budget`, default 5000 / hard cap
    50000) and optional on-demand blob resolution (`?resolve_blob=true`).
  - `/ui/messages`: list view with filter form, keyset paging and truncated payload
    preview. Truncated scans are always disclosed in the UI.
  - `/ui/message`: envelope detail view with `context` and `hop` headers rendered
    separately, pretty-printed payload, lazy blob loading, and pivot navigation
    (trace view, parent-message chain, correlation, reply-to, dead letters).
  - Dead-letter view: new "Original" column linking to the originating message where
    it exists in the message log.

### Notes

- Messages that fail before the log write exist only as dead letters; the dead-letter
  entry itself carries the full message. Tracked as a documented deferral.
- The new endpoint is read-only and not EDA-dispatchable (like `/colony/dead_letters`).

## [0.1.0] — 2026-06-17

Initial public release: the MeClaw DSL substrate — directory tree as topology, 12
built-in cell types plus hive scoping, colony actor runtime with hot/cold lifecycle,
graph mutations and templates, long-running cells, HTTP API + web UI, stdio
direct-mode bridge, English specification (overview, cell types, config).
