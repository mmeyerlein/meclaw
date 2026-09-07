# templates/

Reference topologies you can instantiate. Every one of them is pure DSL: directories,
`config.json` files and edges, with no new Rust and no plugin API anywhere in them. A booted
colony does not depend on this folder at all.

## What a template is

A template is a physical subtree on disk. At its root sits a `template.json` (name, version and
the four-slot description the colony serves over `/colony/templates`), then one `config.json`
per cell, with sub-cells as nested directories. That is the whole format, and what you read
here is what the substrate reads.

Instantiation copies. When a mutation names a template, the colony materializes the subtree
into your colony root, substituting `${uuid7:*}` on disk and keeping `${VAR}` as a late-binding
token. From that moment your instance is yours and has no link back to this library. A booted
colony never reads this folder: delete it and every colony that was built from it keeps
running.

A template is self-contained. It has no edges leaving its own subtree, and wiring it into a
colony is the job of the mutation that instantiates it.

## The hive boundary

An edge is laid at the hive. An edge asks for something by content, without knowing the
structure. The inner edge that receives that request is what knows what to do about it.

The rule, its three requirements and the reasoning behind them are in
[`../docs/meclaw-overview.md`](../docs/meclaw-overview.md) § The hive boundary, together with
the table of which templates have arrived. It binds every hive and every template: a hive that
declares no `params.ports` is one the substrate does not enforce it on, and such a hive is
unfinished. Ruled 2026-08-18
([#197](https://github.com/mmeyerlein/meclaw/issues/197),
[#200](https://github.com/mmeyerlein/meclaw/issues/200)).

### Authoring a hive template

Five things, all checkable, all in the template's own files.

1. `params.ports: []` in the hive marker's `config.json`. The empty list is the statement "the
   hive path is the only address". No `ports` key at all means unsealed, which means
   unfinished.
2. A door per accepted lane: an edge with `"from": "."`, a `condition` testing the lane, and a
   `to` naming the inner cell that serves it. This is the only place the structure of the
   inside may be known.
3. A `params.contract` with `accepts` and `emits`, and lane names that say what the caller
   wants, never where it lands inside. `writer`, `recall`, `render`, `policy` are inner cell
   names; renaming a port into a lane of the same name satisfies the letter of rule 1 and
   misses its point. The test: does the name survive a reimplementation of the inside?
4. No address in your prose that the boundary would refuse. `template.json` and the README
   describe lanes. The `description` slots are the interface a caller reads, and a
   `from:`/`to:` in them is a wiring instruction, which is why
   [#203](https://github.com/mmeyerlein/meclaw/issues/203) was a defect and not a typo. The
   gate is `crates/meclaw-cells/tests/gh203_documented_port_addresses.rs`, and it asks the real
   boundary validator.
5. No context key you set on one of your own edges survives an exit edge. A hive has no
   `cell.db`, so a cell that has to remember which round trip an answer belongs to promotes a
   marker to `context` on an interior edge: `<name>_origin`, plus a phase and a carry. Context
   is persistent for the life of a chain and nothing removes those keys again, so every edge
   with `"to": "."` clears the hive's own keys with `delete_context`. Otherwise the marker rides
   out on the answer, the next request re-enters wearing it, and the cell that reads it before
   it reads the inbound lane dispatches a stranger as its own echo
   ([#481](https://github.com/mmeyerlein/meclaw/issues/481),
   [#490](https://github.com/mmeyerlein/meclaw/issues/490),
   [#494](https://github.com/mmeyerlein/meclaw/issues/494)). Two exemptions, both named in the
   gate: the context vocabulary the templates are meant to pass between hives (`session_id`,
   `channel`, `turn_id`), and a key a round trip out of the hive has to bring back because the
   answer re-enters through a door that does not re-establish it. The gate is
   `crates/meclaw-cells/tests/gh494_no_interior_marker_leaves_a_hive.rs`, which carries both
   lists; `workshop/tools/hive_context_sweep.py` is the same sweep on the command line.

### Wiring one in

An edge from outside points at the hive path and carries its request on `hop.route`:

```json
{"from": "./ingress", "to": "./agent",
 "modifier": {"set_hop": {"route": "'in_turn'"}}}
```

Which lanes a hive accepts and emits is in its `params.contract`. If you find yourself needing
a segment after the hive name, what you want is a lane whose name says it, plus a door inside
the hive that knows where that belongs.

### Writing a cell a tool round will call

A tool cell answers on the collector's `in_tool` lane, and what it may hand back is its
`messages[]`: one `tool_result` turn per call it answers, each carrying the `id` of that call.
All of them travel, so a tool that got a bundle of calls in one message may answer the whole
bundle in one message.

Nothing else travels: a `system` slot or a top-level body slot on that lane is dropped.
`system.*` reaching an `llm` cell is upserted into that cell's own `cell.db` and stands in the
prompt until something overwrites the same slot path, which makes it durable state of the agent
instead of evidence of one round. A tool with structure to return serialises it into the text
of its result, and a producer that means to install a lasting constraint addresses the `llm`
cell's `system` tree on a lane of its own. The long version, with the reasoning and the worked
example, is
[`collector/README.md` § What a tool result may carry](collector/README.md).

## The library

The table below is the published library, and a working tree may carry more than it lists.
Every template the export ships has a row here, and that is a gate:
`crates/meclaw-cells/tests/gh235_every_public_template_has_a_row.rs` compares the rows against
the export's own allow-list, and its twin `gh235_readme_library_table.rs` compares each row's
version against that template's `template.json`. A development tree can hold further
directories under `templates/` that the export does not ship; they carry no compatibility
promise, nothing here references them, and no row depends on one.

A row says what the template is, what it needs and what it hands back. The template's own
README, linked from the name in the first column, carries the version history, the lane
contract and the reasoning.

| Template | Version | What it is |
|---|---|---|
| [`access`](access/) | 2.5.0 | The capability broker as one sealed hive of six cells and no model, so every verdict is a comparison and a request costs two store hops. An agent asks in natural language, a handle travels on the wire, and a credential leaves the vault beside it only sealed, under a key the requester minted for that one call. A request may also be check-only, which returns a verdict and no grant. Seven policy rows ship seeded and five of them disabled; what ships enabled is the pair a colony cannot start without. Every behaviour knob is a param, and the passphrase stays an environment variable this hive only names. |
| [`affinity`](affinity/) | 3.3.0 | The curated record of the people and agents a colony knows: whole AIeOS documents in a store, plus the four things that standard has no vocabulary for, which are relations, trust, disclosure and an append-only audit. One writer, one reader, no model, and the audience filter lives in exactly one place. The record travels: one lane walks all nine tables out as a versioned document, another takes a part back into a running hive, so a member reborn from an export knows who its people are. Every knob is a param, and this hive reads no `.env` at all. |
| [`archive-bridge`](archive-bridge/) | 1.1.0 | Turns an llm's last answer into a store-native insert and swallows the store's reply echo, so an append-only archive costs one cell and no loop. The target table is a param of the cell, so two bridges in one colony archive into different tables. |
| [`argus`](argus/) | 1.1.0 | The colony's control loop and its watcher, which is the same thing seen from both ends: a charter of goals as rows, a deterministic measurement, a judge that simulates before it decides, a params update to the cell it names, and keep-or-revert after the window. Every tick leaves a receipt, including the tick that had nothing to do and the cycle that died at a store reject. It ships with every goal disabled, so a freshly grown `argus` measures nothing and changes nothing until an operator turns a charter row on. Renamed from `steward` in [#462](https://github.com/mmeyerlein/meclaw/issues/462). |
| [`assistant`](assistant/) | 2.6.0 | One generation of one person's agent, as a composition level: the conversation surface (`talky`), the reasoning core (`cogny`) and the tool surface (`tools`). Three refs and no container of its own. It is the only place that can decide which model each of its two brains infers with, because it is the only place that references both, and it routes the memory road, the tool road, the sidecar sections and the transfer lanes of its surface's session keeper. It keeps no memory, no screen and no channel, because those belong to the member, and it is complete at birth with no per-channel follow-up mutation. |
| [`builder`](builder/) | 1.7.4 | The intake that turns a structural wish into a manifest: an ordered list of mutation declarations somebody else submits. Two classes on one lane, a fast lane of parameterised recipes that calls no model, and a design lane that consults `builder-librarian`. It applies nothing, and that is a property of the files: no cell in it has an edge onto the control plane. Its `grow_level` recipe renders the transit edge set a new organisation, member, assistant, channel, screen or app gets from its parent, and the doors of a container level carry `context.org` or `context.member` beside the lane, so a second member of one organisation is a second address. |
| [`builder-librarian`](builder-librarian/) | 2.2.0 | Lexical retrieval over the builder's knowledge base, in one sealed hive: a store holding the corpus in an FTS5 table, a retriever that turns a request into a BM25 query and the rows back into a briefing, and a catalogue cell that reconciles the corpus against the colony's own template registry. No embeddings and no model. A catalogue row carries `CONTRACT --`, `STORES --` and `PARAMS --`, the three demands the mutation door enforces. A store failure comes back marked `degraded`, so a corpus outage cannot hang a build. Its seed is a build product, generated from the public sources alone. |
| [`canvy`](canvy/) | 2.2.0 | One interactive canvas of the colony on a port of its own: a timer takes a topology snapshot, a `code` cell turns it into display objects, and a `web` cell serves the page. Deprecated since [#455](https://github.com/mmeyerlein/meclaw/issues/455), which withdraws it: this template fuses a screen, which belongs to a person, with a view of the colony, which is one application among many. Those two are `display` and `colony-view` in the rows below. An instance grown from `canvy` keeps running, because instantiation copies, and the template takes no further work. |
| [`clock`](clock/) | 1.0.1 | A periodic tick from one `timer` cell that carries one schedule and decides nothing. Seven shipped templates carry a timer inside them and none of them was instantiable on its own: an `add_nodes` entry needs a name and a template, there is no form for a bare cell, and so no manifest could give a running colony a periodic tick. |
| [`cogny`](cogny/) | 5.0.1 | One hive holding the agent core: a `collector` and a `dispatcher`, one `llm` brain, and a `schemas` cell that hands out the schema of the errand this core takes. One brain, one lane, one class of question: synthesis, a development over time, multi-step work, research. A fast memory lookup belongs to the conversation surface, which already holds the window and answers it in one tool call. No new cell type, no Rust. |
| [`collector`](collector/) | 4.1.0 | Context assembly in a hive of two cells: `assemble`, the state machine, and `window`, the store. It decides in one place what enters an agent's context window and what leaves it, then hands the result to the brain over one message on one route. It keeps a rolling short-term window, folds each tool round back in, and asks for its tool menu on the mutation receipt the level above carries in, so there is no timer in it. What a tool result may hand back is its `messages[]` and nothing else. |
| [`colony-view`](colony-view/) | 1.1.0 | One app that draws the colony: a committed mutation triggers a topology snapshot, a `code` cell turns it into one component tree, and the view leaves the hive towards a display. The browser owns the drag and where you are looking. It asks no timer for a change the mutation door announces by itself. |
| [`daily-digest`](daily-digest/) | 2.1.0 | Scheduled fetch-and-forward inside one hive. A timer fires a schedule, a `web_fetch` reads the URL, a `code` cell formats the result and a proxy delivers it. A caller can also ask for a digest out of turn on a lane of its own, and the answer goes back to whichever of the two the run came from. |
| [`dispatcher`](dispatcher/) | 1.2.0 | The fan-out half of a tool loop, in one `code` cell. A brain answers either with something final or with a bundle of tool calls, and this cell turns that bundle into messages a graph can route. Its counterpart is the fan-in, `collector`, which assembles the round and re-enters the brain. Routing a call and assembling a context window are different jobs, which is why they are two cells. |
| [`display`](display/) | 1.1.0 | One screen on a port of its own that many agents and applications write onto at the same time. A view is a named, owned, optionally expiring piece of that screen: whoever sends one owns it, replaces it under the same name and takes it down again, and nobody who writes to it needs to know that anybody else does. Two cells, `compose` and a store of what is up, sit in front of a `web` cell that serves the page. |
| [`door`](door/) | 1.0.2 | The first cell of a colony: a `code` cell that names the first lane. The HTTP ingress puts the request headers into `context` and leaves `hop` empty unless the poster seeds one, `set_hop` is an edge's job, and above the first cell there is no edge. Ten lines of Python that emit once and keep no state. |
| [`fetcher`](fetcher/) | 1.0.0 | One `web_fetch` cell for an outbound HTTP GET, with five knobs and no target. A `web_fetch` sits inside three shipped templates, and none of those was instantiable on its own, so no manifest could give a running colony the ability to read a document off the network. |
| [`firewall`](firewall/) | 2.3.0 | Deterministic screening on an ingress channel, drawn as topology: one `code` cell and one store of rules between the surface and the agent. Every inbound turn is measured and ends on exactly one of two lanes, `pass` with the body byte-identical or `reject` naming the reason and the rule that fired. A `hold` row parks a turn in a pile until a person answers, and then it ends on one of the same two lanes. Above every row stands the hardline: consulted first, able to say only reject, and unreachable by any update an operator or an attacker could write. |
| [`freeswitch`](freeswitch/) | 1.0.1 | A telephone becomes one channel of a person, in two halves inside one hive. The media half is a `voice` cell that binds a WebSocket port and puts ordinary text turns on the topology; the signalling half is a small state machine of a `code` cell that offers `call` and `hangup`, one that keeps the book, a `web_fetch` to the switch and a store of the calls. They live in one hive because one id binds both halves: the channel UUID of the switch is also the session an answer is spoken back into. FreeSWITCH stays the media edge in gateway mode; nothing here speaks SIP. It succeeds `phone@1.0.0`, a name that no longer exists, and the migration is in its own README. |
| [`meclaw-os`](meclaw-os/) | 1.8.5 | The colony shell: the outermost of the four composition levels and the tree everything else is grown into. It holds four occupants, one empty container and the transit graph between them, and no cell of its own; its whole job is to be the boundary those things share. The submitter is an occupant of the front door, so a submission lives in one place and the road can be read off the graph. The drafter and the submitter are still two nodes that share no edge, and `/colony/mutations` is still an endpoint no mutation may draw at any scope (ADR-0015). |
| [`member`](member/) | 1.7.0 | One person, as a composition level: the memory (`memory-hive`), the curated record (`affinity`), the screen (`firewall`) and the keys (`access`), plus three open containers for that person's assistants, channels and apps. Two assistants of one person must know the same person, meet one attacker and be reachable on the same bot, which is why all four holders stand here and not one level down. This level stamps the round a recall is asked in, fires the memory's close pass when a session ends, and sorts a screen's events and receipts by the owner the display wrote onto them. It owns no cell of its own. |
| [`memory-drain`](memory-drain/) | 2.0.6 | The adapter between a write batch and the central memory, for bulk import of foreign history: a benchmark haystack, an exported transcript from another agent, a month of chat out of an archive. Nothing on a live path, and nothing shipped wires it (ADR-0012). |
| [`memory-hive`](memory-hive/) | 3.4.0 | A member's memory: a hive of fifteen cells, all of existing types. Every turn becomes an append-only episode row, written without a model, so the agent never waits. Every durable row records who was present when it was learned, and the read path answers only with rows the current round could have heard, fail-closed on both sides. A close pass reads a finished session whole, a dreamer consolidates, a judge decides what survives, and the hive answers `memory_recall` as an ordinary tool whose schema it publishes itself. |
| [`operator`](operator/) | 1.2.0 | One front door into the OS and one place a submission lives: a sealed hive at the colony shell with one occupant per subject, reached by naming a lane and never a cell. It decides nothing and remembers exactly one thing, a draft nobody has said yes to yet. The submitter is one of its occupants; the drafter and the submitter are two nodes that share no edge, and the reach onto the mutation door is birth topology or nothing. |
| [`org`](org/) | 1.4.1 | The namespace, and nothing else: one hive with one open container and nothing but transit edges. The members of one organisation share a name and a boundary. They do not share a memory, an identity, a broker or a firewall, so this level owns the name and the boundary and stops there. |
| [`receptionist`](receptionist/) | 2.1.0 | One agent per channel, built the moment a channel first speaks. Two cells under one hive: `greet`, a `code` cell, and `ledger`, a store of the channels already met. The first turn from an unknown channel submits one mutation carrying the `add_nodes` and all four crossing port edges, so a shared communicator stops mixing the windows of many chats into one. |
| [`retry`](retry/) | 1.0.0 | A bounded retry loop around one tool, in one `code` cell that counts nothing. The attempt counter is edge authority: the wiring increments it, the wiring bounds it, and the cell only decides what travels on which lane. The call parks itself in `context`, which survives the tool hop, so when the error reply comes back the cell rebuilds the original call from it. |
| [`scriptlet`](scriptlet/) | 1.0.1 | One `code` cell, shipped blank, for a script. Thirty-two shipped templates carry a `code` cell inside them and every one of those scripts has a purpose of its own, so there was no plain one for an `add_nodes` entry to name and a composer that needed application logic between two cells had nothing to reach for. Its `script_inline` is what an `override_params` names. |
| [`session-keeper`](session-keeper/) | 2.2.0 | A session lifecycle in a hive of five cells: `stamp` in the ingress path, `close` for the night, `sessions` holding the whole state, a timer, and a porter that carries the ledger to another keeper and back. A session is a channel generation and the model is a phone call: it begins on a channel with a turn, it ends on that channel, and the context window, the episode a memory hive is handed and the answer to what we talked about this morning all hang off one id. |
| [`shelf`](shelf/) | 1.0.2 | One `store` cell with one table and no opinion about what goes in it: a place to put rows. Eighteen shipped templates carry a store, and not one of them could be instantiated alone, so a running colony had nowhere to keep what it produced. |
| [`steward`](steward/) | 2.0.13 | The colony's control loop in a hive of seven cells: a charter of goals, rules and thresholds as rows, a deterministic meter, an `llm` judge, and the apply-and-revert path around them. Deprecated since [#462](https://github.com/mmeyerlein/meclaw/issues/462) and renamed: a new control loop is an `argus`. Instantiation copies, so an instance grown from `steward` keeps running; the template itself is done. |
| [`submit`](submit/) | 2.3.1 | Two occupants behind one door, and the only reach onto the mutation door in the whole tree. It asks who may submit and, when the diff itself asks for it, whether this manifest may author code and whether this identity may open its own push lane. The broker answers all three. What `submit` decides is exactly one thing, the form of a subscribe edge, because that is the half no policy row can state. `gate` is the whole of the logic and `store` the whole of the memory, and the store has no address of its own. |
| [`summarizer`](summarizer/) | 2.1.0 | The session handover step, a hive of two cells: `prep`, the glue, and `writer`, an `llm` that writes the prose. When a generation closes, its successor should wake up with yesterday. `talky` no longer carries this step, because the handover now comes out of the member's memory recall bundle; the template stays in the library for a tree that wants the step on its own. |
| [`talky`](talky/) | 5.1.0 | One template holding a whole conversational agent: `session-keeper`, `collector` and `dispatcher` as referenced units under one hive, plus an `llm` brain, the sidecar splitter and one error collector. Every internal edge is pre-wired, so instantiating it is one `add_nodes` plus the four port edges the parent draws anyway. It implements `thread_recall` out of a slate no other cell may read, takes a pack of durable `system.*` slots on a lane of its own and answers each one, and passes its keeper's transfer lanes straight through. |
| [`telegram-connector`](telegram-connector/) | 2.0.1 | A Telegram chat behind one cell: one `proxy`, one credential, one wire in and one wire out. No persona, no `llm` cell and no answer of its own; it carries turns between a chat and whatever you put behind it. Up to 1.0.0 it was a sealed hive around that one cell, which is not a level, so the hive is gone and the cell moved up. A caller that wired the old hive path rewires to the cell. |
| [`terminal`](terminal/) | 1.0.1 | The last cell of a lane: a `code` cell that accepts anything and emits nothing. A lane with no destination dead-letters in this substrate, and a dead letter is an alarm, so a terminal turns an undecided destination into a documented stop. Until 1.0.0 it was also offered for a `reject` or an `error` lane, and that offer is withdrawn ([#284](https://github.com/mmeyerlein/meclaw/issues/284)): it logs nothing and alarms nobody, and a refusal in the dead-letter queue is a signal about the topology. |
| [`tools`](tools/) | 1.4.2 | The tool surface of one assistant, one node with one contract: `tool_call` in, `tool_result` out. It answers a second question, and that one is about itself: a cell that uses tools names them, and the hive hands back their schemas. A surface that decides which cell serves a call and never says which calls there are to make is a contract with a hole in it. |
| [`vault`](vault/) | 1.3.0 | A secret store in one cell, with no operation that returns a secret. `put` and `rotate` come from the user channel, `use` signs on the broker's behalf so the secret does the work and stays home, and `deliver` hands the connector case a ciphertext sealed to a key that lives in one task's memory and dies with it. `get` is refused the way an unknown op is, because the absence of a read is structural and cannot be argued into an exception later. |
| [`voice`](voice/) | 1.3.0 | A spoken conversation behind one cell: one WebSocket surface, one pair of provider credentials, one wire up and one wire down. It has no persona, no memory and no answer of its own, and it carries turns between somebody talking and whatever you put behind it. The connection is the session: a client opens the socket, the cell mints a `session_id`, and closing the socket ends it, so there is nothing to resume and nothing to collect. Providers are chosen per instance, and the recogniser takes a list of keyterms for names a general model has never heard. |
| [`web`](web/) | 1.1.0 | A display in one cell with a port of its own: one `web` cell, one listener, one `cell.db`, and a token stylesheet in the visionOS design language shipped as seed data, so a display looks like something before anybody has designed anything. The cell owns the listener, so a colony opens a second display by instantiating this template again on another port, and the two share nothing but the substrate. |

## Instantiating one

Point a running colony at this directory and send an `add_nodes` mutation. The short form:

```bash
./target/release/meclaw --root ./mycolony --templates ./templates \
                        --daemon --api 127.0.0.1:7777

curl -s -X POST http://127.0.0.1:7777/colony/mutations \
  -H 'Content-Type: application/json' \
  -d '{"scope":"/","ctx":{},"diff":{
        "add_nodes":[{"name":"agent","template":"talky"}]
      }}'
```

Because a template has no outgoing edges, the node above lands connected to nothing, and a
subtree that nothing crosses into derives inactive, so its long-running cells never spawn. Wire
it in the same mutation: one `add_edges` entry from an already active cell onto the template's
own path is enough to bring the whole subtree up on that one recompute.

To have it wired and not running (a long-poll consumer whose upstream tolerates only one
reader, say), declare `"birth": "inactive"` on the `add_nodes` entry. A cell born inactive is
registered, addressable and persisted inactive, and no task is built for it, not even when the
same mutation wires it. The next mutation that NAMES it wakes it, and only one that names it:
the declaration is durable, so a mutation elsewhere in the tree leaves it asleep however far
its recompute reaches, and a restart brings it back the way it was
([#491](https://github.com/mmeyerlein/meclaw/issues/491)).

```bash
curl -s -X POST http://127.0.0.1:7777/colony/mutations \
  -H 'Content-Type: application/json' \
  -d '{"scope":"/","ctx":{"model":"openai/gpt-4o-mini"},"diff":{
        "add_nodes":[{"name":"agent","template":"talky"}],
        "add_edges":[{"from":"./ingress","to":"./agent",
                      "modifier":{"set_hop":{"route":"'in_turn'"}}}]
      }}'
```

The `ctx` block feeds the `${ctx.*}` placeholders a template declares, and `talky` wants a
resolved model literal.

That edge is the finished shape (§ The hive boundary). Since
[#228](https://github.com/mmeyerlein/meclaw/issues/228) every hive template that ships is
sealed: the address is the template's own path and the request is a lane on `hop.route`.
Nothing here names a cell inside a hive any more, which is what lets one template be swapped
for another one arranged differently. What a hive accepts and emits stands in its
`params.contract`.

Working colonies built this way live in [`../examples/`](../examples/). Start with
[`hello`](../examples/hello/README.md) for the model itself, then
[`swarm`](../examples/swarm/README.md) for a tool loop. The mutation format is specified in
[`../docs/meclaw-overview.md`](../docs/meclaw-overview.md); the cell types a `config.json` may
declare are in [`../docs/cell-types.md`](../docs/cell-types.md).

## Versioning

A reference in a mutation is either `name` or `name@major.minor.patch`. With a version it is an
exact match (`talky@2.0.0`); without one it resolves to the one version registered under that
name. This line used to say "the highest version on disk wins", and GH #277 withdrew it: the
scan aborts as soon as two `template.json` files declare the same `name`, whatever their
versions, so a scanned library holds exactly one entry per name and a bare-name reference has
exactly one answer. A library with one entry per name has no "highest" to pick from.

Semver ranges (`^`, `~`) are not parsed today, so `talky@2` is not a resolvable reference. When
you see `talky@2` in prose here or in an issue, it names the major line of a template, and it
is not a string you put in a mutation.

A template's own README carries its version in the H1, and a bump moves three places in one
commit: `template.json`, that H1, and the row in the table above.

Version numbers here move only forward, and a bump never reaches a colony that is already
running. Instantiation copied the subtree, so a running instance is pinned to the bytes it was
built from. Upgrading is therefore always an explicit act: instantiate the new version next to
the old one and move the edges. Nothing changes under you between restarts, which is what the
copy is for.

The pin has a limit. A template lives in exactly one directory, so a version bump replaces it:
after `talky` goes to `2.0.0`, a mutation asking for `talky@1.2.0` finds nothing and is
rejected, which is precisely the request a pin is supposed to survive. The pin protects a
running instance, which already holds its copy, and it does not protect a new instantiation
reproducing an old one. That asymmetry is a gap in the library. The intent is on record:
starting with 0.9.0, superseded template versions remain available, so a pinned reference keeps
resolving after the version it names has been superseded. Until that lands, read
`name@exact-version` as a statement about what you built against, and vendor the directory if
you need to rebuild it later.

GH #277 also changed how that promise has to be read. The obvious reading was to keep the old
directory beside the new one, both declaring `"name": "talky"`. That reading is a scan error
now: the uniqueness rule refuses the second `template.json` declaring an already-seen name,
whatever the version, and the whole scan aborts, taking the boot or the `RescanTemplates` with
it. The promise stands, and it now needs a mechanism it did not need before, an archive or a
registry that holds superseded versions outside the one-directory-per-name library. Until such
a mechanism exists, one directory per name is the only shape a library may have.

## Env knobs and params

The experimental knob surface is closed. Wave 0904 finished what
[#138](https://github.com/mmeyerlein/meclaw/issues/138) opened: a behaviour knob is a param of
the cell that reads it, and what remains in `.env` is the provider lane. About a hundred and
forty knobs across twenty-one templates moved, counting the twenty-six the `collector`
migration ([#136](https://github.com/mmeyerlein/meclaw/issues/136)) moved first and set the
form for. Defaults stayed bit-identical throughout, and the per-strand pins are the
`gh138_*_params.rs` tests under `crates/meclaw-cells/tests/`.

The form is three copies of one value, and a test compares them: the value under
`params.<knob>`, the declaration in `contract.settings.<knob>`, and, where a script reads it,
the literal that script falls back to. The reason is `override_params`, which may only name a
key the addressed cell carries under `params`
([#294](https://github.com/mmeyerlein/meclaw/issues/294)). A knob that existed only as a
`${VAR}` inside a `script_inline` could not be tuned per instance at all: it was colony-wide or
it was nothing. Two collectors in one colony could not have different windows, two assistants
could not have different file roots, and two clocks could not tick at different cadences, which
was the defect. `meclaw-os` now asks the environment for seven keys instead of twenty-nine,
because a declared key that binds nothing asks an operator for a value that goes nowhere.

A gate holds the line. `scripts/check_tree_rules.py` R6 refuses a new `${KNOB}` in any
template's `params`, and its `TRANSITIONAL` table, the list that carried the wave, now holds
exactly one row: `steward`, deprecated since
[#462](https://github.com/mmeyerlein/meclaw/issues/462), which ships one more release and takes
no further work, so its seven knobs are parked. `templates/_cell-types/` sits outside the scan
under the same `_`-prefix rule the other tree rules run under, which is a matter of scope.

The gate reads the `.env` lane off the name, so the rule is one anybody can apply while
writing. A variable whose name ends in `_API_KEY`, `_TOKEN`, `_BEARER`, `_BASE_URL`,
`_ENDPOINT`, `_MODEL` or `_PROVIDER`, or begins with `MODEL_`, is the provider lane. Three
further names are on the list one by one, because each was a judgement and not a category.
`OPENROUTER_HTTP_REFERER` and `OPENROUTER_X_TITLE` are provider attribution and travel with the
endpoint. `MEMORY_EMBED_DIM` is coupled to `MEMORY_EMBED_MODEL` and to the shipped embedding
seed, so the three stay together or the embeddings stop matching. A secret in a `config.json`
is a secret in the repository, which is why that half does not move. Everything else is
behaviour, and behaviour is a param.

New templates arrive by the same rule that governs everything else in this repository: a
subtree plus its gates, and the substrate stays as it is. Contributions are welcome; see
[`../CONTRIBUTING.md`](../CONTRIBUTING.md).
