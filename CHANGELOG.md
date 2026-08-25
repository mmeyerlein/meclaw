# Changelog

All notable changes to MeClaw are documented in this file. One entry per released
package. The format loosely follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows SemVer (0.x: minor/patch bumps for additive features).

The public contract is the HTTP API, the template DSL, the template ports and the
documented `error_code` strings (README § Stability). Anything that breaks one of
them is listed under **Breaking** in its release, with the migration named. The
Rust crates are internals and move without notice.

## [Unreleased]

## [0.22.1] — 2026-08-25

### Fixed

- **The Rust toolchain is pinned** ([#406](https://github.com/mmeyerlein/meclaw/issues/406)).
  `rust-toolchain.toml` said `channel = "stable"`, so the gate depended on the
  calendar: clippy passed on `3a1f9858` at 20:29Z and failed on a `meclaw-core`
  that was **byte-identical** at 21:48Z, because CI's stable had moved to 1.98.0
  and that release denies `result_large_err`. It also meant a green local run
  proved nothing — development runs 1.95.0, so CI was the first place any new
  lint was ever seen, reliably mid-release. Now `1.95.0` with explicit
  `clippy`/`rustfmt` components; raising it is deliberate work with its own
  issue, and no product code was touched to get there.

- **`canvy@2.0.1`: the canvas works over the display it is actually built on**
  ([#402](https://github.com/mmeyerlein/meclaw/issues/402)). `canvy/web` is a
  `ref` to `web@1.0.0`, and that template **seeds a demo page at `/`** — so the
  `query` a fresh `canvy` sends succeeded, and the layout read success as "this
  page is already mine". The bootstrap branch never ran, the `canvy-*`
  components were never defined, and every `object.create` in the tick's bundle
  came back `unknown_component` (412 of 418 legs on a 73-cell colony) while the
  `object.delete` legs — which need no component — landed. Within a few ticks
  the display's `objects` table was empty and `/` served `404`, once a minute,
  forever. The condition is now **"is this page mine"** rather than "did the
  query fail": a page whose object tree does not contain this cell's root is the
  same situation as no page at all, and reaches the same bundle. Adoption is
  non-destructive — `/` is re-pointed at canvy's own root and the tree behind
  the old one is left standing, because another route may still reach it. Pinned
  by three cases in `crates/meclaw-cells/tests/canvy2_pipeline.rs`, one of them
  end to end against a `web` cell carrying the shipped seed — the state every
  real instance starts in and no test had ever used.
- **`canvy` MIGRATION.md: the retirement step warns before it can take a colony
  down, and the way back stops promising what the substrate refuses**
  ([#403](https://github.com/mmeyerlein/meclaw/issues/403)). canvy 1.x ships
  `<hive>/probe -> /colony/graph`. Where the canvas is the only thing talking to
  `/colony`, that is the **only boundary-crossing edge of the whole subtree**,
  and § 5 removed it as a side effect of retiring `probe` — so the connectivity
  recompute did what it documents and flipped the entire subtree to
  `active = false`. The mutation commits cleanly and `/health` still answers
  `status: ok`; a real instance went from 47 active cells to **0**. § 5 now
  carries a pre-flight that counts the canvy-independent boundary-crossing edges
  and tells the operator to draw a connectivity-only anchor from the **hive
  path** first. § 6 retracts its unqualified undo: `<hive>/probe -> /colony/graph`
  predates hive sealing and **cannot be re-drawn** (`hive_port_boundary`), so the
  one edge whose removal does the damage is the one the way back never covered;
  it also now says the retirement is one-shot against a running colony
  (`stop_wiring_unavailable` after a reconnect). Both refusals are asserted to be
  strings the substrate actually emits, not quoted from a report
  (`crates/meclaw-cells/tests/canvy2_position_migration.rs`).

- **`steward@2.0.11`: a colony grown from templates can start again**
  ([#401](https://github.com/mmeyerlein/meclaw/issues/401)).
  `templates/steward/clock` shipped a `params.schedules[0]` the `timer` cell type
  rejects on all four counts the schema states: no `schedule_id`, `name` where
  the parser reads `schedule_name`, and neither `emit_to` nor `emit_body`. Two
  consequences, and the second one is the serious one. The control loop had never
  ticked, because a schedule with the wrong name key produces no `hop.schedule_name`
  and the hive's own edge conditions on it. And because instantiation does **not**
  deserialize a cell's params, the mutation **committed** — so the colony ran
  normally and refused to start at the *next* boot with `InvalidParams`, a long
  way from the declaration that caused it. `meclaw-os@1.0.1` refs the steward, so
  this reached every tree grown from the shell, `examples/organism` included.
  The entry now has the shape `templates/access/clock` already shipped. Pinned by
  `crates/meclaw-cells/tests/gh401_a_grown_steward_survives_a_reboot.rs`, which
  grows the shipped declaration, shuts the colony down and boots the same
  filesystem again — the only shape that could have caught this, since every
  existing test stopped at the mutation. A second sweep,
  `gh401_shipped_timer_schedules_deserialize.rs`, puts every shipped `timer` (7
  of them) through the factory call the boot makes, so this was the last one
  rather than the first of several.

- **The shipped counts and hop chains describe the composite that ships**
  ([#391](https://github.com/mmeyerlein/meclaw/issues/391)). The sidecar splitter
  (#379) landed and three reader-facing documents kept describing `talky` as it
  stood before it, including one example that carried three different cell counts
  and agreed with itself in none of them. Re-measured in one pass rather than
  re-worded: `never-forgets` registers **17** cells (the overview row said 18, its
  own README said 16 twice), `talky` brings **12** of them over **16** internal
  edges (the grow row said 11 and twelve), and the tool-round chain reads
  `brain -> splitter -> dispatcher` in every walkthrough that renders it.
  `templates/cogny` is untouched on purpose — that composite has no splitter, so
  `brain -> dispatcher` is its real topology. The repaired numbers do not go in
  bare: `crates/meclaw-cells/tests/gh391_talky_counts_match_the_shipped_tree.rs`
  derives every count in `talky`'s README from the shipped tree and reads the
  sentence that states it, so prose and tree cannot part again
  (`docs/development-rules.md` § 2d); the example total names the pin it already
  had (`CELLS_AFTER_GROW`). Three stale code comments went with them, all outside
  the byte-frozen corridors: the `remove_nodes` comment that is the likely origin
  of the promise #390 retracted, a connectivity counter-pin citing a declaration
  that draws no edge any more, and a test helper for an edge that no longer
  exists — removed and proven obsolete by a green run, not by inspection.

## [0.22.0] — 2026-08-25

### Added

- **A `web` cell type: a display owns its own listener**
  ([#380](https://github.com/mmeyerlein/meclaw/issues/380), ADR-0014). Until now
  a colony had exactly one HTTP surface and it belonged to the process, not the
  tree: `--api` bound the one port, so a colony could not open a second display,
  a display could not be created by mutation, and nothing in the topology said
  where a surface was reachable. The new long-running cell type binds the port
  named in its **own** `params` and holds its own `cell.db`. The type is
  deliberately multiple — several instances per colony, each with its own port.
  `port` is required (`0` is refused: nobody could be told in advance where the
  display went) and `bind` defaults to loopback, because authentication and TLS
  are external permanently (a reverse proxy in front) and a type that never
  authenticates must not be reachable off-host by default. Both keys are
  immutable: rebinding a running display would pull it out from under the proxy
  pointed at it. Pinned by
  `crates/meclaw-cells/tests/web_cell_serves.rs`.
- **`meclaw-surface`: the LiveView serving machinery is its own crate**
  ([#381](https://github.com/mmeyerlein/meclaw/issues/381)). It lived in
  `meclaw-api` until a second consumer appeared, and a cell must not depend on
  the HTTP API — that would be a layering cycle. The axum handler that binds it
  to the API's router stayed behind. `/surface/*` behaves byte-identically; the
  gh159 and gh163 families are the pin.
- **`templates/web@1.0.0`: the display substrate as a one-cell template**
  ([#382](https://github.com/mmeyerlein/meclaw/issues/382)). A `web` cell with a
  port of its own, a `cell.db` of its own, and the Vision token stylesheet as a
  seed row (`/vision.css`). It is the first shipped template that declares
  `contract.ingress` at all (`context: ["session_id"]`) — the entry edge lifts
  the id into the context with `set_context`, exactly as the proxy precedent
  does. `port` and `bind` are immutable, so a second display is a second
  instance with its own port via `override_params`, never a rebind of a running
  one. Authentication and TLS stay external (a reverse proxy in front) and the
  default bind is loopback.

  It ships the Vision design language as seed data rather than as prose: the
  token stylesheet, nine components (`stack`, `card`, `heading`, `text`,
  `table`, `button`, `input`, `badge`, `ornament`) and a `/demo` page composed
  of nothing but those nine. Two of the design language's rules are **enforced
  by the cell instead of documented by the template** — a `content`-layer
  component may not wear the glass material, and `object.create` refuses glass
  directly inside glass. The first check runs at `component.define` *and* at
  seed time, at one shared call site, because a rule that only guarded the
  message path would be a rule every shipped template walks past.

- **`canvy@2.0.0`: the first application of the `web` cell**
  ([#383](https://github.com/mmeyerlein/meclaw/issues/383)). canvy draws
  nothing any more. A timer takes a snapshot, a `probe` asks `/colony/graph`, a
  `layout` cell turns the snapshot into display objects, and a `web` cell holds
  them and serves the page on a port of its own. Node positions are `editable`
  object props, so a drag is local CRUD plus a diff to every viewer rather than
  a round trip through the topology, and the camera never leaves the browser at
  all.

### Breaking

- **`--api` returns to its pre-surface scope; `cell.surface` is gone**
  ([#383](https://github.com/mmeyerlein/meclaw/issues/383)). The API binary no
  longer serves cells: the `/surface/*` route, its handler, `SurfaceState` and
  the `meclaw_colony::surface` parser are removed, and the API's route table is
  back to the set it had before the surface landed — pinned by
  `crates/meclaw-api/tests/gh383_api_is_pre_surface.rs` so it cannot drift back.

  `cell.surface` is therefore **not a key any more**. It is not renamed and not
  relocated: the block it declared has no reader left, so it falls under the
  closed `cell` key list and is a **hard boot refusal** naming the key and the
  file, on every path that reads the block. Loud on purpose — a tree still
  carrying it was being served by a route that no longer exists, and ignoring
  it silently would boot a colony in which nobody answers.

  **Migration.** A surface is a cell now. Instantiate a `web` cell
  (`templates/web`), which owns its own port through `params.port` — no shared
  prefix, and nothing declared in the `cell` block. For an existing 1.x canvas,
  the recipe with its position export is `templates/canvy/MIGRATION.md`.

- **`canvy` 0.3.2 → 2.0.0: every old address is gone**
  ([#383](https://github.com/mmeyerlein/meclaw/issues/383)). The template was a
  hive whose `render` cell drew SVG server-side and whose `store` cell held the
  positions, served through `/surface/*` in the API binary. Both cells are
  retired with their subject, the drawn page no longer leaves on `surface`, and
  the picture is now assembled by the `web` cell the template instantiates. The
  first digit rather than the second, because documented addresses are taken
  away and template ports are public contract (README § Stability).

  **Migration** is instantiation side by side, not an in-place edit: grow
  canvy 2.0.0 with its own port, replay the saved positions as `object.*`
  patches, and retire the old hive by disconnecting it (no-delete). The recipe
  and the export script for the old positions ship with the template —
  see `templates/canvy/MIGRATION.md`. Running it against a live colony stays an
  operator-owned act (R-W8-9); this repository ships the recipe, never the run.

### Fixed

- **Mutation staging no longer builds another cell type's schema**
  ([#398](https://github.com/mmeyerlein/meclaw/issues/398)). Instantiating a
  template by mutation materialised each `seed/<table>.jsonl` into the new
  cell's database at staging time, creating the tables from the seed file's
  header. That header describes **rows** — column names and a coarse type — so
  everything a schema means beyond that was lost: keys, `NOT NULL`, defaults,
  indexes, column order. For the `store`, whose tables are declared per
  instance, that was right. For a type whose tables are fixed in code it was
  destructive: the cell's own `CREATE TABLE IF NOT EXISTS` found the
  constraint-free tables standing and left them.

  Measured on the shipped `web` cell: `pages` stood as
  `("root" TEXT, "route" TEXT, "title" TEXT)`, so `page.set` — an upsert on
  `route` — was **impossible for every display grown by mutation**, `ord` sorted
  lexicographically, and `idx_objects_parent` did not exist. A cell instantiated
  from the filesystem at boot was always correct, because the staging seeder
  never ran there; that divergence was the defect.

  A factory now declares `CellFactory::owns_schema`, and staging writes nothing
  into such a cell's database — no tables, no rows, not even the file. The cell
  builds its own schema and loads its own seed at first spawn, which both the
  `store` and the `web` cell already did. **A display instantiated before this
  fix does not heal itself**: instantiate it again (templates are copied,
  instances belong to the operator).

- **The `web` cell serves the `assets` table it ships**
  ([#393](https://github.com/mmeyerlein/meclaw/issues/393)). The type shipped a
  four-table schema whose `assets` table a seed file could fill and nothing
  could read: the router answered pages and bundles, and every other path was a
  404, so a seeded stylesheet was reachable by nobody and a client hook shipped
  as an asset would not have existed at runtime. A GET now asks both surfaces
  through **one** handler — pages first, assets second — so the `pages` table
  stays the only route source and neither table can quietly become unreachable
  behind the other's wildcard. The `Content-Type` comes from the row rather
  than from the file name, and the read path goes through `ValueRef`, because
  the seed loader stores every JSON string as TEXT even into a BLOB column: a
  hand-written INSERT would have passed while the seeded case failed. Pinned by
  `crates/meclaw-cells/tests/web_cell_assets.rs`, which seeds rather than
  inserts for exactly that reason.

## [0.21.0] — 2026-08-25

### Breaking

- **`telegram-connector@2.0.0`: the connector is one cell, and its address is
  the node** ([#303](https://github.com/mmeyerlein/meclaw/issues/303),
  ADR-0002 § Nachtrag 2026-08-20). The template shipped as a sealed hive
  (`params.ports: []`) whose only occupant was the credential-bearing `proxy`
  cell. That hive grouped nothing: it existed to normalise the cell's two
  emission shapes onto named lanes (`turn` for an inbound turn, `error` for the
  connector's own failure) and to give a caller one address to wire. A level
  that groups a single occupant is not a level — the normalisation belongs to
  the level that HOLDS channels. So the wrapper is gone and the cell moved up:
  `templates/telegram-connector/config.json` **is** the `proxy` cell, params and
  contract byte-unchanged.

  This is the first digit rather than the third, because a **documented address
  is taken away** and template ports are public contract (README § Stability).
  Neither rule in `docs/development-rules.md` § 4 covers a removal — a repair
  moves the third digit, an addition the second.

  **Migration**, two edges per instance:
  - *Inbound.* An edge that named the hive path plus `hop.route == 'in_reply'`
    now names the **cell** and needs no lane rewrite. `context.chat_id` still
    selects the chat.
  - *Outbound.* The lanes `turn` and `error` no longer exist. Every emission
    leaves on **one** wire and the caller tells them apart by `hop.error_code`:
    absent → one user-origin turn (`hop` carries
    `chat_id`/`user_id`/`message_id`/`platform`); present → the connector's own
    failure (`missing_chat_id`, `missing_assistant_turn`, `send_failed`,
    `invalid_body`). The two shipped edge conditions are
    `!has(hop.error_code)` and `has(hop.error_code)`.
  - *`override_params`.* A second instance takes its own credential variable in
    the **flat** form; a single-cell template has no sub-path to address, so a
    key naming `telegram-connector/proxy` no longer resolves.

  **What the collapse costs, stated rather than glossed over:** the hive paired
  `in_reply → error` in `required_drains`, and that promise is gone with it —
  wiring the inbound edge and *not* draining the failures is the one mistake
  this template can no longer refuse on the caller's behalf. The pairing is owed
  by the level that holds the connector, and this wave's `assistant@1.0.0`
  declares it in lane form (`in_turn → error`, on the level itself — the
  `channels` container inside it declares no contract at all, because a lane
  declared on an empty container would owe a door to a cell that is not there
  yet). Pinned by
  `crates/meclaw-cells/tests/gh303_the_connector_is_one_cell.rs`.

### Added

- **The four composition levels ship as templates**
  ([#302](https://github.com/mmeyerlein/meclaw/issues/302),
  [#26](https://github.com/mmeyerlein/meclaw/issues/26)) — `meclaw-os@1.0.0`,
  `org@1.0.0`, `member@1.0.0` and `assistant@1.0.0`, authored top-down under one
  rule: **a level owns what its siblings must share** (ADR-0013,
  `plans/adr/0013-a-level-owns-what-its-siblings-must-share.md`). Each is a
  `ref` composite over templates that already shipped, plus its own topology,
  plus one real, open, **empty container hive** the level beneath it is
  instantiated into — `orgs`, `members`, `assistants`, `channels`. A container
  carries neither `params.contract` nor `params.ports`: the lanes are declared by
  the level, so an assistant with no channel yet is a legitimate intermediate
  state instead of a colony that refuses every later mutation.

  | template | what it owns | lanes in / out | edges | cells of its own |
  |---|---|---:|---:|---:|
  | `meclaw-os@1.0.0` | the capability broker (`access@2.0.5`) and the control loop (`steward@2.0.10`) | 7 / 9 | 19 | 0 |
  | `org@1.0.0` | a name and a boundary, and nothing else | 4 / 7 | 11 | 0 |
  | `member@1.0.0` | the memory (`memory-hive@3.0.1`), the curated record (`affinity@3.0.0`) and the screen (`firewall@2.0.4`) | 4 / 7 | 18 | 0 |
  | `assistant@1.0.0` | the reasoning core (`cogny@4.0.2`) and the tool surface (`tools@1.0.0`) | 6 / 7 | 21 | 0 |

  Two consequences worth naming. **The memory belongs to the member**
  ([#122](https://github.com/mmeyerlein/meclaw/issues/122)), because two
  assistants of one person must know the same person and must meet one attacker
  with one rate window. And **a level that shares nothing is still a level** when
  what it is worth is the namespace — that is `org`, which holds no cell at all.
  Pinned by `gh302_meclaw_os_shell.rs`, `gh302_org_is_a_namespace.rs`,
  `gh302_member_holds_the_memory.rs` and `gh302_assistant_wires_channels_once.rs`.

- **`tools@1.0.0` — the tool surface of one assistant as ONE node with ONE
  contract** ([#286](https://github.com/mmeyerlein/meclaw/issues/286),
  [#283](https://github.com/mmeyerlein/meclaw/issues/283)): `tool_call` in,
  `tool_result` out. Sealed (`params.ports: []`), so which tools exist is a
  change INSIDE the hive and never a change to the caller's edges — replacing
  three tool cells with one code-executing cell is a single `swap_nodes` and not
  one edge of the caller moves. Four occupants: a sandboxed one-shot shell, a
  GET-only fetcher, a search wrapper, and a fourth cell that turns an unknown
  tool name into a **named refusal** instead of a dead letter. The distribution
  happens inside: three positive per-tool edges plus **one guarded default**
  (#283), never an exclusion chain. Two declarations make a swap honest rather
  than quiet — the **union of every occupant's sandbox** and **reentrancy per
  occupant** — so widening the blast radius has to be written down.

- **`examples/organism` — the whole stack grows from templates**
  ([#302](https://github.com/mmeyerlein/meclaw/issues/302)). The same empty seed
  `examples/meclaw-os` uses — a `colony.json`, one empty root hive, **zero
  cells** — plus five declarations, one per level. Out of that: **55 cells and
  287 edges**, of which **48 edges were written by hand**. The registry records
  the true origin at every level: a leaf carries its own template and version,
  and `registry.template_chain` carries the outer levels, outermost first. A
  second assistant is one instantiation with its own parameters; a second channel
  is two instantiations into `channels`, still one mutation, with no intermediate
  hive.

- **`access@2.0.5` and `affinity@3.0.0` join the public template library**
  ([#302](https://github.com/mmeyerlein/meclaw/issues/302), owner decision
  2026-08-25). Not a change to either template — a consequence of the levels:
  `meclaw-os` references `access` and `member` references `affinity` as `ref`s,
  and a level whose `ref` resolves to nothing in the public tree is worse than an
  absent level, because no gate catches it (`a_documented_template_reference_resolves`
  skips a name the tree does not carry). Either a level travels whole or not at
  all. Both ship inert: every seeded `access` policy row is disabled, and both
  seed sets carry placeholder identifiers only — the template is here, the
  instance is not.

- **A shipped `talky` answers a time-range question without its owner
  hand-writing a tool schema** ([#55](https://github.com/mmeyerlein/meclaw/issues/55)).
  `templates/talky/brain/seed/system.jsonl` is new and carries exactly two lines,
  the provider-native function objects for `memory_recall` and `thread_recall`.
  The brain's **first** request already carries them in `tools[]`, and the
  `window_from`/`window_to` the model answers with reach the `recall` port as
  `context.recall_window_from` / `-_to`. Nothing was seeded beside them: no
  identity, no instructions, no persona. The line is *tools the composite
  implements itself* — a tool the parent wires is the agent's, not the
  template's.

### Removed

- **`channel@1.0.3` is retired**
  ([#303](https://github.com/mmeyerlein/meclaw/issues/303)). The level grouped
  nothing: the plurality it was built for never arrived. It held one connector
  and one generation slot, and no colony ever put a second occupant beside them
  — so what looked like a level was a wrapper around a pair, which
  ADR-0002 § Nachtrag 2026-08-20 rules is not a level. A level that groups a
  single occupant is not a level, and the normalisation it performed belongs to
  the level that holds channels.
  `templates/channel/` is gone from the library and from
  the export allow-list, together with its byte pin
  (`crates/meclaw-cells/tests/channel_template.rs`, which guarded copies that no
  longer exist).

  **What moves where.** The lane normalisation the hive performed and its
  `in_turn → error` drain pairing move up to `assistant@1.0.0`, which owns the
  `channels` container and is where more than one channel actually meets.
  Both of the level's occupants stay in the library as the building blocks they
  always were: `terminal` is unaffected, and `telegram-connector` is unaffected
  **by this retirement** — it has its own Breaking entry above, from the change
  that landed one commit earlier.

  **Migration.** A colony instantiated from `channel@1` is untouched — an
  instance is a copy and has no link back to the library. What breaks is a *new*
  mutation naming `"template": "channel@1.0.3"`, and a `override_params` key or
  edge endpoint addressing `telegram-connector/proxy` inside it: build the
  channel from `telegram-connector` plus a `talky` under a `channels` level
  instead. Pinned by
  `crates/meclaw-cells/tests/gh303_no_channel_level_survives.rs`.

  The No-Delete policy is not in play here: it governs a colony `{root}` — the
  instantiated tree a colony owns — not this repository's template library,
  which is source and is versioned like source.

- **No shipped topology routes a `reject` or an `error` into a swallowing sink**
  ([#284](https://github.com/mmeyerlein/meclaw/issues/284), ruling Q2). Four
  edges did: `firewall → sink` on `reject` and `talky → sink` on `error` in
  `examples/meclaw-os/grow.json`, `steward → sink` on `error` in
  `examples/meclaw-os/grow-steward.json`, and `talky → sink` on `error` in
  `examples/never-forgets/grow.json` — the fourth found by the gate's own sweep
  rather than by the plan. All four are **deleted**, not re-pointed: none of the
  three senders declares a `required_drains` pairing that the sink edge
  satisfied, so the emission is now `no_route` and localises itself in the
  dead-letter queue. **The DLQ is the record** — a refusal lane has exactly two
  honest states, a real consumer that does something with it, or nothing at all.
  A cell that accepts a refusal and returns `[]` is the third, dishonest one.
  Pinned by `crates/meclaw-cells/tests/gh284_no_shipped_topology_silences_a_reject.rs`,
  a sweep over every `config.json` under `templates/` and every `*.json` under
  `examples/`.

### Changed

- **The `remove_nodes` spec row said more than the substrate holds — retracted**
  ([#390](https://github.com/mmeyerlein/meclaw/issues/390)). The mutation-operations
  table promised "inkl. Subtree-Kaskade bei Hives" / "including subtree cascade at
  hives". Neither half was true. A hive path is resolved against the **cell registry**
  only (`swap_nodes` beside it asks the hive scopes too, `remove_nodes` does not), so a
  hive has no registry row, the entry is `match_no_hit`, and all-or-nothing validation
  fails the **whole** mutation — the well-formed entries beside it included. And edge
  removal runs on **exact path equality**: an edge between two descendants of the
  matched node survives, deliberately, so the disconnected unit stays whole and
  re-connectable (the same intent `swap_nodes` states, #256). What does cascade over
  the subtree is the connectivity recompute, which flips the nodes below to
  `active = false` and stops their tasks. No substrate change — the row now says what
  the code does, names what does not work, and points at `remove_edges` for edges with a
  hive at one end (`docs/rewiring*.md` § Die alte Hive trennen carries the worked
  recipe). Pinned by `remove_nodes_refuses_a_hive_path_and_the_whole_mutation_with_it`
  and `remove_nodes_leaves_an_edge_between_two_descendants_standing`.

- **`talky@4.2.0`: the composite serves its own two tools**
  ([#55](https://github.com/mmeyerlein/meclaw/issues/55),
  [#283](https://github.com/mmeyerlein/meclaw/issues/283)). A `talky`
  instantiated from the library answers `memory_recall` and `thread_recall`
  without a single edge from its parent. Two internal edges
  `./dispatcher → ./collector` set the lanes `in_memory_call` and
  `in_thread_call`, which `collector@3.0.0` has accepted since 2.0.1 and which
  talky's own hive door has always routed — downstream nothing changes. Both
  carry a lane guard on `hop.route == 'tool'`.

  **Both READMEs retract, explicitly** (`docs/development-rules.md` § 3): the
  wiring recipe for those two self-loops, and the promise that the composite
  carries no tool schemas. The new line: *a tool the composite IMPLEMENTS is
  topology; a tool the parent WIRES is the agent.*

  **Migration, for an instance being lifted to 4.2.0.** A parent that still
  draws the old `memory_recall` / `thread_recall` self-loop now delivers every
  such call **twice** — the loop belongs deleted in the same step that lifts the
  template. And the brain seed of #55 takes effect only at `OpenStatus::Created`:
  an existing instance does **not** acquire the two tool schemas by bumping, it
  acquires them by being instantiated fresh or by having them written into its
  brain's system tree.

- **`cogny@4.0.2`: the tool exit is a guarded default edge, not a negation
  chain** ([#283](https://github.com/mmeyerlein/meclaw/issues/283), ruling Q1) —
  a repair, so the third digit. The exclusion condition
  (`!has(hop.tool_name) || hop.tool_name != 'escalate_to_deep'`) is **gone**
  rather than shorter: the reserved name stays an ordinary edge and silences the
  default for exactly itself, because suppression is per SENDER. `talky` takes
  the same shape in its own 4.2.0 bump.

  **This is not a null diff, and the difference is outward.** Inside the
  composites nothing changes. But a parent that had wired a tool cell **directly**
  to `<composite>/dispatcher` used to receive the call **twice** — once from the
  positive edge and once from the chain edge, the second copy dying as `no_route`
  — and under the guarded default it receives it **once**. A parent that counted
  emissions, or that read its own dead-letter queue for those `no_route` rows,
  sees the change.

- **`terminal@1.0.1` retracts its own words**
  ([#284](https://github.com/mmeyerlein/meclaw/issues/284)). `config.json` and
  `template.json` offered the cell as the place "where a rejection is logged,
  where errors are alarmed". It never did that: it writes `[]` and returns. The
  README now carries the explicit retraction and the two honest states of a
  refusal lane, and the `examples` prose in `template.json` that advertised the
  retracted use is pulled with it — a bump that moves one of the two would ship
  half a change. The cell's behaviour is unchanged; what it claims about itself
  is not.

- **`memory-drain@2.0.5`** — the one place its documentation still promised that
  a colony "built from `channel@1.0.3`" already carries `context.audience_set`
  and `context.channel` now names the `channels` level of `assistant@1.0.0`
  instead (README and the hive's own `use_when`). A repair of an existing
  promise, so the third digit; nothing about the adapter's behaviour, ports or
  ledger changed.

## [0.20.1] — 2026-08-25

### Fixed

- **An emission raised while the colony is still applying its own bootstrap is
  held, not lost** ([#389](https://github.com/mmeyerlein/meclaw/issues/389)). A
  `proxy`, `timer` or `mcp` cell emits from the moment its I/O task spawns — and
  the colony spawns those cells *inside* the initial-apply window, between
  `BeginInitialApply` and the commit of the `InitialApply` bundle, when the cells
  are registered but the edge table is not yet in place. An emission landing in
  that window was routed against a topology that did not exist yet and died
  one-shot: `no_route` on a first boot, `unresolved_path` on a reboot, straight
  into the dead-letter queue, with no retry and nothing in the emitting cell to
  notice. The colony loop now **closes its outputs arm for the duration of the
  window** (a `select!` guard raised at `BeginInitialApply` and dropped when the
  bundle has committed). The emissions stay in the bounded channel — back-pressure
  on the emitter, so a delay rather than a loss — order is preserved, and they
  route once, afterwards, against the topology that now stands. The guard cannot
  starve anything: inside the window the colony handles bootstrap traffic only
  (`Register`, `SetNodeContract`, `InitialApply`) and delivers nothing.

  The repair is in the substrate rather than in any one cell type, and it covers
  **both boot paths** — a reboot hydrates its edges at boot start but still
  registers its targets during the apply, so it had a residual window of its own.
  § Startup algorithm states the guarantee in both languages. Pinned by
  `colony::tests::emission_during_bootstrap_apply_window_survives_until_initial_apply`,
  a deterministic pin that fails against the previous code; the flake it was found
  through (`proxy_promotion_edge_e2e`) went from 9 of 30 green to 30 of 30 under
  load. Carried with it, permanently: on a routing-half timeout the e2e helper
  `recv_routed` now drains the **dead-letter queue into the panic message**, which
  is the only place this defect was ever visible — the lost emission sat there as
  `no_route` while every other signal the test had looked healthy.

- **`requirement_missing` covers both instantiating operations, not just
  `add_nodes`** ([#347](https://github.com/mmeyerlein/meclaw/issues/347), gap 1 of
  two). The `requires` walk introduced with
  [#292](https://github.com/mmeyerlein/meclaw/issues/292) read `add_nodes` only.
  The instantiate form of `swap_nodes[].with` — the one that names a `template` —
  performs the same copy with the same `${ctx.X}` substitution, and was waved
  through: a missing key surfaced late, at staging, as `ctx_key_missing`, after
  the copy. Both paths now run through one shared body, `requires_for_reference`
  (resolve, own declaration, `ref`-chain walk), so they cannot form a second
  opinion about what a template requires. The existing-node form of `with` (no
  `template`) references a cell that is already there, stages nothing and owes
  nothing here. `resumed_names` stays restricted to `add_nodes`: a swap always
  stages, so there is no address at which it could be a reconnect.

- **The resume exemption from `requirement_missing` is per node, not per diff
  entry** ([#347](https://github.com/mmeyerlein/meclaw/issues/347), gap 2 — the
  "known limit" the previous entry left standing). An `add_nodes` at an existing
  path is a Reconnect/Resume and was exempted from the `requires` check on the
  existence of the entry's **root** directory alone. For a composite that is half
  the truth: if the root stands and children are missing, the merge path stages
  exactly those children and substitutes their `${ctx.X}` like any fresh
  instantiation — and a key missing there kept failing late, as `ctx_key_missing`
  during substitution. Exempt are now exactly the nodes the merge skips, answered
  by the same derivation the staging side asks
  (`subtree::classify_subtree_nodes`): one helper, one answer, no second opinion
  about what a resume is. The filter bites on the `ref` walk — a `ref` belongs to
  the node it hangs under, so a resume is asked for a referenced template's keys
  only when it actually creates that node — while the named template's own
  declaration is made for the whole tree and is owed as soon as the entry stages
  anything at all. A resume over a fully existing subtree stages nothing and stays
  entirely exempt. Both halves are stated in the overview's `requirement_missing`
  paragraph, in both languages.

  **Stability surface.** Both repairs introduce **no new `error_code`** and retire
  none; what moves is **when** an existing one fires. A mutation whose `requires`
  are unmet is now refused **pre-staging** as `requirement_missing` in two cases
  that previously died later — a `swap_nodes[].with` instantiation (formerly
  `ctx_key_missing` at staging, or a stage-4 `Schema` verdict on a `with` that was
  also malformed) and a merge resume over a partially existing composite (formerly
  `ctx_key_missing` during substitution). A caller that matches on the string it
  gets back sees the earlier, more precise code; a caller that only asks whether
  the mutation was refused sees no change, and a mutation that was accepted before
  is accepted now.

### Documentation

- **`llm`'s `provider` names the wire protocol, not the vendor**
  ([#387](https://github.com/mmeyerlein/meclaw/issues/387), owner ruling of
  2026-08-25). The only accepted value, `"openai"`, was described as a phase-8
  vendor restriction — which reads as "this cell can only talk to OpenAI", while
  what it always meant is the OpenAI-compatible HTTP API, the first protocol
  implemented. The vendor is chosen by `base_url` (OpenRouter, vLLM, LiteLLM,
  Ollama), which is exactly what every shipped `llm` cell does. No rename,
  no change to the constraint and **no behaviour change** — `docs/cell-types.md`
  (both languages) and the doc comments in `llm/params.rs` now say what was meant.
  A second wire protocol is registered as a deferral in `docs/roadmap.md` with its
  trigger named, so the open question behind #387 does not run dry.

## [0.20.0] — 2026-08-25

### Breaking

- **`affinity@3.0.0`: the round has one name, and it is `audience_set`**
  ([#330](https://github.com/mmeyerlein/meclaw/issues/330), ruling Q12 of
  2026-08-21). The hive read the round under an internal name of its own,
  `context.participants`, while every round producer that ships writes
  `context.audience_set` — `session-keeper`, `memory-drain`, and the
  receptionist's ingress edge per ADR-0002 E8 — and nothing bridged the two.
  The colony's spelling won: `participants` is **retired, not aliased**. The
  door's four-candidate precedence chain collapses to two steps per fact
  (`context.asker` → `hop.audience` → `''`; `context.audience_set` →
  `hop.audience_set` → `''`), `brief` reads the canonical key, and the internal
  `./push → ./brief` edge promotes it.

  This is the second digit's neighbour and neither: no caller gains an ability,
  but a **documented `contract.accepts[].context` key is removed** —
  `accepts[0].context` is now `["asker", "audience_set"]` — and template ports
  are public contract (README § Stability). So it is Breaking rather than a
  repair. **Migration:** promote `context.audience_set`; a caller still
  promoting `context.participants` is refused `no_round`. A colony wired the way
  the rest of the library already spells it needs no change at all —
  `receptionist` and `memory-drain` promote `audience_set` today, and #302's
  member template now has exactly one name to promote.

  **Blast radius, measured rather than asserted.** `grep -rn
  "context.participants" templates/` returned **ten** hits before this commit:
  six in `templates/affinity/README.md` (`:58`, `:67`, `:77`, `:147`, `:229`,
  `:245`), two in `templates/affinity/template.json` (`:6`, `:9`) and two in
  `templates/builder-librarian/store/seed/docs.jsonl` (`:371`, `:372`) — and the
  last pair is the generated librarian corpus mirroring affinity's own
  `template.json` verbatim, regenerated in this commit. The behavioural writers
  and readers were already migrated one commit earlier; what remained was the
  prose that still told a builder to send the dead key. So there is **no second
  writer of the key** and the behavioural radius really is affinity-only — which
  is a fact about this library, not a promise about a colony somebody wired by
  hand.

  **Same commit, same issue:** the affinity README now carries the
  source-of-truth rule for the *values* inside a round. Affinity alone mints and
  maps identity references (UUID-backed internally, so a rename is one mapping
  edit and the history stays attached); on the wire the affinity-minted
  vocabulary travels **byte-identically**, and `receptionist`, the memory hive
  and `talky` only transport it. The memory hive's writer note ("it never LOOKS
  UP an identity") is asserted behaviour rather than prose —
  `a_present_speaker_is_written_exactly_as_it_arrived` — and both halves are
  registered in the spec-claims registry.

### Added

- **`/colony/ledger`: the colony answers counts about its own books**
  ([#267](https://github.com/mmeyerlein/meclaw/issues/267), ruling Q14 of
  2026-08-21). A new virtual endpoint — message target and HTTP route alike —
  returning **aggregates over one time window** out of `message_log`,
  `dead_letters` and `mutation_log`: totals, error counts, per-model calls and
  token sums, dead-letter and mutation counts by status. **Never rows and never
  header contents**: whoever needs to know *how much* moved may ask, *what*
  moved stays out of the answer. That class distinction is what earns it the
  second slot in `MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS` beside `/colony/graph`
  (`docs/meclaw-overview.md` § Flächen (`/surface`)), so a template can declare its
  own lane to it, and it is what keeps `/colony/trace` and `/colony/messages`
  out of that list.

  Filters: `?since=` (inclusive, default `now - 3600`), `?until=` (exclusive,
  default `now`), `?path_prefix=`, `?cycle_id=`, `?group_by=model`, `?tag=`,
  `?scan_budget=` (default 50000, clamped into `1…200000`). It is the **fifth**
  read under the GH #341/#359 refusal rule and introduces **no new
  `error_code`**: an unreadable filter is refused as `invalid_query` (HTTP:
  `400 bad_query`), `scan_budget` is clamped and `tag` truncated rather than
  dropped, while `cycle_id` is refused rather than truncated because it filters.
  `scan_truncated` says that one of the three windowed sub-queries exhausted its
  budget — deliberately not which one.

  The spec carries this in both languages (`docs/meclaw-overview.md` +
  `.en.md`), including the **explicit retraction** of the sentence that said a
  message query would have to be a design pass over the `store` rather than a
  colony endpoint — that pass happened and went the other way for counts. This
  is a **second-digit** change (a caller can do something never promised
  before); the release carrying it is **0.20.0**, and the `Cargo.toml` bump
  belongs to that release commit, not to this one.

### Fixed

- **`steward@2.0.10`: the shipped judge can spawn with its own default**
  ([#387](https://github.com/mmeyerlein/meclaw/issues/387)). `judge/config.json`
  defaulted `params.provider` to `${STEWARD_JUDGE_PROVIDER:-openrouter}`, and
  `LlmParams::parse` accepts no adapter but `openai`. A colony that grew the
  steward and set every documented setting **except** `STEWARD_JUDGE_PROVIDER`
  therefore got a judge that refused to spawn — the one cell in the hive whose
  absence stops the loop before it decides anything. The default flips to
  `openai`; `base_url` is untouched and still points at OpenRouter, which is
  exactly the pattern the seventeen other shipped `llm` cells use: `openai`
  names the Chat-Completions **wire**, not the vendor. The README's settings
  table said `openrouter` too and now says what ships. **What is not fixed**:
  whether the substrate should accept other adapter names at all stays open on
  #387 — this repairs the half that was shipped broken, not the question behind
  it.

- **An empty `/colony/ledger` window is refused instead of answered with zeroes**
  ([#267](https://github.com/mmeyerlein/meclaw/issues/267)). A read whose
  resolved window satisfies `until <= since` used to return counts of zero,
  which is the one place where *"we did not look"* and *"we looked and saw
  nothing"* read alike from the outside — a caller that swapped its two bounds
  would have concluded its colony was quiet. `parse_read_query_ledger_filters`
  now refuses at parse time under the existing `invalid_query` (no new
  `error_code`), with the details `empty window: until <= since`. The test runs
  on the **resolved** window, which is the only place it can run: both bounds
  have defaults (`until` = now, `since` = now - 3600), so a caller who sends
  neither still gets an answer, and both shipped steward scripts, which send
  `since` only, are unaffected. Pinned by
  `gh267_the_ledger_answers_aggregates.rs::an_empty_ledger_window_is_refused_rather_than_answered_with_zeroes`.

- **`steward@2.0.9`: the judge is shown the tool its charter orders it to use**
  ([#342](https://github.com/mmeyerlein/meclaw/issues/342)).
  `templates/steward/judge/config.json` declared the judge's one tool as
  `params.tools` and its whole charter as `params.system` — and `LlmParams` has
  **neither** field. `LlmParams::parse` is a plain `serde_json::from_value`
  without `deny_unknown_fields`, so both keys were dropped at spawn without a
  word. (The same keys arriving later as a params-update message would be a loud
  `invalid_input` against `KNOWN_PARAM_KEYS`; in `config.json` at birth they were
  silence.) The charter's closing sentence — *"Answer with exactly one tool_call
  to `steward_change`, and nothing else"* — was therefore addressed to a model
  that had been shown neither the tool nor the charter, which makes the rule the
  whole loop rests on unenforceable at the only place it could be enforced.

  Both keys move into **`judge/seed/system.jsonl`**, the mechanism every other
  tool-carrying brain in this library already uses and the only spawn-time route
  into the persistent `system` tree: six charter rows moved verbatim, plus
  `tools.steward_change` carrying the tool object as the JSON string
  `extract_tools` parses. `params.system_order` (`role`, `method`, `radius`,
  `revert_plan`, `quality`, `answer`) replaces them in the config, because the
  charter is an ordered argument and the default would have delivered it
  alphabetically, opening with its own closing instruction. `tools` is
  deliberately **not** in that list — `concat_system_prompt` skips the subtree.
  Pinned by `gh342_the_shipped_judge_tool_reaches_the_wire.rs`, which spawns the
  **shipped** judge directory through the real `llm` factory and asserts on what
  the provider recorded.

- **`steward@2.0.8`: the control loop stops opening a database it does not own**
  ([#267](https://github.com/mmeyerlein/meclaw/issues/267), ruling Q14 of
  2026-08-21). `meter` and `probe` were the last two cells in the shipped
  library to open `colony.db` themselves — read-only, with `sqlite3.connect`,
  against § *Datenbank-Isolation*'s "no exception any more"
  ([#160](https://github.com/mmeyerlein/meclaw/issues/160)). Both now **ask**
  instead: two new edges `./meter -> /colony/ledger` and
  `./probe -> /colony/ledger`, ordinary messages, aggregates back. The meter's
  read became an ask-and-wait (the ask leaves a `waits` row in the hive's own
  receipts and the answer finds it back by the `tag` it echoes); the probe has
  no `cell.db` at all, so its whole memory is the echo
  (`<cycle_id>#<attempt>`). No verdict string changed and neither did the
  deterministic property — no model runs in this path and none can.

  **One env knob is retired and two keep their names with new meanings.**
  `STEWARD_COLONY_DB` is **gone**, with a retraction line under the README's
  configuration table rather than a quiet deletion — nothing in this hive opens
  a database any more, so there is nothing left to point it at.
  `STEWARD_MAX_LEDGER_ROWS` (200000) is now the `scan_budget` the meter **asks**
  for, and `STEWARD_PROBE_LEDGER_TRIES` (3) is now how often the probe
  **re-asks** — one round trip per try, still 100 ms apart, still closing the
  same write-lag race.

  **Carried in the same entry, because it is a behaviour change:**
  a `scan_truncated` answer is now **discarded**
  ([#385](https://github.com/mmeyerlein/meclaw/issues/385)). It is the third
  way an answer can fail to be one, beside `unavailable` and `invalid_query`,
  and the only one that used to fail **open**: counts covering a part of the
  window were read as counts. A part of a cost is not a small cost, so the
  meter receipts `unmeasured` and the probe answers
  `probe_unavailable` / `scan_truncated: partial counts` instead of ruling on
  it. The price is named on the page: a colony with more in-window rows than
  the budget reverts every change until the window shrinks, and the ledger read
  stalls the colony inbox for its duration — `mutation_log`'s window query is an
  un-indexed scan bounded only by that budget.

  The hive's edge test gained the **only** carve-out it has: an endpoint may be
  `.`, a child, or one of the literal pair `["/colony/graph", "/colony/ledger"]`
  — a precedent being followed rather than set, since `templates/canvy` has
  drawn `./probe -> /colony/graph` out of a sealed hive since
  [#163](https://github.com/mmeyerlein/meclaw/issues/163). A foreign **cell**
  path is still refused, and so is every other `/colony/*` endpoint.

- **`affinity`'s `subscribe` takes both identity facts from the edge**
  ([#288](https://github.com/mmeyerlein/meclaw/issues/288)). The `subscribe`
  branch of `affinity/gate` read `cell_path` and `audience` out of the body —
  out of a document a model may have written — while the row it writes is
  exactly what `./push` reads every tick to decide **where** a pack goes: an
  address, not a description. It now takes the subscriber's address from
  `context.subscriber` and the disclosure audience from `context.actor`, and
  keeps only `subject`, `channel` and `slots` from the body. Three new
  documented `error_code` strings: `identity_from_body` (the body named
  `cell_path` or `audience` at all — refused rather than silently narrowed, so
  the audit row cannot disagree with the request it audits),
  `subscriber_not_on_edge` and `actor_not_on_edge` (fail-closed, like `no_round`
  on the read lane). The refusals are checked in that order, after
  `subscription_target_empty`. **Migration:** the `in_propose` edge must promote
  `context.subscriber` beside `context.actor`, and a caller that used to send
  `cell_path`/`audience` drops both keys. `affinity@2.0.8`.

### Changed

- **Per-turn memory extraction is a fenced block in the answer, not a tool call**
  ([#379](https://github.com/mmeyerlein/meclaw/issues/379); owner ruling
  2026-08-24 on [#373](https://github.com/mmeyerlein/meclaw/issues/373)). The
  shipped inline contract asked the model to `call remember` after its answer.
  Measured across seven model families it did not hold — the best case carried
  the call on 44 % of turns, most were far below — and a completion that mixed a
  sentence with an asynchronous call stranded its own round
  ([#378](https://github.com/mmeyerlein/meclaw/issues/378), open). The same
  rules delivered as a ```` ```memory ```` block inside the answer were adopted on
  12 of 12 turns by every one of five models, with zero malformed blocks. So the
  delivery changed and the rules did not: everything from `DELTA, NOT STATE`
  down is byte-identical. A new `code` cell, `talky/splitter`, sits between the
  brain and the dispatcher and cuts the block back out into its own message on
  the composite's new `extraction` lane; a round with tool calls, and an answer
  with no block, pass byte-identically. **Without the contract in the brain's
  instructions the splitter is a pure pass-through**, so a colony that does not
  extract per turn sees no change at all. `talky@4.1.0`, `memory-hive@3.0.1`;
  the `remember` tool is gone from the shipped tool list and the `dispatcher`
  is untouched.

  **Migration for an existing colony** — templates alone are never enough, the
  edges live in `colony.db`, so this is a mutation, in this order:

  1. `add_nodes` the `splitter` from `talky@4.1.0` into the composite.
  2. Replace the `./brain -> ./dispatcher` edge with `./brain -> ./splitter`
     and `./splitter -> ./dispatcher`, both on the unchanged condition
     `has(hop.finish_reason) && (hop.finish_reason == 'stop' || hop.finish_reason == 'tool_calls')`.
  3. Add `./splitter -> .` on `has(hop.route) && hop.route == 'extraction'`, and
     the parent edge out of the composite:
     `{"from": "./talky", "to": "<the memory hive>", "condition": "has(hop.route) && hop.route == 'extraction'", "modifier": {"set_hop": {"route": "'in_remember'"}}}`.
  4. Swap the contract text in the brain's instructions (INSTANCE data —
     `brain/cell.db` or the seed, never the template) for the new block in
     `templates/memory-hive/inline-contract.md`, and drop the `remember` entry
     from its `system.tools`.
  5. Remove the old parent edge on `hop.tool_name == 'remember'`, and take
     `remember` out of `DISPATCHER_ASYNC_TOOLS`.

  The hive's `in_remember` door is **unchanged** and still accepts the old hop
  shape, so a colony that stops at step 3 keeps writing — it just writes twice
  until step 5.

### Documentation

- **`affinity`'s push lane ships with the recipe that wires it**
  ([#289](https://github.com/mmeyerlein/meclaw/issues/289)). `out_push` was
  complete and wired to nothing: the lane, the silence rule and the subscription
  row were all documented, and nowhere did a document say how a parent draws the
  two edges the lane needs. The instance defect the issue measured is exactly
  what that gap produces — one unconditioned `hop.route == 'answer'` edge, on
  which the push follows the tool lane and the subscriber's `llm` cell never sees
  its slot update. `templates/affinity/README.md` now carries § *Wiring
  `out_push` for a subscribing brain* (one edge per subscribing cell; the brief
  edge **must** carry `hop.subscriber == ''`; a `subscribe` writes a row and
  never an edge, because mutation authority is the colony's) and § *The persona
  is a projection, not a copy* (the seed is the birth state, the record is
  pushed after it — with `templates/bot-basic`'s per-turn literal named as the
  N-transcriptions counter-example that ships today). `templates/talky` and
  `templates/cogny` say the same thing where the brains live: `system_order`
  begins with `identity`, that slot is the projection target, and a brain with
  nobody pushing into it is not a broken brain — a missing `system_order` key is
  simply skipped when the prompt is concatenated, and a composite that means to
  bind the lane later declares a **slot** in its contract from birth
  ([#285](https://github.com/mmeyerlein/meclaw/issues/285)) instead of parking a
  placeholder. No
  behaviour changed and no config moved — the discrimination itself is asserted
  by `the_two_answer_lanes_are_told_apart_by_the_subscriber_key`. The
  `template.json` `PORTS` slot carries the same recipe, and while it was being
  written it also stopped offering `./push` as an endpoint inside the
  `in_propose` clause: that sentence describes the cell that reads the
  subscription row, not an address a parent may wire, and `gh311` reads a `PORTS`
  slot as wiring instructions. Which brains subscribe to what by default stays a
  composite decision
  ([#302](https://github.com/mmeyerlein/meclaw/issues/302)). `affinity@2.0.9`,
  `talky@4.1.1`, `cogny@4.0.1`.

## [0.19.0] — 2026-08-23

### Fixed

- **The hive no longer ingests its own recall answers as conversation**
  ([#282](https://github.com/mmeyerlein/meclaw/issues/282)). Three producers on the collector's write path put text
  into `episodes` that nobody in the room ever said, and all three are now
  closed on the one write path that is left. **An interim sentence is not a
  turn**: the answer a dispatcher speaks while an async call is still running is
  marked `interim` in the window and is not written. **An advisor's answer is
  not the user's**: a reply arriving on the consult lane carries its
  `consult_id`, and a row that has one is never filed as a `user` turn.
  **A close trace invents no speaker**: the close path used to attribute the
  fold-up of a generation to whoever spoke last, and now attributes it to
  nobody. Each of the three is pinned by its own case in
  `gh282_only_what_somebody_said_is_drained`.

- **Speaker granularity: identity travels per message, not per batch**
  ([#272](https://github.com/mmeyerlein/meclaw/issues/272), ruling Q11 of 2026-08-21). The issue asked for a way to
  say who spoke inside a batch of turns. It is **dissolved rather than built**:
  with the batch gone from the live write path, one message carries one turn and
  its own provenance, so two turns of one session can name two different people
  without a mechanism of their own. `in_episode` states that in its contract and
  `gh272_identity_travels_per_message` pins it.

- **A round whose last brain iteration is a lone async tool call now still
  answers** ([#372](https://github.com/mmeyerlein/meclaw/issues/372)). When a
  model replied with nothing but an async call — `remember` with no sentence
  beside it — the channel got **no answer at all**: no interim, no final, no
  error, no dead letter. The dispatcher sends its interim answer only when text
  stands beside the bundle, the collector filed the assistant row as *already
  fired* (so no guard could win it and no `in_round_sweep` could find it open),
  and `round_idle_ms` does not fire on a quiet channel. The colony stayed alive
  and silent, and the turn was lost. Measured in three of five full harness runs
  on 2026-08-23; the per-turn contract ([#298](https://github.com/mmeyerlein/meclaw/issues/298))
  makes call-only iterations common.

  Filing the round as over is now a **condition**, and exactly two things
  satisfy it: the model spoke beside the bundle (that sentence is already on the
  channel as the interim answer), or a **handoff** call took the turn with it.
  Neither, and the round stays open: the synthetic acknowledgement completes the
  fan-in, the regular guard fires, and the seam re-enters the brain for the
  iteration the model never spent — bounded by `params.max_iter` like every
  other round, which leaves on route `answer` with `hop.round_capped=1` when it
  is spent. No new route and no fourth answer sort: a round still ends as a real
  answer, `round_capped`, or `degraded`
  ([#343](https://github.com/mmeyerlein/meclaw/issues/343)).

### Added

- **`topics`: where the conversation stands, written by the model that was in
  it** ([#298](https://github.com/mmeyerlein/meclaw/issues/298), [#299](https://github.com/mmeyerlein/meclaw/issues/299)). The annotation a turn carries
  now has **two** parts. `facts` is the delta of world state; `topic` is the
  movement of the conversation — `start`, `continue` or `end` — and a `start` or
  an `end` writes a row in the new `topics` table next to the facts. Only the
  answering model can produce it: what a conversation is *about* is not
  recoverable from the turns afterwards without paying for a second reading of
  them. Nothing reads the table at recall time yet; the close pass is its
  consumer, and whether the recall bundle should learn about topics is
  [#281](https://github.com/mmeyerlein/meclaw/issues/281)'s question, deliberately not answered here.

- **The close pass: one ended session, read whole, on a strong model**
  ([#300](https://github.com/mmeyerlein/meclaw/issues/300), ruling Q9 of 2026-08-21). Per-turn annotation is blind in
  one direction — the front model annotates the turn it has just answered and
  cannot know the turn after it. `memory-hive@3.0.0` gains a lane for the other
  direction: send a session to **`in_close_pass`** when it ends and the hive
  reads its own turns, the records they left standing, the open topics and the
  turns nobody annotated, puts all four to `MODEL_CLOSER` under a four-point
  contract (*add only what is missing; correct only by superseding the record
  you name; a sharpening points at a record and never replaces it; do not
  restate what is already there*) and writes the verdict back through the
  ordinary inline ingress. Two new cells, `close-glue` and `closer`; a new exit
  lane **`close_report`** carrying eight numbers (`added`, `sharpened`,
  `corrected`, `closed`, `restated`, `unseen_refs`, `exceptions`, `truncated`),
  and it must be drained — the writes answer nobody, so without the report a
  caller cannot tell a pass that ran and changed nothing from a pass that never
  ran. New knobs: `MODEL_CLOSER` (**required, no default** — the ruling puts this
  lane on a strong model and a silent fallback would revoke it),
  `MEMORY_REASONING_CLOSE`, `MEMORY_CLOSE_TURN_ROWS`, `MEMORY_CLOSE_FACT_ROWS`.

  **It also brings the replacement window back.** Since #298 removed the batch
  prompt, nothing produced the list of open statements a `replaces` is checked
  against, and every closure was being discarded. The close pass is the one
  party that sees an axis whole, so it renders that list and parks it — and
  being shown is **edge truth**: only a block that arrived through the close
  lane may carry a `shown` array, decided from a context key the hive's own edge
  stamps, never from anything the payload claims.

  **Cost, measured rather than estimated:** ≈ **0.077 EUR per closed session**
  on `anthropic/claude-opus-5` (three harness runs on 2026-08-23: 0.0774 /
  0.0768 / 0.0766 EUR, priced from `scripts/prices-openrouter-2026-08-22.json`).
  On a 26-turn conversation that was **about 80 % of everything the colony
  spent**. Budget one such call per closed session.

- **`episodes.episode_written`: the per-turn write is idempotent**
  ([#282](https://github.com/mmeyerlein/meclaw/issues/282), [#298](https://github.com/mmeyerlein/meclaw/issues/298)). The collector's turn window carries a
  column saying whether a row has already left as an episode, so a replay, a
  retry or a second close writes no second episode. It replaces the drain's own
  ledger rather than sitting beside it.

- **The nightly run sweeps the lane scratch** ([#375](https://github.com/mmeyerlein/meclaw/issues/375)). Nothing ever
  deleted from `memory-hive`'s two bookkeeping tables, so `scratch` and
  `recall_scratch` grew with the traffic — and since the close pass parks four
  rows plus a meeting per closed session, per conversation rather than per batch.
  The closing phase of the nightly consolidation now deletes what is older than
  `MEMORY_SCRATCH_TTL_DAYS` (default `7`) days before its own window end, in two
  bounded `delete`s and nothing else: no memory table, no provenance, no durable
  row. The cutoff is derived from the run's `delta_to` like every other value a
  night writes, so a replayed window sweeps the same rows and a window end the
  lane cannot parse sweeps none. Pinned by
  `gh375_the_night_sweeps_the_scratch`.

- **A conversation-guide harness: the per-turn contract is judged by running it**
  ([#301](https://github.com/mmeyerlein/meclaw/issues/301), ruling Q10 of 2026-08-21). `workshop/evals/conversation-guide`
  drives a scripted conversation through a **real** colony — front model, tool
  loop, inline contract, close pass — and measures the result against **seven
  invariants** rather than against expected strings. Invariant 2 is the
  acceptance criterion #298 was re-based on by ruling Q9: **no claim exists
  twice in two phrasings**, evaluated after the close pass so both writers answer
  for it together. The run carries its own spend ceiling (`--budget-eur`,
  default 10.00), reports spend per model **and per buying cell**, refuses an
  unpriced model rather than guessing, and aborts with a *partial* verdict rather
  than overspending — an invariant that was never evaluated is reported as such,
  never as green. A counter-run against a deliberately wrong contract must fail,
  naming the invariant it failed.

- **The handoff tool class: `DISPATCHER_HANDOFF_TOOLS` and `hop.handoff_calls`**
  ([#372](https://github.com/mmeyerlein/meclaw/issues/372)). Which of two things
  an async call is — *fire-and-forget*, after which the model still owes this
  turn an answer, or a *handoff*, after which the answer comes from a **later**
  turn — is tool semantics, so it is declared, in the one cell that sees the
  whole bundle. A tool named in `DISPATCHER_HANDOFF_TOOLS` is async as well (the
  two lists are unioned, so one declaration does both jobs) and its
  `tool_call_id`s ride out on `hop.handoff_calls` beside `hop.async_calls`; the
  collector reads that marker and files the round as over even when no sentence
  travelled with the call. Both keys always travel, empty included.

  **Wiring:** an advisor consult (`consult_cogny`, `ask_memory`) and `cogny`'s
  own `escalate_to_deep` belong in the new list — their answer arrives as its
  own turn. A memory write (`remember`) does **not**: it answers nothing and
  never comes back. The fan-in behaviour of either list is unchanged; what the
  second one adds is the right to end a turn without a word. An instance that
  meant a handoff and does not say so is the migration below.

### Changed

- **Extraction is per turn, through one ingress, written by the model that
  answered** ([#298](https://github.com/mmeyerlein/meclaw/issues/298), the 2026-08-20 directive; rulings Q9/Q10/Q11 of
  2026-08-21). The front-line model annotates the turn **in the answering turn
  itself** on `in_remember`, and that is the only lane that mints facts
  mid-conversation. A second party reading the same turns a second time bought
  duplicates, not coverage. What replaces the batch is not a cheaper batch:
  behind the single ingress stand two **readers** and no second writer — the
  night, which reads what is already in the store, and the close pass, which
  reads an ended session once.

  A fact is now queryable in the same turn that carried it, instead of after the
  gate interval the batch lane imposed.

- **The annotation is an OBLIGATION, and it has two parts** ([#299](https://github.com/mmeyerlein/meclaw/issues/299)).
  Every turn is annotated. A turn that carried nothing is annotated as carrying
  nothing — `{"nothing_new": true, "facts": [], "topic": {"movement":
  "continue"}}` — because with one lane a turn nobody annotated is a turn nobody
  extracts, so an absent call is a fault and not a modest answer. The shipped
  contract (`templates/memory-hive/inline-contract.md`) was rewritten from a run
  of prohibitions into a statement of what to DO; its drift lock now has **one**
  direction plus a length bound, because what is in that block is paid for once
  per call.

- **`pending_extraction` is an EXCEPTION list, not a work list**
  ([#52](https://github.com/mmeyerlein/meclaw/issues/52), [#298](https://github.com/mmeyerlein/meclaw/issues/298)). An annotation settles the queue rows of
  the turns it covered with a status of its own — `inline` where the block
  carried content, `nothing` where its answer was an honest empty one. Both are
  answers and neither is a rejection. With no claim, no gate and no recovery
  sweep left to move a row, **`pending` now means exactly one thing**: no
  annotation ever arrived. The close pass reads those rows first and sweeps the
  ones belonging to the session it closes; a pass that reaches no verdict leaves
  them untouched, because a turn nobody looked at must not be booked as looked
  at.

- **`memory-drain` is out of every live wiring** (ruling Q11 of 2026-08-21,
  `plans/adr/0012-per-turn-extraction-and-what-is-left-of-the-drain.md`).
  A live conversation writes its turns one message at a time straight at
  `in_episode` and never passes through the drain. The template stays at
  `2.0.4` and is **not** deprecated: nothing about its behaviour changed — it
  lost its wiring, not a promise it made — and it keeps one honest job, an
  import adapter for foreign history that genuinely arrives as a batch. The
  shipped examples and seeds no longer wire it.

- **`dispatcher@1.1.0` and the `collector/assemble` contract at `1.3.0`**
  ([#372](https://github.com/mmeyerlein/meclaw/issues/372)). The handoff class
  above is a new capability nobody was ever promised, so it moves the second
  digit: `settings.handoff_tools` and `emits.hop.handoff_calls` on the
  dispatcher's contract, `consumes.hop.handoff_calls` on the assembler's.
  `collector`, `talky` and `cogny` absorb it inside their first-digit move above
  rather than taking a number of their own.

### Breaking

- **`memory-hive@3.0.0`: the `in_flush` lane and the `extractor` cell are gone**
  ([#298](https://github.com/mmeyerlein/meclaw/issues/298), ruling Q11 of 2026-08-21). A mutation that wires
  `in_flush` is refused after this release, and the hive's cell list is thirteen
  cells (`close-glue` and `closer` in, `extractor` out) instead of twelve. Twelve
  environment knobs went with the lane and are read by nothing:
  `MODEL_EXTRACTOR`, `MEMORY_REASONING_EXTRACT`, `MEMORY_BATCH_TOKENS`,
  `MEMORY_BATCH_MAX_AGE_MIN`, `MEMORY_BATCH_MAX_ITEMS`,
  `MEMORY_BATCH_CLAIM_LEASE_MIN`, `MEMORY_EXTRACT_ERROR_BUDGET`,
  `MEMORY_EXTRACT_BACKOFF_SEC`, `MEMORY_EXTRACT_VOCAB_ROWS`,
  `MEMORY_EXTRACT_WINDOW_AXES` / `_SCAN` / `_ROWS`. `MODEL_CLOSER` takes
  `MODEL_EXTRACTOR`'s place in the set of variables that must be present at
  instantiation.

  **Uplift of a RUNNING hive — one mutation, and `MODEL_CLOSER` must be in the
  environment before it runs:** `remove_nodes` the instance's `extractor`,
  remove the three edges that addressed it (`. → ./extract-glue` on `in_flush`,
  `./extract-glue → ./extractor`, `./extractor → ./extract-glue`) and the
  `in_flush` port edge; `add_nodes` `close-glue` and `closer` and add the eight
  edges of the close lane. One mutation, because an island without a crossing
  edge is never woken. Never a filesystem delete — the No-Delete policy holds
  here as everywhere.

- **`collector@3.0.0`: route `turn_write` hands out one message per turn**
  ([#298](https://github.com/mmeyerlein/meclaw/issues/298), ruling Q11 of 2026-08-21). It used to hand out one batch
  of the day. **Every caller wired to the old shape breaks** — that is the
  first-digit case, and calling it a repair to keep the number small would be
  inflation in the other direction. `talky@4.0.0` and `cogny@4.0.0` move with it
  because they re-export the route on their own contract.

  **Uplift of a RUNNING talky:** replace the edge from the talky's `turn_write`
  route into a `memory-drain` with a **direct** edge into the hive's
  `in_episode` lane, promoting `session_id`, `turn_id` and `happened_at`; then
  disconnect the drain node with `remove_nodes`. Never a filesystem delete.

- **`memory-hive/extract-glue`: contract `2.0.0`** — the two dead
  `contract.settings` declarations `extract_window_scan` and
  `extract_window_rows` are removed. They lost their reader when the batch lane
  went; a declaration nothing reads is still a declaration, so it leaves on the
  first digit rather than quietly.

- **Config migration: consult tools move from `DISPATCHER_ASYNC_TOOLS` to
  `DISPATCHER_HANDOFF_TOOLS`** ([#372](https://github.com/mmeyerlein/meclaw/issues/372)).
  No API and no DSL changed, but a tree wired per the **pre-#372** READMEs
  behaves differently and must be re-declared. Affected is every tool whose call
  is meant to end the turn because a later one answers: `consult_cogny` and
  `ask_memory` on the talky side, `escalate_to_deep` inside a `cogny`.

  **What happens if the declaration is not moved:** such a call now leaves its
  round open, exactly like a fire-and-forget write. On `cogny`'s fast lane —
  whose seed prompt tells the model to call `escalate_to_deep` and *say nothing
  else* — that costs one wasted fast-lane inference, and the escalation's own
  turn defers behind the round it is supposed to replace, so **the answer comes
  from the lookup lane instead of the thinking lane**. A misclassified errand was
  meant to cost a worse formulation and never a wrong lane; leaving the knob
  where it was breaks that.

  **Migration:** move the consult tool names out of `DISPATCHER_ASYNC_TOOLS` and
  into `DISPATCHER_HANDOFF_TOOLS` — one list entry, not two: the handoff list
  declares the async class as well. `remember` stays where it is. The knobs are
  colony-global `.env` keys, so in practice this is two lines instead of one:

  ```
  DISPATCHER_HANDOFF_TOOLS=consult_cogny,ask_memory,escalate_to_deep
  DISPATCHER_ASYNC_TOOLS=remember
  ```

- **`/colony/graph`: the deprecated top-level `{"scope": …}` request body is
  removed** ([#341](https://github.com/mmeyerlein/meclaw/issues/341)). It was
  deprecated in 0.17.4 "for exactly one release round", and the round it was
  booked for — 0.18.0 — shipped with the alias still in the tree. This entry
  pays that promise. A request body carrying a top-level `scope` is now refused
  the way any unreadable filter is refused: `{"graph": {"status": "error",
  "error_code": "invalid_query", "details": "…"}}`, with no node list. It is
  refused rather than ignored on purpose — a caller still sending the old shape
  would otherwise receive the whole, unfiltered topology and read it as an
  answer to a filter that no longer applies. A body carrying **both** forms is
  refused too, for the same reason.

  **Migration:** send the documented read envelope every `/colony/*` read
  shares — `{"query": {"scope": "<path>"}}` — instead of a top-level `scope`.
  Nothing shipped in this tree sends the old shape (`templates/canvy` has sent
  the documented one since the deprecation), and the HTTP surface is untouched:
  `GET /colony/graph?scope=<path>` is a URL query parameter, not a request body,
  and keeps working unchanged.

## [0.18.0] — 2026-08-23

### Added

- **An out-edge may declare itself the default: `"default": true`**
  ([#283](https://github.com/mmeyerlein/meclaw/issues/283)). Until now the
  substrate had no fallback construct. An unconditional out-edge is not one —
  it is an **always edge** that fires *in addition to* every matching edge, never
  instead of them — so a topology that wanted a real default had to spell it out
  as the negation of every other arm, and every new arm had to be added to that
  negation by hand. Anything nobody enumerated dead-lettered as `no_route`. A
  boolean `default` beside `from`/`to` replaces the spelling: routing runs in two
  phases, and an edge carrying the key is consulted **only after no regular
  out-edge of the same sender fired for this message**. That makes it the
  declared consumer for exactly what would otherwise dead-letter — including at
  a hive path, where an out-edge `{"from": ".", "default": true}` takes the
  remainder that would leave as `hive_no_route`. A default edge may carry a
  `condition` as well (the remainder narrowed to the part the hive really
  means); whatever the guard excludes still dead-letters as before. An
  **unguarded** default edge is legitimate: the boot notes it in the colony's
  advisories and an `add_edges` commits it with a `warn` log line — never a
  refusal.

  The key is live on all three surfaces that carry an edge: `params.graph`
  in a `config.json`, `add_edges[].default` on a mutation (a non-boolean value
  is `edge_schema`), and `remove_edges[].match.default` as a pattern term. The
  match term follows the convention of the two optional fields beside it: with
  the key absent the routing phase is unconstrained and the pattern hits regular
  **and** default edges alike; with the key present the edge must run in exactly
  that phase. **An existing topology does nothing.** An edge without the key is
  unchanged in every respect, `colony.db` migrates **v6 → v7** and every existing
  row reads `is_default = 0`.

- **`/colony/graph` names an edge's routing phase**
  ([#367](https://github.com/mmeyerlein/meclaw/issues/367)). Every edge object
  in the graph read carries a **`default`** key beside `id`, `from`, `to`,
  `condition` and `modifier`. Unlike those last two it is emitted **always, on
  both values**: an absent `condition` *is* the statement that the edge has
  none, but a routing phase is never absent — every edge runs in one of the two
  — so omitting it on `false` would leave a reader unable to tell a regular edge
  from a server that does not report phases. Without the key a default edge and
  an ordinary one were the same object from outside, and
  [#283](https://github.com/mmeyerlein/meclaw/issues/283) shipped a phase nobody
  could observe.

  The reader that needed it most is the colony's own boot. The two boot probes —
  the required-drain check and the hive-contract lane-door check — do not read
  the live edge table; they **rebuild** one out of `/colony/graph` and then ask
  it routing questions. With no phase on the wire that rebuilt table said
  "regular" for every edge, so a default edge appeared in phase one, where it
  fires *beside* the regular arms instead of after them, and both probes judged
  a topology the colony does not run. They now judge the one that does — which
  can change a boot advisory: a hive whose only door to its interior is a
  default edge shadowed by a regular out-edge that always fires has, correctly,
  no door for that lane.

- **A hive port may be a slot: an address that is allowed to stand empty**
  ([#285](https://github.com/mmeyerlein/meclaw/issues/285)). `params.ports`
  entries take a second form beside the plain child name — the object
  `{"name": "gen", "slot": true, "unbound": "park" | "drop" | "error"}`, both
  keys mandatory. The declaration buys exactly two exemptions, and no more: an
  edge onto the slot is not a dangling endpoint at boot, and a mutation may wire
  it with `add_edges` before anything is behind it. An address that is *not*
  declared a slot and carries no occupant stays what it was: a hard error under
  `--validate-strict`. Without the construct a topology could not be wired
  before it was populated, which is the ordinary shape of a colony that grows
  its own workers.

  `unbound` says what happens to a message that reaches the unbound slot **over
  an edge**: `drop` discards it silently, `error` dead-letters it as
  **`slot_unbound`**, and `park` holds it FIFO until something binds at the
  address, at which point the queue leaves in emission order ahead of anything
  the caller sent after the binding. The park queue is bounded per slot by the
  new `colony.json` key **`slot_park_max`** (default 64, `0` is a valid kill
  switch); at the bound the **newest** arrival is refused as
  **`slot_park_overflow`**, so the beginning of a history — the part a later
  reader cannot reconstruct — is the part the bound protects. The queue lives in
  the colony task, not on disk: a shutdown discards whatever is still parked. A
  message addressing the slot path directly from outside does not meet the
  declaration and stays `unresolved_path`; a slot is a valid `add_edges`
  endpoint but never a `remove_nodes` or `swap_nodes[].match` target; and a slot
  declared at the root scope buys nothing, because the root has no port
  boundary.

- **The store answers N operations in one message**
  ([#295](https://github.com/mmeyerlein/meclaw/issues/295)). A body with **N
  `tool_call` turns** is answered with **one** reply carrying N `tool_result`
  turns in call order. What is counted are `tool_call` turns, not entries in
  `messages[]`. Before, the store read `messages[0]` and silently dropped every
  call after the first — an `llm` cell that emitted three calls got one answer
  and no word about the other two — so every caller that needed N operations
  serialised them into N round trips and carried the correlation itself. On the
  bundle path (two or more `tool_call` turns) anything that is not a `tool_call`
  is skipped: the prose an `llm` puts beside its calls is not an operation. At
  **N == 1 nothing changes at all** — the same reply as before, byte for byte,
  no `results[]`, no `bundle_errors`.

  The turns stay **schema-pure** (`origin`, `type`, `text`, `id`): `TurnObject`
  in `ubf-body.json` is `additionalProperties: false`, and a turn carrying
  per-operation metadata would dead-letter the whole reply as `InvalidUbfBody`.
  That metadata therefore rides in the store's own **top-level body slot
  `results[]`** — one entry per operation, in call order, with `tool_call_id` as
  the correlation key to its turn, plus `operation`, `rows_affected`,
  `duration_ms` and `error_code` if that one operation failed. The headers
  describe the bundle as a whole: `operation: "bundle"`, `rows_affected` as the
  raw sum, `duration_ms` as the total, and the new header key **`bundle_errors`**
  as the number of operations that failed. That last one is what an edge needs
  ([#343](https://github.com/mmeyerlein/meclaw/issues/343)): the routing
  decision — did any leg fail — is taken at the header with a single read,
  without opening the body. `bundle_errors` is present on **every** bundle reply,
  `0` included (checked-and-clean is not the same as nobody-counted) and **never**
  on a single-operation reply. The header's own `error_code` keeps its hard
  meaning: the whole reply is a refusal, never a partial failure.

- **The shipped tier-0 recall asks once instead of nine times**
  (`memory-hive@2.3.4`,
  [#295](https://github.com/mmeyerlein/meclaw/issues/295)). The first consumer
  of the bundle. A tier-0 read used to cost nine store round trips — three
  `select`s, three `recall_scratch` inserts, one select asking whether all three
  had landed, one guarded update electing the hop allowed to fire, one final
  select reading the parked payloads back. It is now one message with three
  `tool_call` turns, one bundle reply, and the bundle assembled in that same
  hop; a tier-0 round writes no `recall_scratch` row at all. The emitted bundle
  is byte-identical to the one the nine-hop chain produced — the expectation was
  recorded from the old chain before the change, not copied from the new code. A
  bundle reply carrying `bundle_errors > 0` stops the recall on `reject` instead
  of reporting a memory that knows nothing.

### Breaking

- **A hive lane's `accepts[].context` is a requirement, and it is now checked at
  the mutation** ([#291](https://github.com/mmeyerlein/meclaw/issues/291)). The
  key has been documented as a requirement since GH #173 and was enforced
  nowhere: `docs/config.md` said in as many words that `accepts[].context` was
  *not* checked. A hive could declare that its `in_query` lane needs
  `recall_query` promoted beforehand, and an `add_edges` that named the lane
  without promoting a thing committed without comment — after which the hive's
  interior refused every message with a key it had asked for in writing. From
  this release an edge that names an `accepts` lane **constantly** must carry
  that lane's `context` keys, on the edge itself (`set_context`) or reachable
  backwards from its `from`; otherwise the mutation is rejected as
  `hive_contract`, pre-destructively, and the refusal names the key, the lane,
  the hive path and the lane's own `because`. Two cases stay unchecked out of
  the same conservatism the rest of `hive_contract` already applies: an edge
  whose route is **computed** rather than stated (what cannot be placed must not
  be rejected), and an edge whose caller side is a hive path with **no inbound
  edge** (nothing can be delivered there, so the requirement is dormant). At
  boot the finding is **reported**, not fatal — the colony comes up and
  `--validate --validate-strict` turns the same finding into a non-zero exit.

  **Migration:** either fill `accepts[].context` on the lane so it states what it
  really needs, or wire the promotion on the edge that names the lane. **The
  shipped stack instantiates unchanged** — every lane of `memory-hive`, `access`
  and `cogny` was migrated to a declaration its own cell contracts back
  (`memory-hive@2.3.4`, `access@2.0.5`, `cogny@3.0.11`), and the shipped
  `in_query` declaration is pinned as satisfiable by a colony-level test.
  `firewall@2.0.4` is the one contract that was **relaxed** rather than
  declared: its `in_turn` prose demanded `user_id`, the runtime tolerates an
  empty one and screens it itself, so the key left the lane's context list with
  a `because` sentence saying why — a declaration nobody can satisfy is a false
  statement, not a stricter one.

  **And the sharp edge, named because it is new for this key:** the check judges
  the **full post-state** of the graph, not the diff. A colony that already
  carries a legacy violation can therefore see an *unrelated* mutation refused
  with a `hive_contract` finding about an edge the diff never mentioned. That is
  the same rule the locality validator has always applied (a mutation is
  accepted only if what stands afterwards is sound), and the finding names the
  offending edge and key — but a caller that has never met it reads a refusal
  about something it did not touch. Repair the named edge, then repeat the
  mutation.

### Fixed

- **A reboot dropped the default flag off every edge it hydrated**
  ([#365](https://github.com/mmeyerlein/meclaw/issues/365)). Found and fixed
  inside this wave, before either half shipped: the reboot hydration arm rebuilt
  its edges without the new column, so a default edge declared in a
  `config.json` was a default edge until the colony restarted and a regular one
  afterwards — the worst shape a routing flag can take, because the topology
  file and the running table disagree in silence. The persistence half's test
  had not caught it because it never went through the real reboot path; the
  regression is pinned there now.

- **The hive store's `emits` contract names every header it stamps and the body
  slot it writes** (`memory-hive@2.3.4`,
  [#366](https://github.com/mmeyerlein/meclaw/issues/366)). `contract.emits.hop`
  of `templates/memory-hive/store/config.json` declared `operation`,
  `rows_affected` and `error_code` — but the cell has always stamped
  `duration_ms` as well, and since [#295](https://github.com/mmeyerlein/meclaw/issues/295)
  it stamps `bundle_errors` on every bundle reply. `contract.emits.body` had the
  same gap one compartment over: it named `messages` and not the bundle's
  top-level slot `results[]`. All three are declared now, and `emits_meaning`
  says what they mean — including the rule that `bundle_errors` and `results[]`
  ride on every bundle reply and never on a single-operation one. Nothing gated
  on the gap, but `emits` is the promise surface a reader wires against, and
  `emits.hop` is what the locality validator reads to decide whether a
  downstream cell may condition on a header. The recall lane already conditions
  on `bundle_errors`
  ([#343](https://github.com/mmeyerlein/meclaw/issues/343)'s terminal guard), so
  the key it routes on is now the key the store says it stamps. Declaration
  only: no runtime behaviour changes, and not a byte of a reply moves.

- **Every shipped store tells the same truth about its replies, not just the
  hive's** (`affinity@2.0.7`, `builder-librarian@2.0.4`, `canvy@0.3.2`,
  `channel@1.0.3`, `coder-pipeline@2.0.4`, `collector@2.1.2`,
  `llm-registry@2.0.3`, `llm-unit@2.0.4`, `memory-drain@2.0.4`,
  `receptionist@2.0.4`, `research-assistant@2.0.3`, `session-keeper@2.0.4`,
  `steward@2.0.7`, `talky@3.0.14`,
  [#368](https://github.com/mmeyerlein/meclaw/issues/368)). The fix above landed
  in one template, and the bundle reply is a property of the `store` **cell
  type**, not of `memory-hive`: sixteen shipped templates carry a store cell,
  and fifteen of them still declared the pre-bundle contract. All sixteen now
  declare the same five hop keys (`operation`, `rows_affected`, `duration_ms`,
  `error_code`, `bundle_errors`) and the same body slots (`messages`,
  `results`), and every `emits_meaning` that existed says what the three new
  ones mean, in the hive's wording. One of the fifteen was further out than the
  issue counted: `builder-librarian`'s store never declared `rows_affected` at
  all, which is the very number a bundle sums, so it moved in the same batch.
  Declaration only, additive only —
  `emits` schemas leave `additionalProperties` at the default, so no reply
  validated differently before and none does now. `access@2.0.5`,
  `firewall@2.0.4` and `cogny@3.0.11` were minted inside this unreleased block
  and absorb their change without a further number; `channel@1.0.3` and
  `talky@3.0.14` are pin rounds (a bumped sub-unit's `cell.type: "ref"` marker
  and the instantiation recipes that name it move in the same commit, or a
  pasted block asks for a version nobody can resolve). A sweep over the shipped
  tree keeps it that way: it discovers the store configs by walking the library
  rather than from a list, so the store of a template added tomorrow is checked
  by the test written today.

- **`operation` is `required: true` in every shipped store, not in fourteen of
  sixteen** (`steward@2.0.7`,
  [#369](https://github.com/mmeyerlein/meclaw/issues/369)). The entry above gave
  all sixteen stores the same five hop keys; two of them — `steward`'s `charter`
  and `receipts` — still declared `operation` **optional**, where the other
  fourteen had long declared it required. It is not optional: every emit path of
  the `store` cell stamps it, the error surface included
  ([#331](https://github.com/mmeyerlein/meclaw/issues/331)), and since
  [#370](https://github.com/mmeyerlein/meclaw/issues/370) the degraded
  substitute does too. `required: false` told a reader the field may be absent,
  which is the reason a return edge conditioned on `hop.operation` would be
  written to tolerate a gap that does not exist. Unlike the entry above this one
  is **not** purely declarative: `required: true` puts the key into the emits
  schema's `required` list, so a steward store reply without it would now be
  discarded as `contract_violation` rather than delivered. That is why it waited
  for #370 — the one store path that could still emit without the key was the
  degraded cell, and tightening the declaration first would have widened a false
  statement from fourteen templates to sixteen instead of correcting it. The
  shipped-store sweep asserts the flag now, so the next store added cannot
  re-open the split; the same sweep also grew `rows_affected` into its checked
  key set, the key #368 found furthest out. `steward@2.0.7` was minted inside
  this unreleased block by the change above and absorbs this one without a
  further number.

- **A store that lost its database said so, and the substrate threw the answer
  away** ([#370](https://github.com/mmeyerlein/meclaw/issues/370)). When a
  `store` cell cannot open its `cell.db` at wake or at respawn, the factory
  installs a degraded cell whose whole job is that every message comes back
  naming the defect (`error_code: "sql_error"`) instead of vanishing
  ([#57](https://github.com/mmeyerlein/meclaw/issues/57),
  [#63](https://github.com/mmeyerlein/meclaw/issues/63)). Its reply carried no
  `operation` header — but fourteen shipped stores declare `hop.operation`
  `required: true`, and the colony's emits check is on in every debug build. The
  reply was therefore discarded and replaced by a generic `contract_violation`,
  which hides exactly the diagnosis the degraded cell exists to deliver, and a
  return edge conditioned on `hop.operation` dropped it either way. The degraded
  reply now names the refused op by the rule the healthy store states
  ([#331](https://github.com/mmeyerlein/meclaw/issues/331), `cell-types.md`
  § store): the operation the caller asked for, the literal `error` when nothing
  parseable arrived, and `bundle` when two or more `tool_call` turns were
  refused together — naming one leg would claim the others had been weighed on
  their own. It is the stated rule, not the healthy cell's behaviour
  byte-for-byte: two inbound shapes where that cell answers `error` because a
  PARSE refused (a call sitting behind a prose turn, and a bundle with an
  unparsable leg) are named here instead, because this cell never dispatches
  anything and so has nothing to refuse. Both are contract-conform, both are
  pinned. Runtime only: no declaration moves, so no template number does
  either.

## [0.17.4] — 2026-08-23

### Breaking

- **An unknown key in a `config.json`'s `cell` block refuses the boot, and now
  refuses the mutation too** ([#353](https://github.com/mmeyerlein/meclaw/issues/353)).
  `docs/config.md` § Block definition has always closed the `cell` key list —
  `id`, `type`, `timeout`, `restart_limit`, `idle_timeout_ms`, `mailbox_size`,
  `message_timeout`, `provenance`, `surface`, plus `template` in a `ref` marker
  — and the boot did enforce it, through a hand-maintained allow-list inside
  `bootstrap.rs`. Nothing else consulted that list. So the same template a boot
  refused went through a mutation without a word, and a typo in a real key
  (`idle_timout_ms`) took effect as the default for the field it was meant to
  set: the cell ran with the default idle timeout, the default restart limit,
  the default mailbox capacity, and nothing said the operator's intent had been
  dropped. The list now lives on the `CellHeader` deserializer itself
  (`#[serde(deny_unknown_fields)]`), which every read path goes through — the
  bootstrap scan and the mutation/staging parse alike, and inside a multi-cell
  (subtree) template every node's own `cell` block, which the subtree parser
  keeps as raw JSON and used to hand on unchecked. Both refusals name the
  offending key and the `config.json` it stands in: a boot error at boot, a
  normal pre-destructive validation refusal (`error_code: "schema"`) on the
  mutation path.

  A second half of the same rule: **a `cell.template` (or `cell.type: "ref"`) in
  an instantiated tree refuses the boot.** A `ref` is template-time only — it is
  resolved at instantiation and never stands in an instantiated `config.json` —
  and it used to be a loud boot error for free, because the hand-maintained
  allow-list simply did not list `template`. Declaring the key on `CellHeader`
  (so the closed list admits the shipped `ref` markers) would otherwise have
  traded that loudness for a silent parse of an unresolved reference. The
  refusal names the key and the file, like every other one here.

  **Migration:** remove the unknown key from the `cell` block. It was doing
  nothing before — it was either a typo for a real key, in which case fix the
  spelling and the value takes effect for the first time, or a slot someone used
  as a comment, in which case it belongs in the top-level `description` block.
  The shipped `templates/` and `examples/` trees were swept and carry no unknown
  `cell` key. Rust-internal (not part of the public contract, listed for
  completeness): `BootstrapError::UnknownCellField` is gone — the boot reports
  the same refusal as `BootstrapError::InvalidJson`, whose `reason` now names
  the `config.json` alongside serde's message.

### Deprecated

- **`/colony/graph`: the top-level `{"scope": …}` request body, for one release**
  ([#341](https://github.com/mmeyerlein/meclaw/issues/341)). The documented shape
  is the one every `/colony/*` read shares — `{"query": {"scope": "<path>"}}` —
  and it is what the handler reads from now on. The undocumented top-level
  `scope` it used to read stays accepted as an alias for exactly one release,
  logs a deprecation warning naming the scope it applied, and then goes. The
  alias is consulted only when the documented shape carries no scope, so it can
  never override it. **Migration:** move `scope` inside the `query` object. The
  retirement is tracked in `docs/roadmap.md` § HTTP-API / Web-UI (due in the
  first release after this wave's 0.17.4 cut) — a deprecation without a booked
  removal date is a promise nobody keeps.

### Fixed

- **A store refusal is no longer read as an empty result set**
  (`collector@2.1.1`, `memory-hive@2.3.1`, `talky@3.0.13`, `cogny@3.0.10`,
  [#343](https://github.com/mmeyerlein/meclaw/issues/343)). The four lanes with
  double-digit dispatch sites — `collector/assemble` (21), `memory-hive/dream-glue`
  (28), `memory-hive/extract-glue` (23), `memory-hive/recall` (16) — are state
  machines over `(context.<phase>, hop.operation)`, and not one of them read
  `hop.error_code`. `operation` says WHICH op answered; it does not say WHETHER
  it worked. The store stamps it on failing replies too: always for SQL-level
  failures (`unknown_table`, `unknown_column`, `constraint_violation`,
  `sql_error` travel through the ordinary reply builder), and since
  [#331](https://github.com/mmeyerlein/meclaw/issues/331) for `invalid_input`,
  `query_timeout` and `write_denied` as well. So a refusal arrived looking
  exactly like an answer — same phase, same op, an error sentence where the rows
  should be — and every one of the four walked on. Measured: a refused window
  read made the collector write an **empty** window leg, fire the seam and let
  the model answer the turn with no conversation at all; a refused keyword leg
  made `recall` record zero hits, so the bundle said "memory knows nothing"
  (which is [#308](https://github.com/mmeyerlein/meclaw/issues/308), one hive
  over); a refused vocabulary read made `extract-glue` prompt the extractor with
  an empty known-predicate vocabulary and an empty dedup set; and a refused
  scope read made `dream-glue` book the nightly run `status: "done",
  facts_in_window: 0`, after which every later night derives `delta_from` from
  that row and the window nothing ever looked at is skipped forever — the one of
  the four a later run cannot repair by itself. All 88 dispatch sites now read
  both fields, and a refusal is terminal: no further store op leaves, the phase
  does not advance, and the lane says so. The collector reports on lanes its
  parents already drain (`prune` for the prune chain, `answer` for everything
  else) with `hop.degraded=1`, `hop.store_error` and `hop.store_operation`; the
  memory-hive lanes report on `reject` with `hop.reject_reason ==
  'store_refused'` and the same two keys — `store_error` stays a free string
  because the store's code list is open, and an enum that had to grow with it
  would turn the next new code into a failed emit. `memory-hive`'s cron lane got
  its own door to that egress (`./dream-glue -> .` on `hop.route == 'reject'`);
  it is the one lane with no caller of its own, and the alternative was
  reporting nowhere. `talky` and `cogny` are pin rounds: their `collector` ref
  marker names `collector@2.1.1`, the rest of their contract is unchanged.

- **The same guard on the nine small lanes, and #343 is closed**
  (`research-assistant@2.0.2`, `coder-pipeline@2.0.3`, `llm-unit@2.0.3`,
  `memory-drain@2.0.3`, `session-keeper@2.0.3`, `firewall@2.0.3`,
  `receptionist@2.0.3`, `channel@1.0.2`,
  [#343](https://github.com/mmeyerlein/meclaw/issues/343)). The rest of the
  issue's table: the lanes that read `hop.operation` in one, two or four places.
  Every one was measured against a real refusal before it was touched, and every
  one was live.

  - **`session-keeper/stamp`** read an unanswered lookup as "this channel has no
    open session" and **opened a second generation** for one that had. Two
    sessions, two close batches, one history split down the middle.
  - **`firewall/screen`** failed **open**, twice: an unanswered `rules` read left
    it with no blocklist, no allowlist and no pattern rule and it walked on to
    the rate phase; an unanswered `rate` read counted zero arrivals and emitted
    `pass`. It now refuses with `reject_reason: "store_refused"` and `rule_id:
    "store-refused"` — the fail-closed reflex it already had for an unreadable
    rule row, one level down.
  - **`receptionist/greet`** read "no row" as "a channel nobody has met" and
    emitted a **mutation** that grows a second agent, plus a duplicate ledger row.
  - **`memory-drain/drain`** probed a ledger its `park` insert never reached, and
    dropped the day in silence at the other end. Nothing was marked either way,
    so nothing was lost for good — but nobody was told.
  - **`session-keeper/close`** swept nothing and said nothing (an empty row list
    reads as "no idle channel"), and read a refused `seal` as the lost guard race
    it is written to tolerate (`rows_affected` is 0 either way).
  - **the three small collectors** (`research-assistant/collector`,
    `coder-pipeline/collector`, `llm-unit/collector`) handed the store a `select`
    for a thread the refused `insert` never wrote — and on a refused `select` they
    **died**: `rows_of` calls `json.loads` on the reply text with no `try`, and an
    error sentence is not JSON.

  All of them now read `error_code` beside `operation` and report instead of
  walking on: `hop.reject_reason == 'store_refused'`, the store's own code in the
  free string `hop.store_error`, the refused op in `hop.store_operation`. Where
  the lane needed a door it got one — `reject` at the hive path for
  `session-keeper` and `receptionist`, `./collector -> ./drain` in the two
  pipelines (whose `errors` row now takes its `kind` from `hop.store_error`
  instead of filing every refusal as `unknown`), and `./collector -> .` renamed
  onto the existing `error` lane in `llm-unit`. `coder-pipeline/taskarchive` is
  the one entry judged theoretical and left unchanged: its single `hop.operation`
  read is an echo guard whose every branch parks, including for the literal
  `error` op #331 stamps when nothing parseable arrived — it is pinned as such
  rather than asserted.

  **And a refusal says what is still true at the step it was refused at.** Some
  of these steps are fire-and-forget: their store op rides in the same emission
  as the thing that step produced, so their refusal arrives *after* that thing
  is gone. The screen's arrival `mark` travels with the `pass` verdict, the
  drain's `mark` with the episodes it covers, the stamp's `touch`/`open` with
  the stamped turn, the reception's `open` with the mutation *and* the turn.
  Three of the four now report per step what actually still holds — for the
  drain's `mark` that the episodes **already left** and only the high-water mark
  is missing, for the stamp's `open` that the turn travels with a session id
  whose row was never written (so the next turn opens yet another generation),
  for the reception's `open` that the agent exists and every later turn will
  repeat the instantiation the colony then refuses as a name collision. The
  screen is the exception, because `pass`/`reject` is documented as exactly one
  of two lanes: a refused arrival mark is **not** reported at all, since a
  second verdict about one turn leaves the parent with no way to pick. The
  documented cost is that this arrival is not booked, so the turn did not spend
  its rate slot and the window undercounts by one.

  **`talky` drains it.** The keeper's new lane got a subscriber inside the
  composite — `./session-keeper -> ./errors` on `hop.route == 'reject'`, onto
  talky's one already-declared error exit — so a colony whose session store
  stops answering is not a silent room. `talky/errors` learned to read
  `hop.store_error`: a keeper refusal carries no `error_code` of its own,
  because the keeper did not fail, the store it spoke to did.

  **Two copy-paste guards went with it.** `talky`'s `answer` lane carries three
  sorts and only one is an answer, but `channel`'s internal reply edge and the
  edge `receptionist/greet` draws per channel both guarded on
  `!has(hop.round_capped)` alone — and a store-refused turn carries `hop.degraded`,
  never `round_capped`. Both would have read a store refusal out to a person as a
  real reply. Both now guard on both keys; in `channel` the degraded sort leaves
  on `error` beside the capped one, in the reception it dead-letters where the
  capped sort already did. **Migration:** nothing for a caller — but a parent that
  copied either edge out of a README adds `&& !has(hop.degraded)` to it.

- **The two brain descriptors stop calling the memory bundle durable state**
  (`talky@3.0.13`, `cogny@3.0.10`,
  [#348](https://github.com/mmeyerlein/meclaw/issues/348)). `collector@2.1.0`
  ([#278](https://github.com/mmeyerlein/meclaw/issues/278)) moved the ambient
  recall bundle out of `system.memory`: it arrives as a synthetic `memory_recall`
  tool result inside the round it was fetched for, and what stays under
  `system.memory` is a revocation carrying no data. Both brain `config.json`
  `description.purpose` texts still listed the bundle among the things that
  *accumulate* in the cell's own `cell.db` — false in the one property the
  sentence makes a claim about. The bundle is out of that list in both files, and
  each purpose now says where it went instead. The rest of each list (identity,
  instructions, tool schemas, and talky's handover of the last closed generation)
  was and stays correct, as does talky's `consumes_meaning`, which #278 made
  exactly right. Documentation only, no behaviour change; the repair rides inside
  the two unreleased versions above rather than bumping them again.

- **The builder stops on a failed rescan instead of dying one cell later**
  (`builder-hive@2.0.3`,
  [#355](https://github.com/mmeyerlein/meclaw/issues/355)). `promote` runs in two
  phases: it copies the approved draft into `templates/`, asks the colony to
  re-read its template library, and phase B receives that reply. The reply has
  two shapes — `{"rescan": {"status": "ok"}}` and `{"rescan": {"status":
  "error", "error": "…"}}` — and phase B read neither. It forwarded the reply
  verbatim into `deploy.json` and stamped `stage: promoted` on both, so a failed
  rescan let the deploy edge fire against a registry that never learned the
  class, and the run died there on `template_missing`. A duplicate template name
  was reported as a missing one, at the cell after the one that could have said
  so. Phase B now branches on the status and fails closed — anything that is not
  `ok` becomes `stage: promote-failed`, which no deploy edge accepts — and the
  scanner's own string rides on **verbatim** in the plan, because an operator has
  to be able to search for the words the error was printed with. The error stays
  in the body and out of the header: a hop key is a routing surface and a
  Debug-rendered scanner error is unbounded text. A `promote-failed` run ends at
  `approval-log`, the terminal sink a failed G1 already uses; without that edge
  the stop would have had no route at all, which would have replaced a misleading
  symptom with a silent one. **Migration:** none for a caller. A colony that
  reads the builder's lanes gains one hop value, `promote-failed`, on the same
  `hop.stage` key the pipeline already used — but note that a run stopped there
  writes **no `receipt.json`**, because `report` is not on the `approval-log`
  lane; the cause is in the message the sink received, not on disk. That is
  unchanged from every other terminal failure lane in this hive (`g1_failed`,
  `escalated`, `rejected`). What the run it replaces produced was not a truthful
  receipt either: the receipt did record the refusal (`deploy: {outcome:
  "rejected", error_code: "template_missing"}`) — under a cause that named the
  wrong thing — while the **run** claimed to have deployed, because the stage
  stamped into the message log said `deployed`. The lie was in the trace, not on
  disk.

- **A mutation the colony refused is no longer reported as a deployment**
  (`builder-hive@2.0.3`,
  [#360](https://github.com/mmeyerlein/meclaw/issues/360)). The sibling of #355,
  one cell further on, found in the same read. `deploy`'s phase B receives the
  colony's verdict — `outcome: "committed"` or `outcome: "rejected"` — and
  stamped the literal string `deployed` on both. Milder than #355 and repaired
  more narrowly for that reason: the verdict did ride along visibly in
  `hop.outcome`, `report` wrote the full reply into `receipt.json`, and the one
  edge reading the stage led to that receipt either way, so no wrong action was
  taken and nothing was lost. What was false is the one word an operator scanning
  the message log reads first. Phase B now reads the outcome before naming the
  stage and fails closed exactly as `promote` does: only `committed` is a
  deployment, anything else — including a verdict shape the script does not
  recognise — is `stage: deploy-rejected`. That stage goes to **`report`** on its
  own edge, not to the `approval-log` sink that takes `promote-failed`: a
  promote-failed run never reached the colony and has nothing to book, while a
  rejection is a decision the colony made about this build, and the receipt is
  where a refusal belongs — routing it to a sink would have destroyed information
  to fix a wording bug. **Migration:** none for a caller; one more value,
  `deploy-rejected`, on the `hop.stage` key. `hop.outcome` is unchanged. A parent
  that copied the `./deploy -> ./report` edge out of this README and wants
  refusals in its own receipt takes the second edge with it.

- **The lease and the in-flight marker are given back, and given back together**
  (`builder-hive@2.0.3`,
  [#361](https://github.com/mmeyerlein/meclaw/issues/361)). The third finding
  from the same read as #355 and #360, and the one that stopped the hive
  outright. The pipeline took two things under the staging root — the
  one-builder-per-scope lease and the `.inflight` marker naming the running
  build — and gave back neither, **not even on the run that succeeded**. The
  first build that finished on a scope therefore held that scope's lease
  forever, and every later request there died with `lease_held`; the marker of
  a run that ended weeks ago stayed lying next to it. The scenario suite never
  saw it, because every run gets a throw-away colony. The release sits on the
  terminal cell of each lane, because exactly three lanes take the lease:
  `deployed` and `deploy-rejected` end at `report` (#360), `promote-failed` at
  `approval-log` (#355), and all three now give the run state back. `capture`
  stays the pure sink: it also serves `in_report`, traffic a deployed subtree
  writes from out in the colony, and the cell that may delete run state does not
  belong on that lane. Two properties give the release its shape. **One act over
  both files, marker first** — the two masked each other, a stale marker is
  harmless only for as long as the never-released lease keeps a second builder
  out, so repairing only the lease would have re-opened the correlation hole the
  marker closes; the single intermediate state is "marker gone, lease held",
  where nothing may start, and never "lease free, marker stale". And **released
  is only what names this run**: `approval-log` is the sink of four lanes, three
  of which never took a lease, so a sink that deleted whatever it found would
  have let a request refused with `lease_held` release the very lease it failed
  on. Fail closed follows from the same reasoning: a marker that cannot be
  removed — any `OSError` that is not "already gone" — **keeps the lease held**,
  because that costs one `lease_held` naming the run, while failing open costs a
  double mutation. **Migration:** none for a caller, and nothing to do on an
  existing staging root beyond what the fix now does by itself. A `receipt.json`
  gains a `released` list (`["inflight", "lease"]`) recording what this run
  actually gave back — it never claims a release that did not happen. **Not
  fixed here:** a run that never reaches a terminal cell at all (crash, TTL, a
  `/colony` reply that never comes) still leaves both behind. Reclaiming those
  on the next acquire needs a timestamp in the lease and belongs to the
  mutation-CAS work that replaces this lease outright; it is booked in
  `docs/roadmap.md` § CAS / Permissions.

- **`/colony/graph` filters on the shape the spec documents and its consumer sends**
  ([#341](https://github.com/mmeyerlein/meclaw/issues/341)). The handler parsed
  its filter from a top-level `body.scope`, while the spec's read envelope and
  the shipped `templates/canvy` probe send `{"query": {"scope": …}}`. Neither
  side errored: the handler found no `scope`, applied no filter, and answered
  the unfiltered topology — an ignored filter and an empty filter were
  indistinguishable from the outside, the failure class of
  [#298](https://github.com/mmeyerlein/meclaw/issues/298). Nobody saw a wrong
  answer in practice: canvy asks for the root scope `/`, whose filtered and
  unfiltered answers are the same graph, so the canvas always drew the whole
  colony — which is what it wanted. A caller asking for a sub-scope got the
  whole colony too, and had no way to tell.

  Along with it, the loudness half: **a filter that is present but unreadable is
  now an error, never an unfiltered answer.** A `query` that is not an object, or
  a `scope` that is not a string, answers
  `{"graph": {"status": "error", "error_code": "invalid_query", "details": …}}`
  and carries no `nodes`. An absent filter is still the documented root default,
  and a `query` object without a `scope` field still means "root", as documented.

- **The other three `/colony` reads refuse an unreadable filter too, instead of
  answering unfiltered** ([#359](https://github.com/mmeyerlein/meclaw/issues/359)).
  `/colony/registry`, `/colony/templates` and `/colony/trace` parsed their
  filters with `.as_str()` / `.as_bool()` / `.as_u64()` chains that end in `None`
  on a type mismatch — and `None` meant "no filter". A cell that emitted
  `{"query": {"active": "true"}}`, `{"query": {"cell_type": 7}}` or
  `{"query": {"limit": "50"}}` got a well-formed, *unfiltered* answer and no way
  to tell its filter had been thrown away: the failure class of
  [#298](https://github.com/mmeyerlein/meclaw/issues/298) and
  [#341](https://github.com/mmeyerlein/meclaw/issues/341), one level down. All
  three now answer in their own top-level slot, in the shape `/colony/graph`
  established: `{"<slot>": {"status": "error", "error_code": "invalid_query",
  "details": …}}`, carrying no result list.

  `/colony/trace` had the sharper case: `trace_id` and `correlation_id` ran
  through `Uuid::parse_str(s).ok()`, so a syntactically **broken UUID string**
  also became "no filter" — a caller asking for one trace got the newest 100
  entries of every trace back. A broken UUID is now refused like a wrong type.

  Unchanged on purpose: an absent `query`, an absent field, or either of them
  `null`, is still the documented default and answers everything; and a `limit`
  out of *range* is still clamped to 1…1000 — clamped is not dropped. The HTTP
  twins of these reads already refused a wrong-typed filter with `400`
  (axum's typed `Query<T>` extractor, plus the explicit UUID check in the trace
  handler); this closes the gap on the EDA side, where the body is free-form
  JSON.

- **A `bash` command of 128 KiB or more says so instead of reporting an I/O fault**
  ([#351](https://github.com/mmeyerlein/meclaw/issues/351)). `bash` hands the
  command to the shell as a single `argv` string (`/bin/sh -c <command>`), which
  Linux caps at `MAX_ARG_STRLEN` = `32 * PAGE_SIZE` = 131 072 bytes —
  independent of `ARG_MAX` and not raisable, the same wall that broke `code` in
  [#349](https://github.com/mmeyerlein/meclaw/issues/349). A command at or above
  that size died inside `spawn()` with `Argument list too long (os error 7)` and
  came back as `error_code: "io_error"`, which reads like the child failed —
  while in truth no child had ever existed. Unlike a `script_inline`, a `bash`
  command is not template-authored: it arrives per message as the `command`
  argument of a `tool_call`, so a generated heredoc, an inlined file or a base64
  payload is the realistic route to the cap. Such a command is **now refused
  before the spawn** with the existing `error_code: "invalid_input"`, and the
  message names the actual byte size next to the 131 072 byte limit. No new
  `error_code` string enters the public contract, and nothing below the cap
  changes. The `#349` remedy — materialise into a per-spawn temp file — was
  deliberately not carried over: `sh <file>` is not `sh -c <command>`, `$0`
  becomes the script path and `$1…` shift by one, so a command reading
  positional parameters would change meaning above the cap. Whoever needs
  128 KiB of program or more uses `code`. Regression lock:
  `gh351_an_oversized_command_is_refused.rs`, driving `BashCell::handle`.

- **The librarian's corpus carries whole sections instead of their first 4000
  characters** (`builder-librarian@2.0.3`,
  [#344](https://github.com/mmeyerlein/meclaw/issues/344)). The seed chunker cut
  every section at `MAX_CHARS` with `body[:MAX_CHARS]`: no continuation, no
  warning, and no gate that could see it — the existing corpus gate regenerates
  and byte-compares, and truncation is perfectly deterministic, so a chunk that
  had lost half its section matched its own regeneration forever. What that cost
  was measured: when the six `--vault` rows pushed `docs/meclaw-overview.md`
  § Flags past 4461 characters, the paragraph at offset 4043 — "Info-only Flags
  sind side-effect-frei", the promise that `--version` and `--help` write nothing
  — stopped existing for the librarian, and the chunk simply ended mid-sentence.
  A section over the cap is now carried across continuation rows that keep
  `source`, `section` and `kind` and take the base row's id with a `-cont<N>`
  suffix; cuts land on a line break or a space, never mid-word. Corpus-wide that
  recovered content the tree had already lost: 311 rows became 411, of which 100
  are continuations, and no row sits on the cap any more (57 did).

  Two consequences of that split are part of the same change. **The briefing
  heading carries the row id** — `### <source> -- <section> (<kind>) [<id>]`, with
  `, continued` after the id of a continuation — because those three columns
  stopped identifying a row: two pieces of one section arrived under an identical
  heading with different bodies, and a fragment starting mid-argument read as a
  whole statement. And the generator prints a **counted headroom warning** for
  every *unsplit* row within 5 % of the cap: 6 today, including the 3899/4000
  cookbook note the issue measured as the next casualty. An already-split row is
  excluded on purpose — it lies near the cap because the splitter seams as late
  as it can, so listing it (44 rows) warns about the split working as designed
  and buries the genuine near-misses.

  Regression lock: `no_chunk_is_a_silent_truncation` in
  `crates/meclaw-cells/tests/librarian_seed_corpus.rs`, which reads the product
  rather than regenerating it and so asks the question the old gate could not —
  whether the corpus still CONTAINS its sources.

- **The gates of the published tree now run before the publication, not after**
  ([#356](https://github.com/mmeyerlein/meclaw/issues/356)). `check_claims.py`
  and `check_adr_anchors.py` behave differently in the private and the published
  tree by design — the published one carries no German document and no ADR
  corpus, only the derived registries — and until now CI was the first place
  either of them ever ran in that shape. That is after the push: the previous
  release needed three follow-up rounds for defects nothing local could see. The
  release gate now materialises the export tree and runs both scripts *inside*
  it — by default, with an explicit `--no-public-gates` to skip it, because a
  check that only fires when a human remembers it is the same defect one level
  up. Alongside it runs an unconditional check that every `pinned` row of the
  claims registry names a test the exported corpus actually contains — the class
  that shipped last time, where a pin resolved privately and pointed into a file
  the export deliberately leaves behind. Both were proven against a planted
  defect: private gate green, public gate red.

  Three consequences travel with it. The CI test job runs with `--no-fail-fast`,
  because stopping at the first failing binary out of 400+ turns one red run into
  one repaired defect instead of all of them (exactly what happened last time).
  The gate's compile check over the published tree gained `--all-targets`, so it
  now compiles that tree's *test* targets too — without it, a test that builds
  privately and not publicly walks straight past it. And two test files stop
  travelling, because they read templates that do not:
  `gh355_a_failed_rescan_stops_the_promotion.rs` and
  `gh360_a_rejected_mutation_is_not_a_deployment.rs` (both `builder-hive`, the
  same standing arrangement as the other builder-hive tests).

  A third, `gh343_the_small_cells_read_the_error_code_too.rs`, was blocklisted
  with them as an emergency brake and has since been **split** instead
  ([#362](https://github.com/mmeyerlein/meclaw/issues/362)). It was mixed: six of
  its thirty-two cases read private collector copies
  (`research-assistant/collector`, `coder-pipeline/{collector,taskarchive}`,
  `llm-unit/collector`), the other twenty-six drive `memory-drain`,
  `session-keeper`, `firewall`, `receptionist` and `talky` — all published. Those
  six now live in `gh343_the_private_collectors_read_the_error_code_too.rs`,
  which is what stays behind, and the twenty-six travel. The precedent is the
  `gh235` pair: split rather than guard, because an `exists()` skip would ship a
  test that asserts nothing ([#234](https://github.com/mmeyerlein/meclaw/issues/234))
  — the test *is* the proof over the shipped files. The cost is the helper block,
  duplicated across both files by choice rather than by necessity: a shared home
  exists (`meclaw-testing`, a dev-dependency both halves already import), and the
  duplication follows the sanctioned `gh235` shape instead. Thirty-two cases
  before, thirty-two after.

- **The cost report stops pricing embeddings at a twentieth of what they cost**
  ([#357](https://github.com/mmeyerlein/meclaw/issues/357)). The only price
  snapshot shipped alongside `scripts/cost_report.py` was
  `prices-openrouter-2026-08-15.json`, and it maps the embedding role to
  `qwen/qwen3-embedding-8b` at 0.01 USD/M. The shipped embedding generation moved
  to `google/gemini-embedding-2` — 0.20 USD/M, measured — on 2026-08-19, so every
  report run after that date booked the embedding lane twenty times too cheap,
  and the `/embed` fallback rule attributed those tokens to a model the colony had
  stopped calling. `scripts/prices-openrouter-2026-08-22.json` lands **beside** the
  old file, never on top of it, exactly as `docs/costs.md` § 4 requires: the
  2026-08-15 list stays because the M-tier window further down was computed from
  it, and both travel. Re-checking the chat rows against
  `https://openrouter.ai/api/v1/models` on 2026-08-22 turned up a second drift
  nobody had filed: `openai/gpt-5.6-luna` has doubled to 0.20 / 1.20 USD/M
  (`anthropic/claude-opus-5` is unchanged). Measured against one eval colony's
  message log, the two corrections together move a 0.33 h window from 0.070 to
  0.141 USD. The eval pre-flight
  (`workshop/evals/p5-longmemeval/preflight.py`) carried the retired model id in
  three separate default literals; all three now derive from the one active row in
  `templates/memory-hive/store/seed/emb_models.jsonl`, which is the file the
  `cell.db` is actually built from.

## [0.17.3] — 2026-08-22

### Changed

- **A tier-1 recall delivers two documents in one message** (`memory-hive@2.3.0`,
  [#296](https://github.com/mmeyerlein/meclaw/issues/296)). The bundle on
  `system.memory.bundle` now carries what answers the question — the claim, the
  axis it sits on, the days it held — and the retrieval's own bookkeeping moved
  into `recall_diagnostic`, a top-level body slot beside `system` and
  `messages`. The `collector`'s `in_bundle` lane keeps `system` and `messages`
  and nothing else, so the record reaches the message log and never the next
  prompt. Whether the smaller bundle answers questions any better is a
  measurement this release does not have; what it has is a bundle whose fields
  a reader can act on and a trace that still holds everything the old one did.

- **The readable half is written for the reader who has to answer**
  ([#281](https://github.com/mmeyerlein/meclaw/issues/281)). The flat ranked
  list with a `- [fact keyword]` tag in front of every row is gone from the
  bundle text. What a model gets instead is two sections — `FACTS (extracted,
  canonical, dated)` and `WHAT WAS SAID (verbatim, not interpreted)` — under a
  header that says what the document is and as of when. A past question no
  longer renders in the same shape as an asserted fact.

- **The bundle stops describing the run and starts describing itself**
  ([#279](https://github.com/mmeyerlein/meclaw/issues/279)). The opening line
  used to name the retrieval — how many candidates, fused by RRF over which
  legs — and a model told it holds N ranked hits reads every row under it as a
  maybe. It now says what the document IS and as of when. The run's own
  description is not gone: `MEMORY (tier 1, N candidates, RRF over …)` and the
  flat ranked lines with their leg tags are the `text` field of
  `recall_diagnostic`, byte for byte what the bundle used to show.

- **The two ranking legs that could not filter themselves now can**
  ([#297](https://github.com/mmeyerlein/meclaw/issues/297)). `similar` and
  `search` RANK, they never filter, so a question with no near neighbour still
  came back with a full page and every row in it voted in the fusion as loudly
  as a real hit. Each leg got a floor measured on its own scale
  (`MEMORY_SEM_MAX_DISTANCE`, `MEMORY_KW_MIN_SCORE_RATIO`), the fusion pays a
  factor for two legs agreeing (`MEMORY_RRF_AGREEMENT`), and a leg with weight
  0 no longer nominates candidates it does not vote for. This is a PoC memory
  and the retrieval quality it buys is not measured end to end yet; the knobs
  exist so that it can be, including with the cuts switched off. The
  measurement was attempted and could not run at the time: the `recall` script
  had grown past the 128 KiB that a single `-c` argument may carry on Linux, so
  the cell did not spawn at all. That blocker is **fixed**
  ([#349](https://github.com/mmeyerlein/meclaw/issues/349), commit `1dff52f9` —
  see § Fixed below).

  **The cuts have now been measured, and they cost nothing.** 18 questions from
  LongMemEval-S, each answered three times — default, `MEMORY_KW_MIN_SCORE_RATIO=0.0`,
  `MEMORY_RRF_AGREEMENT=0.0` — over the *same* store with the *same* vectors, so
  nothing but the knob differs. All three arms return the identical
  `R@1 72.2 % / R@5 88.9 % / R@10 94.4 % / MRR 0.7908`, and across all 54 paired
  passes exactly one thing moves at all: on one question the keyword leg grows
  from 17 to 19 candidates when its floor is removed, without changing the rank
  of anything. The defaults therefore stay where they are. Two honest limits:
  the sample is half deliberately-hard questions and is **not** a benchmark
  figure — only the difference *between* arms carries meaning, and that
  difference is zero — and the run was retrieval-only, so it says nothing about
  the end-to-end gap of
  [#148](https://github.com/mmeyerlein/meclaw/issues/148), which stays open.
  Receipt: `plans/wellen-2026-08-21/receipts/w2-longmemeval.md`.

- **The shipped embedding generation is `google/gemini-embedding-2`**
  (`memory-hive@2.3.0`). `store/seed/emb_models.jsonl` named
  `qwen/qwen3-embedding-8b`; the deployment it mirrors moved off that model on
  2026-08-19 because its tail latency was burning whole query-embedding calls
  against the read lane's timeout, while the replacement answered comfortably
  inside it. The seed is copied verbatim into the store while `similar` filters
  on the `model_id` it finds there. Left behind, the seed and the env would have
  disagreed and the semantic leg would have gone **silently empty** — a bad
  retrieval number instead of a config error. Dimension stays 1024. Anyone
  running their own embedder keeps setting `MEMORY_EMBED_MODEL` and must keep
  this seed in step with it; the eval's pre-flight refuses to spend anything
  while the two disagree, and the shipped tree's own three statements of the
  value — the seed, the `${MEMORY_EMBED_MODEL:-…}` in `embed` and the
  `contract.settings.model.default` beside it — are now pinned against each
  other ([#204](https://github.com/mmeyerlein/meclaw/issues/204)).

- **A bundle with no candidates says so instead of showing an empty list**
  ([#297](https://github.com/mmeyerlein/meclaw/issues/297)). The asserting
  header over no rows read as "the lookup ran, this is what memory holds" and
  invited an answer from somewhere else. The empty case now states the result
  in one sentence, names the case where hits came back and every one of them
  died at a relevance floor, and drops the completeness hedge — a hedge about a
  list that does not exist. `answers: "none"` in the JSON, `hop.recall_empty`
  for a router. It does not suppress a tier-2 answer from a directly relevant
  belief: those are statements about different things.

- **The ambient recall leg arrives as evidence of the round, not as durable
  state** (`collector@2.1.0`,
  [#278](https://github.com/mmeyerlein/meclaw/issues/278)). The per-turn bundle
  used to be written into `system.memory` and stay there — upserted per slot
  path in the brain cell, in the place an agent's instructions live, with
  nothing marking it as the answer to one question asked once. It now leaves the
  seam as a synthetic `memory_recall` `tool_call` / `tool_result` pair at the end
  of `messages[]`, under a call id derived from the bundle itself (a sha256 over
  `as_of` and the query), so a re-assembly of the same turn stays the same call
  instead of looking like a second question. The slot it used to occupy is
  revoked on every turn — an empty leaf on the fixed path `system.memory.recall`
  plus the `$replace` marker on the node above it — unconditionally, and no
  longer tied to `memory_form`. This is the channel the hive already served on
  `in_memory_call` ([#78](https://github.com/mmeyerlein/meclaw/issues/78)), so it
  is not a new mechanism; a model's own `memory_recall` call is untouched and
  still answers under its original `tool_call_id` with no synthetic pair.
  `talky@3.0.12` and `cogny@3.0.9` re-pin their `collector` reference to the new
  version.

- **`memory_chars` caps the bundle where the bundle now travels**
  (`collector@2.1.0`, [#278](https://github.com/mmeyerlein/meclaw/issues/278)).
  The knob bounds the synthetic tool result, and it is ONE cap over that text:
  under `memory_form: both` it bounds the readable block and the
  machine-readable form *together* rather than each of them separately, which
  changes what a given number buys. `hop.memory_capped` is measured on the
  result. The bundle's bytes always counted towards the curator's budget — they
  reached it as part of `sys_chars`; what changes is that they are now
  attributable, item by item, in the round they belong to.

- **A cell's provenance names the template the CELL came from, not the composite
  it sits in** ([#277](https://github.com/mmeyerlein/meclaw/issues/277)). A
  subtree template used to stamp `cell.provenance` of *every* node of the
  instance — nested cells and hive markers included — with the subtree
  template's own name. So a `collector` cell inside `talky` claimed to be an
  instance of `talky`, and the question "which cells are instances of
  `collector`?" had no answer anywhere in the system. Each node now records the
  template **it** is an instance of, and the new `template_chain` beside it
  names the composites that placed it (see § Added). `template` and
  `template_version` are the projection of the chain's last element; an instance
  of a ref-free template carries a one-element chain, so nothing that read the
  old two fields reads anything different. `instantiated_at` stays the same
  timestamp for every node of one instance.

- **`talky@3.0.12` and `cogny@3.0.9` reference their sub-units instead of
  carrying copies of them**
  ([#277](https://github.com/mmeyerlein/meclaw/issues/277)). `talky` names
  `collector`, `summarizer`, `session-keeper` and `dispatcher`; `cogny` names
  `collector` and `dispatcher`. The instantiated tree is byte-identical to what
  the copies produced — pinned per file by the golden manifests in
  `gh277_composite_instantiation_is_byte_identical.rs` — so a colony grown from
  either template gets exactly the tree it got before, with one difference: the
  cells inside now say where they came from. The two byte pins that guarded the
  copies against drift (`the_sub_unit_copies_are_byte_identical_to_their_templates`
  in `talky_composite.rs` and its `cogny` counterpart) retired with the copies:
  there is nothing left to drift. `templates/channel` keeps its pin — it is
  scheduled for dissolution in [#303](https://github.com/mmeyerlein/meclaw/issues/303)
  and gets no throwaway conversion; the check moved whole into
  `channel_template.rs`.

- **A refused mutation names every violation of the stage that refused it, not
  the first one** ([#293](https://github.com/mmeyerlein/meclaw/issues/293)).
  Validation runs as seven ordered stages and stops at the first stage that has
  anything to say; that stage then reports **all** of its findings instead of
  only the first. A diff with four bad edge endpoints used to cost four round
  trips. **Where the findings are readable:** the structured `violations` array
  is a Rust-internal field on `MutationOutcome::Rejected` — a crate that embeds
  the colony (the substrate's own tests, an in-process host) reads the findings
  item by item. The HTTP and EDA wire carries what it always carried: the
  unchanged `error_code` plus the rendered `details` string, which now lists
  every finding of the refusing stage instead of one. No wire field was added.
  **Stability note:** the `error_code` of a diff with defects in *different*
  stages may now name a different one of them, because the stages were put into
  the order the spec gives (template resolution before requirements). Each
  individual verdict — accept or refuse — is unchanged; what changed is which
  refusal speaks first when there is more than one. `details` stays
  substring-compatible with what it said before.

### Added

- **A bundle candidate carries the provenance the store already held**
  ([#280](https://github.com/mmeyerlein/meclaw/issues/280)). Four items:
  `confidence` per fact (a hedged claim stops reading like a certain one),
  `query_hygiene` (built in #88, only now written down), `complete` /
  `complete_reason` naming which of the three cuts shortened the list, and the
  store's own `valid_until` / `superseded_by` instead of only the derived
  currency marker. In the payload the last pair travels as `until` — the day
  the statement stopped — because a successor's row id says nothing to a
  reader.

- **Three configuration knobs for the fusion, all with defaults that can be
  switched off** ([#297](https://github.com/mmeyerlein/meclaw/issues/297)).
  `MEMORY_SEM_MAX_DISTANCE` (`0.5` of the embedding's bit width),
  `MEMORY_KW_MIN_SCORE_RATIO` (`0.10` of the page's own best bm25 rank) and
  `MEMORY_RRF_AGREEMENT` (`0.5`; `0` restores the plain rank sum). Documented
  in `templates/memory-hive/README.md` § Variables, together with a re-worded
  `MEMORY_TIER1_TOKENS`: the budget measures the payload candidate, never the
  record in the trace beside it.

- **`contract.emits` entries may carry a `description`** — one sentence saying
  what a declared slot means, for whoever reads the contract rather than the
  code. Permitted by the parser all along and now documented in
  `docs/config.md`; the `recall` cell's `recall_diagnostic` and `recall_empty`
  declarations are the first users
  ([#296](https://github.com/mmeyerlein/meclaw/issues/296)).

- **A template can put another template inside itself: `cell.type: "ref"`**
  ([#277](https://github.com/mmeyerlein/meclaw/issues/277)). A directory whose
  `config.json` says `"type": "ref"` describes no cell — it names a template
  (`cell.template`, as `<name>` or `<name>@<version>`, the same form
  `TemplatesRegistry::resolve` takes) and the referenced tree is put at that
  position when the composite is instantiated. `ref` is a **template-time**
  type: it is resolved during staging, has no factory, no dispatcher path and
  no registry row, and never reaches an instantiated `config.json`. A `ref`
  directory carries its `config.json` and nothing else — a second file there
  would give one address two sources and is refused at parse time. At a
  resolved reference the referenced root's `README.md` is dropped together with
  its `template.json`: the two are the descriptor pair of a standalone template
  and belong to it, not to the instance that placed it; the composite's own
  README is untouched. `override_params` sits **top-level beside `cell`**,
  addresses the referenced template's cells by their path inside that template
  (`""` is its root) and layers **below** a mutation's `override_params`: the
  reference sets the default, the caller overrides it key by key. Two new
  `error_code` strings: **`template_ref_cycle`** — a ring of references,
  rendered as the ring itself (`a@1.0.0 -> b@1.0.0 -> a@1.0.0`) — and
  **`requirement_missing`** (below). A reference that resolves to nothing keeps
  the existing `template_missing`, now naming the versions the registry does
  hold under that name (or `none`). Documented in `docs/config.md` § Spezialfall
  Template-Referenz and `docs/cell-types.md`; the decision behind it is
  ADR-0011 (*A template references another template as a sub-unit*).

- **`registry.template_chain` — the composites that placed a cell**
  ([#277](https://github.com/mmeyerlein/meclaw/issues/277)). `colony.db` schema
  **v6**, purely additive: one nullable `TEXT` column holding a JSON array of
  `[name, version]` pairs, outermost first, the cell's own template last
  (`[["talky","3.0.12"],["collector","2.1.0"]]`); `version` is `null` when the
  template declares none. An update to a composite finds its instances through
  the **first** element, an update to a referenced sub-unit through the **last**
  — the question GH #277 was filed about. The column is written at
  instantiation and at every boot; `NULL` and an unreadable value both read as
  "no chain recorded", because the instance's `config.json` stays the source of
  truth and the table is the index. Older databases migrate in place; nothing
  that read schema v5 changes.

- **A template declares what it needs: the `requires` block**
  ([#292](https://github.com/mmeyerlein/meclaw/issues/292)). `template.json` may
  carry `requires.ctx` / `requires.env` — the keys an instantiation must supply,
  each with an optional `because` that the refusal quotes back. It is checked
  where a contract belongs: `validate_requires` runs inside `handle_mutation`
  right after the template resolution and **before the first byte is staged**,
  so a forgotten `ctx.model` is refused instead of being copied to disk and
  breaking during substitution, and a missing `env` key is caught at all (it
  used to surface only at run time, on a cell that was already born). The
  requirements of a template's `ref`s are requirements of the composite. Refusal
  code: **`requirement_missing`**. A `resume`/reconnect `add_nodes` entry is
  exempt — it re-attaches an existing cell and does not repeat the template's
  contract. **Documented limit:** a composite whose subtree already partly
  exists takes the merge path and still stages before the check bites, so there
  the old late `ctx_key_missing` remains; tracked with the `swap_nodes[].with`
  gap in [#347](https://github.com/mmeyerlein/meclaw/issues/347). **Shipped with
  a `requires` block in this release:** `talky@3.0.12`, `cogny@3.0.9`,
  `summarizer@2.0.1` and `llm-unit@2.0.2` — each declaring the `ctx` keys its
  own cells substitute (`model` for all four, plus `model_fast` for `cogny`'s
  lookup lane). The blocks were derived from what the trees already read, not
  invented beside them.

### Breaking

- **The tier-1 bundle candidate no longer carries the retrieval's bookkeeping**
  ([#296](https://github.com/mmeyerlein/meclaw/issues/296)). Gone from
  `system.memory.bundle`: `id`, `rank`, `score`, `legs`, `session_id`,
  `episode_id`, the successor row id `superseded_by`, the full `history` chain
  and the exact instants (days now). **The history cut applies to the JSON
  payload slot only:** `previously` there carries exactly one entry, the claim
  this one immediately replaced, while the rendered text block travelling in the
  same prompt still ends a superseded line with the whole chain — both renderers
  share one annotation helper, and giving it a limit changes a rendering other
  tests pin byte for byte, so it is its own package. Gone from the bundle level:
  `legs_present`, `leg_sizes`, `semantic_degraded`. **Migration:** every one of
  them is in the SAME message, in the `recall_diagnostic` body slot — which the
  message log stores whole, so `/colony/messages` is the place a past run is
  read back from. The tier-2 `dialectic` call still receives the full internal
  records. A consumer that read these off the bundle must be re-pointed at
  `recall_diagnostic.candidates`; reading them off the payload does not raise,
  it silently yields nothing, which is the failure mode to look for.

- **A consumer reading the recall bundle out of `system.memory` finds a
  revocation there** (`collector@2.1.0`,
  [#278](https://github.com/mmeyerlein/meclaw/issues/278)).
  `system.memory.recall` carries an empty text on every turn, and the `json`
  form's per-bundle keys are gone from the brain message entirely — the
  `$replace` marker on `system.memory` clears them. **Migration:** the bundle is
  the last `tool_result` of the round, answering the synthetic `memory_recall`
  call immediately before it; read it out of `messages[]` instead. An `llm`
  cell's `system_writable` allowlist is unaffected and must still carry `memory`
  as a prefix, because the replace ROOT is checked.

- **An `override_params` key must name a param the target cell really has**
  ([#294](https://github.com/mmeyerlein/meclaw/issues/294)). The check that a
  key addresses an existing *cell* has been there since #140; what it never
  checked is the key inside it, so `{"brain": {"temprature": 0.2}}` was written
  into the instance and silently did nothing. A key that names no declared param
  is now a refusal that lists the params the cell declares. **Migration
  note — this refuses instantiations that used to succeed.** A template whose
  cell carries `"params": {}` (or no `params` at all) declares **no** params, so
  every `override_params` key against that cell is now refused: an opt-in
  knob is only settable if the template declares it. This tree was swept in the
  same change; an out-of-tree template that relied on the silent write must
  declare the param. The escape is documented and costs one line: declare the key with
  `null` — a declaration whose value is `null` says "this cell takes this param,
  and has no default for it", and an override against it is accepted. The same
  layering as before applies: the `ref` marker's `override_params` underneath,
  the mutation's on top. **What the new check covers, precisely:** the param
  half — "does the key name a param the cell declares?" — is checked on the
  **mutation's** `override_params`. A `ref` marker's own `override_params` gets
  the older cell half only ("does it address a cell that exists?"); its keys are
  not yet held against the target cell's declared params. That gap is tracked
  with the `swap_nodes[].with` one in
  [#347](https://github.com/mmeyerlein/meclaw/issues/347).

- **Two templates under `templates/` may no longer declare the same `name`**
  ([#277](https://github.com/mmeyerlein/meclaw/issues/277)). A bare-name
  reference — which is what `cell.template` and a mutation's `"template"` field
  usually are — must have exactly one answer, so the scanner refuses a second
  `template.json` declaring an already-seen `name`, **regardless of its
  version**, with `duplicate template name … — a template name must be unique so
  a bare-name reference has one answer`. **Migration note — this is breaking on
  user trees.** A library that keeps two versions of one template side by side
  (`templates/talky-3.0.11/` and `templates/talky-3.0.12/`, both declaring
  `"name": "talky"`) no longer scans at all: the scan aborts, and with it the
  boot or the `RescanTemplates` that would have loaded the library. Keep one
  directory per name; a version that must stay reachable belongs in a separate
  library root, not beside its successor.

### Fixed

- **A rescan triggered from inside the colony walks the template library, not
  the whole workspace**
  ([#277](https://github.com/mmeyerlein/meclaw/issues/277)).
  `/colony/templates/rescan` has two doors. The HTTP door always handed the
  colony the `--templates` path; the EDA door — a cell emitting to the endpoint
  — handed it the colony ROOT instead, so a rescan from within the colony
  descended into `main/`, `blobs/` and every other directory under the root and
  offered as a class whatever `template.json` it found on the way. The
  divergence was survivable while a repeated name was merely shadowed. With the
  Q7 uniqueness rule of this wave a repeated name aborts the scan, and the
  builder hive keeps the approved draft in its staging directory while moving a
  copy into the library — two directories, one name, both under the root. The
  scan aborted, NOTHING was registered, and the deploy that followed failed with
  `template_missing`. Both doors now scan the same library; the CLI passes the
  resolved `--templates` path to the colony task. Regression lock:
  `gh277_rescan_scans_the_templates_root.rs`.

- **A `code` cell whose `script_inline` exceeds 128 KiB spawns again**
  ([#349](https://github.com/mmeyerlein/meclaw/issues/349)). The substrate handed
  an inline script to the runner as one `argv` string (`<runner> -c <script>`).
  Linux caps a **single** `argv` string at `MAX_ARG_STRLEN` = `32 * PAGE_SIZE` =
  131 072 bytes — independent of `ARG_MAX` and not raisable — so every `code`
  cell above that line died at `spawn()` with `Argument list too long (os error
  7)`. `memory-hive/recall` crossed the cap during this wave (141 063 bytes as
  shipped, 140 387 with `${VAR:-default}` resolved) and its read path could not
  start at all: every query timed out as "no bundle" instead of reporting that
  the cell never ran. An inline script above the cap is
  now written to a per-spawn temporary file (mode `0600`, unlinked when the spawn
  ends) and the runner is pointed at that path — the very `<runner> <path>` form
  `script_path` already uses. stdin is untouched and still carries the document.
  Under `trust: "restricted"` the substrate grants that one file a read right and
  nothing else, so the standing promise that a `script_inline` needs no
  filesystem declaration of its own survives. **Below the cap nothing changes**:
  the `-c` form stays, and with it `sys.path[0]`, `__file__` and the shape of a
  traceback. No test had ever driven the argv path — every probe of a shipped
  script pipes it to `python3 -`, where no cap exists — which is why the suite
  stayed green while the shipped cell could not boot; the regression lock
  (`gh349_a_big_script_still_spawns.rs`) goes through `CodeCell::handle`.

## [0.17.2] — 2026-08-21

### Fixed

- **A `store` error reply carries `hop.operation` like every other reply**
  ([#331](https://github.com/mmeyerlein/meclaw/issues/331)). The three hand-built
  error emitters (`emit_invalid_input`, `emit_write_denied`, `emit_query_timeout`)
  wrote `finish_reason`, `error_code` and `duration_ms` and left `operation` out —
  while `output::build_tool_result` stamps it on every regular answer and the
  shipped template contracts declare `hop.operation` as `required: true`. An edge
  conditioned on `hop.operation` therefore lost exactly the replies that report a
  failure. The operation is bound once after `parse_tool_call` and feeds the
  write-surface check and all four post-parse emitters; the params-slot sites carry
  `params_update`, the parse-failure site the literal `error`, because there
  nothing parseable ever arrived.

  **Behaviour change for template authors.** A lane that dispatches on
  `hop.operation` used to see error replies fall through and now receives them —
  `error_code` is from here on the **only** field that separates a failed answer
  from a successful one on that lane. Every such lane needs an `error_code` guard,
  or it will treat a refusal as a result. The lanes in the shipped library that
  still dispatch this way are tracked as
  [#343](https://github.com/mmeyerlein/meclaw/issues/343); each one pulls its own
  template bump behind it. `builder-librarian@2.0.2` is the first: two places in it
  asserted the absence this fix ended — a comment in `retrieve`'s `script_inline`
  and a README paragraph — and the conclusions they carried get *stronger*, not
  weaker, because a return path over the context marker is independent of the
  header shape by construction. Pinned in
  `gh331_store_error_paths_stamp_operation.rs`, four cases, all four red before.

- **A topology-only declaration survives the vacuity projection**
  ([#333](https://github.com/mmeyerlein/meclaw/issues/333)).
  `consumes.topology.inbound_edges` (#160) is not a message chamber: it unlocks a
  spawn capability (`NeighbourhoodView`), not an ingress, and so writes nothing
  into any of the three mandatory-key lists. `CompiledConsumes::is_vacuous`
  counted exactly those lists plus `body_declared`, so a cell whose entire
  contract *is* that one declaration was vacuous, its view was dropped at spawn
  (`consumes: None`), and the gate behind it answered "not declared" and withheld
  the handle — no error, no dead letter, no diagnosis. The same silent shape
  `body` had until #323, one field over. `is_vacuous()` counts `topology` now;
  pinned as a unit case in `contract.rs` (positive and negative — an empty block
  stays vacuous) and end to end in
  `gh333_topology_only_declaration_survives.rs`, where a factory logs the handle
  the substrate hands out at spawn. No existing verdict flips: a cell declaring
  `topology` beside mandatory keys was never vacuous.

- **`code`: `external_timeout_ms: 0` is refused instead of failing every run at
  the deadline** ([#334](https://github.com/mmeyerlein/meclaw/issues/334)).
  `as_u64()` let the zero through, it reached `CodeParams` unchanged and became an
  A-timeout of zero milliseconds: the deadline had passed before the script could
  start. The siblings (`bash`, `web_search`, `web_fetch`) have always refused the
  same input; `code` now does it with their literal
  ("params.external_timeout_ms must be >= 1"), in the shape of the #322
  `max_concurrency` guard three lines below. The parity test covers both axes.
  Deliberately not swept along: the non-integer message here still reads
  "external_timeout_ms must be integer" without the `params.` prefix — a second
  parity detail, recorded on the issue rather than changed in silence.

- **`steward@2.0.4`: the way back is checked like the way out**
  ([#326](https://github.com/mmeyerlein/meclaw/issues/326)). The mutator's revert
  branch ran only the radius half of the check, so three degenerate stored plans
  went onto the `mutate` lane unbraked — a `to: ""` that becomes
  `params {"max_tokens": null}`, a `target` no edge condition matches, and a
  missing `target` — and under each of them stood a receipt reading
  `closed/reverted` though nothing had merged. A return path that skips a barrier
  the outbound path had to pass is not a return, it is a second door. The
  change-shaped half of `check()` is `check_change(change)` now and the revert
  branch calls it; the decide side keeps its reason codes character for
  character. The step limit is made inert **in the code** rather than in the
  prompt — the revert branch drops `from` before it checks — because the limit
  measures in percent of the value being left and is not symmetric: the way back
  from a permitted decrease would have been refused as `step_too_large_67_pct`,
  leaving standing precisely the unproven value the revert exists to remove.

- **`steward@2.0.5`: the probe asks about the mechanism the loop actually uses**
  ([#338](https://github.com/mmeyerlein/meclaw/issues/338)). Since #304 the decided
  change leaves the hive as an ordinary params update on the `mutate` lane — this
  steward authors no mutation at all. The probe still asked `mutation_log` whether
  a mutation had committed and got "no" for **every** healthy cycle: verdict
  `unhealthy`, reason `mutation_not_committed`, and an immediate revert of a change
  that had worked exactly as intended. `look()`'s first question is now whether
  this cycle's params update reached the cell it names, answered from the
  `message_log` rows the function reads anyway. The race is closed by construction
  rather than by luck: mutator and probe order leave in one batch, so the ledger
  row can trail the probe by milliseconds and the read is retried
  (`STEWARD_PROBE_LEDGER_TRIES`, 100 ms apart, bounded against the cell's own
  `external_timeout_ms`). Why it went unnoticed: the only probe pin ran in the
  crate directory, where `colony.db` does not exist — `probe_unavailable` was the
  only reachable branch.

- **`steward@2.0.6`: the README names the three questions the probe judges**. It
  promised "did this cycle's params update reach the cell it names, has the colony
  produced errors since, and has that cell gone quiet" — but `verdict_of()` judges
  `unavailable`, `params_update_seen`, `errors` and `dead_letters`, and
  `target_messages` is collected and never judged. The published promise named a
  verdict that does not exist. The page now names the three that carry one, says
  outright that the message count lands in the receipt without a verdict attached,
  and puts `probe_unavailable` where it belongs — read before the three, failing
  closed. Documentation repair only; the probe is untouched.

- **`access@2.0.3`: the broker's clock declares the write surface its twin
  declares** ([#332](https://github.com/mmeyerlein/meclaw/issues/332)). The clock is
  a `timer` cell, and a `timer`'s `cell.db` **is** its schedule.
  `contract.write_surface` was missing, missing means `open`, and `open` bounds
  nothing: a `transfer` `import` on `/access/clock` is answered by the substrate in
  `cell_task`, before the consumes gate and before `handle()`, so an imported
  `schedules` row is a firing with an `emit_to` of the writer's choosing — the
  clock dials numbers nobody in this hive decided on, on a cadence nobody set.
  `affinity/clock` has declared exactly this since #260. Two pins, both seen red
  first; the red run showed the smuggled row sitting in the table.

- **`access@2.0.4`: the policy store does not travel**
  ([#336](https://github.com/mmeyerlein/meclaw/issues/336)).
  `contract.write_surface` (#260) bounds only the import half of the transfer slot
  — an export is a read and was deliberately untouched by it. Through that half the
  broker's entire state left in one answer: `grants` (every row a live bearer
  handle), `cred_refs` (which variable name stands behind which connector) and the
  complete `audit` history. The store declares `contract.transfer: "none"` now, the
  #314 mechanism `./vault` already carries, so the refusal (`transfer_exempt`) is
  decided before the arguments are read and reads the same to every question. A
  grant is a bearer handle and migration means re-granting at the destination —
  the README's `The honest limit` section says so, and names which tables ride
  along as a catalogue seed and which stay behind.

- **Five templates stop wiring readers past the boundary they seal**
  ([#337](https://github.com/mmeyerlein/meclaw/issues/337)). Each of them declares
  `params.ports: []` and then offered lanes at addresses `validate_hive_port_boundary`
  refuses with `hive_port_boundary` — copy-paste-ready JSON that cannot work.
  `affinity@2.0.6` (five lanes at `./brief`, `./gate`, `./push`),
  `llm-unit@2.0.1` (entries at `./prep` and `./collector`, exits at `./dispatch`
  and `./llm` — and the exits carried `finish_reason` conditions that are dead at
  the unit path, because the egress edges translate the reason into a lane inside),
  `talky@3.0.9` (six interior addresses, plus a closing sentence that stated the
  defect as a rule), `channel@1.0.1` (its documented generation mutations pinned
  `talky@3.0.8`, a version that stopped existing with the bump above) and
  `dispatcher@1.0.1` (three endpoints at `./collect/assemble`, refused at **both**
  ends because the boundary check reads `from` and `to`). The `DECLARED_DEBT`
  list in `gh311_ports_slot_addresses.rs` is gone with its last subject — an
  assertion over an empty list would be red from now on.

- **`coder-pipeline@2.0.2`: the interpreter bytecode cache gets a documented
  guard** ([#111](https://github.com/mmeyerlein/meclaw/issues/111)). In the workshop
  scenario from #104 an agent edit swapped one character at equal file size within
  one second. CPython's `pyc` header is whole-second mtime plus size, so the stale
  cache counted as fresh and the re-run tested the **old** module — the exact shape
  an edit-test loop produces continuously. The new cookbook note
  `workshop/cookbook/interpreter-bytecode-caches.md` carries the measured failure,
  the guard (`python3 -B`, or `PYTHONDONTWRITEBYTECODE=1` where the loop does not
  own the command line) and the analogues in other toolchains, with the
  distinguishing property named: a freshness test over a content hash is safe, one
  over mtime plus size is not. The substrate half was deliberately **not** built —
  a `bash` cell has no `env` key, so there is no place in the configuration where
  the variable could be set, and the honest deliverable is the convention on the
  command line.

- **The spec trias says what the code does, in three places it did not**
  ([#254](https://github.com/mmeyerlein/meclaw/issues/254)). The count of cell types
  with a `cell.db` is **eight**, not ten: the `transfer` slot was described as
  covering "all ten" in six places and two doc comments, in a list that counted
  `code` and `stdio_child` — neither of which holds a `DbConn` (`code/factory.rs`
  says so itself). Three `error_code` surfaces were missing entirely: the `vault`
  had no failure classification although it writes seven codes onto the wire (the
  closed list is there now, and it states why `transfer_exempt` is *not* on it —
  that is the substrate's answer to the body slot, not the vault's emission);
  `query_timeout` was absent from all four sections that emit it (`subcolony`,
  `mcp`, `proxy`, and `timer`, where the A-timeout wraps every `cell.db`
  operation); and the `mcp` cell writes no `finish_reason` at all, so a failover
  edge on `hop.finish_reason == 'error'` never fires for it and the section now
  names the trap and the code such an edge must read instead. Third part: the six
  `--vault` CLI flags reached the flag table (they have been declared since #151),
  `happened_at` reached the English overview, and `inject_map` the German
  cell-types — two sections that had been living in one language each.

- **An unconditional out-edge is an always-edge, not a default**
  ([#283](https://github.com/mmeyerlein/meclaw/issues/283), documentation half). The
  spec promised "default routing" as a "settable catch-all out-edge" in four
  places. The substrate has no such thing: `apply_edges` yields one decision per
  edge whose condition holds, and `condition: None` means *always take* — no
  ordering, no first-match, no exactly-one. An unconditional edge fires **beside**
  every matching edge, never in their place, and cannot express "only if nothing
  else fired" at all. Both language versions say what holds today and name #283 as
  where the real default construct is tracked; the new pin
  `an_unconditional_edge_fires_beside_a_matching_one` anchors the sentence to the
  behaviour, because the existing fan-out test uses two unconditional edges and
  proves nothing about the mixed case. The construct itself and the rewiring stay
  open.

- **`harness`: neither `allowed_tools` nor `permission_mode` is an upper bound,
  and `approval: "channel"` is a fixture promise**
  ([#46](https://github.com/mmeyerlein/meclaw/issues/46), closed with a receipt).
  The acceptance smoke never reached the control path — it *granted* `Write` under
  `acceptEdits` and the model reached for `Bash`, which that mode allows anyway.
  The rig topology is parameterised now, and a second smoke takes away instead of
  giving. Measured 2026-08-21 against CLI 2.1.237 with `sonnet`, two otherwise
  identical runs: with `--allowedTools` omitted entirely, `Bash` ran under
  `--permission-mode default` **and** under `--permission-mode plan`, both ending
  `status: ok` with the shell output, neither sending a `control_request`. So the
  real CLI reads stdin once at startup as a prompt lane and does not hold it open
  as a control lane, and **neither** knob the cell type offers today yields a
  measured bound. `--disallowedTools` and `--tools` are **not** measured and are
  therefore neither documented nor wired — no guessed flag, in documentation or in
  production code. Both language versions say this, and the two decisions that
  remain (`--input-format stream-json`; a first-class upper-bound param) live in
  `docs/roadmap.md` rather than in a successor issue, so there is exactly one
  place for them. Point 3 of the old defer register — the harness busy check
  running before the dedup check — was decided and pinned back in 0.9.0 and moved
  to the resolved history with its ruling.

### Added

- **A template README's H1 version is checked against its `template.json`**
  ([#335](https://github.com/mmeyerlein/meclaw/issues/335)). The H1 names the
  version the page below it describes (`templates/README.md` § Versioning) and
  nothing checked it; on 2026-08-20 the two ran apart twice. The sweep walks every
  direct child of `templates/` carrying both files and judges any `@<version>` in
  the H1 against the declared one. Two guards against a sweep that goes green by
  doing nothing: a floor of 15 judged templates — sized for the **smaller**
  published subset, because a subset is not a defect and an empty sweep is — and
  the versionless templates must stay a subset of the seven known ones, so a new
  template cannot simply drop its version and swim along in the silence. Measured
  in the tree: 31 templates, 24 judged, 7 exempt, all 24 agreeing.

- **A README wiring example is checked against the seal it wires past**
  ([#337](https://github.com/mmeyerlein/meclaw/issues/337)).
  `gh311_ports_slot_addresses.rs` gains a second surface: besides the PORTS slot it
  now scans every `templates/**/README.md`
  (`a_readme_wiring_example_addresses_the_hive_it_seals`). Only `"from"`/`"to"`
  **inside a fence** are read — there they are the substrate's own keys, and a
  `./<parent>/<child>` is an address and cannot be prose about one. Seal index,
  boundary query and the prose exemption are the slot's; only the unit of the
  exemption differs (the text since the previous fence or heading instead of the
  sentence).

- **The export receipt can audit instead of only stopping**
  ([#339](https://github.com/mmeyerlein/meclaw/issues/339)). Three findings at the
  same gate. R2b was line-blind: its expression anchors on the path literal
  `../../templates` but read `git grep` output, and `cargo fmt` spreads a longer
  `Path::join` chain over three lines — the same reference, in the other spelling,
  was invisible. `_normalise_joins()` brings the chain into the path form it
  describes, and the rule reports the **original** line, so the structural and the
  literal rule are one rule (measured over the export tree: 4 additional hits, 0
  lost, each one judged). `--keep-going` turns `red()` into a collector: the build
  path is byte-for-byte unchanged without the flag, and an audit run never builds a
  commit, not even green — whether an export comes into being hangs on the caller's
  intent, not on a measurement's outcome. And `head_tail()` replaces the failure
  truncations that kept the summary and threw the evidence away: a checker prints
  its findings first and its summary last, so `[-1500:]` kept exactly the half
  without evidence.

- **The `builder-librarian` generator follows its tree, and can say so**
  ([#329](https://github.com/mmeyerlein/meclaw/issues/329)). The generator carried
  an older state than the committed template and would have written it back
  silently on the next run. The tree is the truth, so the generator was reconciled
  to it, never the other way round — seven places across `config.json` and
  `retrieve/config.json`, including the guarded CEL form
  (`has(hop.route) && hop.route == 'lsearch'`). New: `--check` (tree diff with
  `MISSING`/`STRAY`/`CHANGED` per path, a bounded unified diff, and the temp tree
  kept for re-diffing) and `--out DIR`, both in the shape of its sibling
  `build_builder_hive.py`. R11 attests three generated artefacts now instead of
  two.

- **`STEWARD_PROBE_LEDGER_TRIES`**, a documented knob on the steward's probe
  (default 3, 100 ms apart): how often the health check re-reads the ledger before
  this cycle's params update counts as missing. It exists because the mutator emits
  the update and the probe order in one batch — see #338 above.

## [0.17.1] — 2026-08-21

A remediation release, and the third digit is the whole point of it. Every entry
below restores something that was already promised — by a template's own
`template.json`, by the spec trias, by a cell type's documented refusal — and
nothing here hands a caller a capability that was never on offer.

Two entries look like they move the second digit and do not. `contract.transfer:
"none"` is a **new declaration**, but what it restores is the `vault`'s oldest
promise: two callers, passphrase or nothing. The substrate's `transfer` slot
(0.17.0) answers above `handle()`, so it walked straight past that ACL and handed
out the vault's inventory, its salt and its whole audit trail to anyone with an
edge. A door that was supposed to be shut and was not is a **repair**, not a
feature. And the three new gates do not change what the substrate does — they
change what can quietly stop being true about it.

Twenty-two issues, of which twelve are shipped templates whose documents,
addresses or numbers had drifted from what the code does. That is the shape of
this release: the 2026-08-20 consistency audit read the whole tree against
itself, and this is the night that answered it.

### Fixed

- **A rejected mutation leaves no registered cell**
  ([#276](https://github.com/mmeyerlein/meclaw/issues/276)). Four rejects could
  fire *after* `handle_mutation` had already registered the node half of a diff —
  `required_drain_missing`, `hive_contract`, `stop_wiring_unavailable`,
  `term_timeout`. None of them took the registry back and none wrote a terminal
  `mutation_log` row. The caller got a 422 and the colony kept half the mutation:
  in the first observed case thirteen cells, including a second `proxy` polling
  the same bot credential as the one already running.

  Registration now runs **behind every check that can judge the diff itself** —
  the spawn loop and the subtree registration moved after both post-state
  validations, which cannot be pulled forward because they need the real
  post-state `EdgeTable`. The two runtime rejects get a real rollback:
  `UpsertRegistry`/`SetRegistryProvenance` travel in the write buffer instead of
  fire-and-forget, so `colony.db` never sees the row; the registry entry and
  `node_contracts` are taken back out; an already-spawned cell is peace-stopped
  **and waited for** before its directory is swept, on the same `term_timeout`
  budget as any other colony-initiated stop, because sweeping a live task's
  directory turns a clean reject into a half-removed tree — worse than the orphan
  the audit model promises. Freshly renamed-in subtree roots are swept too, not
  only staged ones. `terminalize_apply_reject` writes the `failed` row.

  The error-model paragraph of the overview described two bands (before / from
  the rename phase) and therefore lied about exactly this case; it names three
  now, in both language versions, and `required_drain_missing` joined the
  `error_code` enum. Pinned by
  `crates/meclaw-colony/tests/gh276_rejected_mutation_leaves_no_residue.rs`, five
  tests, one per code, each against the registry (RAM *and* `colony.db`), the
  `mutation_log`, and — for the subtree case — a teardown counter that proves the
  rollback waited.

- **A cell can declare that its database does not travel**
  ([#314](https://github.com/mmeyerlein/meclaw/issues/314)). The `transfer` body
  slot is answered by the substrate above every cell type, before `handle()` and
  before the consumes gate — which put it out of reach of every rule a cell type
  enforces inside its own `handle()`. The `vault`'s two-caller ACL never saw a
  transfer. An `export` therefore returned the full inventory (`name`, `version`,
  `status`, `created_at` in `vault_secrets` are cleartext), the salt, and the
  complete call history in `vault_audit`. None of it needed a passphrase.

  `contract.transfer: "none"` (default `"all"`) takes a cell's `cell.db` off the
  slot — `export` **and** `import`, because what may not leave may not be
  overwritten across the same seam either. The refusal carries
  `error_code: "transfer_exempt"` and is decided **before the arguments are
  read**: a refusal that sounds different per table name would itself be an
  inventory. Deliberately **not** a type-name blocklist in the substrate — a list
  in `db_transfer.rs` is invisible in the affected cell's `config.json`, invisible
  in a diff, and has to be touched again for the next cell type with the same
  need. Both shipped vault templates declare it: `vault@1.0.1`, `access@2.0.2`.

  `contract.write_surface` (0.17.0) and `contract.transfer` now travel as one
  value through the spawn helpers and stay independent: `write_surface` says
  *who* may write and leaves an `export` alone — which is precisely the gap #314
  opened, because giving away the vault was a **read**. Documented in both
  language versions (`cell-types` § Content transfer and § vault, `config` §
  contract); pinned end-to-end against a real vault on a real `cell.db`,
  including the negative pin that shows what still travels without the
  declaration.

- **`code`: `max_concurrency: 0` is refused instead of silencing the dispatcher**
  ([#322](https://github.com/mmeyerlein/meclaw/issues/322)). Zero passed the
  integer check, survived the `unwrap_or(4)` default and landed as
  `Semaphore::new(0)`: no permit, no acquire, no drain. The cell was registered,
  active and permanently mute — no error, no dead letter. Its five siblings
  (`bash`, `file`, `edit`, `web_search`, `web_fetch`) have always refused it;
  `code` was the outlier, and it takes over their wording verbatim so an operator
  reads one sentence and not six. A parity test holds the six together.

- **An optional `consumes.body` declaration is a declaration**
  ([#323](https://github.com/mmeyerlein/meclaw/issues/323)). `required` governs
  the ingress check, not the declaration — but the capability switch read the
  *required* projection. A cell declaring `consumes.body.attachments` with
  `required: false` got no `AttachmentReader` at spawn, and because the two
  `None` branches are deliberately indistinguishable to the cell ("no store
  wired" vs "not declared"), the withdrawal was silent. Measured rather than
  guessed: `required: false` appears in 46 `consumes.body` entries across 43
  shipped configs, so the sentence "there is no optional `consumes.body` field"
  was simply false and refusing would have broken all 43.

  The trias said that sentence four times and says the true one now, in both
  language versions: `required: false` drops the **presence obligation and the
  type check with it** — an optional key never enters the projection
  `validate_consumes` walks, so its `type` token is documentation and not a gate
  today. Checking present optional keys against their token would be a behaviour
  change on 46 shipped usages and belongs in its own issue with its own
  measurement.

- **The surface locator asks for the root cell instead of assuming it is called
  `main`** ([#324](https://github.com/mmeyerlein/meclaw/issues/324)). The locator
  resolved `root.join("main")`, a literal for something boot has never required —
  `assert_single_root_dir` takes whatever the single top-level directory with a
  `config.json` is called. A colony whose root directory has any other name
  booted cleanly, registered its surfaces, and answered **every** surface URL
  with 404. No warning, no dead letter; the page was simply not there. The
  locator now asks `find_root_cell_dir`, the same anchor mutation and boot
  already use, including for the containment check. The two #159 fixtures never
  wrote a root `main/config.json` at all — a tree no boot would have accepted —
  and now write and name one.

- **`steward@2.0.3`: the loop can commit, for the first time in its life**
  ([#304](https://github.com/mmeyerlein/meclaw/issues/304)). The mutator emitted
  a `swap_nodes` diff with `params` at entry level on **both** paths, decide and
  revert. The validator has always demanded `match.name` + `with` and reads no
  `params` there, so the steward has never once committed a change — and
  `swap_nodes` could not have carried it anyway: it is an edge swing, not an
  in-place rewrite of a running cell.

  The decided change now leaves the hive as an ordinary params update — body
  `{system:{}, params:{…}}`, `hop.target` naming the cell — the shape
  `llm-registry/hand` already drives. No `messages[]` (that would buy an
  inference on delivery), empty `system` (otherwise it is not a UBF body). The
  radius gets **narrower**, not wider: an edge cannot be computed from a body, so
  the parent draws one per reachable cell instead of handing the loop the whole
  tree through `/colony/mutations`.

  Three review rounds found the same defect class twice more, on the other half
  each time. The numeric radius had no receiver at all — there is no
  `OverlayParams` for `CodeParams` and the shipped cap sits on a `code` cell, so
  a receipt would have said `applied` while nothing happened. The radius is a
  **key set** now (`STEWARD_NUMERIC_PARAM_KEYS`, the runtime-mutable numeric
  `llm` params); a key outside it comes back as `key_outside_radius_<key>`,
  receipt `refused`, nothing emitted. Both directions call one `in_radius`
  function, because the revert branch called `check()` never and emitted
  unconditionally. And a revert whose stored plan is gone no longer reads as a
  completed way back: the meter closes the row in the same breath as it issues
  the revert command, so the row is reopened as `status=applied,
  outcome=revert_refused` with a `reason_code` — the true state (the unproven
  change is still running) and the loudest one.

- **`builder-hive@2.0.2` can deploy again**
  ([#305](https://github.com/mmeyerlein/meclaw/issues/305),
  [#327](https://github.com/mmeyerlein/meclaw/issues/327),
  [#328](https://github.com/mmeyerlein/meclaw/issues/328)). It had been unable to
  ship anything since 2026-08-18, broken three independent ways by two changes
  that were each correct on their own.

  *#327 — boot died before a cell spawned.* The comment in the `stager` script
  that **explains** strict substitution triggered it: substitution reads
  `script_inline` as text and knows no difference between comment and code, so
  there stood a variable named `VAR` with no value and no default and
  `meclaw --validate` aborted with `env_var_missing`. It carries the escape form
  now.

  *#305 — the deploy plan violated the hive boundary.* The plan drew its
  report-home edge onto `./<hive>/capture`, a cell **inside**. Hours after the
  #215 fix the hive was sealed with `ports: []`, and from then on the colony
  refused the builder's mutation with `hive_port_boundary`: stage, gate, release,
  promote — and then no deploy. Repaired with the **lane**, not an exception:
  declaring `capture` a port would reopen exactly the interior address the seal
  closed. The plan addresses the hive path and stamps `hop.route = 'in_report'`,
  a lane `params.contract` already accepted.

  *#328 — freshly drafted cells read a retired wire.* Cells the builder drafts
  read their body with `json.load(sys.stdin)`, correct on the old single-object
  wire and wrong since the three-object wire landed on 2026-08-15: the first
  stdin object is no longer the body, so deployed cells parsed the wrong object
  and emitted degenerate output.

  The shape all three share is that nothing re-ran a drafted cell end to end
  after a correct substrate change. That is what R12 below is for.

- **`builder-hive`'s own gate knows the types the registry spawns**
  ([#325](https://github.com/mmeyerlein/meclaw/issues/325)). `g1` checked every
  `cell.type` in a staged draft against a hand-written `KNOWN_TYPES` set — a copy
  of the registry that had drifted from it: it carried `hive`, which no factory
  spawns, and lacked `vault`, which #151 added as a real cell type. Measured
  consequence: a valid draft containing a vault cell failed the house gate with
  `unknown_cell_type`, stage `g1_failed`, and was never promoted. The set is
  exactly `built_in_factories()` (14 types) now, and the hive branch runs
  **before** the set check — a hive is a scope marker, not an actor, so it does
  not belong in the spawnable set and is still a legal draft node. Pinned set-equal
  against the registry, not merely as a subset: a type `g1` knows and nobody can
  spawn would stay invisible until the mutation.

- **`affinity@2.0.5`: the audience-set rule is fail-closed in every shipped
  topology** ([#306](https://github.com/mmeyerlein/meclaw/issues/306)).
  `config.json` promised the caller that the hive promotes audience and
  participants itself. The door edge carried no modifier, and **no modifier in
  the whole repository** ever wrote `context.participants` — so the refusing half
  of the audience-SET rule was dead code everywhere it shipped. Measured: a round
  `{a,b,c,d}` reading a line released to `{a,b,c}` was served, and the fourth
  participant was unreportable.

  The door promotes now, `has()`-guarded because a failing `set_context` makes the
  modifier fail and a failed edge is **skipped** — the caller would get nothing
  instead of a no. The fallback to "the asker alone" is gone: it called itself
  fail-closed and was the opposite, since `{asker}` is the *widest* set you can
  hand a subset test. A round nobody declared is refused.

  Two review rounds sharpened the precedence chain, and the second one mattered:
  sorting it per **spelling** left `hop.participants` above
  `context.audience_set` — and `audience_set` is the one round a real colony pins
  by edge today (talky, receptionist, memory-drain, session-keeper), so the very
  danger round 1 fought (a cell stamping itself a smaller room via a hop key)
  survived untouched on the spelling that is actually used. Level now beats
  spelling: both context spellings stand above every hop spelling. The door also
  resets `aff_phase`, `aff_carry` and `aff_subscriber`, because `context` travels
  colony-wide and an inherited `aff_subscriber` would land the answer on someone
  else's push lane. The two names for one round are tracked as
  [#330](https://github.com/mmeyerlein/meclaw/issues/330); `affinity` reads both
  until it is bridged.

- **`talky@3.0.8`: a prune answers, and the answer has a door**
  ([#312](https://github.com/mmeyerlein/meclaw/issues/312)). `in_prune` has
  always been in `contract.accepts` and was passed to `./collector`. The
  collector's report is unconditional — one per cut session, or the single
  zero-report — and both leave on `hop.route == 'prune'`. Talky's exits from
  `./collector` were `answer`, `write`, `turn_write` and `recall`. `prune` was
  neither door nor `emits` entry, so **every** prune request, including the one
  that found nothing, cost a `no_route` dead letter for its own answer. The
  deletion happened; what was lost was the account of it, which is the reason to
  trigger a prune at all. Three lines of contract that belong together: the door,
  the `emits` entry naming both kinds, and the `required_drains` pairing (lane
  form, #237) — send `in_prune`, take `prune`.

- **`collector@2.0.6` and `session-keeper@2.0.2`: the address of a sealed hive is
  the hive** ([#311](https://github.com/mmeyerlein/meclaw/issues/311)). Both carry
  `params.ports: []`, so the hive path is the only address and a mutation naming
  a cell inside is refused with `hive_port_boundary`. Both templates recommended
  exactly that — in the same breath as "wire the ports in the **same** mutation
  that instantiates the hive", which is precisely the surface the seal guards.
  `collector/README.md` said it three times and wrote the correct address once in
  the same paragraph; `session-keeper` offered `./stamp` and `./close` in the
  from-column of its exit table. Both `template.json` files carried the same
  addresses in their `PORTS` slot, which `templates/README.md` § Versioning
  presents as the caller's interface.

  New gate: `gh311_ports_slot_addresses.rs` reads that one slot — not prose in
  general — and asks the substrate whether each `./<child>` it names would be
  admitted. `gh203_documented_port_addresses` reads literal `from:`/`to:` keys
  and said in its own header that a sentence naming an address is not
  distinguishable from prose about one; this is the form it left out.

- **`builder-librarian@2.0.1`: a hyphen was syntax, and the error wore the face of
  an answer** ([#308](https://github.com/mmeyerlein/meclaw/issues/308)). Every
  multi-word template name in the corpus this librarian indexes is hyphenated —
  `daily-digest`, `builder-hive`, `memory-hive`, `coder-pipeline`. The tokeniser
  left the hyphen in the token and joined terms unquoted with `OR`; FTS5 reads a
  bare hyphen as syntax, so `daily-digest OR cell` came back as
  `no such column: digest`. The query the template exists for was exactly the
  query that could not run. Each term travels as a quoted phrase now, which is
  the FTS5 spelling for "these tokens, adjacent" — what a hyphenated name is.

  The second half is worse: the store writes `operation` on every answer and
  `error_code` on top, so a failed search arrived in the same shape as a
  successful one. The cell recognised its phase by `operation` alone, parsed the
  error text as a result set, found no rows and rendered "(no matching
  patterns)" — indistinguishable from an honest empty answer. It reads
  `error_code` now.

- **`coder-pipeline@2.0.1`: the verdict survives its spelling**
  ([#309](https://github.com/mmeyerlein/meclaw/issues/309)). `revout` compared the
  reviewer's whole first line against `{APPROVE, REFINE, FAIL}` with `fail` as the
  fallback. Measured over 15 spellings, six fell through: `APPROVE - looks good`,
  `APPROVE.`, `**APPROVE**`, `REFINE: add the marker`, `**REFINE**` and
  `1. APPROVE`. `fail` routes to `taskarchive` — the false rejection was archived
  as the verdict and ended the loop, indistinguishable from an honest one. The
  verdict is read as the first alphabetic token of the first line now (markdown,
  numbering, punctuation and case fall away); an unknown leading token stays
  `fail`. The README, which still described the v1 topology and a `coderloop`
  cell that is not among the graph's 15 endpoints, was rewritten against the
  actual edges.

- **`access@2.0.2`: two dead vault edges removed, and the policy store bounds an
  import** ([#307](https://github.com/mmeyerlein/meclaw/issues/307)).
  `./invoke -> ./vault` on `hop.route == 'vault'` plus the answer edge back could
  never fire: `invoke` calls `emit()` with four literal routes, none of them
  computed, and the hive is sealed — `./vault` was reachable from nowhere and the
  two edges were decoration shaped like a channel. The new test forbids the class
  rather than the string `vault`: any edge leaving a **cell** of this hive on a
  `hop.route ==` comparison must find that route as a literal in that cell's own
  script. Door edges are exempt; their route is stamped outside. Second half,
  same hive: the policy store declared neither half of the write surface, and
  absence means `open` — `contract.write_surface: "internal"` now, without which
  a `transfer` import from any sender plants policy rows in bulk.

- **`llm-registry@2.0.2`: the writer table names the writers that exist**
  ([#310](https://github.com/mmeyerlein/meclaw/issues/310)). Three documents, three
  stories. The README credited `models` and `subscribers` to a "hand (admin
  lane)" and `incidents` to "hand or a drain" — `hand` takes one operation and
  writes exactly `tiers` and `resolutions`. Fifty lines above stood "v1 has no
  admin lane"; `store/config.json` carried the contradiction inside a single
  sentence; and `template.json` claimed `models`, `subscribers` and `incidents`
  came from the seed files, while `store/seed/` holds `models.jsonl` and
  `tiers.jsonl` and the README itself says `subscribers` is deliberately
  unseeded. Re-measured: `models` does have a writer (the seed). Without one
  under this hive's lanes are `subscribers` and `incidents` — reachable by a
  **boot-graph** edge from the parent, which the port seal deliberately does not
  govern (it checks a mutation's `add_edges`), and which this repo's own test tree
  wires. The store declares `write_surface: "internal"`, pinned at runtime rather
  than by comparing a config line to itself.

- **Template documents and descriptions that sent a reader somewhere the code does
  not** ([#321](https://github.com/mmeyerlein/meclaw/issues/321)). No behaviour
  change; each one is a shipped JSON or README making a false statement about the
  tree, and each moves a third digit because a shipped JSON under the template
  moves with it. `collector@2.0.6` counted "ten entry lanes" and "five exit
  routes" where the script has eleven and the contract six, and omitted
  `in_thread_call` entirely — a `description` slot travels over
  `/colony/templates`, so it is a statement to the caller and not a note.
  `affinity@2.0.5` claimed all four legs of its precedence chain were guarded and
  skipped when empty (the two hop legs only test `has()`), and that only
  `affinity/gate` writes into `affinity/store` (`brief` writes audit rows).
  `firewall@2.0.2`, `session-keeper@2.0.2`, `vault@1.0.1` and the hive and
  infrastructure READMEs carry their measured numbers now.

  The rule this batch establishes, and then had to apply to itself: **whoever
  carries a byte copy carries its version**. A composite embedding another
  template's `config.json` byte-for-byte moves its own third digit when that copy
  moves — `talky` 3.0.6 → 3.0.7 → **3.0.8** and `cogny` 3.0.5 → **3.0.6** across
  this release, with the whole chain of README titles, library rows and
  `"template": "x@y"` literals swept rather than assumed. `talky`'s and
  `access`'s README H1 had drifted from their own `template.json`, which is where
  a reader takes the version from before copying the `add_nodes` block below it.

- **The English cell-types file had the two `mcp`/`subcolony` sandbox rulings
  swapped** ([#313](https://github.com/mmeyerlein/meclaw/issues/313)), across
  section boundaries: the subcolony argument stood inside `## mcp`, word for word,
  closing with a sentence *about* `mcp` in the third person, and the mcp one-liner
  stood inside `## subcolony`. Swapped back.

- **`docs/roadmap.md` stopped describing shipped work as future**
  ([#316](https://github.com/mmeyerlein/meclaw/issues/316)). A defer register
  lives on every row being an open debt; fourteen were not. Ten are fully redeemed
  and moved to `docs/archive/roadmap-resolved.md` with their evidence — each
  checked against the code, not against the issue. Four were half true and are
  narrowed to their true half rather than struck. Plus a dead anchor that stood
  twice in the same item and pointed about 530 lines into an unrelated chapter,
  which is what makes absolute line numbers in prose worse than merely
  impractical: they do not just drift, they eventually assert something false.

- **The public surfaces say what the tree holds**
  ([#317](https://github.com/mmeyerlein/meclaw/issues/317)). Eight lines claimed
  more or older than the repo, every replacement measured against the tree today:
  the test count, the cell-type table (`vault` had no row while two paragraphs
  down stood "all 14"), the `builder-hive`'s place (real and tested — in the
  **private** tree; it is not in `PUBLIC_TEMPLATES`, so nothing in the published
  tree instantiates it), CONTRIBUTING's version, README § Stability's claim that
  everything under `crates/` is `publish = false`, `memory-hive`'s cell count, and
  `.env.example`'s template pin next to the quickstart. The retroactive
  `memory-hive@2.2.1` entry under 0.17.0 comes from the same pass.

- **`PROGRESS.md` stopped claiming status**
  ([#318](https://github.com/mmeyerlein/meclaw/issues/318)). It declared itself the
  repo's sole status owner on line 3 while its newest entry was wave 7 of
  2026-08-14 — 24 releases and eleven minor lines behind — and `AGENTS.md`
  ordered it as first reading of every session *and* classified it as
  non-authoritative, two lines apart. The body is archived; `PROGRESS.md` is a
  signpost to `CHANGELOG.md` (release truth) and `docs/roadmap.md` (defer
  register). The generated librarian seed carried the old sentence into a shipped
  artefact and is regenerated.

- **`docs/archive/roadmap-resolved.md` restored, and 170 finished records
  archived** ([#315](https://github.com/mmeyerlein/meclaw/issues/315),
  [#320](https://github.com/mmeyerlein/meclaw/issues/320)). 47 rows, 9 bullets and
  the header of the resolved register were lost to an overwrite on 2026-08-18 and
  are recovered. The sanitation pass that followed archived 170 finished records
  with archive headers, fixed the 7 real dead links in live documents plus 34 link
  occurrences the moves would have broken, and added the indices that close the
  orphaned-index hole — `README.md` now links `docs/README.md`, from which 38 of
  430 documents were reachable before.

### Added

- **A spec-claims registry, checked in CI**
  ([#254](https://github.com/mmeyerlein/meclaw/issues/254)). A review compares code
  against spec, so a spec that runs ahead of the code passes every review by
  construction. This runs the other direction: 47 rows classifying every
  behavioural claim of the spec trias — 15 `pinned` (claim plus a named test), 8
  `built-unpinned` (built, and still waiting for the test that would catch its
  regression), 24 `specified-not-built` as 12 twin pairs. `scripts/check_claims.py`
  checks three things: an anchor must appear verbatim in its document, so a
  paragraph cannot be rewritten out from under its row; a `pinned` test must exist
  in the corpus; and a `specified-not-built` paragraph must carry its visible
  marker in **both** language versions — a marker the reader cannot read is not
  one.

  The gate knows both tree shapes, recognised by a property rather than a flag: a
  tree is published when `docs/` carries no `*.en.md` at all, which is exactly
  what the export's `DOCS_MAP` leaves behind. There the English rows resolve to
  the plain name and the German originals are skipped, counted and named
  individually. A tree without `.en.md` that nonetheless carries
  `plans/spec-claims/claims.tsv` is a hard error — that is a work tree that lost
  its twins, not a publication.

  First entry under the rule: the overview has promised **instantiation at
  bootstrap** since 2026-05-21, in both language versions, and the substrate never
  had it — `GraphHints` accepts `edges` and nothing else under
  `deny_unknown_fields`, so a hive `config.json` carrying the documented `nodes`
  block is a hard boot error, not an ignored hint. The paragraphs are not deleted;
  they are the target picture of
  [#277](https://github.com/mmeyerlein/meclaw/issues/277) and now carry the marker
  ([#277](https://github.com/mmeyerlein/meclaw/issues/277)).

- **Every accepted ADR names what holds it up, checked in CI**
  ([#319](https://github.com/mmeyerlein/meclaw/issues/319)). The decision record had
  broken in half — the predecessor holds `decisions/0001-0025`, this repo had
  `plans/adr/0001` and `0002` and a numbering restarted at 1. Legacy ADR 0011 read
  `Accepted` for three months after its capability had vanished, while the spec
  kept promising it, because nothing connected the decision to a line of code that
  can disappear. Port-or-supersede over all 25: **eight ported as 0003–0010**, each
  with a provenance line; seventeen superseded-by-rebuild with one line of reason
  each in `plans/adr/README.md`; 0011 deliberately **not** ported — its row points
  at #277, because re-promising an unbuilt capability is the exact defect the rule
  exists to prevent. The two half-holders state today's fact rather than the old
  promise. 10 ADRs, 28 anchors; `scripts/check_adr_anchors.py` resolves each one.

  Both gates use the same two-tree arrangement, and for the same reason: the
  corpus lives under `plans/`, which never travels, so a **derived** copy travels
  as `.github/gates/*.tsv` and the public job resolves it against `crates/`, which
  does travel and is exactly where the deletion these gates exist to catch would
  be visible. The private half — the texts, the missing-`Pinned-by` check, the
  byte-exactness of the derived copies — rides `cargo test`
  (`a3_spec_claims_gate_drift.rs`, `a4_adr_anchor_gate_drift.rs`), so a divergence
  between registry and travelling copy can no longer wait for someone to think of
  it.

- **The builder-scenario suite is an export gate (R12)**
  ([#305](https://github.com/mmeyerlein/meclaw/issues/305),
  [#320](https://github.com/mmeyerlein/meclaw/issues/320)). It is the only thing
  that drives the `builder-hive` end to end, and #305/#327/#328 are three
  independent breaks it would have caught the day they landed. It runs before an
  export is built. It also no longer believes an empty suite: the runner exits 0
  when its filter matches no case (`passed != len(results)` is false for two
  zeros), which is the silent-skip class this receipt was built against — R12
  derives its expectation from the case directory and refuses a run that asserted
  nothing.

## [0.17.0] — 2026-08-19

The longest wave so far, and the second digit is earned by exactly two of its
entries. Everything else in it is a repair, and a repair takes the third digit
however much work it was: a template that shipped sealed with no door was
**broken**, not feature-poor, and giving it a contract restores what it already
promised.

What moves the second digit is the pair that hands a caller something never
promised — a cell's database can now be handed out and taken in
([#253](https://github.com/mmeyerlein/meclaw/issues/253)), and a `system.*`
subtree can be revoked rather than only overwritten
([#264](https://github.com/mmeyerlein/meclaw/issues/264)). Both add public
surface: a body slot, a contract declaration, an error code.

Four of the repairs were found by the fixes for the others, which is the shape
of the wave: #258 and #259 came out of the review of #252, #260 out of building
#253, #264 out of what #259 could not reach, and #265 out of fixing #256.

### Added

- **Content can leave a `cell.db` and enter a running one**
  ([#253](https://github.com/mmeyerlein/meclaw/issues/253)). The spec promised a
  cell that "receives an `EXPORT` message and writes its database out as JSONL".
  It never existed — the `store` knew twelve operations and none of them was an
  export, and the seed loader could only fill a database at birth. Rebuilding a
  colony from the library therefore meant seeding a new store at creation or
  losing what it knew.

  A substrate body slot, `transfer`, answered before `handle()` and serving **all
  ten cell types with a `cell.db`** — `code`, `harness`, `llm`, `mcp`, `proxy`,
  `stdio_child`, `store`, `subcolony`, `timer`, `vault` — with no per-type code
  and no trait. `{"operation":"export"}` returns an inventory,
  `{"operation":"export","table":…,"key":[…]}` a document, and
  `{"operation":"import",…}` a receipt, into a **running** cell.

  It is the inverse of the seed loader on format and mechanism, proven rather
  than asserted: a test writes an export document out as `seed/<table>.jsonl` and
  hands it to the existing loader, and the reborn cell holds byte-identical rows.
  One asymmetry is deliberate and now in the spec — the export does not write the
  file itself. The loader reads only `seed/<table>.jsonl`, so the filenames the
  spec proposed would never have been read back; a cell writing into its own tree
  is a second output channel no edge carries and the message log does not know;
  and a file inside the cell's own tree crosses no colony boundary, which is the
  actual need. Writing one stays a deliberate act by the owner of the tree.

  Import rules are the memory porter's, one level down: the target wins every key
  collision, additive never replacing, and a part applies whole or is refused
  whole — validation before the first write, writes in one transaction,
  re-applying idempotent. Audience leakage is prevented **structurally** rather
  than by a name list: a part whose declared schema differs from the target by one
  column in either direction is refused, in both directions, with the new
  `error_code: "import_schema_drift"`. It runs on the cell's own connection, so a
  row is searchable in the FTS index the moment it lands.


- **`memory-hive@2.2.1` can hand its remembered content over to another running
  hive** ([#243](https://github.com/mmeyerlein/meclaw/issues/243)). *(Retroactive
  entry, added by the 2026-08-21 audit: this shipped in 0.17.0 and no entry
  announced it, while a later paragraph already spoke of the `in_import` lane as
  if a reader knew it.)* The template ports are public contract, and this release
  moved them: `memory-hive` went 2.1.0 → 2.2.1 and grew two accepted lanes,
  `in_export` and `in_import`, one emitted lane, `dump`, and a twelfth interior
  cell, `templates/memory-hive/porter/` — a `code` cell, no new Rust.

  Until 2.1.0 there was no way out of a hive and no way into a running one: the
  only substrate-native content path was the JSONL seeder, and that one is
  birth-only. Every migration, every backup and every "benchmark against the same
  remembered state" was a hand-built sqlite3 pipeline around the very boundary
  #132 and #160 hold shut. `in_export` walks the fifteen content tables and emits
  one part per table on `dump`, each carrying its store's schema header — a part
  written to disk **is** a `seed/<table>.jsonl`, so the birth path and the
  transfer path speak one format. The last part carries `hop.export_final`; a
  document without it is incomplete and is not a backup. `in_import` takes one
  such part into a **running** hive, idempotently: probe the key column, insert
  only what is missing, so the same document applied twice leaves the same state.
  The two store-keyed families (alias tables, refusal logs) arrive as
  `set_alias` / `reject_pair`, the store's own upserts on the key it created —
  the half of #243 the seeder cannot reach. The audience gate travels with the
  rows: the export projects `audience_set`, `channel` and `speaker` explicitly,
  and a part that lost one of them in transit is refused with nothing written.
  This is the template-level answer to #243; #253 above is the substrate one.

  **What a caller has to wire.** Four new `required_drains` pairings come with
  the lanes (three pairings before, seven now): `in_export` → `dump`,
  `in_export` → `reject`, `in_import` → `dump`, `in_import` → `reject`. Drain
  `dump` with a **plain** `hop.route == 'dump'` edge — an edge that also tests
  `dump_kind` reads as no drain under the probe. Undrained, an export runs, reads
  the whole store and reaches nobody. Pinned by
  `crates/meclaw-colony/tests/gh243_a_memory_can_leave_a_hive_and_arrive_in_another.rs`;
  the lanes and their phases are documented in
  `templates/memory-hive/README.md`.


- **A `system.*` subtree can be revoked, not merely overwritten**
  ([#264](https://github.com/mmeyerlein/meclaw/issues/264)). `system.*` is durable
  state of the `llm` cell: one upsert per slot path, and no DELETE anywhere. A path
  that is not sent is a path that is not touched. For a writer with **fixed** paths
  that is enough — send the slot with an empty rendering and the upsert overwrites
  it (#259 did exactly that). For a writer whose sub-keys are **data** it is not:
  the `json` form of the recall bundle (`memory_form: json|both`) is keyed per
  bundle by the memory hive, so the writer does not know the previous turn's paths
  and cannot name them empty. A key written last turn stayed in the prompt
  indefinitely.

  A node of the incoming `system` subtree may now carry `"$replace": true`: below
  this node, exactly what this message brings holds. The root is **the node
  itself**, never a path named elsewhere; everything at and below it is deleted in
  the **same** transaction, before this message's leaves land — no window in which
  the subtree is gone and its replacement is not there yet, which a separate delete
  operation would have opened. A marker with no leaves under it is the pure
  revocation, in one message.

  **A plain write still replaces nothing.** The marker is opt-in, always: `system.*`
  is deliberately accumulated by several independent writers under different paths —
  an identity pack, a recall bundle, a consult list — and a silent default replace
  would turn each of them into the last one standing. Two writers over one root:
  the last one wins, and that is forced rather than chosen — a cell knows no
  topology and the write gate deliberately does not gate on the sender, so there is
  no identity with which to arbitrate. The protections are the scoping (the root is
  the writer's own node) and `system_writable`, which now checks the replace **root**
  too, not only the leaves: a marker reaches paths the message never names. A marker
  directly under `system` has the empty root, which is under no declared prefix, so
  a cell with an allowlist can no longer be cleared wholesale by one message.

  `$` is reserved inside a `system` subtree. `"$replace"` takes only `true`/`false`;
  any other `$` key and any non-boolean is a loud reject (`error_code:
  "invalid_input"`, nothing written, nothing deleted) — a misspelled marker must not
  pass as a silent no-op, or the writer would hold a revocation that never happened.
  The `WARN` line on `meclaw::llm::system_gate` gains two `reason` values,
  `root_not_writable` and `malformed_replace_marker`.

  Documented in `meclaw-overview.md` § Replace semantics and in `cell-types.md`
  (`llm` state, system write gate). The reference templates are unchanged: the
  `json` form of the collector's bundle gets its own pass.


- **`cogny@3.0.1` can be given a tool, a memory and an error drain**
  ([#240](https://github.com/mmeyerlein/meclaw/issues/240)). The agent core sealed
  to one lane in and one out, so the tool round it is built around had no way to
  call anything, the memory leg R-CG-1 moved onto its collector had nowhere to
  ask, and a failed inference on either brain dead-lettered as `no_route`. All
  three are declared at the hive path now, with the doors to match, the way
  `talky@3` already carries them: `in_tool` and `in_bundle` in, `tool`, `recall`
  and `error` out. `escalate_to_deep` stays the composite's own lane and never
  leaves. The two brains are normalised onto the single `error` lane by the exit
  edges' own `set_hop.route` — there is no `errors` cell here (R-CG-2 names three
  units), and `llm-unit` has done it this way since it shipped.

  **What a caller has to wire.** Nothing, to keep working as before: the lanes
  are additive and the two existing ports are untouched. To use one, an edge pair
  per tool (`tool` out on `hop.tool_name`, `in_tool` back), a pair to the memory
  hive (`recall` out, `in_bundle` back), and one edge draining `error` — undrained
  it dead-letters, loudly. The recipes are in
  `templates/cogny/README.md` § Per-instance lanes.

- **The five demo bots have an address**
  ([#246](https://github.com/mmeyerlein/meclaw/issues/246)). `bot-basic@2.0.1`,
  `slack-agent@2.0.1`, `egon@2.0.1`, `daily-digest@2.0.1` and
  `research-assistant@2.0.1` shipped sealed (`ports: []`) with no
  `params.contract` at all — the closed state: no edge could name a cell inside
  them and no lane was declared at the hive path either, so nothing in a colony
  could be wired to them or from them. Each now declares the lanes it already
  serves. The four chat-shaped ones take a turn on `in_turn` and hand the answer
  back on `answer`; `daily-digest` takes `in_digest` (a run demanded outside the
  timer, with `context.chat_id` brought by the caller) and hands the formatted
  digest back on `digest`.

  **Behaviour is unchanged for a colony that wires nothing.** A turn the channel
  brings in is still answered into the chat and a scheduled digest still goes to
  the notifier; the two paths are told apart by `context.turn_origin` /
  `context.digest_origin`, which the ingress edges stamp and the two
  complementary reply edges read, so exactly one of them fires per turn.

- **`telegram-connector@1.0.0` — a chat channel as a building block**
  ([#246](https://github.com/mmeyerlein/meclaw/issues/246)). Every proxy cell the
  library shipped sat inside a complete demo bot, so a colony that had assembled
  its own agent had no way to put a chat surface in front of it and started with
  a hand-written proxy. The connector is now a template of its own: one sealed
  hive around the credential-bearing `proxy` cell, taken verbatim from
  `bot-basic@2.0.0` but for `params.emit_to`. A turn out of the chat on `turn`, a
  finished answer back in on `in_reply`, the connector's own failures on `error` —
  and `required_drains` pairs the two, because an answer that never arrived is
  invisible at exactly the end where somebody is waiting for it.

- **`channel@1.0.0` — a channel is a hive, its agent is a generation**
  ([#246](https://github.com/mmeyerlein/meclaw/issues/246), ADR-0002 E6/E7/E8).
  One hive per chat or room: the connector that owns the credential, plus the
  slot the current `talky` occupies. The hive IS the channel — the identity that
  stays — and the talkies inside it are its generations, one active and the older
  ones disconnected and preserved. A generation arrives, and is replaced, by one
  mutation: `add_nodes` the new talky and `swap_nodes` the slot's occupant for
  it, which swings every lane atomically. The slot ships occupied by
  `terminal@1.0.0`, so every lane of the contract has a door from the moment of
  instantiation rather than after somebody finishes the wiring.

  **`context.channel_open_history` has a home** (ADR-0002 O1). Whether a room
  shows joiners its history cannot be detected — the Telegram Bot API does not
  expose the setting — so it is declared, on the one edge every turn crosses on
  its way in, and read by the audience gate on the recall path. The default is
  `'0'`, closed: it is the value a missing declaration already means, and of the
  two possible mistakes only one hands somebody a conversation that happened
  before they arrived.

### Fixed

- **A talky the reception builds now closes with the round it was spoken in**
  ([#274](https://github.com/mmeyerlein/meclaw/issues/274), follow-up to #273).
  `receptionist` is the template for "one agent per channel, built on demand" —
  the shape that produces most of the generations a colony will ever have — and
  it draws the ingress edge into every talky it instantiates. Its contract asked
  the caller for one thing, `context.channel`, so since #273 every one of those
  generations was recorded with an empty participant set and its swept-closed
  days were refused at the drain with `missing_audience`.

  The reception genuinely cannot derive the round, and that is settled at the
  code rather than argued: `greet` reads exactly one identity, and it is a
  **channel** — a chat id, a room, a phone number. A participant is a different
  thing (`member:alex`, `agent:scribe`), and translating a connector's own user
  id into that vocabulary is the talky edge's business, never a hive's
  (`memory-hive`: "this hive looks nothing up"). Building a member out of a room
  would invent somebody nobody named. So the round is asked of the caller, on the
  same edge that already names the channel, and the reception hands it on.

  That is ADR-0002 E8 rather than a shortcut: the participant set is a constant
  of a generation's lifetime — a change *ends* it — which is precisely why it
  belongs at the door the opening turn arrives by and why nothing further in can
  still know it. E12 is untouched: the keeper writes it once, at the open, so
  wiring the key afterwards takes effect on the next generation of a channel and
  never on one already running.

  The round did in fact reach those talkys already, as plain `context` that
  nothing on the path happened to delete — the same accident #273 refused to
  build on. **A value that arrives because nobody threw it away is not a
  promise.** It is one now. The set travels as a JSON string; a native list in
  the context is re-serialised rather than stringified, and it never enters a CEL
  string literal, so no caller-supplied text is ever escaped into an expression.

  Nothing is guessed. A caller that declares nothing leaves the key **present and
  empty** — a missing hop key makes the CEL modifier fail and a failed modifier
  skips the edge, so a turn that cannot name its round would vanish instead of
  being refused downstream. Never `["*"]`, never a participant built out of a
  channel id, never a set read off the ledger. `receptionist` 2.0.1 → 2.0.2.

- **A session closed by the nightly sweep now carries the round it belonged to**
  ([#273](https://github.com/mmeyerlein/meclaw/issues/273), follow-up to #269). The
  sweep is a timer firing and carries no `context`, so nothing on the close path
  could name the room or the participant set from the message it was handling — the
  `./session-keeper -> ./collector` edge promoted only `session_id`, and since #269
  such a day was *visibly* refused at the drain instead of silently lost, but it
  still did not land.

  The room, as it turned out, already arrived — through a keeper-**internal** edge
  setting `context.channel`, which travels hop by hop. By accident rather than by
  promise; it is a promise of talky's close edge now. The participant set existed
  nowhere on that path at all: `sessions` had no column for it and the collector's
  window has none either, so there was nothing to promote.

  The participant set is a constant of a generation's lifetime (ADR-0002 E8: a
  change *ends* it), so it is now recorded where it is known exactly once — at the
  ingress door of the opening turn. `sessions` gains an `audience_set` column, the
  `stamp` writes it on the row when it opens a generation, the sweep reads it back
  at the seal, and the close request carries it beside `session_id` and `channel`
  on the hop, where `talky`'s close edge promotes all three into `context`. No
  database read, no topology knowledge.

  Nothing is guessed: a door that declares nothing leaves the column empty, an
  empty round is refused (`missing_audience`, zero ledger rows, the batch stays
  deliverable), `["*"]` is never a default, and no room is parsed out of the
  `session_id` prefix. The empty value travels as **present and empty** rather than
  missing — a missing hop key makes the CEL modifier fail and skips the edge, so
  the close would vanish instead of being refused.

  Provenance is never rewritten (E12) — a turn into a *running* generation does not
  touch it, with its own regression pin — so wiring the key afterwards takes effect
  on the next generation of a channel, not on the open one. `session-keeper` 2.0.0 →
  2.0.1, `talky` 3.0.4 → 3.0.5.

- **`memory-drain` frisst keinen Tag mehr, den es nicht ausliefern kann**
  ([#269](https://github.com/mmeyerlein/meclaw/issues/269)). Die `in_episode`-Lane
  der `memory-hive` verlangt seit dem Publikums-Gate (0.16.0) `audience_set` und
  `channel` und weist ohne beides ab; `templates/memory-drain/config.json` nannte
  keines von beiden. Der Eingang des Drains **trägt** den Teilnehmerkreis
  allerdings sehr wohl: `context` wird Hop für Hop weitergereicht, und weder die
  Kanten des Drain-Hives noch das Skript noch die Runde über den Ledger fassen
  ihn an. Das Rezept war nicht unvollständig.

  Der Defekt lag eine Ebene tiefer und war zerstörend. Kam ein Batch **ohne**
  Publikum, zerlegte der Drain ihn trotzdem, die Hive wies jeden Turn korrekt mit
  `missing_audience` ab — und der Drain schrieb im **selben Multi-Send** die
  `mark`, die den Tag als gedraint verbucht. Danach liefert keine spätere
  Zustellung diese Turns je wieder aus: abgewiesen **und** als erledigt vermerkt,
  von keiner Wiederholung, keinem Replay und keiner korrigierten Kante erreichbar.
  Der laute Fehler der Hive und ein stiller, endgültiger Verlust im Ledger, im
  selben Vorgang.

  Der Drain weist jetzt an der **eigenen** Tür ab, bevor irgendetwas geparkt wird:
  Route `reject`, `hop.reject_reason` `missing_audience` oder `missing_channel`
  (die Vokabel der Hive, damit ein Betreiber eine Ablehnung lesen kann, ohne zu
  wissen, welche Zelle sie geschrieben hat), null Ledger-Zeilen, null Episoden.
  Der Batch bleibt damit lieferbar — Verdrahtung korrigieren, denselben Tag noch
  einmal schicken, alles landet. Geraten wird nichts: `["*"]` ist kein Default und
  kann keiner sein, weil es „von jedem in jeder späteren Runde lesbar" heißt und
  der eine Wert ist, den keine Zeile je wieder loswird.

  Beide ausgelieferten Beispiele deklarieren ihren Teilnehmerkreis jetzt an der
  Eingangstür — `never-forgets` hatte seit 0.16.0 gar keinen und schrieb über
  diesen Weg nichts mehr, was niemandem auffiel, weil das Ausbleiben einer
  Erinnerung wie Stille aussieht.

- **An empty `api_key` is no `api_key`, and `llm` now says so on both wires**
  ([#271](https://github.com/mmeyerlein/meclaw/issues/271)). `LlmParams::parse`
  refuses a *missing* `api_key` but accepted `""` — `Some("")` is not `None` —
  and both lanes handed that straight to the wire, so a keyless configuration
  sent `Authorization: Bearer ` with nothing after it. The recorded header was
  `Bearer`, on chat-completions and on responses alike. Against an endpoint that
  would have answered anonymously that can be a flat rejection, and the failure
  then reads as "the provider is down" rather than as "a key nobody set".

  **Not refused, omitted.** #270 made an empty *required* credential a
  parse-time error, and that is right for a chat bot token: a `proxy` without
  one can do nothing at all. An `llm` against a local OpenAI-compatible server
  can do everything — the server ignores the header — so refusing would break a
  keyless setup the library itself ships. The key must still be **declared**
  (missing is a configuration error at spawn); its **value** may be empty, and
  that is the explicit statement "this endpoint needs no credential". The rule
  across the tree is therefore one rule with one criterion: an empty credential
  is an absent credential, and whether an absence is legal is decided by what
  the absence *means*, not by an optional flag.

  The OAuth subscription lane is untouched: the broker's access token never
  crosses a params boundary, and the wire layer presents verbatim whatever
  bearer it is handed. That split is pinned, so moving the filter into the wire
  layer — which would silently change the broker lane too — turns a test red.

  Where the rule is written down, once each: `docs/config.md` § "The empty
  value", `docs/cell-types.md` § `llm` (the `api_key` param), and
  `.env.example`, whose header explained only the *missing* value although
  `VAR=` in a half-filled copy is exactly the form that kept #270 invisible.

- **Der Bearer der `mcp`-Referenzvorlage stand an einer Stelle, die niemand liest**
  ([#268](https://github.com/mmeyerlein/meclaw/issues/268)).
  `templates/_cell-types/mcp-min/config.json` deklarierte den Token als
  `params.bearer`; die `mcp`-Zelle liest ihn aus `params.auth.bearer`.
  `MCP_BEARER` wurde aufgeloest, gespeichert und nie benutzt — es ging kein
  `Authorization`-Header raus, und nichts meldete etwas, weil ein ungelesener
  `params`-Schluessel kein Fehler ist. Diese Datei ist die Vorlage, aus der ein
  Autor die Vertragsform kopiert, also pflanzte sich die falsche Form in jede
  Kolonie fort, die von ihr ausging.

  Sieben von acht angrenzenden Prosa-Behauptungen bestaetigt und korrigiert, eine
  widerlegt. Die schwerste war `access/README.md`: es verschwieg die
  `vault`-Zelle, die das Template mitliefert, und behauptete darueber, eine
  verschluesselte Tabelle in dieser Hive gaebe es nicht und solle es nicht geben —
  wer liest, um zu entscheiden, ob er dem Template ein Geheimnis anvertraut,
  bekam ein falsches Bild davon, wo das Geheimnis liegt. Dazu: `llm-unit` und
  `builder-hive` fuehrten sich als „not sealed", obwohl beide `ports: []`
  deklarieren; `firewall`, `llm-registry` und `receptionist` nannten Innenzellen
  als Endpunkte, die eine Mutation heute mit `hive_port_boundary` abweist;
  `memory-drain` nannte einen `turn-write`-Port der `memory-hive`, deren Lane
  `in_episode` heisst; `door` beschrieb einen HTTP-Ingress ohne `hop`, den es seit
  #175 so nicht mehr gibt.

  Neu gepinnt: jedes `"template": "name@version"` in einem ausgelieferten Dokument
  muss durch `TemplatesRegistry::resolve` gehen — `channel`s zwei
  Generationswechsel-Mutationen nannten `talky@3.1.0` und `cogny`s Beispiel
  `cogny@2.0.0`, beides `TemplateMissing` fuer ein Template, das auf der Platte
  liegt.

- **Ein leerer Bearer ist kein Bearer**
  ([#270](https://github.com/mmeyerlein/meclaw/issues/270)). `web_search` nahm
  `params.api_key` ohne Leer-Filter — `Some("")` ist nicht `None`, also trug
  **jede** Suche `Authorization: Bearer ` mit nichts dahinter. Der Schluessel ist
  in allen drei ausgelieferten Konfigurationen als `${SEARCH_API_KEY:-}`
  deklariert, und `.env.example` liefert die Variable **leer gesetzt** aus, mit
  dem Satz darueber, sie duerfe „left empty for an unauthenticated local SearXNG
  instance" bleiben. Das war also nicht der Sonderfall, sondern der
  dokumentierte. Gegen einen Endpunkt, der anonym geantwortet haette, kann dieser
  Header eine harte Ablehnung sein — und die sieht nach „Suchdienst kaputt" aus
  statt nach „nie konfiguriert". Dieselbe Reparatur, die #268 eine Zelle weiter in
  `mcp` gemacht hat; eine dritte optionale Fundstelle hat der Durchgang ueber alle
  sechs Credential-→-Header-Stellen nicht gefunden.

  Die gespiegelte Frage — wo wird ein leerer **Pflicht**-Wert akzeptiert — hat
  fuenf gefunden, im selben Zug repariert: `mcp`s `endpoint` und `command`, das
  `bot_token` der Telegram-Variante sowie `app_token` und `bot_token` der
  Slack-Variante. Alle fuenf sind als `${VAR}` ohne Default deklariert, die
  *nicht gesetzte* Variable scheiterte also laengst laut und mit Namen; das Loch
  war `VAR=` in einer `.env` — die Form, die eine halb ausgefuellte Kopie von
  `.env.example` hat. Ein leerer Wert wird jetzt beim Parsen abgelehnt, mit
  derselben Meldung und demselben Variablennamen wie der fehlende, statt eine
  Zelle zu erzeugen, die gesund aussieht und bei jedem Aufruf an einem Dritten
  scheitert.

  Beide Haelften sind am **aufgezeichneten Request-Header** gepinnt, nicht am
  gesetzten Param — dass der Param leer ist, war schon vorher gruen, geschickt
  wurde er trotzdem.

- **The `json` form of the recall bundle can be revoked — `collector@2.0.4`**
  ([#266](https://github.com/mmeyerlein/meclaw/issues/266)). #259 built the
  revocation a writer with **fixed** paths can manage: send the slot with an empty
  rendering and the `llm` cell's per-slot upsert overwrites it. The `json` form
  (`memory_form: json|both`) could never do that — its sub-keys are named by the
  memory hive **per bundle**, so the collector does not know the previous turn's
  paths and cannot name them empty. A key written last turn stood in the prompt
  until something happened to write that exact path again; for a key derived from
  what was recalled, possibly never. #264 built the substrate half and deliberately
  left the template alone.

  The marker now sits on `system.memory` — **not** `system.memory.recall`. That is
  the node the collector fills wholesale every turn and the only one that carries
  both legs. One segment up (`system` itself) the root is **empty** and would fell
  every foreign slot in the brain with it — instructions, handover, the affinity
  push lane. One segment down there is no node to sit on: the readable form owns
  the fixed leaf `memory.recall`, the hive's keys are its **siblings** rather than
  its children, and a node carrying `text` is a leaf that nothing hangs below
  anyway. The segment boundary does the rest: `memory` never reaches `memoryx`, and
  `system.consult` is untouched.

  The marker travels unconditionally rather than under `memory_form`: the knob is
  per-instance and retunable, and an instance switched from `json` back to
  `readable` would otherwise carry its last json keys for the rest of its life. It
  is stamped **last**, which is not cosmetic — `$` is the substrate's namespace
  inside a system subtree, the revocation is the collector's own statement about a
  node it owns, and a bundle key of the same name must not overwrite it with data.

  **The empty leaf from #259 stays**, beside the marker rather than replaced by it.
  The two are not substitutes: the marker revokes the keys the collector cannot
  name, the leaf is its claim on the one path it **can**. In the prompt both forms
  read alike (`walk_collect` drops an empty text), but dropping the leaf would make
  that claim depend on what the hive returned — and under `both` a bundle carrying
  a key of its own called `recall` would then reach the prompt on exactly the turns
  the readable leg came back empty, and be shadowed on all others. A fixed path
  does not change owner per turn. The bare node `{"$replace": true}` remains the
  honest form where there is no fixed leaf: in the `json` form a leg that found
  nothing has nothing to write and everything to withdraw, and before this that
  turn sent no `system.memory` at all.

  **Rollout.** No shipped template declares a `system_writable` allowlist. An
  instance that does must carry `memory` as a prefix — `memory.recall` alone no
  longer suffices, because #264 checks the replace **root** as well. Stated in the
  `brain` lane of `templates/collector/README.md`.

  `talky@3.0.4` and `cogny@3.0.4` carry the repaired collector as their
  byte-identical sub-unit copy; nothing else about them changed.

- **A push updates a slot; it no longer buys an inference — `affinity@2.0.3`**
  ([#263](https://github.com/mmeyerlein/meclaw/issues/263)). `brief` answers on two
  lanes through one `answer()`, and that one set `body["messages"]` on both. An `llm`
  cell stays silent only for a body with **no** `messages[]` at all — it persists the
  system slots and returns — and that silence is the whole reason the push lane was
  described as costing nothing. With a turn beside them the same cell does the
  opposite: it calls the provider, on a `tool_result` whose `call_id` it never opened.
  Against a real provider that is an orphaned tool result and therefore a 400; the mock
  accepted it, which is why no test noticed.

  The push lane — the one where `req["subscriber"]` is set, and only `./push`'s edge
  sets it — now carries `system` and nothing else. `readable()` still runs **once**
  above the fork, so the two lanes can never carry two wordings of the same disclosure
  decision (#258). With nothing to disclose the lane sends nothing at all: an *empty*
  body is not silence to an `llm` cell, it is a parse error. `emits.body.messages` is
  no longer `required` in consequence.

  The pin measures the **absence**: it drives a real tick through `./clock → ./push →
  ./brief` of the shipped hive and hands the answer to a real `llm` cell in front of a
  counting mock provider — zero requests on the push lane, the four documented slots
  still in the subscriber's `cell.db` (a "fix" that simply drops the message fails
  here), and exactly one request on the tool lane.

- **`affinity`'s scope note describes the template that ships — `affinity@2.0.3`**
  ([#262](https://github.com/mmeyerlein/meclaw/issues/262)). `template.json` promised a
  human gate on the proposal lane: a proposal lands with status `open` and *waits for a
  human `decide_proposal`*. R-AF-1 decided the opposite, with its reasoning written
  out, and `gate` implements R-AF-1 — a proposal is accepted as it arrives unless the
  caller deliberately passes `auto_accept: false`. Not the behaviour drifted, the
  description did, and it did so in the one field a reader consults precisely to learn
  where a template's authority *stops*. The same sentence stood in three places at
  once: the manifest, the `gate` cell's own `not_in_scope`, and the README's "No
  curator". All three now name the status a `propose` really writes and the exception
  as an exception.

  Two further claims in the same field no longer held either: sovereignty is not
  unenforced but enforced *in half* — a write from outside the hive is refused with
  `write_denied` (#132) and a mutation naming a path inside it with
  `hive_port_boundary` (#133), while a parent's boot graph and every read stay free.

  The pin reads the two status literals out of the real `gate` script by running it,
  and holds every piece of shipped prose against them: the default status is named, and
  the other never in a sentence that does not also name the knob that produces it. A
  narrow rule, and the file says so — free prose cannot be pinned in general, this
  drift class can.

  The same sweep over the rest of the library found eleven more sentences of the same
  kind, all verified at the code: `memory-hive/recall` denied shipping tiers 1 and 2
  that its own script runs, `memory-hive/store` called `traverse` and `similar`
  unbuilt, `llm-unit` and its scribe named an interior cell a documented port under
  `ports: []`, `access/store` described a hive without the `vault` it ships,
  `coder-pipeline` carried its v1 loop description, both `summarizer/prep` copies
  claimed one `system` path where the script writes two, `_cell-types/mcp-min` listed
  the shipped `stdio` transport among the roadmap defers, `memory-drain` pointed at a
  `writer` port the memory hive lost with #228, and `canvy` called `/colony/graph`
  undrawable by a mutation, which #163 changed.

- **A swapped-out generation was unreachable but still awake**
  ([#265](https://github.com/mmeyerlein/meclaw/issues/265)). Connectivity is
  derived from the edge table, and for a hive the spec counts only **external**
  edges — its own inside says nothing about whether anything still reaches it.
  The predicate read the hive path as lying *outside* its own unit, and the hive
  boundary **mandates** the one wiring that breaks that assumption:
  `{"from": ".", "to": "./cell"}`, an edge whose `from` is the hive path itself.
  So a unit with nothing left but its own inside counted as connected.

  What that costs is not an abstraction leak, it is a second agent. After a
  generation swap the old unit keeps its `timer` — `talky` ships one
  (`session-keeper/night`) — and it keeps firing on schedule, summarising and
  writing into stores nothing reads any more, answering nobody. Nothing
  dead-letters, nothing is refused; a channel simply runs two generations, one
  of them invisible.

  A hive is now connected by exactly one predicate: **at least one edge has
  exactly one endpoint in the unit and the other outside it**, where the unit is
  the hive path **together with its subtree**. Both external forms the spec named
  fall out of it — a parent-level edge naming the hive path, and a depth-port
  edge naming a descendant — and the mandated inward wiring falls out as what it
  is, the unit's own inside.

  The question the fix had to answer first was the other one: *what connects a
  hive that nothing points at?* Nothing does, and that is now written down. The
  other ways to **reach** a unit — `POST /messages` naming a hive path, a `proxy`
  or `timer` inside it minting messages with no incoming edge — are entries into
  a unit, not connections of it. A unit with no external edge has no way out for
  an answer either: a message running back to the hive path finds no outbound
  edge and dead-letters as `hive_no_route`. A self-contained unit is therefore
  wired like any other, with a single edge to the outside — which the library
  already requires (`templates/daily-digest/README.md` § Activation asks for
  exactly "ONE crossing port edge … (it may be connectivity-only)").

  **Who could notice.** A hive whose only reason to be awake was its own inward
  wiring now sleeps, subtree and all, until one edge connects it. Measured over
  every colony checked in here and every shipped `grow` file — the examples grown
  with their mutations, the workshop corpus, the test fixtures — no hive changes
  status. If a hand-built topology relied on it, the repair is one `add_edges`.

  And the clock really does stop: the disconnect peace-stops the long-running
  cell and aborts its I/O sub-task. The pin measures that directly on a frozen
  tick counter rather than inferring it from a quiet dead-letter queue.

- **A generation swap swings the outside edges, not the inside**
  ([#256](https://github.com/mmeyerlein/meclaw/issues/256)). `swap_nodes` promises
  to re-dedicate **all external edges** of one implementation onto another. What it
  tested was one endpoint only: every edge naming `match` exactly. On a leaf those
  are the same set. On a **subtree** they are not — the wiring with which the unit's
  root serves its own cells (`<unit> -> <unit>/<cell>` and back, the form the hive
  boundary mandates) names the root exactly too, and was carried over. Afterwards the
  new generation addressed the old one's cells and the old one's cells answered the
  new one: one turn ran through **both** generations — two model calls, two episodes,
  and the generation that is supposed to be silent writing along. Nothing dead-lettered
  and nothing was refused; the answer arrived, twice.

  It was invisible on the FIRST generation change, because a channel ships its
  generation slot occupied by a `terminal` and a leaf has no inside to drag. That is
  the worst ordering a defect can have: it passes the demo and misbehaves in
  production, on a colony that has run long enough to have had a roster change.

  **The inside stays where it belongs.** An edge is external when its *other* endpoint
  lies outside the subtree rooted at `match` — segment-aware, so the sibling `talky-2`
  is not read as a child of `talky`. The spec calls that wiring internal itself
  (overview § Connectivity and activity, hive sharpening), and leaving it is the only
  variant under which the disconnected unit stays **whole**: the documented swing-back
  restores a working generation rather than a hollow one. Re-pointing it at the new
  subtree's corresponding children — which is what a caller seems to want — would
  instead **double** it, because a subtree arriving via `add_nodes` already carries its
  internal edges from its template's `params.graph`. Refusing the swap was the other
  substrate-shaped answer and was not needed: there is a semantics that performs the
  mutation whole instead of by halves.

  `move_nodes` shares the helper and is unaffected: a move of a node with anything
  beneath it is refused in validation, so a moved node has no descendants to name.

- **A diff that replaces an edge no longer loses it to its own check**
  ([#257](https://github.com/mmeyerlein/meclaw/issues/257)). The header mirror — the
  projection the header-contract-locality check takes its post-state from — read edge
  operations as `add_edges` before `remove_edges`. The apply arm has done the opposite
  since [#158](https://github.com/mmeyerlein/meclaw/issues/158), and the spec says so
  too. A `remove_edges` pattern matches every edge between a `{from, to}` pair, so the
  mirror took back the fresh edge the same diff had just laid, and the mutation was
  refused — naming a context key on an edge that was fine. Anyone believing the message
  went and changed the edge that was not broken.

  It hits the everyday case: growing a wiring by adding a promotion to an existing edge
  belongs in ONE mutation, so the lane is never missing in between. The workaround —
  writing the old modifier into the match pattern — required the caller to know that
  the checker and the executor disagree about order.

  **Why sequence and not target state.** `remove_edges` is a pattern, not a set
  subtraction: what it takes out depends on what the table holds at the moment it runs.
  A "post state" without naming that moment is undefined, and the only authority on it
  is the apply arm — which does let `remove_edges` see the swap-swing and subtree edges,
  just not the `add_edges` of the same diff. The mirror therefore stays a sequence
  mirror, aligned step for step with the arm; a second, independent model would be
  exactly the double definition that drifted apart here. The only other place that
  rebuilds a post-state from the diff (`connectivity::post_state_edges`, the eager-spawn
  gate) already computed remove-before-add.

- **The right place is not yet the right shape: `affinity@2.0.2` renders what it
  pushes** ([#258](https://github.com/mmeyerlein/meclaw/issues/258)). `brief`
  attached the disclosed pack to the push lane as `body["system"]` — the pack
  object itself. `system.*` is the right place for that lane: it addresses an
  `llm` cell, which keeps the four slots per path in its own `cell.db`. But an
  `llm` cell flattens `system.*` into leaves, stopping at the first object that
  carries a `text` key, and concatenates exactly those `text` values into the
  prompt. The pack had none anywhere, so it produced **no leaf at all** — not
  persisted, not rendered, not even counted against the 256-slot budget. The push
  delivered, the `audit` table said `ok`, and the subscriber's model answered from
  everything except the brief.

  Every slot now ships its rendering beside its structure: `path: value` lines
  under a heading that names the subject, because a leaf reaches the prompt on its
  own and has to say what it is about without the receipt line next to it. The
  `text` sits at the top of the slot, so the leaf walk stops there and the four
  documented slot paths (`system.identity`, `system.peer`, `system.relationship`,
  `system.channel`) stay exactly four leaves — a subscriber that pins
  `system_writable` to them keeps accepting the write, and each slot goes on being
  upserted on its own. One rendering for both lanes: `answer()` renders once and
  the tool lane serialises that same object, so there is no second wording of the
  same pack that could drift away from the first.

  Same class as the tool-lane defect of #242, one lane over — which is why the pin
  drives the shipped template through a real colony and hands the answer to a real
  `llm` cell in front of a mock provider: what is asserted is the recipient's
  composed system prompt, not that the slot arrived.

- **A `system` slot is revoked, not merely abandoned — `collector@2.0.3`**
  ([#259](https://github.com/mmeyerlein/meclaw/issues/259)). `system.*` is durable
  state in the receiving `llm` cell: one upsert per slot path, so a path that is
  not sent is a path that is not touched. The collector built `system.consult`
  only under `if consults:` — once the advice turns fell out of the window the
  branch was skipped, and the prompt kept naming a consultation that had closed
  long ago. The comment directly above it promised the opposite ("they expire
  exactly when the window forgets the event they belong to").

  **The same class on the memory leg, and the heavier half.**
  `system.memory.recall` was set only when the recall leg had found something. A
  leg that came back empty therefore left the PREVIOUS turn's bundle standing as
  "this turn's memory". The reasoning `collector@2.0.2` shipped with — that the
  bundle tolerates `system.*` because it is "re-sent under a fixed path on every
  turn and can never go stale" — did not hold for the empty case. It does now.

  Both slots the collector owns travel unconditionally, with an empty rendering
  when there is nothing to say. The `text` leaf stays in the empty form and is
  not cosmetic: `flatten_to_leaves` stops at `text`, so a slot offered without
  one produces no leaf and no write, and the stale row would stand exactly as
  before. An empty `text` no longer contributes a part to the system prompt
  either — before, an emptied slot left a hanging `"\n\n"` separator in every
  prompt that followed, and a tree of nothing but emptied leaves produced a
  system message made of whitespace.

  **What this cannot reach, said out loud:** the `json` form of the memory bundle
  (`memory_form: json` or `both`). Its sub-keys are named by the memory hive per
  bundle, so the collector does not know which paths a previous turn wrote and
  cannot name them empty. A key written last turn and absent this turn stays in
  the prompt until something writes that exact path again. Documented in the
  knob table of `templates/collector/README.md`.

  `talky@3.0.3` and `cogny@3.0.3` carry the repaired collector as their
  byte-identical sub-unit copy; nothing else about them changed.

- **A store's `write_surface` now bounds a transfer import too**
  ([#260](https://github.com/mmeyerlein/meclaw/issues/260)). The `transfer` body
  slot is answered by the substrate before `handle()` — deliberately, because
  `consumes` describes what `handle()` needs and a transfer never reaches it. The
  consequence was a real gap: `store`'s `params.write_surface: "internal"` is a
  cell-level check, so an import walked straight past a store that was sealed
  against foreign writers. A declaration a new surface can bypass is not a
  promise any more, it is a trap.

  **New public contract surface, and that is the load-bearing part of the fix:
  `contract.write_surface`** (`"open"` | `"internal"`, absent means `"open"`). It
  had to be new because the substrate is type-agnostic and must not read a cell
  type's `params` — so a boundary the substrate enforces has to be declared where
  every cell type declares in the same grammar: the `contract` block, beside
  `consumes`/`emits`/`ingress`, in the same spirit as `consumes.topology` (#160)
  and `contract.ingress` (#185). All ten cell types with a `cell.db` can set it,
  not just the `store`.

  `"internal"` refuses an `import` whose sender lies outside the cell's own parent
  scope, with `error_code: "write_denied"`, decided after the arguments are parsed
  and before the first row is written. Same scope arithmetic and the same
  fail-closed rule for a message with no sender as #132, so a cell declaring both
  halves gets one boundary rather than two that disagree — and the two are
  deliberately not derived from one another, because one bounds what `handle()`
  runs and the other what the substrate runs before `handle()` is ever reached.

  It is a **provenance** rule and nothing else — own path plus sender, no look
  inside the document. What it does not refuse: an `export` (a read; no write
  surface has ever bounded a read), an import into a cell that declares nothing,
  an import from inside the cell's own scope — the shipped
  `memory-hive` `in_import` lane runs `porter -> store` and is untouched — and an
  import into a cell directly under the colony root, whose parent scope is `/` and
  contains every cell.

  The shipped sealed stores declare both halves now: `memory-hive`, `canvy`, and
  the `steward`'s `charter` and `receipts`. Documented in both language versions:
  `docs/config.md` § contract (the declaration and its relation to #132) and
  `docs/cell-types.md` § Content transfer, whose paragraph described the gap as a
  property and now says both boundaries hold.


- **A tool result is its `messages[]`, and `collector@2.0.2` keeps all of it**
  ([#252](https://github.com/mmeyerlein/meclaw/issues/252)). The `in_tool` lane kept
  `messages[0]` and discarded the rest, so a tool that answered two calls in ONE
  message closed one of them: the other stayed in the round's expectation set,
  the fan-in parked, and the turn was only ever finished by the idle exit
  (`round_idle_ms`) with a synthetic stand-in for a result that had already
  arrived. Every turn of a result now enters the round under the call id it
  answers, and the fan-in reads all of them. A tool behind a sealed agent hive
  has no other way back — `params.ports: []` means no edge from outside may name
  a cell inside — so this was the whole return path for the shape.

  **What a result may NOT carry, decided rather than inherited.** A `system` slot
  or a top-level body slot on that lane is still dropped, and the `in_bundle`
  lane that keeps `system` is not the precedent it looks like: what leaves the
  collector's seam in `system.*` is upserted into the brain `llm` cell's own
  `cell.db` and stands in the prompt until something overwrites that exact slot
  path. That is durable state of the agent, not evidence of one round. The recall
  bundle survives it because it is re-sent under a fixed path on every turn and
  can never go stale; a single tool result gets no second chance to correct
  itself, and `system.*` is out of the curator's reach and out of the round's
  byte budget by design, so nothing downstream could cut it back. A tool with
  structure to hand back serialises it into the text of its result — the pattern
  `affinity@2.0.1` already uses, now the documented answer instead of a local
  workaround. Written down where a tool author stands:
  `templates/README.md` § Writing a cell a tool round will call,
  `templates/collector/README.md` § What a tool result may carry, and
  `docs/store-backed-tool-loop.en.md` § 3.

  `talky@3.0.2` and `cogny@3.0.2` carry the repaired collector as their
  byte-identical sub-unit copy; nothing else about them changed.


- **The lane a curator stub names is a lane the hive admits**
  ([#245](https://github.com/mmeyerlein/meclaw/issues/245)). `collector@2.0.1`
  declares `in_thread_call` in its hive contract, and `talky@3.0.1` declares it
  at its own path and forwards it through its door edge. The collector has
  served `thread_recall` on that lane since wave 11 — it is what every stub the
  curator leaves points at, by name, in the text the model reads — but no
  `params.contract` in the library listed it, so the one edge that makes the
  tool reachable was refused at mutation time with `hive_contract`. The defect
  was invisible until somebody set `context_window`: the knob that produces
  stubs is on by default, the knob that triggers them is off, and the first
  person to tune for context would have seen a curator that eats tool results
  and a round that stalls once per elided item until `round_idle_ms` closes it.

  **What a caller has to wire.** One edge, the same shape the memory tool has
  had since #78 and next to it in every recipe:
  `{"from": "./<agent>", "to": "./<agent>", "condition": "hop.route == 'tool' &&
  hop.tool_name == 'thread_recall'", "modifier": {"set_hop": {"route":
  "'in_thread_call'"}}}` — plus the tool's schema in the brain's
  `system.tools`, which is per instance as it always was. Wire it whenever you
  set `context_window`.

- **`collector`'s contract no longer offers a lane nothing behind its door
  reads.** `in_batch` was declared and never dispatched on: an edge wiring it
  passed validation and every message on it was swallowed in silence. It is out
  in 2.0.1, so such an edge is refused rather than parked. Nothing in the
  library, the examples or the workshop ever sent `in_batch` to a collector —
  the lane belongs to `summarizer` and to the drains, and both keep it.

## [0.16.0] — 2026-08-19

### Breaking

- **`memory-hive@2.1.0` refuses a turn that does not say who was in the room**
  ([#244](https://github.com/mmeyerlein/meclaw/issues/244)). The write lanes
  `in_episode` and `in_remember` now require two context keys — `audience_set`,
  the participant set the turn was said in front of, and `channel`, the room —
  and a caller that omits either is refused on the `reject` lane with
  `hop.reject_reason` set to `missing_audience` or `missing_channel`. Nothing is
  written. `in_episode` gained a `required_drains` entry for that lane, so a
  colony can no longer wire the lane and let the refusal fall on the floor.

  **Migration.** An edge that carries a turn into the hive promotes the two keys
  into context, the way it already promotes `session_id`. The participant set is
  a constant of the caller's lifetime rather than something looked up per turn —
  see the design ruling in the issue. A store that already holds untagged rows
  can be backfilled only while it has provably seen a single participant set;
  after that, an untagged row can be guessed at but not filled honestly.

- **The read lane `in_query` requires `audience_now` and `channel`**, and refuses
  rather than answering with an empty bundle. A refusal and an empty answer are
  different sentences and the caller has to be able to tell them apart.

### Added

- **A fact remembers who was there, and the recall will not tell it to anyone
  else** ([#244](https://github.com/mmeyerlein/meclaw/issues/244)). `episodes`
  carry `speaker`, `channel` and `audience_set`; `facts` and `entity_edges`
  inherit the room and the set from their episode; `beliefs` and `skills` — which
  are derived rather than said — carry the **intersection** of their sources'
  sets, so two private facts cannot be laundered into one shareable claim. The
  filter runs in the tier-0 bundle and in all four tier-1 legs, before RRF
  fusion, so a hidden row cannot influence ranking either.

  The rule, in the order it is evaluated: an untagged row is invisible; a
  universal set is visible; a round that is a subset of the recorded set is
  visible; and a row from the *same* channel is visible when that channel shows
  its history to people who join. The last clause never crosses a channel
  boundary — material from a private conversation stays out of a group one no
  matter what the group's history policy says. It is `affinity`'s subset rule
  from [#154](https://github.com/mmeyerlein/meclaw/issues/154), now applied on
  both halves of the ruling instead of one.

- **Recall says when it cannot vouch for currency.** The temporal leg filters
  before it builds a version chain, because the other order would let the
  existence of an invisible version show through a validity span — ask often
  enough and you map out when something was said in a room you are not in. The
  cost of filtering first is that a claim superseded by an invisible version
  would otherwise look current, so such a candidate now carries
  `supersession_unknown`, and neither tier 0 nor tier 2 asserts currency for it.
  It is a boolean and nothing more: no count, no instant, no channel of what was
  removed. What the asker learns is not something about the other room, but
  something about the agent's own certainty.

## [0.15.1] — 2026-08-19

### Fixed

- **`params.required_drains` can name a LANE, and a sealed hive can insist
  again** ([#237](https://github.com/mmeyerlein/meclaw/issues/237)). The port
  form pairs a port with a route and fires when something outside wires that
  port — which a sealed hive has no way of letting happen, so since the seals
  (0.15.0) the declaration could never fire and `memory-hive` shipped one
  release without the one guarantee it had. The same obligation now exists in
  the vocabulary the boundary leaves standing:

  ```json
  "required_drains": [
    {"accepts": "in_remember", "emits": "reject",
     "because": "a refused block leaves the hive on this lane"}
  ]
  ```

  Read as: *a caller that sends me `in_remember` must subscribe to `reject`.* A
  mutation that wires the ingress without the drain is refused with
  `required_drain_missing`, pre-destructively, carrying the hive's own sentence;
  boot only warns, as it does for every hive declaration. Both names must be
  lanes of the hive's own `params.contract`, or the reader drops the entry —
  a rule that cannot fire reads exactly like one that can.

  `memory-hive` **2.0.1** declares two pairings (`in_remember` and `in_query`,
  both draining `reject`). A colony that already runs the template is not
  affected: an instance carries the config it was born with. The old port form
  keeps working for hives that never sealed themselves.

  One documented limit: the drain is found with a route-only probe. An
  unconditional out-edge counts, and a condition that fails to evaluate against
  the probe counts as unknown and therefore as drained — only an out-edge that
  evaluates cleanly to `false` is read as "no drain". A subscription that guards
  a second hop key with `has()` falls into that gap; give the lane an edge of
  its own.

- **A path segment matches a template's name whole or not at all**
  ([#238](https://github.com/mmeyerlein/meclaw/issues/238)). The documented-port
  scan resolved a segment to a template whose directory name merely *ended* with
  it, so `./agent/session-keeper/stamp` in `templates/README.md` — where `agent`
  stands for whatever the reader named their own hive — was blamed on
  `slack-agent`. Right about the address, wrong about the file. The shortened
  instance name (`memory-drain` → `<drain>`) survives where it is one: inside
  the template's own documents, and a finding that reads it says so.

## [0.15.0] — 2026-08-18

### Breaking

- **Every hive template that ships is now behind its own boundary**
  ([#197](https://github.com/mmeyerlein/meclaw/issues/197),
  [#228](https://github.com/mmeyerlein/meclaw/issues/228)). With `steward` and
  `memory-hive` the four templates whose ports carried the name of a cell inside
  are migrated, and with the fourteen that declared no `params.ports` at all the
  library has no unsealed hive left. The rule they all satisfy is the one ruled
  on 2026-08-18: an edge is laid at the HIVE, a caller asks for something by
  content, and the inner edge that receives the request is what knows where it
  belongs. **A lane is named for what the caller wants, never for where it
  lands.**

  | Template | Was | Now | The interior addresses that stop resolving |
  |---|---|---|---|
  | `steward` | 1.0.1 | **2.0.0** | `meter`, `mutator` |
  | `memory-hive` | 1.4.0 | **2.0.0** | `writer`, `recall`, `extract-glue` |
  | `talky` | 2.0.0 | **3.0.0** | `session-keeper`, `collector`, `dispatcher`, `errors` |
  | `cogny` | 2.0.0 | **3.0.0** | `collector` |
  | `firewall` | 1.0.0 | **2.0.0** | `screen` |
  | `receptionist` | 1.1.0 | **2.0.0** | `greet` |
  | `llm-registry` | 1.0.0 | **2.0.0** | `select`, `hand`, and the hand-wired `store` admin edge |
  | `llm-unit` | 1.1.0 | **2.0.0** | `prep`, `collector`, `dispatch`, `llm` |
  | `builder-hive` | 1.1.0 | **2.0.0** | `intake`, `brief`, `capture`, `deploy`, `promote` |
  | `builder-librarian` | 1.0.2 | **2.0.0** | `retrieve` |
  | `coder-pipeline` | 1.0.0 | **2.0.0** | its transit door now names a lane |
  | `bot-basic`, `slack-agent`, `egon`, `research-assistant`, `daily-digest` | 1.0.0 | **2.0.0** | every cell; these carry their own network surface and are addressed by no edge at all |

  **The migration is one edit per address: drop the last segment and name the
  lane.** Per template, in and out:

  | Template | In | Out |
  |---|---|---|
  | `steward` | `in_cycle` | `mutate`, `error` |
  | `memory-hive` | `in_episode`, `in_query`, `in_remember`, `in_flush` | `bundle`, `reject` |
  | `talky` | `in_turn`, `in_sweep`, `in_tool`, `in_advice`, `in_bundle`, `in_memory_call`, `in_prune`, `in_round_sweep` | `answer`, `write`, `turn_write`, `recall`, `tool`, `error` |
  | `cogny` | `in_turn` | `answer` |
  | `firewall` | `in_turn` | `pass`, `reject` |
  | `receptionist` | `in_turn` | `turn`, `mutate` |
  | `llm-registry` | `in_select`, `in_hand` | `answer`, `ack`, `update`, `error` |
  | `llm-unit` | `in_task`, `in_tool` | `tool`, `answer`, `error` |
  | `builder-hive` | `in_spec`, `in_request`, `in_report` | `mutate`, `rescan` |
  | `builder-librarian` | `in_request` | `brief` |
  | `coder-pipeline` | `in_task` | — |

  So `{"from": "<surface>", "to": "./talky/session-keeper", "modifier":
  {"set_hop": {"route": "'in_turn'"}}}` becomes `"to": "./talky"` with the
  identical modifier, and `{"from": "./talky/collector", "to": "<sink>",
  "condition": "hop.route == 'answer'"}` becomes `"from": "./talky"`. The
  per-instance tool lane changes shape rather than only its address: it used to
  read `{"from": "./talky/dispatcher", "condition": "hop.tool_name == 'x'"}` and
  is now `{"from": "./talky", "condition": "hop.route == 'tool' &&
  hop.tool_name == 'x'"}` — the dispatcher's `tool` route was always on the
  wire, it just had nowhere to be declared. The three shipped examples are
  migrated and are the worked reference.

  **Three things the hives took over from their callers**, because the inner
  edge is where structure is allowed to be known: the memory hive's own door
  stamps the `phase: "recall"` that starts a fresh recall chain (#152) and the
  `store_origin`/`mem_phase` of the inline lane; the firewall's exits drop the
  four context keys its screen parks a turn in, which every caller previously
  had to remember. A caller can no longer get any of them wrong.

  **`receptionist` draws its per-channel wiring from the hive now.** Its greet
  cell writes that mutation itself and is self-locating; under its own seal an
  edge out of `./reception/greet` is a breach the validation refuses, so it
  locates the HIVE instead. Its three address knobs (`RECEPTIONIST_INGRESS`,
  `RECEPTIONIST_REPLY_FROM`, `RECEPTIONIST_ERROR_FROM`) default to **empty** —
  the instance path — instead of `session-keeper`, `collector` and `errors`. A
  `.env` that still names those three wires a composite at addresses `talky@3`
  no longer has.

  **`memory-hive` lost `params.required_drains`, and that is a real loss rather
  than a cleanup.** The pairing rule (#147) hangs on a PORT and fires when
  something outside the hive wires that port; a sealed hive has none, so the two
  entries could never fire again. A rule that cannot fire reads like one that
  can, so they were removed rather than left as decoration — the README says in
  their place that the `reject` lane must be drained and why. The substrate gap
  (`required_drains` has no way to name a LANE) is
  [#237](https://github.com/mmeyerlein/meclaw/issues/237), and is recorded where
  the sweep that measured them lives.

  **A running colony is not affected.** Instantiation copies the subtree, so an
  instance built before this release keeps its own bytes and its own interior
  addresses; the boundary is checked on `add_edges`, never at boot. What changes
  is what the NEXT instantiation gets, and that a wiring recipe written against
  the old shape is refused rather than silently mis-wired.

- **`canvy` and `access` are behind their boundary, and their interior
  addresses are retired**
  ([#197](https://github.com/mmeyerlein/meclaw/issues/197)). Ruled 2026-08-18:
  an edge is laid at the HIVE, access from outside is abstract and functional,
  and the inner edge that receives the request is what knows what to do with it.
  A port that carries the name of a cell inside satisfies the letter of that and
  misses the point, so both templates were rebuilt on lanes rather than renamed.

  | Template | Was | Now | Why |
  |---|---|---|---|
  | `canvy` | 0.2.0 | **0.3.0** | `render` and `refresh` were two cells, declared as ports |
  | `access` | 1.0.1 | **2.0.0** | `policy` and `invoke` were cells; `store` was the bypass its own README calls its honest limit |

  **The migration is the same edit in both: drop the last segment and name the
  lane.** `./canvy/refresh` becomes `./canvy` with
  `modifier.set_hop.route: "'in_refresh'"`; the drawn page leaves on `surface`.
  `./access/policy` becomes `./access` with `"'in_request'"` and
  `./access/invoke` becomes `./access` with `"'in_invoke'"`; the answers come
  back out of the hive path on `grant`, `ack`, `connect` and `error`. Every lane
  and the sentence saying what it is for are in each template's
  `params.contract`.

  `access`'s two invariants are unchanged and one of them got stronger:
  **R-AC-1 still says the requester comes from the edge**, and that edge now
  addresses the hive — so promoting the caller to `context.requester` is part of
  wiring the broker rather than part of reaching into it. And the third retired
  port is the interesting one: `store` as a declared port was an invitation to
  write a policy row without asking anybody, and since `access@2` a mutation
  that tries is refused with `hive_port_boundary`.

  **A running colony is not affected.** Instantiation copies the subtree, so an
  instance built before this release keeps its own bytes and its own interior
  addresses. What changes is what the NEXT instantiation gets, and that a
  wiring recipe written against the old shape is refused rather than silently
  mis-wired.

  `steward` and `memory-hive` are the other two templates #197 names. Both are
  still on cell-named ports: each needs an edit outside the template files to go
  with the migration, and that is a separate, sanctioned change rather than a
  side effect of this one.

### Added

- **The library table names every template that ships**
  ([#235](https://github.com/mmeyerlein/meclaw/issues/235)). `canvy` was
  exported with the public tree and had no row in `templates/README.md`, which
  came to light only because three numbers were compared by hand. Two gates now
  read the table instead: every row names a template that exists at the version
  it exists at — a version in that column is an exact reference, so a stale one
  does not resolve — and every publicly exported template has a row.

- **The failure lane of a hive is measured rather than reasoned about**
  ([#176](https://github.com/mmeyerlein/meclaw/issues/176)). `hop.finish_reason`
  is a provider's word for why a completion stopped, and a hive that carries it
  across its own boundary makes it part of an interface whose whole purpose is
  that a caller does not know what is behind it. Six tests: the leak shown
  through the real contract check, the fix (`set_hop` on the out-door, the
  caller conditioning on the route) shown on a live colony to deliver **exactly
  one** message to the error sink with nothing looping back into the model cell,
  the negative control that shows the loop is a real shape, and two sweeps over
  the shipped library.

  **And the substrate learned the case it was missing.** The contract check
  probed an exit with the lane on the hop and never applied the door's own
  modifier — so a door that recognises `hop.finish_reason` and PRODUCES the lane
  was invisible to it, and a hive could not declare a lane it demonstrably
  emits. `exit_exists` now also reads the out-door's `set_hop.route`: naming the
  declared lane as a constant on an edge that crosses the hive path is an exit
  for that lane, whatever its condition reads. The refusals the check exists for
  are unchanged and pinned next to the fix — a door that names a **different**
  lane is no exit, a door that names the lane on an edge staying **inside** is
  no exit, and a computed lane name is still not judged at all. The boot warning
  carries the modifier along now for the same reason, so it stops warning about
  hives that got their failure lane right. This is what the six templates in
  [#228](https://github.com/mmeyerlein/meclaw/issues/228) with a
  model-conditioned exit were waiting on.

## [0.14.0] — 2026-08-18

### Breaking

- **Every sealed hive template gets a new major, and an interior address stops
  resolving.** Template ports are one of the four public-contract surfaces
  (README § Stability), and sealing a hive behind `ports: []` retires every
  address inside it. A caller who wired `talky/keeper/stamp`,
  `collector/assemble` or `drain/drain` has a topology the mutation validation
  now refuses with `hive_port_boundary` — not a warning and not a dead letter,
  a rejected `add_edges`. The seal itself and its reasoning are under
  **Changed** below; what follows is the version arithmetic and the migration.

  | Template | Was | Now | Why |
  |---|---|---|---|
  | `affinity` | 1.0.0 | **2.0.0** | `brief`, `gate` and `push` were declared ports and are none |
  | `collector` | 1.2.0 | **2.0.0** | `./assemble` was the address every caller used |
  | `session-keeper` | 1.0.0 | **2.0.0** | `./stamp` and `./close` were the ingress and the sweep |
  | `summarizer` | 1.0.0 | **2.0.0** | `./prep` was the batch address |
  | `memory-drain` | 1.0.0 | **2.0.0** | `./drain` was the batch address |
  | `talky` | 1.2.0 | **2.0.0** | three sub-units renamed, two of them sealed |
  | `cogny` | 1.3.0 | **2.0.0** | `split` renamed, `collector` sealed |
  | `receptionist` | 1.0.0 | 1.1.0 | its own entry is unchanged; its shipped defaults now name hive paths |
  | `canvy` | 0.1.0 | **0.2.0** | not a port change — see below |
  | `access`, `steward` | 1.0.0 | 1.0.1 | `params.ports` respelled, same ports (#196) |
  | `builder-librarian` | 1.0.0 | 1.0.1 | its seed corpus was rebuilt from the sources this release changed |

  **The migration is the same edit everywhere: drop the last segment and name
  the lane.** An interior address carried its meaning in the path; a hive path
  carries it on `hop.route`, and a door edge inside the hive picks the lane up.
  So an edge that used to read

  ```json
  {"from": "./surface", "to": "./talky/keeper/stamp",
   "modifier": {"set_hop": {"route": "'in_turn'"}}}
  ```

  becomes `"to": "./talky/session-keeper"` with the identical modifier. The
  `set_hop.route` was already there in most wirings, because the interior cell
  discriminated on it too — which is why for a great many edges this is a
  one-token change. Where it was **not** there, add it: an edge onto a
  contracted hive whose lane is missing or misspelled is refused with
  `hive_contract` and the hive's own sentence explaining what the lane is for.
  The lanes each template accepts and emits are in its `params.contract`, and
  the four composite ports are in `talky`'s and `cogny`'s `template.json`.

  Address by address: `collector/assemble` → `collector` (lanes `in_turn`,
  `in_calls`, `in_tool`, `in_answer`, `in_bundle`, `in_advice`, `in_close`,
  `in_prune`, `in_batch`, `in_round_sweep`, `in_memory_call`; out on `brain`,
  `recall`, `answer`, `write`, `turn_write`, `prune`) · `keeper/stamp` and
  `keeper/close` → `session-keeper` (`in_turn`, `in_sweep`; out `turn`,
  `close`) · `summary/prep` → `summarizer` (`in_batch`; out `summary`,
  `summary_error`) · `<drain>/drain` → `<drain>` (`in_batch`; out `episode`) ·
  `affinity/brief` and `affinity/gate` → `affinity` (`in_brief`, `in_propose`;
  out `answer`, `ack`, `error`).

  **The renames are a second, independent edit,** and they bite exactly the
  edges the first one does not: an instance carries its template's name, so
  `split` is now `dispatcher`, `keeper` is `session-keeper` and `summary` is
  `summarizer`. Inside `talky` and `cogny` that means the DIRECTORY moved, so a
  parent edge naming `./agent/split` finds no such node at all, which the edge
  endpoint check refuses before the boundary check ever looks at it.
  `dispatcher` is the one sub-unit that stayed addressable — it declares no
  ports and is a single cell — so `./agent/split` → `./agent/dispatcher` is the
  whole change there, and the tool lanes keep their conditions verbatim.

  **A running colony is not affected and does not need to be migrated.**
  Instantiation copies the subtree, so an instance built before this release
  holds its own bytes, keeps its own interior addresses and keeps working —
  including the topologies that reach into it. What changes is what you get the
  NEXT time you instantiate, and a wiring recipe written against the old shape
  will be refused rather than silently mis-wired. That refusal is the point: the
  same recipe applied to `memory-drain` before the seal delivered every episode
  twice, because the door and the interior cell were both reachable and both
  fired.

  **`canvy@0.2.0` is in this list for a different reason.** Its ports are
  unchanged and nothing about addressing it moved; what changed is the shape of
  a row in its own `cell.db`. A hive's position is stored as the shift it was
  dragged by rather than as a box origin (`kind` `hive` → `hive_shift`, #170),
  because both rectangles the origin could be measured against move on their own
  when the colony grows. A row in the old shape is read once through the layout
  it was written against and rewritten, so a hand-made arrangement comes back
  exactly as it was left — the bump says the stored shape changed, not that a
  caller has anything to do.

- **A cell that is an ingress has to say so
  ([#185](https://github.com/mmeyerlein/meclaw/issues/185)).** The `config.json`
  schema is one of the four public-contract surfaces (README § Stability), and
  it gains one optional block that is not optional for an ingress:

  ```json
  "contract": { "ingress": { "context": ["chat_id"] } }
  ```

  Read as: *messages are born at this cell, carrying these `context` keys.* The
  keys must come from the standard header convention — `turn_id`, `session_id`,
  `user_id`, `chat_id`, `locale`; a claim outside that set is refused by name.

  **What it replaces.** The build-time reachability check used to decide who the
  graph entry was by counting incoming edges: a node with none was treated as
  the entry and handed the whole standard header set. In-degree is not a
  property of a cell. The ordinary connector — a proxy that accepts inbound
  traffic and gets the answers routed back to it — has an incoming edge and lost
  the branch, so a correctly wired ingress could be refused for being wired the
  way ingress cells are wired; and an unconnected island gained it for free.
  Worse, the answer changed when an unrelated edge was added somewhere else.

  **The migration.** Add the block to the cell where messages enter — in the
  shipped templates that is the `proxy` cell of a chat topology, and in a
  hand-built colony it is whichever cell the HTTP ingress or the stdio bridge
  posts to. Nothing else changes: an edge `modifier.set_context` promotes keys
  exactly as before, and a topology whose keys all come from promotions needs no
  edit at all. Two ways to find out whether yours does:

  - a **first boot** refuses, naming the node, the key and the field to add
    (`… requires consumes.context 'chat_id' but context presence not reachable
    from any setter — no edge promotes it and no cell on the way declares
    contract.ingress.context 'chat_id'`);
  - a **running colony reboots and reports** — the finding is a `warn!` per
    offending node and the colony comes up (#178). `meclaw --validate
    --validate-strict` turns the same finding into a non-zero exit, which is
    where to look before the restart rather than after it.

### Added

- **`llm-unit@1.1.0` files its own failures, and the lane is unit-private**
  ([#218](https://github.com/mmeyerlein/meclaw/issues/218)). The unit's
  `state` store declared an `errors(id, kind, payload)` table that nothing could
  write. #214 established why: a `store` accepts only a `tool_call` turn carrying
  a store-native op, an edge modifier rewrites headers and never a body, and the
  `llm`'s error emission is a plain report — so no edge can perform the
  translation, and the table stayed in the schema as the thing a writer would
  target. A sixth cell, `scribe`, is that writer: two internal edges,
  `llm -(finish_reason == 'error')-> scribe -(route == 'estore')-> state`.

  | column | value |
  |---|---|
  | `id` | `<trace_id>:<turn_id>:<iter>` — the message chain, and where in the tool loop it happened |
  | `kind` | `hop.error_code`, the closed spec enum, so the column groups |
  | `payload` | JSON: `detail`, `source`, `turn_id`, `iter`, `model`, `latency_ms` |

  **The lane is unit-private: the unit records and forwards nothing**, which was
  the open decision in the issue. A parent already has its lane and it is still
  mandatory — `./llm finish_reason == 'error'` is a documented exit port, and
  edge evaluation fans out, so `scribe` and the parent's drain both fire on one
  emission. A second forward-out port would duplicate a working one. And a parent
  could not read the table in any case: `state` is unit-private and database
  isolation has no exception (#160), so "records AND forwards" would be a forward
  standing next to a write nothing outside can observe.

  Nothing changes for a caller: the same drain edge, the same ports, a MINOR bump
  because a cell and two edges are additive. What changes is what an operator
  finds afterwards — the unit can now answer "what went wrong in here" by itself.
  One asymmetry is named in the README rather than left to be discovered: before
  1.1.0 an unwired error lane fell back to `reply_to` into `prep`'s echo guard,
  and now the internal edge matches instead. Either way the task never answers,
  so the drain stays mandatory and the row is a forensic record, not a
  notification.

- **`hop` can be seeded at both ingresses**
  ([#175](https://github.com/mmeyerlein/meclaw/issues/175),
  [#180](https://github.com/mmeyerlein/meclaw/issues/180)). `POST /messages` and
  the stdio JSON wire take an optional `hop` sibling of `body`/`headers`:

  ```json
  {"target": "/talky", "headers": {"session_id": "s1"},
   "hop": {"route": "in_turn"}, "body": {"messages": [...]}}
  ```

  Both ingresses put every inbound header into `context` and started `hop` empty,
  which is right for a source message and made it impossible to address a HIVE.
  Since the boundary rule a hive distributes on `hop.route`, so a message posted
  at a hive path matched no door and dead-lettered as `hive_no_route` whatever the
  caller wrote. Verifying a freshly sealed hive meant driving a full turn through
  some interior cell — breaking the very rule the door exists for.

  Deliberately NOT "headers go to hop": the two-compartment model is right, and
  what was missing is a way to say which compartment you mean. A seeded hop
  reaches exactly `Headers.hop` and nothing else — the same surface a modifier's
  `set_hop` reaches. The one thing a modifier can do that a compartment map
  cannot is `restore_ttl`, and that is a modifier FIELD rather than a hop key, so
  it is not expressible here at all. The envelope stays out of reach by
  construction rather than by a blocklist.

- **A hive declares its interface, and the declaration is checked**
  ([#173](https://github.com/mmeyerlein/meclaw/issues/173)).

  ```json
  "params": {
    "ports": [],
    "contract": {
      "accepts": [{"route": "in_batch", "context": ["session_id"],
                   "because": "one closed session as a single write batch"}],
      "emits":   [{"route": "episode", "because": "one message per turn"}]
    }
  }
  ```

  A template is supposed to be a class: instantiate it, wire to its interface,
  swap it later for another implementation with a different inside. For hive
  templates none of that held. `contract` was a CELL property; a hive had
  `description` prose, and the prose named cells three levels down ("Ingress:
  `./keeper/stamp`"). So the one unit a person actually instantiates was the one
  unit with nothing machine-readable to check an instantiation against, and every
  caller wrote the template's internal layout into its own topology.

  `params.contract` states the interface in the only vocabulary that survives a
  reimplementation: LANES — the `hop.route` values the hive accepts at its own
  path and the ones it emits back out of it. No cell of the hive appears in it,
  which is what makes the inside free to change.

  Three checks, all on the mutation path, all run through the real router rather
  than by comparing condition strings (the shipped templates open a whole family
  of lanes with one `startsWith('in_')`, which no text comparison can read):

  1. an edge onto the hive path whose `set_hop.route` is a constant must name an
     accepted lane — the typo is refused instead of becoming a runtime dead
     letter that reads like a model failure;
  2. every accepted lane must have a door (`{"from": "."}` inward);
  3. every emitted lane must lead back out through the hive path.

  (2) and (3) are what keep the declaration from decaying into decoration:
  rearranging the inside is free, rearranging it so a promised lane loses its
  door is not. Rejections carry the hive's own `because` sentence and the token
  `hive_contract`.

  **Opt-in and backwards-compatible.** A hive without `params.contract` behaves
  exactly as before, which is the state of every hive instantiated so far — a
  live colony picks the contract up when it next instantiates from a template.
  Boot only warns; the birth `params.graph` stays the author's sovereign design,
  the same rule as GH #133 and GH #147. A hive whose path carries no edge at all
  is dormant, not broken, so a contracted hive can still be taken out with
  `remove_nodes`.

  Shipped with a contract: the templates already sealed behind their boundary —
  `collector`, `session-keeper`, `summarizer`, `memory-drain`, `affinity`, and
  their byte-identical copies inside `talky` and `cogny`.

- **`move_nodes`: a cell can change its address without losing what it is**
  ([#169](https://github.com/mmeyerlein/meclaw/issues/169)).

  ```json
  {"move_nodes": [{"match": {"name": "fetch"}, "to": "talky/fetch"}]}
  ```

  A path is a cell's identity, so until now there was no operation that changed
  one. Tidying a capability into the hive it belongs to meant `add_nodes` at the
  new path, `add_edges` for every edge the old node had, `remove_nodes` on the
  old one and an operator wipe outside the mutation flow — in practice two
  mutations. The cost was not the typing: the new node got a new `cell_id`, a new
  `instantiated_at` and a fresh empty `cell.db` while the old one sat orphaned
  beside it, every condition and modifier was re-entered by hand at the new
  address, and between the two mutations the lane was either wired twice (the
  call fans out and runs twice) or not at all (it dead-letters).

  One committed mutation now does all of it. The directory is moved with
  `rename(2)`, so `config.json`, `cell.id` and `cell.db` travel as one inode; the
  registry row is re-addressed by an UPDATE, so `cell_id`, `created_at` and
  `instantiated_at` survive; and every edge naming the old path names the new one
  afterwards, condition and modifier carried verbatim.

  It is deliberately not `swap_nodes`, which is the closest existing operation
  and the opposite intent: a swap swings edges onto a **different**
  implementation with its own identity and its own `cell.db`. A move keeps the
  cell and changes where it lives.

  Refused rather than half-done: a target outside the mutation scope, a target
  that is not free (registry, hive scopes, filesystem), a target whose parent
  directory does not exist yet, and — for this first version — a **hive** or any
  node with something beneath it. Moving a hive means moving every child's
  registry row, every subtree-internal edge and the hive scope itself, and a
  half-moved hive leaves its children addressed under a path that no longer
  exists, which is the boot failure
  [#168](https://github.com/mmeyerlein/meclaw/issues/168) is about.

  A move does **not** rewrite the parent hive's `params.graph`, and does not need
  to: since #168 the persisted edge table is the boot topology on a reboot, and
  the file is seed rather than state. It also does not touch the canvas store's
  node positions — those live in another cell's own `cell.db`, which § Database
  isolation puts out of the colony's reach. The canvas re-places a node it does
  not recognise.

  The **no-delete policy** now says what it always meant: nothing in `{root}` is
  deleted, and a relocation is not a deletion — the file is not lost, it is
  elsewhere. Paths are stable until somebody changes one on purpose, and the only
  way to do that is a named, validated, atomically committed mutation.

### Changed

- **The hive boundary is documented as a rule, not as a mechanism** (#227). The
  rule was already in `docs/meclaw-overview.md`, the whole colony was migrated
  onto it in one day — and that same day produced four separate defects (#197,
  #200, #203, and ten templates sealed only retroactively). Each one is somebody
  reading a description of how the substrate behaves and not concluding that it
  binds them. § The hive boundary is therefore rewritten rather than extended:
  the rule and its scope ("all hives and all templates") stand first, followed by
  three numbered requirements — the address is the hive; **a lane is named
  functionally**, for what the caller wants and never for where it lands inside;
  **the inner edge is the only place structure may be known**. The last two are
  the halves that `ports: []` alone never said, and they are what the #197
  migration had to add after the fact. New with them: what a template author
  concretely has to do (`ports: []`, `{"from": "."}` doors, a `params.contract`,
  no address in the prose the boundary would refuse), a worked counter-example of
  an edge that addresses the hive correctly and still names a cell in its lane,
  and an honest three-stage table of where the shipped library actually stands.
  `docs/config.md` § `params.contract` now opens with the same requirement and
  carries the structural-to-functional lane renaming table; `templates/README.md`
  states it where a template author actually reads, including why the canonical
  `talky` wiring example is the legacy shape. `docs/rewiring.md` keeps the
  procedure and the two now point at each other. Both language versions.

- **Every shipped hive template stands behind its own boundary.** An edge
  crossing a hive's boundary names the hive, and the hive distributes internally
  with `{"from": "."}` edges — the rule was documented and the templates did not
  follow it. Callers wired straight at `talky/keeper/stamp`, `collector/assemble`,
  `drain/drain`: three levels into somebody else's arrangement. Sealed with
  `ports: []`, door edges and a stated contract: `summarizer`, `memory-drain`,
  `affinity`, `session-keeper`, `collector`, `cogny`, `talky`, `dispatcher`,
  `receptionist`, `canvy`.

  **A seal is a change to every caller**, including the ones that ship: sealing
  `memory-drain` broke four test colonies and two shipped examples, all of which
  addressed `./drain/drain` — reaching past the door delivered each episode twice
  and dead-lettered the second. And some folds are rewrites rather than
  repointings: three sibling error exits that each named an interior cell become
  ONE edge after folding, which then fires three times.

  Renamed, per the rule that an instance carries its template's name: `split` →
  `dispatcher`, `keeper` → `session-keeper`, `summary` → `summarizer`.

  New: `docs/rewiring.md` (and `.en`) — the four places a topology lives, why
  innermost-first, why an in-door is never a catch-all, and the order that ends
  with "start, wait for an answer, and roll everything back if it does not come".

- **The overview no longer claims a mutation is recognised by `hop.msg_type`**
  ([#181](https://github.com/mmeyerlein/meclaw/issues/181)). Nothing reads that
  key; dispatch is by target path alone. The correction is not a deletion,
  though: `hop.msg_type == "mutation"` is a widespread APPLICATION convention —
  `receptionist/greet`, `steward/mutator` and `builder-hive/deploy` all set it and
  condition their mutation edge on it. It is now described as what it is, which
  also makes the paragraph agree with the document's own insistence elsewhere
  that `msg_type` is an application convention core knows nothing about.

- **The stdio JSON wire rejects a non-object `context` with `invalid_frame`**
  ([#182](https://github.com/mmeyerlein/meclaw/issues/182)). A string, a number or
  an array in that field used to be coerced to `{}` and the frame ran anyway —
  taking `turn_id` with it, which is the key the sender correlates the reply on.
  The message was processed, the answer came back, and the sender could not tell
  whose answer it was; nothing along the way said the frame was wrong. It is now
  refused at the ingress, the one place that still knows what the caller wrote,
  with the field named. This is the same call `POST /messages` made for `headers`
  in 0.9.0, and the same one `hop` already got on both ingresses
  ([#175](https://github.com/mmeyerlein/meclaw/issues/175),
  [#180](https://github.com/mmeyerlein/meclaw/issues/180)).

  **It is a behaviour change on an existing field and deliberately not a wire v2.**
  What wire v1 freezes is the frame shape a reader must be able to parse and the
  rule that `v` rises only together with a negotiation step; inbound strictness is
  neither. A sender whose `context` is well-formed — or absent, or `null` — is
  unaffected. A sender that ships a malformed `context` today starts receiving an
  `error` frame instead of silence: it was already losing the compartment, it just
  had no way to find out.

- **The stdio JSON wire rejects a malformed `ttl` with `invalid_frame`**
  ([#187](https://github.com/mmeyerlein/meclaw/issues/187)). A `"ttl": "12"`, a
  `-1` or a `3.5` used to read as "no ttl at all": the frame was accepted and the
  message ran on the colony default. `ttl` is the hop budget, so getting it
  silently wrong does not fail here — it fails as a message that stops somewhere
  mid-lane on a budget nobody asked for, which is among the harder things to
  trace back to a typo in a frame. A `ttl` above `u32::MAX` was worse still: the
  cast wrapped, so the message ran on an unrelated small number that was neither
  the value sent nor the default. The stdio ingress now accepts a positive
  integer in `1..=4294967295` and names the field on anything else — the same
  answer `POST /messages` has given as `422 invalid_ttl` since 0.9.0. This closes
  the last envelope asymmetry between the two ingresses, after `hop`
  ([#175](https://github.com/mmeyerlein/meclaw/issues/175),
  [#180](https://github.com/mmeyerlein/meclaw/issues/180)) and `context`
  ([#182](https://github.com/mmeyerlein/meclaw/issues/182)).

  **Behaviour change on an existing field, and deliberately not a wire v2**, on
  the same reasoning as #182: what wire v1 freezes is the frame shape and the
  negotiation step, not the strictness of inbound validation. A sender whose
  `ttl` is a legal budget — or absent, or `null` — is unaffected and still falls
  back to the substrate default. A sender that ships a malformed one starts
  receiving an `error` frame instead of a message running on a budget it did not
  choose.

- **The stdio JSON wire rejects a `trace_id` of the wrong JSON type with
  `invalid_frame`** ([#190](https://github.com/mmeyerlein/meclaw/issues/190)).
  A `"trace_id": "nope"` was already refused, but a `12345`, an object, an array
  or a `true` read as "no trace_id at all": the frame was accepted and a fresh
  trace id was minted for it. One mistake, two answers, three lines apart — and
  the silent half is the one that costs the most, because a trace is the only
  thing tying a conversation together across the process boundary. The message
  runs and answers as if nothing happened; what is gone is the sender's ability
  to recognise the answer as belonging to its own trace, and nowhere downstream
  is there anything left pointing at the frame that dropped it. The stdio
  ingress now accepts a UUID string and names the field on anything else. There
  is no HTTP counterpart to align with here — `MessageRequest` carries no
  `trace_id` — so this closes the asymmetry inside the parser rather than one
  between the two ingresses, and with it the last inbound field on the stdio
  frame that could degrade in silence, after `hop`
  ([#180](https://github.com/mmeyerlein/meclaw/issues/180)), `context`
  ([#182](https://github.com/mmeyerlein/meclaw/issues/182)) and `ttl`
  ([#187](https://github.com/mmeyerlein/meclaw/issues/187)).

  **Behaviour change on an existing field, and deliberately not a wire v2**, on
  the same reasoning as #182 and #187: what wire v1 freezes is the frame shape
  and the negotiation step, not the strictness of inbound validation. A sender
  whose `trace_id` is a UUID string — or absent, or `null` — is unaffected and
  still gets a trace minted for it when it says nothing. A sender that ships a
  wrong-typed one starts receiving an `error` frame instead of an answer under a
  trace it never wrote.

### Fixed

- **The heartbeat watchdog could not tell a wedged colony from a busy host, and
  the trip was fatal**
  ([#165](https://github.com/mmeyerlein/meclaw/issues/165)). A compile on the
  same box killed a healthy colony three times in one day: `on_trip=exit` is the
  default, so each 500 ms of silence ended the process, and each restart came
  back clean. The trip record even said `supervisor_lag=0ms`, which reads as
  "the process was getting CPU, so this is the colony's fault" — and is not,
  because the supervisor is a `sleep`-driven task with no work to do, and a
  starved runtime wakes a timer roughly on schedule while the loop that is
  actually working does not get through its work item.

  Two signals now stand where that inference stood, and neither is a bigger
  window (a bigger window only moves the point at which the same fallacy is
  committed). First, the **heartbeat carries a phase**: the colony loop declares
  `Working` or `Parked` *before* it can block, because a blocked loop reports
  nothing and the report therefore has to happen on the way in. Everything
  between a `Working` and the next `Parked` is one work item, so the supervisor
  asks how long the loop has been on ONE item instead of only how long it has
  been quiet — and a declared item gets its own, larger budget
  (`WORK_ITEM_BUDGET_FACTOR = 10` × the window, 5 s by default) while the idle
  bar for a parked loop stays exactly 500 ms. Second, a **witness task** runs
  next to the supervisor and must finish a unit of real work each period; it is
  judged by the identical rule the colony is judged by, so "was the runtime able
  to get anything done" stops being a guess. A work item that outlives even its
  budget is fatal again: the budget bounds the suppression, it does not remove
  it.

  `watchdog_on_trip: exit` stays the default, and it stays the right default —
  what changed is that `exit` now fires only on a finding that implicates the
  colony (`starved=colony_loop` or `stuck_work_item`) or proves the task is gone
  (`colony_task_gone`). `host_runtime`, `slow_work_item` and `process_scheduling`
  are reported loudly and the colony keeps running. Turning the default into
  `log-only` would have switched off the response instead of repairing the
  inference. The trip line gains `in_flight_work`, `work_item_budget`, `witness`
  and `witness_missed`; its issue-#6 prefix is unchanged.

- **`hive_scopes` has no delete path, and nothing said whether that was
  deliberate** ([#192](https://github.com/mmeyerlein/meclaw/issues/192)). Ruled
  intended, no code change; what was owed was the sentence. `docs/config.md` §
  "Snapshot vs. Live-Read" and its English twin now state it where the table is
  described: a hive has no registry row, so `hive_scopes` is the second list that
  tells the colony an address holds a transit rather than something it can
  deliver to; `remove_nodes` is disconnect-instead-of-delete everywhere, so a
  scope row outliving its hive is the no-delete policy applied consistently
  rather than an omission in one table; and it is load-bearing, because #186 made
  the table the boot authority for which nodes are hives *because* it is
  append-only — a hive whose directory was wiped is still read as a transit
  instead of as a contract-less cell. The one place it becomes a real decision is
  a hive relocation, which `move_nodes` refuses by name today, and the policy
  already frames the answer: the scope row moves with the hive.

- **`examples/never-forgets` forked the template library to set one param**
  ([#220](https://github.com/mmeyerlein/meclaw/issues/220)). The flagship
  walkthrough's setup copied the whole library and edited
  `talky/collector/assemble/config.json` so that `turn_write` was on. Since #140
  that is one key in the declaration, and `grow.json` now carries
  `"override_params": {"collector/assemble": {"turn_write": "1"}}` on the `talky`
  node — the file the reader POSTs is where the setting lives, instead of a
  `python3` heredoc two steps earlier that nothing downstream mentions again.

  **The library copy stays, and the prose now says what it is actually for.**
  Step 2 writes the brain's `seed/system.jsonl`, and a seed is a *file* read once
  at spawn — no `override_params` reaches a file, so the reader still needs a
  library he may write into. What went away is the second reason, the one that
  had stopped being true.

  `crates/meclaw-cells/tests/never_forgets_example.rs` stops patching the copied
  config and applies the shipped `grow.json` verbatim, which is now the whole
  setting. A new `grow_json_sets_the_per_turn_lane_at_instantiation` pins the
  key **and** its path: `collector` alone is the sub-unit's hive and would take
  the override in silence (#212), which is the failure mode #203 already shipped
  once. Removing the override turns two tests red — the pin, and the freshness
  assertion that finds today's turn in the episode table.

  While in the file: its "Pinned" paragraph still said the test does not need
  the step-2 seed because the mock answers with a canned tool call. That has been
  false since #142 — the test runs the step and then asserts `memory_recall` was
  on the wire, which is the only thing that says a live run would have worked.

- **The tool-lane adapter pattern argued from a reject that no longer exists**
  ([#219](https://github.com/mmeyerlein/meclaw/issues/219)).
  `workshop/cookbook/tool-lane-rewrap-adapter.md` opened with "## The root cause
  (R10)" and taught the adapter as a workaround for `override_params` being
  refused on a subtree template. #140 removed that reject, so the entry was
  teaching a workaround by citing a constraint the validator contradicts.

  **The pattern stays, and its rationale is now the one the substrate actually
  gives.** `llm-unit/dispatch` emits the model's `arguments` as the body
  verbatim, and an edge modifier writes `set_context` / `delete_context` /
  `set_hop` / `delete_hop` / `restore_ttl` and nothing else — no modifier can
  touch `messages[]`. So when a non-`code` builtin consumes its own arg shape
  (`mcp` wants `{name, arguments}`, `edit` wants `{op, path, find, replace}`),
  only a cell between the two can rewrite the body. The second half is
  sharper still: `edit`'s `op` and `path` are fixed by the adapter *because* a
  tool schema describes what the model may write and has no construct for a
  value the topology sets. `override_params['prep']` re-authors what is
  **offered**; it cannot supply an argument that was never sent.

  What #140 does remove is written down as such: the "you only ever get
  `web_search`" premise, and with it the 1→N splitter form. Declare one tool per
  lane in `prep`, and the dispatcher's `hop.tool_name` fans them by edge
  condition with no cell in the middle — so `corpus/15-multi-send-grenze` is no
  longer listed as a proof of this pattern, only as the multi-send and fan-in
  receipt it also is. The two remaining proofs are flagged as pre-#140 trees:
  their `hop.tool_name == 'web_search'` condition and `editadapter`'s own R10
  comment are historical, the body translation in them is not.

- **The example hop chains named a hive where the trace names a hive AND the cell
  behind it** ([#210](https://github.com/mmeyerlein/meclaw/issues/210)). The
  boundary-seal rewrite turned `/talky/collector/assemble` into `/talky/collector`
  everywhere, including in the trace excerpts of `examples/never-forgets` and
  `examples/meclaw-os`, on the unstated assumption that a hive either replaces the
  cell in a trace or is invisible in one. Measured against a running colony, it is
  neither: a hive gets its own `message_log` row when a message arrives at it and
  a second one as the sender of what it forwards, so a transit reads hive, cell,
  hive. Both chains were replaced with the real ones, both READMEs and the
  never-forgets walkthrough now say plainly that a hive transit is a visible hop —
  and that a hive nobody addresses (`/memory`, `/firewall`) never appears at all.
  Pinned by `gh210_a_hive_transit_is_its_own_hop`.

- **Two build products and every shipped version are now gated**
  ([#217](https://github.com/mmeyerlein/meclaw/issues/217),
  [#221](https://github.com/mmeyerlein/meclaw/issues/221)). `builder-hive` is
  generated, and its generator had drifted out of the tree it generates in the
  worst possible direction: `main()` began with `rmtree`, which eats the
  hand-written `README.md` and `LIFT.md`, so the safe move was never to run it —
  and the drift therefore grew by construction. Its `--check` reports MISSING,
  STRAY and CHANGED per file, and STRAY carries its own repair line, because a
  leftover file survives a regeneration untouched and "regenerate" would be
  advice that visibly does nothing.

  Separately, nothing checked that a shipped `template.json` carries a version a
  builder can name. `"version": 1` is not a parse error — the reader takes
  `as_str()`, so a number becomes `None` and the template is simply unreachable
  by `name@version` while sitting on disk. That shipped once already. All 32
  descriptors are now asked four questions, each answered by the substrate rather
  than by the test: it parses, a version reaches the row, the resolver can read
  it, and the registry resolves `name@version` back to the directory it came from
  — the last one also catching two directories claiming one identity.

- **The docs stopped naming template major lines that had moved**
  ([#206](https://github.com/mmeyerlein/meclaw/issues/206),
  [#222](https://github.com/mmeyerlein/meclaw/issues/222)). Roughly 50 sites said
  `talky@1`, `cogny@1`, `affinity@1` in the present tense after those lines went
  to 2. Rewritten to the bare name, which cannot go stale again. Historical
  statements were left standing — "since `collector@1.2.0`" is a record of when
  something happened and is still true — as were the `@1`s that are still
  accurate, so several mixed lists are half-rewritten on purpose.

- **A surface render could wait out its whole budget for an answer that had
  already arrived** ([#223](https://github.com/mmeyerlein/meclaw/issues/223)).
  The render dispatcher learns of a waiting request and of the cell's reply
  through two different channels, and `tokio::select!` gives no order between
  them. `render` registered its waiter and injected the request in one breath,
  without waiting for the registration to land — so a reply that was already
  queued when the dispatcher task next ran could be served *before* the
  registration it belonged to, be dropped as "nobody waiting", and leave the
  render that earned it sitting out its full budget before reporting a timeout
  for a page the cell had drawn correctly. `render` now waits for the
  registration to be acknowledged before anything is injected: a reply cannot
  exist before its waiter does.

  It was found as a flaky test — `crates/meclaw-api/tests/gh159_surface_render.rs`
  failed on roughly a quarter of loaded runs, with a different victim each time
  — but the defect was in the dispatcher, not in the test. One dispatcher serves
  every surface request in a colony, and a colony under concurrent joins is
  exactly the load that opens the window; on the socket path the cost was a 15 s
  dead join.

- **The `builder-hive` template is generated or the build is red**
  ([#217](https://github.com/mmeyerlein/meclaw/issues/217)).
  `templates/builder-hive/` is a build product of
  `workshop/tools/build_builder_hive.py` — the cell scripts live there as real
  python because an inline script buried in JSON is unreviewable — and nothing
  compared the product against its sources, so "generated" said nothing about
  "current". It had already gone wrong twice, both found by hand while fixing
  #215: `main()` did `shutil.rmtree(OUT)`, which deletes the hand-written
  `README.md` and `LIFT.md`, and the shipped `config.json` carried `has(hop.*)`
  guards the generator did not, so a regeneration reverted them silently.

  Worse than the librarian corpus #205 closed, because the two reinforce each
  other: once running the generator destroys hand-written prose, the safe move is
  to not run it — which is exactly what lets the other drift grow.

  `--check` regenerates into a temp directory and diffs the two TREES, naming the
  path for each of MISSING, STRAY (a cell renamed in `CELLS` leaves its old
  directory behind, and that one is not repaired by regenerating) and CHANGED.
  `README.md` and `LIFT.md` are named once as non-products and kept out of both
  the write path and the comparison — the two exclusions fail in opposite
  directions, one eats the files and the other reports them as permanent drift.
  Wired into `cargo test` and, unguarded, into CI's `gates` job, so a runner
  without a `python3` cannot make the gate disappear.

- **The prose stopped configuring colonies that come up unconfigured, and
  stopped naming template versions the library no longer has**
  ([#212](https://github.com/mmeyerlein/meclaw/issues/212),
  [#213](https://github.com/mmeyerlein/meclaw/issues/213),
  [#206](https://github.com/mmeyerlein/meclaw/issues/206),
  [#211](https://github.com/mmeyerlein/meclaw/issues/211),
  [#216](https://github.com/mmeyerlein/meclaw/issues/216)). Five documentation
  defects from the 0.14.0 pass, one of which misconfigured a colony silently.

  `templates/cogny/README.md`'s instantiation recipe set the collector's knobs
  with `"override_params": {"collector": {…}}`. Those are params of
  `collector/assemble`, and `collector` is the sealed sub-unit's **hive** — a
  cell that reads only `graph`, `ports`, `required_drains` and `contract`. Since
  #140 an `override_params` key is a cell path inside the template, and a hive
  path is a valid one: the validator accepts it, nothing consumes the params,
  and the core comes up as if the override had never been written. Same class as
  #203 — a documented recipe that runs and does nothing — and the same class as
  R10's original complaint, arriving through a door #140 could not have closed
  because it predates the seals. Every `override_params` in `templates/**`,
  `examples/**` and `docs/**` was resolved against the template its own recipe
  names; this was the only inert one, and
  `crates/meclaw-cells/tests/gh212_documented_override_params.rs` now asks the
  substrate (`parse_subtree` for which cells are hives, `HiveParams` for whether
  the hive reads what is set) rather than a list kept beside the check.

  Six places still described `override_params` as **rejected** on a subtree
  template, citing R10 — a claim #140 superseded and `validate.rs` contradicts in
  as many words. A reader of any of them concludes they must fork a template to
  parameterise it, which is exactly the workaround #140 removed. Corrected in
  `templates/collector/README.md`, `templates/llm-unit/{template.json,
  prep/config.json}`, both `examples/never-forgets` documents and two test doc
  comments. `templates/collector/README.md`'s params example was headed
  `…/collector/config.json`, the hive again, and now names
  `…/collector/assemble/config.json`.

  Twenty-three sites claimed in the present tense that the library offers
  `talky@1`, `collector@1` or `memory-drain@1`; all five templates went to
  `2.0.0` in this release. Rewritten to the bare name, which resolves to the
  highest available version and cannot go stale again. Historical forms ("since
  `collector@1.2.0`") are records and stay. One of the twenty-three was not
  shorthand but an instruction: `workshop/AGENTS.md` told builders to
  instantiate `llm-unit@1`, and semver ranges are not parsed, so that string in
  a mutation comes back `TemplateMissing`.

  Also: five knob rows in `templates/talky/README.md` § Knobs carried three
  cells in a four-column table, so every value rendered one column left and the
  `where` column — the one saying whether a knob is a colony-global `.env` line
  or a per-instance param — was empty. And
  `crates/meclaw-cells/tests/slack_template_smoke.rs` described `llm-unit` as
  carrying a `"version": 1` defect that `1b99f284` fixed; the failure class it
  illustrates is real and is now written as the record it is.

  `templates/README.md`'s library table gained the `llm-unit` row it was missing
  ([#209](https://github.com/mmeyerlein/meclaw/issues/209)).

- **The shipped templates stopped documenting addresses the boundary refuses**
  ([#203](https://github.com/mmeyerlein/meclaw/issues/203)). The sub-unit renames
  were applied mechanically — `keeper` → `session-keeper` — which replaced the
  leading segment and kept the trailing one, and the trailing one is exactly what
  the seal retired. So `talky/README.md` announced "**these four addresses are the
  port contract**" and listed `./session-keeper/stamp`, which validates nowhere.
  27 occurrences across 16 files, including five the report had not found.

  The worst was not prose: `examples/never-forgets/WALKTHROUGH.md` carries a
  runnable command that patches `templates/talky/collector/config.json`, and
  `turn_write` is a param of `assemble` while the sealed collector hive reads only
  `graph`/`ports`/`contract`. Following the walkthrough wrote a key nothing reads
  and brought the example up with **no per-turn writes at all**, silently.

  The talky and cogny diagrams are redrawn rather than patched — cogny's two were
  half-renamed, `split` in one and `dispatcher` in the next — and now say which
  nodes are sealed hives and that the lane column is what the door behind each one
  reads. Two checks keep it honest: `gh203_documented_port_addresses.rs` pushes
  every literal `from:`/`to:` in a template's prose through the REAL port boundary,
  and `gh204_declared_defaults_match_the_inline.rs` compares every
  `contract.settings.<x>.default` against the `${VAR:-default}` the colony
  actually resolves (69 defaults).

- **`receptionist` gives one answer for where its ingress is**
  ([#204](https://github.com/mmeyerlein/meclaw/issues/204)). The knob had three
  values: the inline default said `session-keeper` (right), the machine-readable
  `contract.settings.ingress.default` said `session-keeper/stamp` (a sealed
  address), and the README said `session- session-keeper` — a rename substitution
  that had run over its own output. The declared default is the half tooling
  reads, so it was the one that mattered most.

- **The librarian corpus is generated or the build is red**
  ([#205](https://github.com/mmeyerlein/meclaw/issues/205),
  [#207](https://github.com/mmeyerlein/meclaw/issues/207),
  [#208](https://github.com/mmeyerlein/meclaw/issues/208)). `docs.jsonl` describes
  the template library and had drifted 289 lines — three stale versions, eight
  templates missing entirely — with nothing to notice. It is now pinned by a
  regeneration diff in CI *and* in the test suite, deliberately in both: the Rust
  test skips where `python3` is absent, and a gate that can silently vanish is not
  one. A source that is present and unparseable is now a hard failure naming the
  file and the parse error, rather than a row that quietly disappears — and the
  test relays that reason instead of advising a regeneration that cannot help.

- **Three templates ship with the README the library says they have**
  ([#209](https://github.com/mmeyerlein/meclaw/issues/209)). `builder-librarian`,
  `builder-hive` and `llm-unit` had none, while `templates/README.md` defines a
  template as "a directory, a README and a `template.json`".

- **The builder-librarian's seed corpus is generated or the build is red**
  ([#205](https://github.com/mmeyerlein/meclaw/issues/205)).
  `templates/builder-librarian/store/seed/docs.jsonl` is a build product of
  `workshop/tools/build_librarian_seed.py`, chunked out of the spec, the
  cookbook, the corpus briefs, the template catalogue and the pinned error
  codes. It is also a committed file, and nothing compared the two, so it was
  free to describe a tree that had moved on. It did: 289 lines stale, carrying
  `memory-hive@1.2.0`, `collector@1.2.0` and `talky@1.2.0` after all three had
  been rev'd, with eight templates missing outright. It was rebuilt in this
  release, which fixes today and nothing after it.

  A stale corpus here is worse than no corpus. The librarian exists to answer
  "what templates are there and what do they do", and BM25 scores a stale
  answer exactly as highly as a true one, so no reader downstream can tell
  them apart — the retrieval is confident and wrong, which is the shape of
  error that survives review.

  The generator grew a `--check` mode in the shape of `scripts/canvy_sync.py`:
  regenerate into a temp file, byte-compare against the committed one, exit
  non-zero on any difference with the regeneration command in the message. It
  runs from two places — `crates/meclaw-cells/tests/librarian_seed_corpus.rs`
  so `cargo test` catches it, and the `gates` job in CI so it is still caught
  where the test skips for want of a `python3`. A source named literally
  (`docs/cell-types.md` and the three like it) that goes missing is now a hard
  error rather than a quietly smaller corpus, which was the same silence one
  level down.

- **The boot topology is the edge table, not the hive's `config.json`**
  ([#168](https://github.com/mmeyerlein/meclaw/issues/168),
  [#178](https://github.com/mmeyerlein/meclaw/issues/178)). Two reports, one
  question: which edge set is the authority when the colony boots. On a **Reboot**
  the persisted `colony.db` edge table is; on a **FirstBoot** the `config.json`
  files are. That was already the rule everywhere else — `colony_task` has always
  hydrated from `colony.db` and logged "params.graph hints ignored" — and the
  bootstrap PLANNER was the only reader still believing the file.

  What it cost before: `remove_nodes` took edges out of the table while the hive's
  `config.json` kept declaring them, so removing the directory too sent the colony
  into a systemd crash loop on `DanglingEndpoint`. And the header-contract check
  ran on the `config.json` view alone, so **declaring a hive's doors could turn a
  colony that had run for days into one that would not boot**: the new incoming
  edge stopped the node being a graph entry, while the setter that really promotes
  the key sat on a mutation edge the check could not see.

  `header_edges` is now derived from `plan.edges`, so the check structurally
  cannot see a different graph than the colony runs. More visible edges means more
  obligations, so the landing differs by boot kind: a FirstBoot violation is still
  a hard refusal, while a Reboot **reports every offending node and boots** —
  refusing there would hand the operator a crash loop after the writes are already
  on disk. `meclaw --validate --validate-strict` turns the same list into a
  non-zero exit as a pre-flight. Spec edges are still parsed and CEL-validated; a
  malformed file stays a loud error, it just no longer decides the topology.

- **The hive-scope table is the boot topology too**
  ([#186](https://github.com/mmeyerlein/meclaw/issues/186)). The other half of the
  same cut: `header_hives` still came from the filesystem walk, so on a Reboot a
  persisted edge whose `from` is a hive whose directory was removed was read as a
  cell with no contract — contributing nothing to the fan-in walk — instead of as
  the transit pass-through it is.

- **An edge can address a node the same diff creates at depth**
  ([#166](https://github.com/mmeyerlein/meclaw/issues/166)). The post-state node
  set held `add_nodes[].name` as written while the multi-segment endpoint check
  resolved to an absolute path, so the two namespaces never met and a diff-new
  node one level down was invisible to its own edge — on both ends. Single-segment
  names took the other branch, which is why every everyday mutation was fine. The
  overview already promised this worked; the fix makes the promise true.

- **A deep `add_nodes` name is checked against the paths that exist**
  ([#179](https://github.com/mmeyerlein/meclaw/issues/179)). The same asymmetry on
  the identity check, and the destructive half: a multi-segment name matched
  nothing in the collision set, so an instantiation over a live cell produced no
  `naming_collision` and went to staging. No-Delete protected the `cell.db` bytes,
  but the cell's identity was re-minted and its config overwritten. Two spellings
  of one deep path in a single diff (`unit/n1` + `./unit/n1`) **panicked the
  colony task** outright. `scoped_name` now decides once which namespace a diff
  name belongs to, and every identity check goes through it.

- **An edge inside a hive is drawn inside it**
  ([#174](https://github.com/mmeyerlein/meclaw/issues/174)). The surface router
  had one notion of "which side" doing two jobs — where an edge attaches to a box,
  and which way it then travels. Those agree for two boxes side by side and are
  exactly opposite for a frame around a cell, so every door edge left its frame,
  looped around the outside and came back. 9 of 9 drawn inside out.

- **A stored arrangement survives a cell being added**
  ([#170](https://github.com/mmeyerlein/meclaw/issues/170)). A hive row held a
  point measured against the computed flow layout — and that layout is a function
  of the whole node set, so one added cell silently redefined the reference every
  stored anchor was measured against. A row now holds the SHIFT it was dragged by,
  which no arrival can redefine. Measured on a 53-cell colony adding 6 cells:
  hand-placed cells moved 17 of 53 → **0 of 53**, hive frames 6 of 19 → **0 of 19**.

- **The canvas does not report its own writes as failures**
  ([#183](https://github.com/mmeyerlein/meclaw/issues/183)). The store answers
  every request including write acknowledgements, and the renderer reported each
  ack to the surface as "the canvas store did not return rows". The store stamps
  the op onto its own reply, so the discriminator is stated rather than guessed —
  but the ORDER is the whole guard: a SQL error carries both `error_code` and
  `operation`, so reading the operation first would have silenced exactly the
  failed writes the error branch exists for.

- **A canvas row that names nothing is reported, and swept only when asked**
  ([#184](https://github.com/mmeyerlein/meclaw/issues/184)). Rows for cells and
  hives that no longer exist accumulated for ever. They are NOT swept
  automatically, and the reason is worth stating: a render-time sweep cannot tell
  a rename from a removal, because the substrate had no rename — a renamed hive is
  one name that vanished plus another that appeared. On the reported colony all
  four dead rows were renames, so an eager sweep would have deleted four
  hand-placed positions and nothing else. The legend reports the count and offers
  one control; pressing it is the operator asserting the one fact the server
  cannot infer.

- **A `required_drains` port written `./recall` still insists**
  ([#202](https://github.com/mmeyerlein/meclaw/issues/202)). `params.required_drains[].port`
  is documented as the same shape as `params.ports`, and since
  [#196](https://github.com/mmeyerlein/meclaw/issues/196) that shape accepts both
  spellings of one node. The drain reader re-derived a stricter rule instead —
  it refused anything containing a `/` — so `"./recall"` was warned about and
  dropped, and a hive that declared a drain requirement silently had none. Broken
  twice over for that spelling: past the guard the port path came out
  `/main/mem/./recall`, which no resolved endpoint can equal, so the requirement
  could not have fired even if it had survived.

  Same severity shape as #196 and the opposite direction of the rest of this
  family: lenient, silent, and it removes a guarantee rather than refusing a
  valid diff. A `required_drains` entry exists to insist that an error lane is
  wired; what stopped applying is the insistence.

  Both readers now share `canonical_port_name`, so the two readers of one
  documented shape agree by construction. No shipped template and no live colony
  spelled a drain port that way, so nothing that used to commit is refused now.

- **A resume target is the node it names, however it is spelled**
  ([#201](https://github.com/mmeyerlein/meclaw/issues/201)). An `add_nodes` at an
  existing path is a Reconnect/Resume — the same node keeping its `cell_id` and
  its `cell.db` — and is deliberately not a `naming_collision`. The list that
  carries that exemption, `resume_names`, was collected AS WRITTEN and then
  subtracted from `registry_names`, which hold canonical registry short names.
  `"./fetch" != "fetch"`, so a resume target spelled the canonical way never
  cancelled its own registry entry and the Resume was refused:

  ```
  add_nodes 'fetch'    at existing /fetch    -> committed
  add_nodes './fetch'  at existing /fetch    -> naming_collision "./fetch"
  ```

  Short-name-only and depth-invisible: the deep half is answered from
  `deep_registry_paths`, filtered by `resume_targets`, and those were pushed
  resolved from the start — the same asymmetry #199 found one file over.
  Lenient-opposite (a legitimate Resume refused, nothing committed wrong), which
  is presumably why it outlived its siblings. The name goes through `scoped_name`
  now, so nothing on the mutation surface is keyed on how a caller happened to
  spell a name.

- **An `add_nodes` named `./successor` is the node a forward reference finds**
  ([#199](https://github.com/mmeyerlein/meclaw/issues/199)). The existing-node
  form of `swap_nodes[].with` may forward-reference a node an `add_nodes` of the
  same diff is creating. The set that answers it, `add_names`, was collected AS
  WRITTEN and then consulted as the short-name namespace, which strips the
  canonical `./` prefix before comparing — so `./successor` sat in a set only
  ever queried with `successor`, and an `add_nodes` written the canonical way was
  invisible to the reference in either spelling (`match_no_hit`, on a diff whose
  own node was right there). Its resolved twin `add_paths` was always correct,
  which is why the defect was short-name-only and depth-invisible.

  #189's exact shape at the one call site #189 did not touch. Lenient-opposite —
  a valid diff refused, nothing committed wrong — which is why it survived four
  passes over this family. `add_names` is canonicalised through `scoped_name`
  now, so no call site in the mutation validator compares a spelling.

- **A diff can wire what it moves or swaps in**
  ([#198](https://github.com/mmeyerlein/meclaw/issues/198)). `swap_nodes[].with.name`
  (the instantiate form) and `move_nodes[].to` are addresses the diff CREATES.
  Neither reached the post-state node view the endpoint check is answered from,
  so an edge in the same diff naming one was refused:

  ```json
  {"move_nodes": [{"match": {"name": "fetch"}, "to": "unit/fetch"}],
   "add_edges":  [{"from": "./anchor", "to": "./unit/fetch"}]}
  ```

  → `edge_schema: to='./unit/fetch' unknown`.

  This undercut the argument both operations were built on. An `add_edges` edge
  may point at a node arriving in the same diff via `add_nodes` because splitting
  into two mutations means choosing between a window where a lane is wired twice
  and one where it is not wired at all; `move_nodes` was shipped (#169) to be the
  relocation with no such window — and a caller who wanted to give the relocated
  cell one extra lane in the same breath could not.

  The view's insert side is now built from `diff_path_claims`, the enumeration
  that already answers "which entries put a node at a path" for the duplicate-claim
  check (#195), rather than from a second list beside it. Both halves of the view
  — short names and absolute paths — are reached through one `occupy`, the mirror
  of #194's `vacate`. The existing-node form of `swap_nodes[].with` stays out for
  the same reason it is no claim: it references a node the pre-state or an
  `add_nodes` of the diff already contributes, and creates nothing of its own.

  Lenient-opposite: a valid diff was refused, nothing committed wrong. An
  endpoint no entry of the diff creates is still `edge_schema`, and an address
  the diff VACATES is still no endpoint (#194).

- **A `params.ports` entry written `./policy` is the port `policy`, and one that
  can never match is reported instead of ignored**
  ([#196](https://github.com/mmeyerlein/meclaw/issues/196)). The port boundary
  compares the SHORT name of a resolved endpoint against each declared entry, so
  a declaration spelled with the canonical `./` prefix compared equal to nothing
  at all. `templates/access` (`["./policy", "./invoke", "./store"]`) and
  `templates/steward` (`["./meter", "./mutator"]`) were therefore sealed exactly
  as strictly as `ports: []` — the hive path the only address — while their own
  READMEs presented those ports as the way in. A mutation wiring a documented
  port was rejected as `hive_port_boundary`, with a message that named the
  boundary rather than the spelling.

  Entries are now canonicalised on read, the way every other reader on the
  mutation surface already handles the prefix (#189, #193), so both spellings
  land on one name and a colony instantiated from either template keeps working
  without touching its frozen `config.json`. An entry that can never name a
  direct child — a deep name, `.`, `..`, or nothing — is reported at `warn` and
  dropped, which is what `params.required_drains` has always done with the same
  shape. That silence was the real defect: the templates looked configured, were
  not, and said nothing, because neither had ever been instantiated.

  The two shipped declarations are written as short names now (a spelling
  change, not an interface change), and `gh196_shipped_hive_ports` runs every
  shipped `params.ports` through the substrate's own reader and boundary, so a
  declaration that opens no door cannot ship again.

- **An `add_nodes` name written `./foo` can be addressed as an edge endpoint in
  the same diff** ([#189](https://github.com/mmeyerlein/meclaw/issues/189)).
  `./foo` and `foo` are the same path, and the rest of the mutation surface
  treats them as such — but the post-state node set took the name as written
  while the endpoint lookup strips the prefix first. So

  ```json
  {"add_nodes": [{"name": "./foo", "template": "echo"}],
   "add_edges": [{"from": "./bar", "to": "./foo"}]}
  ```

  was rejected with `edge_schema: to='./foo' unknown` while the identical diff
  spelling the name `foo` committed. The message was misleading on top of it: it
  called the endpoint unknown when the node was right there in the same diff,
  under a name differing by two characters. The insert now canonicalises like
  every other reader, so both spellings land on one key.

- **A `remove_nodes` written `./foo` takes `foo` out of the post-state, so an
  edge cannot be wired onto it in the same diff**
  ([#193](https://github.com/mmeyerlein/meclaw/issues/193)). The last place in
  the endpoint check that compared a spelling instead of a name. `remove_nodes`
  dropped its `match.name` from the post-state node set **as written**, so

  ```json
  {"remove_nodes": [{"match": {"name": "./foo"}}],
   "add_edges":    [{"from": "./bar", "to": "./foo"}]}
  ```

  committed an edge onto the very node it was disconnecting, while the identical
  diff spelling the name `foo` was refused as `edge_schema`. This is the lenient
  direction of #189 — it accepted a diff it should refuse — and it left behind a
  committed lane pointing at an inactive node, which is what the endpoint check
  exists to prevent.

  **Behaviour change:** such a diff now rejects as `edge_schema`, whichever way
  either side spells the name. A `remove_nodes` that no edge in the same diff
  contradicts is unaffected and still commits, prefix or no prefix.

- **A `swap_nodes[].with` no longer writes over a directory that is already at
  its target path** ([#188](https://github.com/mmeyerlein/meclaw/issues/188)).
  `add_nodes` looks at its target before staging — an existing directory makes
  the entry a Resume and nothing is copied over it. The with-side never looked.
  It staged a fresh template tree and let the apply take its `final_path exists`
  branch, which replaces `config.json` in place: a second `cell.id` for a path
  that already had one, a different `cell.type` reading the `cell.db` beside it,
  and no diagnostic anywhere.

  [#179](https://github.com/mmeyerlein/meclaw/issues/179) closed the half where
  the target carries a registry row. What was left is a directory nothing in the
  registry claims — a hand-placed tree, the residue of an aborted migration, a
  row cleared outside the mutation flow. The registry cannot see it, because the
  registry is what it is missing from.

  **Behaviour change:** such a diff now rejects as `naming_collision`, and the
  message names the directory it found and the diff entry that aimed there,
  instead of the mutation committing in silence. Nothing is written. Re-taking
  that path needs no manual wipe: an `add_nodes` at an existing directory is a
  Resume (decided on filesystem existence, so it covers an unregistered
  directory too), and an `add_nodes[].adopt` entry takes such a tree over
  deliberately with the on-disk `cell.type` checked against what the diff
  expected.

- **A node the same diff is taking out is no longer a valid edge endpoint — at
  any depth, and for `swap_nodes` and `move_nodes` as well as `remove_nodes`**
  ([#194](https://github.com/mmeyerlein/meclaw/issues/194)). The endpoint check
  answers "will this node be there?" from two sets: the scope's short names, and
  the absolute paths of everything that exists at any depth. #193 taught the
  `remove_nodes` loop to subtract from the first. The second arrived as the
  **pre**-state and nothing was ever taken out of it — and for a node that
  already existed at depth, that is exactly the set the check reads. So

  ```json
  {"remove_nodes": [{"match": {"name": "talky/split"}}],
   "add_edges":    [{"from": "./anchor", "to": "./talky/split"}]}
  ```

  committed, and the lane sat in `colony.db` pointing at a node the same
  mutation had disconnected. Newly reachable since
  [#179](https://github.com/mmeyerlein/meclaw/issues/179) made a deep
  `match.name` hit at all.

  The same question asked of the two operations written while that set was
  assumed immutable answers the same way, and there it was never depth-only:

  - a `swap_nodes[].match.name` names the node being **replaced**. Its edges are
    swung onto the target, but the swing runs over the edges that were there
    before the diff — so a lane the same diff added onto the replaced node was
    not carried along and was left naming a disconnected cell.
  - a `move_nodes[].match.name` names an address the mutation **vacates**. The
    directory leaves by `rename(2)` and the registry row is re-addressed, so an
    edge naming the old address committed onto a path with nothing at it.

  **Behaviour change:** all three now reject as `edge_schema` when an
  `add_edges` in the same diff names the node they are taking out, in either
  spelling and at any depth. A `remove_nodes`, `swap_nodes` or `move_nodes` that
  no edge in the same diff contradicts is unaffected and still commits — the
  swap still swings its existing lanes onto the target, the move still carries
  its own along to the new address.

- **Two entries of one diff can no longer claim the same path**
  ([#195](https://github.com/mmeyerlein/meclaw/issues/195)). The naming checks
  measure a diff against the registry, which arrives resume-filtered on purpose:
  an `add_nodes` at an existing path is a Reconnect/Resume, not a collision.
  Beside that sat the only in-diff bookkeeping there was, and it tracked
  `add_nodes` entries among themselves and nothing else. A claim from any other
  entry of the same diff fell between the two — the registry check had been told
  to ignore the path, and the in-diff check was not looking at that side.

  So a `swap_nodes[].with.name` equal to an `add_nodes` target in the same diff
  passed. Where the path already existed,
  [#188](https://github.com/mmeyerlein/meclaw/issues/188)'s staging guard
  refuses the destructive outcome — but with the generic occupied-path message,
  which advises clearing the directory or adopting it deliberately. That is
  advice for a leftover tree and wrong here, where the diff itself is what wants
  the path twice. Where the path did **not** already exist, nothing refused it
  at all: two trees were staged onto one path and the second apply failed
  halfway, which strict-fails the whole colony task. The same hole was open for
  two `swap_nodes[].with` at one name, and for a `swap_nodes[].with` against a
  `move_nodes[].to`.

  **Behaviour change:** a diff in which two entries claim one path now rejects
  as `naming_collision` before anything is staged, and the message names both
  entries by position and the path they share, instead of describing a directory
  that is not the problem. The claim set spans `add_nodes` (resume targets
  included — a resume is not a collision against the registry, but it is still
  this diff claiming that path), the instantiate form of `swap_nodes[].with`,
  and `move_nodes[].to`. Claims are compared as resolved paths, so `unit/n` and
  `./unit/n` are one claim.

  Unchanged: a lone `add_nodes` at an existing path is still a Resume and still
  commits, keeping the node's identity. The existing-node form of
  `swap_nodes[].with` carries no `template`, references a node instead of
  creating one, and is deliberately not a claim — a `with.name` forward-
  referencing this diff's own `add_nodes` keeps resolving.

## [0.13.0] — 2026-08-17

The canvas was correct and unreadable. Its stylesheet was copied from a working
standalone renderer and promised a UI the markup never emitted — so half of what
follows is not new work, it is the markup finally keeping the stylesheet's word.

### Fixed

- **Arrowheads.** `surface.css` has said `marker-end: url(#ar)` since the first
  version and nothing ever defined `#ar`, so every edge in every picture was an
  undirected line — 123 of them on a real colony, with no way to tell which way
  anything flows. A graph whose direction you cannot read is a doodle. The markup
  now carries the `<defs>` its own stylesheet asks for, and a test reads the marker
  ids out of the CSS so a rename is red on both sides.
- **A conditional edge looks conditional.** `.cond` is dashed by the stylesheet and
  was never set; on the colony under test 113 of 123 edges carry a condition, and
  drawing them like the rest hides exactly what an operator came to look for.
- **A surface answer larger than `blob_inline_max_bytes` arrives.** The substrate
  offloads a big body into a blob by design and without asking — and a page is
  precisely the kind of body that gets big. The dispatcher only understood inline
  bodies, so the first canvas to cross 64 KB failed every join with "body is not
  inline": a message that names the symptom and hides the cause. `Dispatcher::with_blobs`
  resolves it, bounded by its own timeout; an unresolvable body is still reported
  rather than served as an empty page.

### Added

- **Selection.** Click a cell: its edges light up, everything unrelated dims, and
  the panel lists what points at it and where it points — each entry clickable, so
  a colony can be walked one hop at a time. Click an edge: its condition and its
  modifier in full, which is what you need when a message did not go where you
  expected. Click the background: cleared. Entirely client-side, because what is
  selected is not a fact about the colony and a round trip per click would make
  reading a graph cost cell calls.
- **A detail panel** to say it in, and a fat invisible twin path per edge to make a
  1.4-pixel line clickable at all — the same construction the standalone renderer
  uses.

## [0.12.3] — 2026-08-17

### Fixed

- **Hive in hive.** A hive's frame was derived from a cell's **direct** parent only,
  so `/a` and `/a/b` were drawn as two unrelated rectangles packed side by side, and
  a hive holding nothing but sub-hives got no frame at all — the picture said almost
  nothing about the tree it is a picture of. Now every ancestor of every cell is a
  hive with a frame, the layout is recursive (a hive's own cells as rows by flow
  depth, its child hives packed into shelves below), and each frame is derived from
  every cell *beneath* it. An ancestor is padded more than its descendants, so a
  parent's frame **strictly** contains a child's instead of sharing an edge with it.
  A colony's real depth is eight and all eight levels are visible.
- **Dragging a nested hive takes its subtree.** The client moved only the direct
  children while the server moved everything below, so the inner frames stayed put
  for one round trip and then snapped into place. The drag now carries every
  descendant cell and every nested frame; ancestors are deliberately left alone,
  because their frames are derived and the server's answer grows them — a parent
  stretched on the client would be guessing.

### Added

- **A tint per depth**, `depth-1` … `depth-10`, so nesting is legible without
  counting dashes or measuring rectangles. Faint and stacking: hives are emitted
  parent-first, so a child paints over its parent and each level adds a shade. Ten,
  because `/org/<org>/member/<who>/assistants/<name>/talky/keeper` is already eight
  and two hives at different depths sharing a colour is the thing the tint exists to
  prevent. `HIVE_DEPTH_TINTS` in `render.py` and the rules in `surface.css` are held
  together by a test that reads both files and refuses a reused value.

## [0.12.2] — 2026-08-17

### Added

- **A hive can be dragged, and it costs one row.** Grab a hive anywhere inside its
  frame — the empty space is the handle — and the frame, its label and every cell
  in it move together; on release the client sends one `hive:moved` with the
  group's new box origin and the server writes one row for the group, whatever its
  size. Twenty cells do not become forty store round trips on an interactive path.
  Reported the moment the cells became draggable, and rightly: a fifty-cell picture
  is arranged in groups, not one box at a time.

  What is stored is an **offset**, never the rectangle. The rectangle stays derived
  from where the members ended up, which is what lets a cell dragged out of a crowd
  grow its hive instead of being stranded outside a stale frame. Precedence reads
  one way: a cell somebody placed by hand, then the offset of its hive, then the
  automatic layout — so moving a hive never silently undoes a hand-placed cell
  inside it.

### Fixed

- **`pointer-events: all` on the hive frame.** An SVG rect with `fill: none`
  receives pointer events on its *stroke* only, so without this a hive would have
  been grabbable by its one-pixel dashed border and nothing else. The cells still
  win inside it, because the nodes group paints after the hives group and therefore
  gets the event first.

## [0.12.1] — 2026-08-17

### Fixed

Three defects in `templates/canvy`, all of them browser-visible, all of them found
by opening the page rather than by a test — which is the finding behind the fourth
item.

- **The canvas offered the client nothing to attach to.** A LiveView hook mounts
  only on an element carrying `phx-hook="<Name>"` **and** an `id`; the rendered
  markup had neither. So `client/surface.js` never ran, and it owns two things the
  server deliberately does not: every edge's `d` (the server sends endpoints and a
  lane, never a path) and the whole drag. What every join served was a picture with
  no lines that could not be moved.
- **The one expression the hook evaluated to draw an edge threw.** It said
  `rounded(route(...))`, but `route` returns `{d, start, end}` while `rounded` takes
  an array of points — a `TypeError` inside the loop, so not one edge in the picture
  got a path. The 19 property tests all called `route(...).d` and were green. There
  is now a single `edgePath()` for that call, and it has its own test.
- **The arrangement was one column.** Hives were stacked vertically, which on a
  14-hive colony is a 3672-pixel-tall strip two boxes wide: every hive at the same
  x, so the layout carried one bit of information where a screen offers two
  dimensions. Hives are packed into rows now (wrapping at 2400 px, sorted by path so
  siblings land side by side); the same colony draws 2346 × 1396. The `<svg>` also
  carries a **`viewBox` over the whole drawing**, so the browser fits the picture
  into the frame before any JavaScript runs — previously the element was exactly the
  size of its container with nothing to scroll, so everything below the fold was
  simply unreachable.

### Added

- **Pan and zoom.** Drag the empty canvas, wheel to zoom around the cursor. The
  camera is applied as a transform on `g.viewport`, server-side from the store row
  (so a saved view survives a reload without the client) and re-applied by the hook
  after every diff, because the server re-renders the whole tree. Drag arithmetic
  now runs in the SVG's user space via `getScreenCTM`, so a box follows the cursor
  at any zoom instead of lagging by the scale factor.
- **The client's test suite is executed.** It was in the tree, in the template's
  file inventory, and run by nothing — not CI, not `cargo test`, not the release
  routine. `cargo test` runs it now (skipping if `node` is absent), and it grew a
  section that mounts the hook against a hand-built DOM: does every edge get a
  usable path, does letting go of a box send exactly one `node:moved` with the drop
  position, does dragging the empty canvas pan without telling the server anything.
  A test file that only exists is a comment.
- Two server-side tests for the seam that had none: the markup must offer the hook
  name that `surface.js` registers (read out of the JS, so a rename on either side
  turns it red), and the layout must not regress to a single column.

## [0.12.0] — 2026-08-17

### Added

- **A surface installs into a colony that is already running**
  ([#163](https://github.com/mmeyerlein/meclaw/issues/163)). One mutation
  (`add_nodes` with the `canvy` template), no restart, no lane for the parent to
  grant. Two rules moved for it, and both are the interesting part:
  - **The egress door is no longer a place.** `EgressPolicy` now decides *where* it
    opens as well as *what* leaves: `All` stays root-only (Direct-Mode, where a
    dead end really is an answer and only at `/`), while `Marked` opens at whichever
    hive a marked message ran out of graph at. The marker is minted by the layer
    that injected the request and is unforgeable by a cell, so a marked message is
    by construction an answer somebody is holding a socket open for — there is no
    hive at which dead-lettering it is better. A surface's answer lane is therefore
    `./render -> .` and stays inside its own subtree.
  - **`/colony/graph` is drawable by a mutation** — the one absolute endpoint that
    is. It addresses the authority's read-only topology endpoint, not a cell, and it
    is the sanctioned alternative to reading `colony.db`, which § Database isolation
    forbids. `/colony/mutations`, `/colony/trace` and `/colony/dead_letters` stay
    out of bounds.
- **`consumes.topology.inbound_edges`: a cell may declare that it needs to know its
  own doorway** ([#160](https://github.com/mmeyerlein/meclaw/issues/160)). A
  declaring cell receives a read-only, self-scoped handle at spawn
  (`NeighbourhoodView`) that answers one question — the `from` of every edge
  pointing at its own path — live from the colony's in-memory edge table, bounded by
  the cell's own operation timeout. Not a message compartment: nothing in
  `consumes.topology` is validated against an incoming message. Undeclared cells
  get no handle and cannot ask.

### Changed

- **§ Database isolation has no exceptions left** ([#160](https://github.com/mmeyerlein/meclaw/issues/160)).
  The `vault` unlock attestation was the last direct `colony.db` read in the
  workspace; `crates/meclaw-cells/src/vault/attest.rs` no longer imports `rusqlite`
  and is now a pure comparison of two lists of paths. The defence is unchanged,
  because it never rested on the database: what protects a vault is the sealed
  contract in its **own** `cell.db`, so a tampered edge is found whether it was read
  or told. An unverifiable neighbourhood (no declaration, no answer, a timeout) is
  treated exactly like a wrong one and the vault stays LOCKED — which means a
  `vault` config without the new declaration never unlocks. Both shipped vault
  templates carry it.
- **`meclaw --vault-add` refuses while a colony holds the root**
  ([#160](https://github.com/mmeyerlein/meclaw/issues/160)). The vault user channel
  deliberately boots no colony and therefore never takes the root lease, which left
  it writing the vault's `cell.db` next to the live cell that owns it — WAL makes
  that survivable, not correct, and the running cell's view goes stale with nothing
  announcing it. The channel now consults the lease before the first
  `Connection::open` and names the holding pid. `--vault-status` stays available
  (read-only), and `--vault-revoke` keeps working with a loud warning: being locked
  out of a vault must never be what stops somebody killing a leaked credential.
- **`templates/canvy` carries both of its outward lanes itself.** The topology lane
  no longer has to be granted in the parent's `config.json`, at bootstrap or by
  mutation. The condition on it (`hop.route == 'ask_colony'`) is still mandatory and
  still the reason [#161](https://github.com/mmeyerlein/meclaw/issues/161) happened.

### Fixed

- **A stray directory can no longer turn a healthy colony into a boot failure**
  ([#163](https://github.com/mmeyerlein/meclaw/issues/163)). A cell directory
  somebody places by hand has always been reported and not adopted; a **hive**
  directory was reported for its children but its `params.graph` was planned anyway,
  so every endpoint in it pointed at a child that was correctly not adopted and the
  boot died on `DanglingEndpoint` — on every restart, until the directory was
  removed. The reboot walk now consults the persisted `hive_scopes` for hives, the
  same way it consults the registry overlay for cells.
- **A blocked delivery names its mailbox before it blocks**
  ([#162](https://github.com/mmeyerlein/meclaw/issues/162)). `route()` delivers with
  an `await` on the cell's mailbox, which is correct — a full mailbox is
  backpressure — but the waiter is the colony's routing loop, so the whole colony
  stops, and the corridor is byte-frozen and silent. From the outside that was a
  colony that stopped after twenty seconds with an **empty** dead-letter queue and
  nothing in the message log; diagnosing it during #161 needed a SQLite client on
  `colony.db`. A pre-check at the call site (the same construction as the TTL twin
  already there) now logs the target, the sender, the trace and the configured
  capacity. Semantics are untouched: a full mailbox still blocks.

## [0.11.1] — 2026-08-17

### Fixed

- **`canvy`: a hive's height is its own flow depth, not the whole graph's.** The
  flow layer is computed across the entire colony and was applied inside one hive,
  so two cells in the same hive could sit 395 empty rows apart because one of them
  is downstream of a deep pipeline somewhere else. Measured on a live 46-cell /
  13-hive colony: the vertical extent was 174828 px, and is 3384 px with the layers
  ranked per hive. Order is preserved — a request still sits above the thing it
  asks.

### Changed

- **`templates/canvy/README.md` no longer claims a surface can be instantiated by
  mutation into a running colony** ([#163](https://github.com/mmeyerlein/meclaw/issues/163)).
  It cannot, and the reason is structural: the egress door exists only at the root
  hive, so the lane carrying a surface's answer must be `-> /`, and a mutation may
  not draw an edge that leaves its subtree. A surface therefore belongs in the
  bootstrap tree of a colony that has not started yet, or in a tree of its own fed
  from outside. Worse, placing the directory by hand instead makes the next boot
  fail with `DanglingEndpoint`, because a pre-existing directory is not adopted —
  so an operator following the old README could turn a healthy colony into a boot
  loop.

## [0.11.0] — 2026-08-17

### Added

- **A colony serves surfaces over HTTP** ([#159](https://github.com/mmeyerlein/meclaw/issues/159)).
  A cell may declare `cell.surface`, and it is then served under **its own cell
  path** — `/surface/<cell-path>` for the page, `…/live/websocket` for the
  transport, `…/@asset/<file>` for its own files. Everything a surface needs sits
  under one URL prefix, so a single nginx `location` block authorises all three
  without knowing anything about MeClaw. Opt-in and absent by default: a cell that
  declares nothing answers 404, never 403.
- **`templates/canvy@0.1.0`** — the first surface, and a working example of the
  above: a `code` cell renders every tag server-side, the browser owns only the
  drag, and the first thing it draws is the colony itself. A page load costs zero
  cell calls; a join or a drop costs two.
- The vendored **Phoenix LiveView client** (`phoenix_live_view.min.js` 1.2.9,
  `phoenix.min.js`, both MIT, byte-for-byte, never edited) ships inside the binary
  and is served at `/surface/@client/…`. No npm, no bundler, no JS toolchain.

### Changed

- **The colony's egress door is now a policy.** `ColonyTaskConfig::with_egress`
  keeps taking everything that dies at the root hive (Direct-Mode, unchanged);
  `with_marked_egress` takes **only** messages carrying a named `context` key and
  leaves every other root-hive dead end in the dead-letter queue. `--api` uses the
  marked form, so opening a return path for surfaces does not silently swallow
  correct dead letters. Behaviour with no egress sink is unchanged.
- **`docs/meclaw-overview.md` § Datenbank-Isolation** writes down a rule that was
  held everywhere but recorded nowhere: a cell touches only its own `cell.db`, and
  no foreign database — **not even reading**. Topology knowledge has a route that
  obeys it (`/colony/graph`, by message). The two pre-existing breaches are
  [#160](https://github.com/mmeyerlein/meclaw/issues/160).

### Fixed

- **A privileged `/colony/*` lane needs a condition, and canvy's did not have
  one** ([#161](https://github.com/mmeyerlein/meclaw/issues/161)). An edge matches
  on the cell it starts at, not on intent, so the unconditional lane
  `./probe -> /colony/graph` also carried the probe's two store writes. Each write
  asked for the topology again and each answer produced two more writes: the growth
  was exponential and the routing loop blocked on a full mailbox within twenty
  seconds. Three further template defects surfaced in the same investigation, all
  of the same class — an emission that is legal to the script and illegal to the
  substrate: a body without a `messages` slot is refused before it reaches an edge;
  an edge modifier reading an absent hop field makes the edge **stop matching**, so
  a drop's position writes dead-lettered while the picture still showed the box
  moved; and a cell that does not recognise the reply to its own write cannot stop.
  `templates/receptionist` and `templates/builder-hive` ship the same lane shape
  and both already carried their conditions.

### Known issues

- **A blocked mailbox send stops the whole colony without naming the mailbox**
  ([#162](https://github.com/mmeyerlein/meclaw/issues/162)). Backpressure into the
  routing loop is deliberate, but from the outside it is indistinguishable from a
  deadlock: the watchdog trips, the dead-letter queue stays **empty** and the
  message log stops growing, because the loop that would record any of it is the
  one that is blocked. Diagnosing #161 needed a SQLite client on `colony.db`.

## [0.10.7] — 2026-08-17

### Fixed

- **A test assertion is removed rather than replaced** (#156, second attempt).
  0.10.6 claimed to fix a flaky liveness check by making it positive: feed the
  task a probe message, wait for the mailbox capacity to come back, call that
  proof the task is running. The first scheduled CI run after the release —
  the Monday cron — tripped on the line after it instead.

  The error in that repair is worth stating plainly, because it looks like good
  practice: **a probe is an input, and an input can end the cell.** A liveness
  check that perturbs the thing it measures is not a liveness check, and
  trading a known flake for a subtler one is a loss even when the diff looks
  more rigorous.

  The test already proved liveness at its other end, and nobody had noticed: it
  closes the mailbox and waits for the task to finish cleanly, which a task
  that never ran cannot do. That is an event rather than an instant — exactly
  the property the repair went looking for, already present. So the assertion
  is gone, the probe is gone, and the test is shorter than before either
  attempt.

  The docstring records the failed repair so it is not tried a third time.

## [0.10.6] — 2026-08-17

### Added

- **The `mcp` child reads `params.sandbox`** (#96). Same profile schema as
  `bash`, `code` and `harness` — one shape, one parser, one set of mistakes an
  operator can make — applied to the `stdio` transport, where there is a child
  process to contain. **Opt-in**: without the key the child keeps the daemon's
  rights, which is the state of every `mcp` cell installed today. The key is
  **immutable**, because a boundary a runtime params update could switch off is
  not a boundary.

  Of the three places in the tree that start a foreign process, this was the
  strongest case: an MCP server is a third-party binary an operator configured,
  and therefore the one least likely to have been written by whoever runs the
  colony.

### Decided

- **`subcolony` takes no sandbox profile, and the reason is now in the code and
  the spec** (#96). A child colony is a colony, not a cell running foreign code:
  its cells carry their own profiles, so a profile here would be a second
  boundary over the same processes, and the two would disagree the first time
  somebody tightened one of them. The filesystem half does not survive contact
  either — the child needs its own root plus every cell directory below it,
  which is most of what it could be denied. The resource caps are scopeable, but
  they belong to whoever runs the daemon rather than to a cell param covering
  one of several process trees.

  What contains the child is unchanged and is not nothing: its own process group
  and `env_clear`. A test pins the decision, so that wiring a profile here later
  fails loudly against the paragraph that argued against it.

### Fixed

- **A unit test that asserted against a clock now asserts against an event**
  (#156). `lr_helper_spawns_task_and_returns_pair` yielded once and then checked
  that the task had not finished — which failed on a saturated CI runner, and
  whose obvious repair (assert the running state repeatedly over ~100 ms) failed
  *locally* every time. That says the assertion was a race in both directions.

  Liveness is proven positively now: a probe message goes into the mailbox and
  the test waits until the task has taken it out. Only a running task drains its
  own mailbox. Same family as #153 and #129 — the fix is always to find the
  event the test actually means.

## [0.10.5] — 2026-08-17

### Fixed

- **A test that waited for a poll cycle now waits for the poll** (#153). The
  proxy promotion e2e suite boots a real tree whose `proxy` polls a mock
  `getUpdates`, and then waited for the relay's first message — which is a wait
  for "a cycle landed", not for anything the test can observe. On a loaded
  runner a missed first poll costs a whole interval, and nothing in the test
  could tell *the cycle has not come round yet* from *the message will never
  arrive*.

  It now waits for the mock to have SERVED a `getUpdates` before it waits for
  the relay, and each half carries its own marker. A red run says which one:
  the proxy never asked, or it asked and the colony delivered nothing. Those
  are different pieces of work, and one 30-second marker across both said
  neither.

  Filed as *not urgent* — and then the CI run of this very release went red on
  its positive twin, which is as good a reason to fix something as exists.

### Added

- **`scripts/trace_latency.py`: read a lane's latency out of the colony's own
  log** (#124). A colony records every delivered hop with its instant, so "how
  slow is this errand" is already on disk — read-only, no model call, no cost,
  nothing to instrument first. Group traces by a cell name, get n / min / p50 /
  p95 / max, and with `--breakdown` the legs the time actually sits in.

  The definitions are stated rather than implied, because a latency figure with
  a fuzzy definition is worse than none: a trace's duration is its last hop's
  instant minus its first — the span the colony worked on the errand, a lower
  bound on what somebody waited, not the user-visible round trip. Percentiles
  are nearest-rank and `n` is printed next to every one of them.

### Changed

- **The consult eta hints are measured now, not guessed** (#124, lever 4). The
  `talky` README recommended telling the user "seconds" for a memory lookup;
  the same colony's log says about ten. The wording now follows the measurement
  — "about ten seconds" for a lookup, "half a minute" for reasoning, "a minute
  or more" with a search — and the README says to measure your own deployment
  before copying the words.

  Optimistic in that direction is the expensive kind: a user told *seconds* who
  waits eleven of them has been misled by the system, not by the model.

  Three of the issue's four levers turned out to be built already (the reasoning
  budget on the chat-completions lane, the separate lookup lane, and routing by
  the tool the model chose). Its done-condition — a memory consult in
  single-digit seconds — is **not** met, and the issue stays open saying so.

## [0.10.4] — 2026-08-17

### Added

- **`override_params` works on subtree templates, addressed by cell path**
  (#140). The keys are the paths of the cells inside the template, `""` being
  the subtree root:

  ```json
  {"name": "coll", "template": "collector",
   "override_params": {"assemble": {"max_turns": 40},
                       "window": {"retention_days": 7}}}
  ```

  R10 (2026-06-11) had rejected the field outright on subtree templates, and for
  a good reason: the flat form committed and applied nothing — a builder
  believing an override took, which is a false accept. The collateral was that
  `collector`, `cogny` and `talky` are subtree templates, so the params surface
  they gained in #136 could not be set at instantiation at all. An operator
  edited the instance config afterwards, which is forking the template by hand.

  Addressing removes the cause instead of the feature. **What R10 protected is
  unchanged**: a key that names no cell of the template is refused
  pre-destructively with `schema`, and the refusal lists the cells that do
  exist, so the next attempt is informed rather than guessed. `${ctx.*}`
  substitution remains the way for values the template itself distributes.

## [0.10.3] — 2026-08-17

### Changed

- **Tier 2 sees the candidates grouped by session, not only ranked** (#148,
  first of three measures; `memory-hive@1.4.0`).

  The measurement this answers: 50 stratified LongMemEval questions against the
  0.9.0 tree returned 96 % R@5 and 58 % end accuracy, and the `multi-session`
  class was the sharp one — **100 % R@5 against 30.8 % accuracy**. In 19 of 21
  wrong answers the retrieval had delivered the gold session. The evidence was
  in the bundle and the synthesis did not combine it.

  It arrived as one flat list ordered by RRF rank, with nothing saying which
  candidates came from the same conversation. A question that counts, compares
  or asks what changed is an aggregation over sessions, and it was being asked
  of a structure that had thrown the session axis away.

  The tier-2 payload now also carries `sessions`: the same candidates, grouped
  by the conversation they came from, oldest first, each group with its span and
  the ids it contributed. The flat ranking travels unchanged next to it — the
  ranking answers *what is most relevant*, the grouping answers *what belongs
  together*, and a spanning question needs both. A candidate with no session
  lands in `unattributed` rather than being dropped or folded into a neighbour.

  A fact candidate now keeps its `session_id` through hydration. The column was
  always selected; the fact branch dropped it, so a fact could never be grouped
  with the conversation it came from — which is most of what a multi-session
  answer is made of.

  The dialectic prompt says what to do with the structure: count sessions rather
  than candidates, read two sessions that disagree as a change over time rather
  than a contradiction, and name a missing session in `gap` instead of
  answering around it.

  **Honest scope**: this is a shape fix and its gate is a benchmark run, not a
  test. The tests pin the structure; they do not claim the accuracy moved. The
  issue's other two measures — whether the top-20 cap truncates the set a
  multi-session answer needs, and whether the dialectic should get a second pass
  for counting questions — are untouched, because both change what is retrieved
  or how often the model runs.

## [0.10.2] — 2026-08-16

### Added

- **`params.required_drains`: a hive can declare which of its ports come in
  pairs** (#147). Read as *if anything outside this hive wires into this port,
  then this port must have an edge that carries the declared hop out of the
  hive*. A mutation that opens the ingress alone is rejected with
  `required_drain_missing`, pre-destructively, and the rejection carries the
  hive's own sentence about what the pairing protects.

  The case it comes from: the memory hive refuses an inline extraction block by
  sending it out a reject egress. With nothing consuming that egress the refusal
  is a dead end, and the write that never happened is never reported. The README
  said "not optional once the inline ingress is wired" in bold. Bold is not a
  check.

  Two things make the rule usable rather than annoying. It **asks the router**:
  the declared hop is run through `apply_edges`, the same function that routes
  the real message, so `hop.route=='reject'`, `hop.route in ['reject','error']`
  and `hop.route != 'bundle'` are all recognised as drains — a comparison of
  condition text would call two of those broken. And it runs against the
  **post-state**, so putting the ingress and the drain in one mutation is the
  answer, not a workaround.

  Opt-in like `params.ports`: a hive that declares nothing behaves exactly as
  before. The **bootstrap warns rather than refuses**, for the same reason the
  port seal leaves the boot alone — the birth topology is authorship, and a tree
  that has been running for weeks should not be stopped from starting.

  `memory-hive` ships the declaration for both of its reject egresses
  (`memory-hive@1.3.0`).

## [0.10.1] — 2026-08-16

### Fixed

- **An edge can be replaced in one mutation again** (#158). `remove_edges` is now
  applied **before** `add_edges`. Previously the order was the other way round,
  and since a `remove_edges` match pattern of `{from, to}` matches *every* edge
  between that pair, a diff that dropped an edge and added its widened
  replacement deleted its own new edge — and reported `committed`.

  Widening a port is the ordinary reason to do this: a lane already exists and
  needs one more hop key promoted into context. Splitting it into two mutations
  works, but leaves the lane missing in between, which is the exact window one
  mutation was supposed to avoid.

  What was silent was the *mutation*: it reported success for a diff that had
  removed a lane. The traffic afterwards was not silent — an emission matching
  no out-edge dead-letters as `no_route`, as it always has. The receipt and the
  DLQ disagreed, and the receipt was the one that lied.

### Documentation

- The mutation-format section now states the apply order, and says plainly that
  a match pattern is a pattern rather than an identity: `{from, to}` alone hits
  every edge between the pair. Pass `condition`/`modifier` to hit exactly one.

## [0.10.0] — 2026-08-17

Three packages from one wave, all of them things a colony needs before it can be
handed to anybody: a place to keep a secret an agent cannot read, a disclosure
rule that matches what a group chat actually means, and a control loop that can
change the colony it runs in and prove the change was right.

### Added

- **A `vault` cell type: a secret store with no operation that returns a secret**
  (#151). Not a policy layer over a store — the route surface has no read on it.
  `put`, `use`, `rotate`, `revoke`, `status`, `unlock`, `lock`, and no `get`. A
  fully compromised model on the other end of an edge can ask the vault to *use*
  a credential inside a granted scope; it cannot ask to see one, because the
  question has no name there.

  Four things carry it. **Two callers**: a message with no `reply_to` is the user
  channel, which no edge can produce because the colony stamps `reply_to` on
  everything a cell emits — that is the only way a secret gets in. The broker
  named in `params.broker` may ask for a secret to be *used*, and may not put
  one. **Unlock attestation**: the port boundary guards mutations and the birth
  topology is exempt by design, so a `code` cell with filesystem access could
  rewrite the tree and let the next boot wire itself in. It still can; it just
  never gets the key — an unexpected inbound edge, or a topology that cannot be
  read at all, leaves the vault locked. **An offline filling workflow**:
  `meclaw --vault /main/access/vault --vault-add <name>` seals straight into the
  cell's own database with no colony running, so the credential never becomes a
  message. **No-delete**: a put onto an existing name is a rotation, a revoke
  flips a status, yesterday's ciphertext stays — and revoke needs no passphrase,
  because being locked out must never stop you disabling a credential that
  leaked.

  Crypto is argon2id plus XChaCha20-Poly1305 per secret; `use` v1 signs with
  HMAC-SHA256 — the ssh-agent shape, where the key does work and stays home. The
  honest limit is stated rather than hidden: a determined code cell in the same
  process can read the key while it is unlocked, and the designed answer is
  placement (own process, own user), which changes no edge.

  Ships with `templates/vault@1.0.0`.

- **`steward@1.0.0`: the colony's control loop** (#155). Charter → deterministic
  measurement → a judge that simulates before it decides → a mutation through the
  **normal** lane with every gate → an immediate health check → the effect over a
  measurement window → keep or revert → a receipt. Seven cells, no Rust.

  The rule that makes it a control loop rather than an agent with write access:
  **a cycle without a pre-authored revert plan is invalid**. The steward has to
  know the way back before it moves, and the plan must actually restore the
  original — a "revert" pointing at the value being moved to would pass every
  structural check and undo nothing. Radius v1 is model choice and numeric
  params; a topology idea is recorded as a proposal for a human, never executed,
  and the radius widens by editing a charter row rather than code. Quality is a
  gate, not a wish. Two clocks on purpose: the probe answers *is the colony still
  working* in seconds, the window answers *was that a good idea* in hours.

  A freshly grown steward changes nothing — every goal in the seed ships
  disabled.

### Changed

- **An audience is a set, not a name** (#154). `affinity` enforced disclosure
  against a single scope, which is not what a group chat means. A fact that
  surfaced in a conversation is implicitly released to exactly the people who
  were there: a disclosure row now records the `audience_set` it was released in,
  and a fact may be used iff the current participant set is a **subset** of it.
  Hearsay never surfacing in front of its subject is a consequence of the rule
  rather than a second mechanism. Strict and fail-closed; the obvious softening
  (`subject == speaker == present`) is a decision not taken. Rows written before
  the rule are read as their addressee alone, the narrowest reading that cannot
  widen an old release by accident.

  In the same package: a curator's proposal is **accepted by default** — the
  system may extend its picture of a person on its own, and the safety net is the
  substrate rather than a queue somebody has to work through — and a verdict
  **appends** instead of overwriting, so the agent stays able to answer why its
  picture of somebody changed.

- **The `llm-registry` write-hand cell is renamed `steward` → `hand`.** The name
  steward belongs to the colony control loop; the registry is the book, the
  steward is the brain. The registry's runtime pin asserting the *absence* of a
  control loop stays true.

  *Migration*: an instantiated `llm-registry` keeps running — instantiation
  copies, so an existing instance is unaffected. A tree that wires
  `./llm_registry/steward` by hand renames that endpoint.

## [0.9.1] — 2026-08-16

Four defects found by running 0.9.0 in a production colony rather than by reading
the code. Two of them were **silent**: the lane kept working, nothing went red,
and the cost was money or a stalled turn.

### Fixed

- **The extraction lane stops paying for a provider that keeps refusing** (#143).
  Measured against a dead endpoint: the drain flushes about every five seconds,
  and a batch whose extractor call came back as a provider error was handed
  straight back to the queue — where the next flush claimed and re-sent it. 107
  of 111 extraction responses inside nine minutes were retries of **one** failing
  batch, on a seven-turn session. Free noise against a local model; against a
  paid endpoint it is a colony that looks idle while it spends.

  The queue row now carries what the batch has already cost: `attempts` and the
  instant `not_before` from which it may be claimed again. The window doubles per
  attempt (`MEMORY_EXTRACT_BACKOFF_SEC`, default `60` → 60/120/240 s), and after
  `MEMORY_EXTRACT_ERROR_BUDGET` (default `3`) consecutive provider errors the
  batch is **parked** — `status = 'error_parked'`, a mark in `scratch`, a line on
  stderr — instead of re-sent. Parked is terminal for the automatic lane; a
  recovery sweep does not take it back, because whatever failed three times in a
  row will fail the fourth. Nothing is deleted: the turns stay on the queue and an
  operator can re-open them.

  Two additive columns on `pending_extraction` (`attempts`, `not_before`);
  existing stores gain them on first open (`ALTER TABLE ADD COLUMN`), so there is
  nothing to migrate. The eligibility filter lives in the lane rather than in the
  store query on purpose: a held-back row has to be invisible to the **gate** as
  well as to the claim, or the gate counts tokens it may not claim and parks on
  every flush forever.

- **The `never-forgets` pin test ran the example without its seed step** (#142).
  The recall lane was wired and `system.tools` was never seeded, so a real model
  never called `memory_recall` and answered out of thin air — while the pin test
  stayed green, because the mock emits a canonical tool call whatever the brain
  was offered. The test now performs the documented seed
  (`templates/talky/brain/seed/system.jsonl`, WALKTHROUGH step 2) and asserts
  against the recorded provider request that the schema actually arrived.
  Removing the seed turns the file red.

- **A recall request carrying a stale `mem_phase` was silently ignored** (#152).
  `mem_phase` and `recall_id` are the hive's own bookkeeping and they are
  *persistent* context: once a consumer has asked memory something, they ride
  along in everything that consumer emits afterwards — including an errand it
  hands to a second agent, whose collector then asks the hive with a phase it
  never set. The echo guard read that as "mid-chain" and parked: no error, no
  dead letter, no log line, and the caller waited for a bundle that would never
  come. Measured in a production colony as a four-minute silent stall, and
  invisible in every shipped example, because a tree with **one** recall consumer
  cannot produce it.

  The request entry now recognises a request by the **hop** rather than by the
  context — the port edge stamps `phase: "recall"` and nothing inside the hive
  ever does — and starts a fresh chain regardless of what the caller carried.
  Nothing is required of the caller. The echo guard is untouched for everything
  else: an echo of the hive's own emission carries no request hop.

- **A `talky` persona consulted the core for what its own window already held**
  (#150). The instructions of a persona naturally enumerate what the core is
  *for* — deep thinking, research, long-term memory — and never what it is not
  for, so "what did I just tell you" reads, literally, as a memory question. The
  answer came back correct, which is why it never looked broken; it just cost a
  bridging sentence, a consult round trip and about six seconds against one and a
  half. `talky@1` now prints the boundary sentence a memory-carrying persona has
  to contain, and `cogny@1` points at it from the side that pays for its absence.

## [0.9.0] — 2026-08-16

### Breaking

- **A `code` cell's stdin is a structured document: `envelope` / `body` /
  `params`.** The stdin a script reads used to be flat — envelope fields lying
  next to the body slots — and a cell could not see its own configuration at
  all. It now reads exactly three top-level objects, all of them always present:
  `envelope` (`header` with both compartments, `target`, `trace_id`, `ttl`, plus
  `reply_to` / `parent_message_id` / `correlation_id` when the message carries
  them), `body` (the message slots) and `params` (a read-only, secret-filtered
  copy of the cell's own configuration, `{}` when there is nothing to hand over).
  **stdout is unchanged** — the emission a script writes is exactly what it was.

  **Migration**, one line per script: read your payload from `doc["body"]`
  instead of from the flat document, and take `header` and friends from
  `doc["envelope"]`. Every shipped template, example and fixture in this
  repository was migrated; a topology built outside this repository has to do it
  itself.

  **Why the shape, and not just a new key.** Scripts used to win their body by
  *subtracting* a hard-coded list of envelope keys — and with that pattern every
  future top-level field falls into the body automatically and travels on with
  the outgoing message. Nothing crashes; the cell simply starts carrying wire
  metadata, or its own configuration, into everything it emits. That already bit
  us once. The old pattern has no successor and no longer exists in the tree:

  ```python
  # gone — the subtraction that turned every new wire field into body content
  ENVELOPE = ("header", "target", "reply_to", "trace_id", "ttl",
              "parent_message_id", "correlation_id")
  def body_of(doc):
      return {k: v for k, v in doc.items() if k not in ENVELOPE}

  # now — the body is a named object, and the top level is closed
  doc = json.load(sys.stdin)
  d = doc["body"]
  ```

  The top level is closed by construction: future wire data travels **inside**
  one of the three objects, never beside them, so this migration is the last one
  of its kind. A body slot can no longer shadow `envelope` or `params` either.
  Context: `plans/meclaw-os/w13-envelope-receipt.md`.

- **`store` ops reject unknown top-level keys with `invalid_input`.** A typo in
  an op — `"wehre"` instead of `"where"` — used to be dropped on the floor, and
  the op ran without it: an unfiltered `DELETE` over the whole table, reported as
  a success with a row count. A forgiving parser is the wrong parser in front of
  a database. Every op's key set is now closed, and a key that is not in it is a
  loud args-level rejection rather than a silent widening of the query.
- **Boot-path edge specs are validated the way mutation edges always were.** An
  unknown field in a `params.graph` edge is now a **hard boot error** instead of
  being ignored: a misspelled `"condtion"` used to produce an *unconditional*
  edge, so the safest-looking typo in the DSL was also the one that routed
  everything everywhere. A tree that boots today will keep booting; a tree with a
  typo will say so at start rather than at 3 a.m.
- **The collector's twenty knobs are params, not environment** (`collector@1.2.0`,
  GH #136). `COLLECTOR_WINDOW_TURNS`, `COLLECTOR_TURN_WRITE`,
  `COLLECTOR_CONTEXT_WINDOW` and the seventeen others were `${VAR:-default}`
  substitution tokens: colony-global by construction, so two collectors in one
  tree could not be tuned apart, and every knob NAME was a public config
  contract that could only be renamed by breaking existing colonies. They are
  now params of `./assemble`, read off the `params` object a `code` cell has
  received on stdin since 0.9.0. Defaults are unchanged in value.

  **This is a clean cut, and it is breaking for a tuned colony.** There is no
  environment fallback: a `.env` that still carries `COLLECTOR_*` lines is read
  by nothing, and the affected collectors fall back to the shipped defaults
  *silently*. Before updating, move every `COLLECTOR_*` line you rely on into
  the `params` block of the matching `…/collector/assemble/config.json`, using
  the lower-case name without the prefix (`COLLECTOR_TURN_WRITE=1` becomes
  `"turn_write": "1"`). A colony that set none of them is unaffected.

  `add_nodes[].override_params` still cannot address the sub-cells of a subtree
  template (R10), so the knobs are set in the instantiated tree rather than in
  the mutation that creates it — which is also the one thing this change does
  *not* buy. `templates/collector/README.md` § "The knobs are per instance"
  has the detail.

  Composites carrying the collector move with it: `cogny@1.3.0`,
  `talky@1.2.0`. The cell contract `collector/assemble` goes to `1.2.0`, its
  `hop.route` enum gains a reserved eighth value `condense` for the later fold
  lane, and `consumes.hop.tokens_prompt` is corrected from `string` to `number`
  (the `llm` cell has always written a number there; the declaration is inert
  today, so this is a documentation fix, not a wire change).

- **`POST /messages` rejects a non-object `headers` with `422 invalid_headers`.**
  A string, a number or an array in that field used to be coerced to `{}` and the
  message travelled headerless — a client that got the shape wrong was told it
  had succeeded. It is now told it has not.
- **The FTS tokenizer is renamed `meclaw_stem` → `meclaw_stem_v1`.** Existing
  stores rebuild their keyword index automatically on first open, so this needs
  no action; it is listed here because the name appears in `params.fts` and in
  any SQL a reader wrote against the index by hand.

### Added

- **An episode reaches memory at the turn, not at the night.** The collector wrote
  its day out once, when the session closed — so a fact stated at nine in the
  morning was unrecallable until the keeper ended the day, a freshness hole of up
  to twenty-four hours. A second exit, `turn_write`, now hands the same batch out
  after every stored turn and every stored answer. It is off by default (the
  `turn_write` param, empty = off, and nothing about the collector moves), it goes
  through the **existing** `in_batch` inlet of `memory-drain@1` rather than around
  it, and the drain's ledger stays the single dedup authority — `episodes.turn_id`
  carries no UNIQUE column, and the writer inserts unconditionally, which is
  exactly why the fast path had to run through the ledger. Every emission carries
  the whole session so far, in order and without a `limit`, so the lane is
  self-healing: a dropped emission is repaired by the next turn rather than
  leaving a hole. `memory-drain@1` did not move a byte for this — two cadences,
  one ledger. Wire both edges into the same consumer or neither; a `turn_write`
  exit that lands somewhere else is a second, unledgered writer.
- **`cogny` grows a second brain, so a lookup does not queue behind a consult.**
  An `llm` cell is one task with one mailbox: a trivial memory question sent to an
  advisor core waited out whatever deep errand was in front of it, and twenty-odd
  seconds is not a lookup. `brain_fast` is that second mailbox — a second `llm`
  cell on the same collector, capped at 512 completion tokens where the thinking
  lane keeps 4096, with the first shipped `llm` system seed in the repository (one
  slot, `brevity`, deliberately not `instructions`, because a system path has one
  writer). The lane is picked by class, not by evidence: `context.consult_class`
  is set on the ingress edge — `ask_memory` is a `lookup`, `consult_cogny` is a
  `consult` — and the two ingress edges are complementary rather than overlapping,
  because a fan-out that matches both would answer twice. Both lanes hang off the
  **same** collector, window and memory bundle, and assembly happens before the
  edge decision, so a misclassification costs phrasing and never a fact. When the
  fast lane finds it does not have enough, it says so: `escalate_to_deep` is a
  reserved tool name inside the composite and its edge re-enters the composite's
  own ingress as a `consult`. Wiring only the first ingress edge is legal and
  gives the previous behaviour — without `consult_class`, every errand takes the
  thinking lane and `brain_fast` never sees a message (#124).
- **The front model can write memory in the same call it answers in.** Extraction
  used to be a nightly batch only, so the strongest thing the model knew about a
  turn — that it *was* worth remembering — was thrown away and rediscovered hours
  later by a cheaper model. The `remember` tool makes the inline path the
  strategic target and leaves the batch as the safety net. It is an async-class
  tool (declare it in `DISPATCHER_ASYNC_TOOLS`, or the round waits for a
  `tool_result` that never comes and dies at the idle window), and it takes
  **two** edges, both load-bearing: the forward edge from the dispatcher into the
  hive's `extract-glue`, and the **reject drain** back out on `hop.route ==
  'reject'`. The drain is not optional once the ingress is wired — without it a
  malformed block dead-letters instead of being reported, and the answer pays for
  it. The tool schema deliberately carries no episode id and no `valid_until`
  binding: no front model can know a uuid minted inside the hive, so the hive
  binds the block itself to the newest `user` episode of the session — `user` and
  not "newest", because the answer episode is written concurrently by the per-turn
  lane and "newest" would be a race. A block with no bindable turn is rejected
  with **zero** store writes and the turn stays `pending`, so the night batch
  picks it up unchanged: the inline path can fail without losing anything.
- **The collector curates its own context window, continuously and without a
  model.** A tool loop rebuilds its thread every round, so it grows monotonically;
  what existed against that was a cap (keeps a prefix) and eviction (keeps
  nothing). The curator keeps the *meaning* and drops the *bulk*, in three staged
  passes, at zero cost and zero latency — there is no provider call anywhere in
  it. It is off by default: `context_window` is a token budget and `0` means
  pre-curator behaviour. Above it, `curate_soft` (0.5) is the working mark and
  `curate_hard` (0.75) is reported rather than acted on, `keep_rounds` (2) leaves
  the newest iterations verbatim at any budget, and what may be elided at all is
  **declared, never guessed**: `recoverability` names each tool as `env`,
  `repeatable` or `unique`, and the default is `unique` — a payload nobody
  classified is a payload nobody touches. An elided result leaves a stub naming
  its size, its tool, its class, a content hash and the call that brings it back,
  and the stub survives the recall as a pointer, because deleting a `tool_result`
  row would orphan its `tool_call` and every provider rejects that turn. The
  trigger is a budget computed **in the cell** rather than a threshold on an edge
  — an exact partition over two CEL conditions is how a turn parks silently — and
  it prefers the provider's real `tokens_prompt` over a `chars/4` estimate that is
  deliberately a *lower* bound, so it fires late rather than early. Because
  nothing is paraphrased, invariance is structural rather than hoped for, and it
  is pinned that way: constraints, details and time markers survive every stage at
  four budgets, and curating an already curated window is a byte-identical
  fixpoint. The emergency fold lane that `curate_hard` is measured for does not
  exist yet — the mark is produced and reported, nothing consumes it.
- **`thread_recall`, the tool that brings an elided payload back.** The collector
  serves the call itself out of its own round table, the same shape the memory
  tool has had since #78, and the scope is this turn and only this turn — the past
  is `memory_recall`'s job. A recall over `thread_recall_budget` (0.2 of the
  window) is answered with a typed result naming the number: a wall, never a
  silent truncation. Switched off, the call is *answered* with a typed result too
  rather than parked, because a tool the model was offered and that never replies
  is a hung round.
- **The memory hive ships** (`memory-hive@1.2.0`, #137). The read and write path
  that has been carrying the private colony — extractor, dreamer, judge,
  dialectic, recall, the glue cells, the store and the nightly consolidation — is
  now part of the distribution, and the public catalogue grows to fourteen
  templates. The system instructions travel with it: the extraction, dream,
  judgement and recall prompts are in the shipped scripts, not held back, and so
  are the curated relation core and the embedding-model seed. Model names appear
  only as `${MODEL_*}` placeholders with no code default, so an unconfigured hive
  refuses to instantiate instead of quietly measuring a different model than the
  report claims. The price of publishing was paid in the fixtures rather than in
  the code: thirty-two test files came off the export blocklist, and the real
  people in them became generic ones first.
- **A hive can be sealed, in two independent ways, both opt-in** (#132, #133).
  Until now "internal" was a convention: any edge could address any cell at any
  depth, and a store inside a hive would serve a write from anywhere in the
  colony. `params.ports` on a hive marker names which of its **direct** children
  may be addressed from outside; an `add_edges` that crosses the boundary in
  either direction is rejected with the new `hive_port_boundary`, before anything
  destructive has happened. Presence of the key is the switch, an empty list is
  legal (pure transit), and everything inside the hive stays free at any depth.
  `store` gains `write_surface: "internal"`, which is a **runtime** refusal rather
  than a wiring one — a `select` and an `insert` travel the same edge, so a wiring
  rule could not tell them apart. Writes (`insert`, `update`, `delete`,
  `create_table`, the alias ops, and the params slot) from outside the owning hive
  are refused with the new `write_denied`, carrying the `tool_call_id` back; reads
  stay open from everywhere. The sender identity is stamped by the substrate, not
  claimed by the message, and a message with no `reply_to` counts as outside —
  fail-closed. Both seals are flipped on for the memory hive itself
  (`writer`, `recall`, `extract-glue` are its ports; its store is internal), which
  turned exactly zero existing tests red.
- **The extraction gate is tuned for freshness rather than for cost** (#51). The
  batch fired at 512 accumulated tokens or a thirty-minute-old item, which on a
  quiet conversation meant a fact stated now was extracted half an hour from now.
  The defaults are 128 tokens and 2 minutes; the item cap stays at 64. They are
  documented as recommended, not mandatory — the gate is a knob per deployment,
  and the README and both contract declarations now say so instead of implying a
  rule.
- **A claim gets a lease, so a live batch is not extracted twice** (#72). A
  claimed row carried no timestamp, so the recovery sweep handed *every* claimed
  row back regardless of age — including the one an extractor was working on at
  that moment. Measured on the private colony: 5,859 batched items for 3,839
  turns (1.53×), thirty reclaims where there should have been one, a third more
  provider calls and half again the wall time. `pending_extraction` grows a
  `claimed_at` column, the claim stamps it, and the sweep only reclaims what is
  older than `MEMORY_BATCH_CLAIM_LEASE_MIN` (5, chosen to outlast a full
  extraction cycle under the extractor's 180 s message timeout). Rows claimed
  before this change carry an empty stamp, sort under every cutoff and are treated
  as expired. No Rust changed — the comparison operator was already there.
- **A turn may state when it happened** (#135). `happened_at` is an optional slot
  on the UBF turn object. It belongs on the turn and not in the header because a
  header carries one time per message while a batch of replayed turns carries a
  different one per turn — which is what an import lane needs to say "this was
  said in January" while the store records "learned in March". The turn object
  stays closed (`additionalProperties: false`): this is one named slot, not an
  open door, and any other extra field is still a whole-body rejection.
- **Two examples that tell a problem instead of a feature.**
  `examples/never-forgets` is a colony that answers a question about January in
  March: six seed files, one `grow.json` of four nodes and ten edges, a replay
  lane that imports nine turns spread over three months, and the collector's
  `memory_recall` tool asking for a time range and getting one back with dates.
  It is honest about its own scope — it demonstrates the memory **port**, not the
  memory hive, and a window with nothing in it comes back empty rather than with
  the next best thing. `examples/hard-shell` is the other half of 0.8.0's
  hardening, written as an invitation: four files, three cells, and **no security
  configuration at all**, because the point is that the defaults hold. It fetches
  a cloud metadata address offline and gets `target_blocked` with no
  `http_status`, because no connection was ever made, and the README walks the
  root lease and the orphan reap from there.
- **A cost number anyone can reproduce.** `docs/costs.md` publishes what a running
  colony costs, and the harness that produced it travels with it:
  `scripts/cost_report.py` plus a dated price list carrying its source, its
  retrieval date and its exchange rate. The script reads `colony.db` strictly
  read-only, touches `message_log` and never a message body, and has two rules
  that make the number honest rather than flattering — a model it has no price for
  lands in an `unknown` bucket that is listed, warned about, and **left out of the
  sum** rather than guessed at, and the twenty-four-hour projection divides by the
  window that was asked for rather than by the span of the rows it happened to
  find, which on a quiet night is a factor of two. The published figures: USD 0.364
  per 24 h over a 27.27 h observation window (110 provider calls), and USD 0.028
  per 24 h for fifteen unattended hours overnight. Roughly 97 % of the bill comes
  from 24 % of the calls, all of them to the frontier model. What is *not*
  measured is marked as not measured.
- **A release is a binary and one line, not a build.** A tag matching `v*` now
  builds a static `x86_64-unknown-linux-musl` binary, refuses to publish it if the
  tag does not match the version the binary reports, smoke-tests it (`--help`,
  `--sandbox-probe`, `--validate` against a copy of `examples/hello`) and uploads
  a tarball plus a separate `.sha256`. `scripts/install.sh` is the other side:
  POSIX `sh`, one downloaded file, checksum verified **before** anything is
  unpacked, atomic move into `~/.local/bin`, no shell profile touched and no
  second `curl | sh`. It takes `MECLAW_VERSION`, `MECLAW_INSTALL_DIR` and
  `MECLAW_REPO`, reads the latest tag off the release redirect rather than the
  GitHub API (whose unauthenticated budget is per IP and therefore shared behind
  NAT), and falls back to the API only if that fails. The workflow can first run
  on a real tag push; the installer's branches were exercised offline under
  `dash`.
- **A stability statement, and its carve-out.** `README.md` § Stability names the
  four surfaces this project holds still: the HTTP API, the template DSL, the
  template port addresses, and the documented `error_code` strings. Inside 0.x
  they move additively; a break gets a Breaking section in this file with the
  migration named, and if it is not in that section it was not meant to break you.
  The counter-statement is just as explicit: every crate is `publish = false`,
  there is no SemVer promise on any Rust item, and a git dependency should pin a
  commit. The `${KNOB}` environment variables in the templates are named as an
  **experimental** surface that is migrating onto `params` across the 0.x line and
  is deliberately *not* covered (#138) — the same two sentences now stand in
  fourteen template READMEs.
- **`door@1` and `terminal@1`, and a seed with no cells in it.**
  `examples/meclaw-os` starts as a colony of zero cells and grows itself: one
  `grow.json` takes it to seventeen, a second declaration adds the advisor core
  and takes it to twenty-two. The door (an HTTP request onto a named lane, ten
  lines of Python) and the terminal were seed cells inside that example — public
  without being components. Moving them into the library is what makes them
  parts. `cogny@1` became public in the same pass.

### Changed

- **The template library lives at `templates/`.** It used to sit under `builder/`,
  which named its author rather than its content and put a library of shipped
  parts inside a tool. The tool kept the old role under a new name (`workshop/`,
  private); the library moved to the top level. The published cut is unchanged in
  kind: `templates/` is mixed, and what travels is an explicit allow-list, so a
  new template is private until somebody enters it there — the checked boundary is
  the list, never the directory name.
- **`--strict` is now `--validate-strict`.** The flag only ever modified
  `--validate`, and its bare name read like a global mode. It appeared in no flag
  table in any document, so it gets no alias and no deprecation window; it does
  have a documented row now. `--tokio-console` and its port are hidden from
  `--help` in the same pass — fully functional, but a debugging instrument rather
  than an operator knob.
- **Port addresses are declared contracts, in the READMEs that own them.** The
  addresses the examples wire literally — `./keeper/stamp`, `./collector/assemble`,
  `./split` and `./errors` on `talky`, `./assemble` on `collector`,
  `./collector/assemble` on `cogny`, `./screen` on `firewall`, `./drain` on
  `memory-drain` — are now written down as addresses rather than left as
  implementation detail that happens to be reachable. What sits behind them may be
  rearranged in a version bump; moving one of them is a Breaking entry and a major
  version.
- **Template pinning says what it will do.** `name@exact-version` requires an exact
  match, and a bump today *replaces* the directory. `templates/README.md` § Versioning
  now states both the current behaviour and the intent that superseded versions
  stay available from 0.9.0 on, so nobody infers a guarantee from a mechanism.
- **`GET /colony/templates?type=` actually filters.** It was parsed and ignored —
  a silent no-op that answered with the full template list, which reads exactly
  like "no other templates match". A caller relying on the old behaviour was
  relying on getting everything.
- **`GET /colony/events` slims its 501 body to `{"error": "deferred"}`**, the
  same envelope shape every other refusal uses.
- **The specification follows the code where the two had drifted apart.** No
  behaviour moved; the documents did. The restart section described the mailbox as
  it was before #18 — the truth is that the message in flight dies with its frame
  while the rest of the mailbox is rescued and delivered to the successor in order,
  and that the stateless dispatcher deliberately carries no such guard. `header` is
  documented as a reserved name that is **never** a body slot: it is cut out of
  every emission and merged into the envelope, so a payload parked there is lost
  silently. The `llm` cell's `error_code` list is declared **additive** rather than
  closed, which means an edge condition must not assume it has enumerated them all
  and needs a default lane. Mutation `error_code`s are stated to fall under the same
  stability promise as the dead-letter codes, and a mutation rejection is `422` and
  never `400` — `400` means unreadable, `422` means understood and refused. Message
  headers are documented as deliberately uncapped (#141): a limit would be a
  breaking change and is not planned. Two smaller ones: `/colony/dead_letters?since=`
  has been filtering since the day it was written and was wrongly listed as inert,
  and with `--api` the stdin/stdout bridge is never spawned at all, so EOF is not a
  shutdown trigger — the spec promised the opposite in two places.
- **The harness pins which refusal wins when two apply** (#46.3). The order is
  occupancy, then workspace, then tombstone, and the first refusal is the reported
  one — so a repeated `task_id` while a task is running answers `harness_busy`, not
  `invalid_input`. The order was never in doubt in the code and is now in the
  documents and in a test, because "which error do I get" is the kind of thing a
  caller writes a branch on.
- **The example colonies and `.env.example` stop papering over a missing key.**
  `swarm`, `hello` and `telegram-research` substituted a placeholder API key, so a
  fresh clone started cleanly and failed with a provider `401` on the first turn.
  They now reference `${OPENROUTER_API_KEY}` with no default: the daemon refuses to
  boot and names the variable. `.env.example` was cut down to the public tree —
  the local-model, builder and Slack blocks are gone, and the memory hive's
  required variables are in, because those are the ones whose absence rejects an
  instantiation outright.
- **The licence declarations agree with the licence.** `deny.toml` allowed
  `proprietary`, and two `template.json` files carried `"internal"` and an author of
  `builder`. Everything now says `MIT OR Apache-2.0` and names a real author. No
  dependency and no template changed — only what the files claimed about them.

### Tooling

- **The export learned to publish a hive, and to rebuild its own history.** Making
  the memory hive public meant proving the fixtures were clean rather than asserting
  it: the language gate grew a `DECLARED_DATA_BLOCKS` mechanism keyed by path *and*
  exact text block, which `make_export.py` imports rather than copies, so the two
  gates cannot drift into measuring different things. The private-tree marker moved
  from a template name onto a structural rule, the repository's own GitHub URL
  stopped counting as a name leak, and `--fresh-history` builds a parentless export
  commit for a history rebuild.
- **A community profile.** Code of Conduct, issue forms and a pull-request template,
  all in the export whitelist.
- **`cost_report.py` survives a non-numeric token count.** A string, a `null`, a
  list or a float where a token count belonged used to throw the whole report away
  as a traceback; those rows now land in the same `skipped` bucket everything else
  unusable lands in.
- **CI builds its tests without debug info.** The public suite grew enough that the
  runner ran out of disk.

### Fixed

- **A single slow query embedding no longer costs the `memory-hive` its whole
  semantic leg** (#146). Measured during a 50-question eval under ten parallel
  colonies: 3 of 30 questions fused three legs instead of four while their
  stores held hundreds of `ready` embedding rows. The corpus was fine — what
  failed was embedding the *query*, and the read lane had no retry, so one slow
  moment cost the most expensive leg of the fan. The timing is the whole story:
  the degraded questions spent 21.5–21.8 s in tier-1 against a median of 3.15 s,
  i.e. they walked into the 20 s bound, while a single call against the same
  provider took 0.26 s. CPU contention on the box, not a dead endpoint.

  The read lane now makes a bounded retry (`query_retries`, default `1`, with
  `query_retry_backoff_ms`, default `250`) and has its **own** timeout,
  separate from bulk corpus embedding: `query_timeout_ms` (default `30000`)
  against `timeout_ms` (default `20000`, the write lane's unchanged value). The
  two lanes pull in opposite directions — the write lane is throughput and its
  retry is the nightly backfill, the read lane is latency with an expensive
  failure — and one number could not serve both.

  **The fail-open contract survives the retry.** After the last attempt the lane
  still answers `vector: null, degraded: true` at exit code 0: silence from this
  cell hangs recall's fan-in forever, which is strictly worse than the degraded
  answer the retry exists to avoid. For the same reason the retry can never
  outlive the cell's own operation timeout — the script reads its own
  `external_timeout_ms` (raised `25000` → `65000` so the new worst case fits),
  keeps a 2 s reserve for spawn and the final write, and skips an attempt it
  cannot finish. `./embed` also declares `cell.message_timeout: 90000` now
  instead of taking the colony's 60 s default, so the B-backstop stays above the
  A-timeout. A test pins that arithmetic.

  Also loud, not just flagged: every failed attempt and the final give-up are
  stderr lines, and the reason travels — `recall`'s three-leg warn line from
  #144 used to say "no query vector" for every cause there is, because the
  `t1-qvec` hop dropped the embedder's `error` on the floor. It carries it now.

  **Migration:** the four knobs live on the `params` surface with **no**
  environment fallback (`collector@1.2.0` is the reference migration). A `.env`
  line for `MEMORY_EMBED_TIMEOUT_MS` is read by nothing — move the value into
  `./embed`'s `params.timeout_ms` before updating, or the cell falls back to the
  shipped default silently. `./embed` `contract.version` 1.0.0 → 1.1.0.
- **A freshly instantiated `memory-hive` can build its semantic leg again**
  (#144). Two defects on top of each other, and neither showed in production
  because both only bite a tree instantiated *after* the #85 default-deny cut.
  First: the hive's `embed` cell — the one cell whose entire job is an HTTPS
  call — declared no `params.sandbox`, so instantiation handed it
  `network: "deny"` like every other template cell. A fresh hive answered
  953/953 embedding calls with "endpoint unreachable" at `exit_code: 0`, the
  retrieval fan degraded to three legs, and nothing failed loudly. It declares
  the narrowest profile that can call out now: `trust: "restricted"`, the bare
  runtime set, `network: "allow"`.
  Second, underneath it: `network: "allow"` did not actually allow. Under
  `trust: "restricted"` the Landlock view was `/usr /lib /lib64 /bin /sbin /etc
  /proc /sys`, and while `/etc/resolv.conf` is inside it, on a systemd-resolved
  host that file is a symlink into `/run/systemd/resolve/` — which was in no set
  at all. Every lookup died in `getaddrinfo`. `allow` now grants the resolved
  target of `/etc/resolv.conf` read-only, and **only** under `allow`: a child in
  a fresh network namespace has nothing to resolve for. Behaviour-changing
  substrate fix with a regression lock
  (`crates/meclaw-cells/tests/gh144_network_allow_resolves_names.rs`).
  Third, smaller: the `recall` cell now writes one stderr line when it fuses
  without the semantic leg, so a dead embedder is greppable in `log.jsonl`
  instead of visible only to whoever reads `semantic_degraded` off the answer.
- **The orphan journal no longer records a child's name before the child has
  one** (#116). `note_spawn` read the child's `/proc` identity the instant
  `Command::spawn` returned — but the kernel releases the `CLONE_VFORK` parent
  *inside* `execve`, before it renames the task, so under load 3–7 % of spawns
  captured the pre-exec image and journalled the spawning **thread's** name
  (`tokio-runtime-w`) as the child's. The next boot then compared that against
  the real name, saw an "identity mismatch" it had manufactured itself, and
  refused to reap a genuine orphan. The spawn path now re-reads the identity —
  bounded, sleep-free — while the child still carries its spawner's name.
  The reaper is untouched: an unverifiable entry is still never killed.
- **A `code` cell that emits a non-object no longer takes its task down.** Two
  `expect()` calls sat on a trust boundary: a script writing `[1]` or
  `{"header": 5}` killed the cell task instead of being refused. It is an
  `invalid_json` rejection now, and the rejection stays total — a broken shape in
  the second message of a multi-send produces exactly one refusal and zero regular
  emissions, rather than half a batch.
- **The three `cargo deny` advisory findings from 0.8.0 are closed** (#127). All
  three were registered rather than suppressed at the time, and `deny.toml` still
  carries an empty `ignore`.
- **Two more tests that raced rather than waited.** The #116 retire-record test
  waits for the record instead of racing it (#134), green 224× under the load recipe
  that reproduced it, and the no-delete assertion (#129) compares under the
  collector's own read order rather than under `rowid`, which is what made it
  order-sensitive in the first place.

## [0.8.0] — 2026-08-14

The hard shell. Two waves in one day, thirteen tracks, and hardly a line of it
about new capability. This release is about the bad day: a daemon killed
mid-flight, a second daemon started on the same root, a redirect that walks into
a cloud metadata endpoint, a prompt slot that grows without a ceiling, a message
that dies of TTL inside a fan-in and takes the whole round down with it silently,
a tool loop whose window fills until nothing useful is left in it. The substrate
could already do the work; what it could not do was fail well. It can now — and
the four sharpest rules in the repository stopped being shell commands in a
document and became CI jobs.

A minor bump: new params on two cell types, a new `colony.json` switch, two new
boot steps, and one sanctioned default change — `web_fetch` now refuses private
addresses, which will break any topology that fetches from a host on its own
network until it opts in.

### Added

- **`web_fetch` refuses the private network, and the address it screened is the
  address it connects to.** The cell runs *in* the daemon process, so no
  sandbox can ever cover its egress — the policy has to live in the cell. It
  does now, as a deny matrix over the *resolved* address rather than the name:
  loopback, RFC 1918, CGNAT, link-local, ULA, multicast, reserved, plus every
  v4-in-v6 form (v4-mapped, v4-compatible, NAT64, 6to4) judged by the address
  embedded in them. Obfuscated literals (`http://2130706433/`, octal, hex) are
  normalised by the URL parser before the deny sees them, which is pinned rather
  than assumed, and every range boundary is pinned from the other side too — a
  deny that also blocks the open internet is an outage, not a hardening. The
  rebinding window between screening and connecting is closed by a custom
  resolver: reqwest only connects to addresses that resolver returns, and a name
  whose record set contains *any* private address is refused whole. The cell
  follows redirects itself (`Policy::none()` plus its own loop), because
  reqwest's policy hook is synchronous and cannot resolve a name — so a hop it
  followed would never face the deny again. Every hop is screened, the budget is
  a knob, a foreign scheme or an `https → http` downgrade is refused, and a
  refusal is a well-formed tool result carrying the call id, never a panic and
  never a dead letter. Three new error codes (`target_blocked`,
  `too_many_redirects`, `invalid_redirect`), a two-step opt-out
  (`allow_private_networks` opens the private ranges but never link-local — no
  one runs anything at `169.254.169.254` on purpose), and two new output headers
  (`redirects`, `final_url`) that appear only when a redirect actually happened
  (#117).
- **A daemon that is killed does not leave its children behind.** Per-spawn
  hygiene — process groups, `kill_on_drop`, the staged terminate — covers every
  path on which the daemon still runs code. It covers none on which it does not:
  SIGKILL, the OOM killer, a power cut. The only thing that can still reach an
  orphan afterwards is a record that was on disk *before* the crash, so that is
  what every spawn now writes: an fsynced JSONL line beside the child carrying
  its pid, its start identity from `/proc`, its `comm` and its owner, with an
  exit line appended on drop. Boot folds the file, verifies each survivor
  against its *living* identity and kills only what it positively recognises as
  its own orphan. Four refusals, each with its own test: a recycled pid (start
  identity differs), a `comm` mismatch, a survivor whose identity was never
  recorded, and an entry whose owning daemon is still alive — those are skipped
  loudly, never killed. No pattern matching, no `pkill`, no heuristic. The
  journal is never rewritten: exits are appended and made inert on read, because
  `{root}` is under the no-delete policy and a rewrite would itself be a crash
  window (#116).
- **One daemon per root, and the kernel decides.** Two daemons on the same root
  used to both boot, share the WAL and spawn a second copy of every cell with a
  second child process each; SQLite's busy timeout serialises writes and is no
  guard against a second colony. Boot now takes a lease: a candidate directory
  holding the holder record is `rename`d onto the lease path, and POSIX refuses
  to replace a non-empty directory — which is what makes "occupied" a single
  atomic kernel answer instead of a check-then-act window. The holder record is
  never believed, always verified against `/proc` by pid *and* start time, so a
  recycled pid cannot impersonate the holder. A live holder refuses the boot and
  names its pid; a dead, zombie or recycled one is reclaimed loudly by
  rename-then-delete, so only the reclaimer that won its rename deletes anything
  and never a fresh lease published in the meantime; anything undecidable fails
  closed. Release compares the token first, so a process wrongly declared dead
  cannot take its successor's lease down on the way out. The lease is taken
  strictly before the orphan reap (#121).
- **The `llm` cell gates what a message may write into its persistent `system`
  tree.** The tree is a prompt that survives restarts, and until now any message
  could put anything anywhere in it, at any size. Two independent halves: limits
  that are always on (`system_max_leaf_bytes`, default 65536, per leaf;
  `system_max_slots`, default 256, distinct slots in the tree) and an opt-in slot
  allowlist (`system_writable`, a list of path prefixes; empty means no allowlist
  and therefore every path, exactly as before). All three are immutable params —
  a message allowed to raise its own ceiling would not be gated at all. Path and
  size are checked before the transaction is even opened; the slot budget is
  checked inside it, because it needs the current state of the tree. A rejection
  is loud (a warning naming slot and rule) and all-or-nothing: the `messages[]`
  half of the same message rolls back with it, the provider is never reached, and
  the reply names the rule and the slot but never a leaf value. The seed path is
  configuration, not message traffic, so it stays ungated — a pinned cell still
  seeds its own identity even under a narrow allowlist, and no message can
  overwrite it afterwards. All nineteen existing writers in the repository were
  inventoried and shown to pass under the default (#118).
- **A TTL death can be answered, if the colony asks for it.** TTL expiry is
  terminal by spec: the message goes straight to the dead-letter queue and the
  reply cascade is deliberately skipped. Inside a fan-in that reads as a silent
  stall — the collector never completes, the origin waits out its own timeout,
  and the topology has nothing to react to. A terminal notice now closes that:
  the canonical substrate error reply (`ttl_expired`, plus the dead target and
  the dead message id for edge conditions), carrying the original `context` so a
  collector can correlate its round, sent from the virtual `/colony` address. It
  carries no `reply_to` of its own, which makes a cascade structurally
  impossible rather than merely marked. It is opt-in
  (`colony.json` `ttl_notice`, default `false`) for the same reason `restore_ttl`
  is: the notice carries a fresh TTL budget, so switching it on means the colony
  has taken its loops out of the TTL bound and limits them by iteration count
  instead. Both frozen corridors are untouched — the notice is built at the five
  dispatch call sites, not inside `route()` (#119).
- **The `llm` cell says where its time goes.** One INFO line per provider call
  on its own target, splitting a `handle()` into persist, translate, wire and an
  unaccounted remainder, with time-to-first-byte taken at the response head and
  the total after the body is drained, attempts folded for the refresh retry, and
  the summary emitted *after* the send so a backed-up colony shows up as the
  remainder rather than as nothing at all. A DEBUG line adds request sizes and
  counts — never conversation content — and only allocates when DEBUG is on.
  Both lanes are instrumented. Nothing about the message model changed: these are
  tracing fields, not a new slot. The first thing it settled is that the chat
  lane has no retry ladder and makes exactly one POST per message, so a slow
  round trip has to be measured against the message log to tell "inside the cell"
  from "in front of it" (#124).
- **A context-compaction lane, as a reference topology.** The store-backed tool
  loop rebuilds its thread from the store every round, so it grows monotonically:
  round six carries round one's tool result for the sixth time. What existed
  against that was capping and eviction — a cap keeps a prefix, an eviction keeps
  nothing. Condensation was the one memory ability the loop lacked. An edge now
  accumulates prompt tokens into the context, the collector's fire lane is
  partitioned in two by a threshold, and the branch over it runs a small hive
  that groups the rounds, places the cut, condenses the prose and writes one
  summary row; the collector then rebuilds compaction-aware — user turn, newest
  summary, every round behind its boundary — while the folded rows stay exactly
  where they were. No new cell type, no substrate, not one line of Rust: three
  existing cells, a hive marker and six edges, shipped beside the existing
  tool-loop fixture with its own walkthrough (#120).

### Changed

- **`web_fetch` refuses private addresses by default.** This breaks any topology
  that points it at a service on its own network — a local mock, a sidecar, a
  documentation server. The repair is the documented opt-out
  (`allow_private_networks: true` in `params`), never a weaker default: the nine
  in-tree tests that fetch from localhost were repaired that way, one by one, and
  a test pins that the default still refuses what they opt into. Link-local stays
  closed under the opt-out (#117).
- **`reasoning` and `reasoning_effort` are ordinary `llm` params.** They pass
  through to the chat-completions body: the short form becomes
  `{"effort": …}`, the object form travels verbatim, the object wins when both
  are set (no merge of two provider blocks), and `provider_extra` still overlays
  everything. Unset means the key is absent, byte-identical to before. Mutable on
  purpose — a thinking budget is a knob, not an identity (#124).

### Tooling

- **The four sharpest rules in the repository now run themselves** (#115). The
  two byte-frozen routing corridors are diffed by a script in CI against
  reference bodies that travel with the export, so a corridor cannot drift
  unnoticed in either tree; a private drift lock keeps those copies byte-honest
  against their originals and pins the extraction rule itself, so the two gates
  cannot quietly start measuring different things. `unwrap`/`expect` in library
  and binary targets became a **ratchet** rather than a ban: a per-package pin
  measured from clippy's JSON output, growing is red, shrinking is green with a
  note to re-pin. Rewriting the existing call sites is a change to load-bearing
  code, not a CI task — but new code cannot add to them. `cargo deny` is split:
  bans, licenses and sources block, because that verdict depends only on the
  lockfile and the policy in this commit, while advisories report and run
  weekly, because that database moves underneath a pull request that changed
  nothing. `deny.toml` gained no `ignore` entry; the file asks for a written
  reason for every suppression, and a finding that is real is registered as an
  issue instead (#127).
- **The export learned two more gates.** R2c refuses an exported test that reads
  `plans/` at runtime — `plans/` has no public subset at all, so such a reference
  is necessarily dead, the same defect class R2b covers for `builder/`. R10
  checks every relative markdown link against the *export* index rather than the
  working tree, which is where the two known dead links in the template READMEs
  had been hiding — along with five nobody had noticed, an entire class caused by
  the export renaming `docs/X.en.md` to `docs/X.md`. Seven fixed, and the gate
  was proved from both sides: red on the commit before the fix, naming exactly
  those seven, green after (#126).
- **The drain's colony-level test goes public, and the memory hive still does
  not.** The test measured against the shipped hive writer, which is why it sat
  on the export blocklist. It is now pinned to a minimal fixture snapshot
  instead: the writer and the one writer-to-store edge byte for byte, the store
  **projected onto the episodes surface only**. A leak guard ran before any code
  was written and blocked the full store config — predicate canon and dream
  machinery — and blocked the shipped writer description as well, so the
  snapshots carry their own. A private drift gate holds all three against the
  living template and, as a ceiling, pins that the public projection publishes
  nothing beyond `episodes`, so a later refresh cannot widen it by copy-paste
  (#125).
- **The private memory hive's recall path got a query hygiene guard.** It is not
  part of this distribution and changes nothing that ships; it is recorded here
  because the issue is public. A caller query longer than a threshold is clamped
  to its last question, its last sentence, or its tail, and the keyword leg now
  keeps the *tail* tokens instead of the head — in the usual contamination shape,
  where a prompt fragment precedes the actual question, the head cap kept exactly
  the wrong half. The verdict is loud rather than a rejection, because the
  session-boot request carries no query at all and must still answer. A healthy
  query leaves the result byte-identical (#88).

### Fixed

- **Two load-sensitive test flakes, both hardened at the premise rather than
  waived.** The collector's idle-window sweep (#114) failed a third of the time
  under parallel load for two reasons, both the same class: a wall-clock second
  of silence was an ambiguous synchronisation point — silence also means the
  round has not opened yet — and the 300 ms window was simultaneously the
  deadline of the test's own chain, so under load the round closed *itself* and
  the sweep proved nothing. The three cases now wait on a positive receipt, the
  collector's own slate, and time the occasion from that observation; the window
  is documented at 2000 ms with the reasoning in the test. Zero red in 672
  executions under the load recipe that used to fail 48 out of 48. The long-
  running respawn re-notify (#128) had a premise that was simply untrue: the
  cell task is spawned *before* the re-notify fires, and the I/O sub-task
  announces its liveness on entry through the same inbox, so on a multi-threaded
  runtime it can arrive first. All three siblings shared the helper and all three
  flaked; fixed together, 192 executions green under load.

### Known

- **#127: `cargo deny check advisories` is red on this tree**, which is why the
  advisory job reports instead of blocking — and why it reports rather than
  blocks is exactly this: the finding count moved between the wave and the
  release without a single commit in between. Three at release time: an
  unmaintained proc-macro crate reachable through the templating dependency with
  no safe upgrade available, a yanked transitive crate, and RUSTSEC-2026-0190
  (an unsoundness in `anyhow`'s `downcast_mut` after `context`, corrected
  upstream). Every repair moves the lockfile and therefore the build surface, so
  none of them is a release-day change. Registered rather than suppressed;
  `deny.toml` still carries an empty `ignore`.
- **#124 and #88 stay open** on purpose: both shipped the measurement, neither
  has its production number yet. The latency line has to be read next to the
  message log on a real deployment before any knob is turned, and the hygiene
  guard is robustness, not a lift — what closes it is a real contaminated query
  counted through its verdict.

## [0.7.0] — 2026-08-14

The advisor. Two tracks, both pure topology: an agent may now hand a question to
a second agent that thinks slowly, answer its channel in the same breath, and
deliver the advice later on a lane of its own — and a day that a conversation
has closed finally reaches memory instead of stopping at a format gap. Not one
line of Rust changed in either; six template files and one new template did all
of it, which is the point the substrate has been built toward.

A minor bump rather than a patch: a new template, a new collector lane, a new
store column, and one sanctioned behaviour change in the dispatcher's fan-out.

### Added

- **`memory-drain@1` carries a closed day into memory.** A collector hands its
  session out as ONE write batch — the whole day in `messages[]` — while a
  memory hive's writer takes one turn at a time. Both forms were right and
  nothing spoke both, so a closed day never arrived. The adapter is that
  translation and it lives *outside* the memory hive on purpose: it speaks only
  the documented `turn-write` port, so no memory internals move and no second
  write path gets invented. Two cells (a `code` phase machine and a `store`
  ledger), one internal edge, two ports. It is **lossless** — every text turn
  becomes exactly one episode, in the order of the day, nothing judged, merged,
  capped or dropped — and it is **idempotent**: the identity an episode travels
  under is minted deterministically as `"<session_id>#<index>"`, read back out
  of the ledger before anything fires, and an already drained turn is skipped
  rather than written twice. The same batch delivered twice moves no row; a
  session that has grown is drained from where it stopped. Both gates are
  measured against the shipped writer in a running colony, not against a stand-in
  (#101).
- **A tool can answer on a lane of its own, and nothing waits for it.** Tools
  named in `DISPATCHER_ASYNC_TOOLS` are classified by the only cell that ever
  sees the whole bundle, and the dispatcher tells the fan-in which
  `tool_call_id`s it must not expect (`hop.async_calls`). The collector
  acknowledges each of them on the spot with a real `tool_result` under its own
  id — the assistant turn stays well-formed for every provider — and if nothing
  else was asked, the round is closed immediately. There is then no open round
  to win, nothing for an idle sweep to find, and no timeout racing a slow
  thinker: the expectation that could expire no longer exists. A mixed bundle
  stays open and closes on its synchronous half (#28).
- **`in_advice`, the return lane.** A late answer comes back as a fresh round on
  the collector's tenth entry lane: stored under its own `advice` role, run
  through the memory leg, the gate and the seam, so the result is verbalised for
  the channel it lands in rather than pasted into it. The turn that asked ended
  long ago with the interim answer and cannot re-enter. The link is bilateral —
  question and answer share `context.consult_id`, and the ids of the
  consultations still in the window are offered to the model as `system.consult`
  so it can answer one by naming it. Additive at the store: `turns` grows a
  `consult_id` column into an existing database (#28).
- **The consult carries an ETA, observe-only.** The dispatcher reads
  `arguments.eta` off a consult call and puts it on the edge as
  `hop.consult_eta`. Nothing consumes it: it travels and it is logged, and the
  guidance for a model that should set it sensibly is a prompt building block in
  the `talky@1` and `dispatcher@1` READMEs, not topology. A measurement first,
  and a lever only once there is something to measure (#123).

### Changed

- **The dispatcher serves `content` and `tool_calls` from the same response.**
  A sentence next to a bundle used to be the odd case; now it is the rule. The
  sentence leaves the cell at once on the `answer` lane marked
  `hop.interim = "1"` while the calls run on, so a channel can say "one moment"
  and mean it. The order the fan-out keeps is unchanged — calls first, because
  they are the expectation set, then the interim answer, then the calls
  themselves. The interim answer travels exactly once: it is already an
  assistant turn in the window, and repeating it inside the tool round would put
  it between an assistant turn and the `tool_result`s answering it, which every
  provider refuses (#28).

### Tooling

- **`memory-drain@1` joins the export allow-list.** Ten templates are entered
  now. The adapter ships; the memory hive it feeds stays private, which is the
  whole reason the adapter is a separate template rather than a hive change. Its
  script-level test travels with it, and the colony-level test that deliberately
  measures against the *shipped* hive writer is registered in the blocklist with
  its reason, as R2b asks: no exported test may read what the subset lacks
  (#101).

### Known

- #114 is still open: `collector_colony`'s idle-window sweep, a deliberately
  tight 300 ms semantic discriminator, went red once more under parallel load
  during this wave and was re-verified green in isolation. The async tool class
  above removes the *product* reason to wait on a slow tool; it does not make
  the discriminator any less tight.

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
