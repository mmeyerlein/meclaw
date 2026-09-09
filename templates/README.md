# templates/

Reference topologies you can instantiate. Every one of them is pure DSL: directories,
`config.json` files and edges, with no Rust and no plugin API anywhere in them. What a template
is, how instantiation copies it, how versions bind and how an app differs from a template is on
one page, [`../docs/templates-and-apps.md`](../docs/templates-and-apps.md). This folder is the
library that page talks about.

## The library

Every template the export ships has a row here, and that is a gate:
`crates/meclaw-cells/tests/gh235_every_public_template_has_a_row.rs` compares the rows against
the export's allow-list, and `scripts/check_catalogue.py` compares each row's version against
that template's `template.json`. A development tree may hold further directories that the export
does not ship; they carry no compatibility promise and nothing here references them. The
template's own README, linked from the name, carries the version history, the lane contract and
the reasoning.

| Template | Version | What it is |
|---|---|---|
| [`access`](access/) | 2.5.0 | The capability broker as one sealed hive of six cells and no model: an agent asks in natural language, a handle travels on the wire, and a credential leaves the vault beside it only sealed. |
| [`affinity`](affinity/) | 3.3.0 | The curated record of the people and agents a colony knows, as whole AIeOS documents plus the four things that standard has no vocabulary for: relations, trust, disclosure and an append-only audit. |
| [`archive-bridge`](archive-bridge/) | 1.1.0 | Turns an `llm`'s last answer into a store-native insert and swallows the store's reply echo, so an append-only archive costs one cell and no loop. |
| [`argus`](argus/) | 1.1.0 | The colony's control loop: a charter of goals as rows, a deterministic measurement, a judge that simulates before it decides, and keep-or-revert after the window. |
| [`assistant`](assistant/) | 2.6.0 | One generation of one person's agent as a composition level, holding the conversation surface, the reasoning core and the tool surface as three refs and no container of its own. |
| [`builder`](builder/) | 1.7.4 | The intake that turns a structural wish into a manifest somebody else submits, on a fast lane of parameterised recipes and a design lane that consults `builder-librarian`. Its `grow_level` recipe renders the transit edges a new level gets from its parent, and the doors of a container level carry `context.org` or `context.member` beside the lane, so a second member of one organisation is a second address. |
| [`builder-librarian`](builder-librarian/) | 2.2.0 | Lexical retrieval over the builder's knowledge base in one sealed hive: an FTS5 corpus, a BM25 retriever and a catalogue cell, with no embeddings and no model. A catalogue row carries `CONTRACT --`, `STORES --` and `PARAMS --`, the three demands the mutation door enforces. |
| [`canvy`](canvy/) | 2.2.0 | One interactive canvas of the colony on a port of its own. Deprecated since [#455](https://github.com/mmeyerlein/meclaw/issues/455), which withdraws it: a screen belongs to a person and a view of the colony is one application among many, so use `display` and `colony-view`. |
| [`clock`](clock/) | 1.0.1 | A periodic tick from one `timer` cell that carries one schedule and decides nothing. |
| [`cogny`](cogny/) | 5.0.1 | One hive holding the agent core, a `collector`, a `dispatcher`, one `llm` brain and a `schemas` cell, for one class of question: synthesis, multi-step work, research. |
| [`collector`](collector/) | 4.1.0 | Context assembly in a hive of two cells: it decides in one place what enters an agent's context window and what leaves it, and folds each tool round back in. |
| [`colony-view`](colony-view/) | 1.1.0 | One app that draws the colony: a committed mutation triggers a topology snapshot, a `code` cell turns it into one component tree, and the view leaves the hive towards a display. |
| [`daily-digest`](daily-digest/) | 2.1.0 | Scheduled fetch-and-forward in one hive, where a timer fires a schedule, a `web_fetch` reads the URL, a `code` cell formats and a proxy delivers, and a caller may also ask out of turn. |
| [`dispatcher`](dispatcher/) | 1.2.0 | The fan-out half of a tool loop in one `code` cell: it turns a brain's bundle of tool calls into messages a graph can route. |
| [`display`](display/) | 1.1.0 | One screen on a port of its own that many agents and applications write onto at the same time, where a view is a named, owned, optionally expiring piece of that screen. |
| [`door`](door/) | 1.0.2 | The first cell of a colony: a `code` cell that names the first lane, because `set_hop` is an edge's job and above the first cell there is no edge. |
| [`fetcher`](fetcher/) | 1.0.0 | One `web_fetch` cell for an outbound HTTP GET, with five knobs and no target. |
| [`firewall`](firewall/) | 2.3.0 | Deterministic screening on an ingress channel drawn as topology: every inbound turn ends on `pass` with the body byte-identical, or on `reject` naming the rule that fired. |
| [`freeswitch`](freeswitch/) | 1.1.0 | A telephone as one channel of a person, in two halves inside one hive: a `voice` cell binds the media port and a small state machine of a `code` cell keeps the calls. |
| [`meclaw-os`](meclaw-os/) | 1.8.5 | The colony shell: the outermost of the four composition levels, four occupants, one empty container and the transit graph between them, and no cell of its own. |
| [`member`](member/) | 1.7.0 | One person as a composition level, holding the memory, the curated record, the screen and the keys, plus three open containers for that person's assistants, channels and apps. |
| [`memory-drain`](memory-drain/) | 2.0.6 | The adapter between a write batch and the central memory, for bulk import of foreign history, and nothing shipped wires it (ADR-0012). |
| [`memory-hive`](memory-hive/) | 3.4.0 | A member's memory in fifteen cells: every turn becomes an append-only episode written without a model, and the read path answers only with rows the current round could have heard. |
| [`operator`](operator/) | 1.2.0 | One front door into the OS and one place a submission lives: a sealed hive at the colony shell with one occupant per subject, reached by naming a lane and never a cell. |
| [`org`](org/) | 1.4.1 | The namespace and nothing else: one hive with one open container and nothing but transit edges, because members of one organisation share a name and a boundary and no more. |
| [`receptionist`](receptionist/) | 2.1.0 | One agent per channel, built the moment a channel first speaks, so a shared communicator stops mixing the windows of many chats into one. |
| [`retry`](retry/) | 1.0.0 | A bounded retry loop around one tool in one `code` cell that counts nothing, because the wiring increments the attempt counter and the wiring bounds it. |
| [`scriptlet`](scriptlet/) | 1.0.1 | One `code` cell, shipped blank, for a script an `override_params` names. |
| [`session-keeper`](session-keeper/) | 2.2.0 | A session lifecycle in a hive of five cells, where a session is a channel generation and the model is a phone call: it begins on a channel with a turn, and it ends on that channel. |
| [`shelf`](shelf/) | 1.0.2 | One `store` cell with one table and no opinion about what goes in it. |
| [`steward`](steward/) | 2.0.13 | The colony's control loop in a hive of seven cells. Deprecated since [#462](https://github.com/mmeyerlein/meclaw/issues/462) and renamed: a new control loop is an `argus`. |
| [`submit`](submit/) | 2.3.1 | Two occupants behind one door, and the only reach onto the mutation door in the whole tree: it asks the broker who may submit, and what it decides itself is the form of a subscribe edge. |
| [`summarizer`](summarizer/) | 2.1.0 | The session handover step in a hive of two cells, so that when a generation closes, its successor wakes up with yesterday. |
| [`talky`](talky/) | 5.1.0 | One template holding a whole conversational agent: `session-keeper`, `collector` and `dispatcher` as referenced units under one hive, plus an `llm` brain, the sidecar splitter and one error collector. |
| [`telegram-connector`](telegram-connector/) | 2.0.1 | A Telegram chat behind one `proxy` cell with one credential, one wire in and one wire out, and no persona and no answer of its own. |
| [`terminal`](terminal/) | 1.0.1 | The last cell of a lane: a `code` cell that accepts anything and emits nothing, which turns an undecided destination into a documented stop instead of a dead letter. |
| [`tools`](tools/) | 1.4.2 | The tool surface of one assistant, one node with one contract, `tool_call` in and `tool_result` out, that also hands back the schemas of the tools it serves. |
| [`vault`](vault/) | 1.3.0 | A secret store in one cell with no operation that returns a secret: `use` signs on the broker's behalf so the secret stays home, and `deliver` hands out a ciphertext sealed to a key that dies with the task. |
| [`voice`](voice/) | 1.4.1 | A spoken conversation behind one cell, one WebSocket surface and one pair of provider credentials, where the connection is the session: opening the socket mints an id and closing it ends the session. |
| [`web`](web/) | 1.1.0 | A display in one cell with a port of its own and a token stylesheet shipped as seed data, so a colony opens a second display by instantiating this template again on another port. |

## Writing one

1. A template is a subtree: `template.json` at the root, one `config.json` per cell, sub-cells
   as nested directories, and no Rust.
2. `template.json` carries the name, the version and the description slots a colony serves over
   `/colony/templates`.
3. No edge leaves the subtree. Wiring the template into a colony is the job of the mutation that
   instantiates it.
4. A hive template seals itself: `params.ports: []`, one door edge per accepted lane, and a
   `params.contract` whose lane names say what the caller wants, never where it lands inside.
5. A bump moves three places in one commit: `template.json`, the H1 of the template's own README,
   and the row above. `scripts/check_catalogue.py` compares the row against `template.json`.
6. An instantiated node lands connected to nothing, and a subtree nothing crosses into derives
   inactive, so the mutation that instantiates it wires it too. To have it wired and not running,
   declare `"birth": "inactive"` on the `add_nodes` entry. Such a cell is
   registered, addressable and persisted inactive, and no task is built for it, not even when the
   same mutation wires it. The next mutation that NAMES it wakes it, and only one that names it:
   a mutation elsewhere in the tree leaves it asleep however far its recompute reaches, and a
   restart brings it back the way it was ([#491](https://github.com/mmeyerlein/meclaw/issues/491)).

New templates arrive by the same rule that governs everything else in this repository: a subtree
plus its gates, and the substrate stays as it is. See
[`../CONTRIBUTING.md`](../CONTRIBUTING.md).
