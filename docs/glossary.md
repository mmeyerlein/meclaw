# Glossary

Sixteen terms you need before the specification reads as prose. Each entry is two
sentences and a pointer to the place that defines it properly. Nothing here is a new
definition; every one is distilled from the document it links to, and a section named in
a pointer is a heading in that file. The table at the end lists the shipped role names.

### colony

A colony is a folder, and it is the sole write authority in the system: it owns the
registry, the routing, the templates, the lifecycle of cells and every mutation. Every
cell registers directly with it, so routing is one lookup however deep the directory
tree goes.

See [`meclaw-overview.md`](meclaw-overview.md) § *Authority model*.

### cell

An actor: one task, one mailbox, one job, single-threaded on the inside. A cell knows
only its own contract, its params and the message in front of it, never the sender, the
receiver, the hop history or another cell.

See [`meclaw-overview.md`](meclaw-overview.md) § *Cell model*.

### hive

A directory whose `config.json` says `type: "hive"`, marking a path prefix as an
authority and mutation boundary. It is a scope marker with no task, no mailbox, no
`cell.db` and no registry entry, and it doubles as a logical transit node: a message
aimed at a hive path has the hive's out-edges evaluated instead of being delivered.

See [`cell-types.md`](cell-types.md) § *`hive`: scope marker + logical transit node
(not an actor)*.

### template vs. instance

Cells in `templates/` are classes; cells in the colony's directory tree are instances.
Instantiation copies the subtree into the colony, mints fresh UUIDs and stamps the
provenance, and from that moment the instance has no link back to the library, so
editing a template never changes a colony that already grew from it.

See [`../templates/README.md`](../templates/README.md) § *What a template is*, and
[`meclaw-overview.md`](meclaw-overview.md) § *Instantiation flow (colony)* for the
mechanism.

### edge, condition, modifier

An edge is a routing rule between two paths, and it is where the logic of a colony
lives. Its condition is a CEL boolean deciding whether the edge is responsible, reading
only the two header namespaces and never the body; its modifier is the sole header
authority, promoting `context.*` and refining `hop.*` before forwarding.

See [`meclaw-overview.md`](meclaw-overview.md) § *Edge model*.

### hop

One of the two header compartments, and the one that lives for exactly one hop: it is
the contract output of the cell that just emitted, refined by the edge the message
travelled, and it is replaced wholesale at the next emission. Its sibling `context` is
the persistent one, so a value survives only if an edge promotes it with `set_context`;
otherwise the hop expires.

See [`meclaw-overview.md`](meclaw-overview.md) § *Headers vs. body: write model*.

### UBF, the universal body format

The one body shape every cell emits and consumes: three top-level slots (`system`,
`messages[]`, `attachments[]`), at least one of them set. Because it is universal,
nothing in a colony needs a format adapter between a chat turn, shell stdout, an HTTP
response body and an inference result.

See [`meclaw-overview.md`](meclaw-overview.md) § *Body format (universal)*.

### params

The `config.json` block that is handed to the cell 1:1 and opaque: the colony
substitutes `${…}` variables into it and does not interpret the rest, because what the
keys mean is the cell type's business. The running cell never re-reads `config.json`;
updates arrive as a message and are persisted in the cell's own `cell.db`, so the file
stays the birth snapshot and a `cell.db` wipe is a reset.

See [`config.md`](config.md) § *`params`*, and § *Access* for the read-once rule.

### port

A named address a producer writes to, as opposed to an implementation detail that
merely happens to be reachable. Two producers arriving at the same port in the same
shape are indistinguishable to whatever is behind it; that indistinguishability is what
makes it a port, and moving one is a breaking change for every parent that wired it.

See [`../examples/never-forgets/README.md`](../examples/never-forgets/README.md)
§ *The one shape worth reading: one port, two producers*, and
[`../templates/collector/README.md`](../templates/collector/README.md) § *Ports* for the
contract framing.

### seal

A hive that declares `params.ports` is sealed: mutation validation rejects any new edge
that pairs a non-port node inside it with an endpoint outside it, in either direction,
with `error_code: "hive_port_boundary"` and before anything is staged. It is opt-in and
it guards runtime mutations only; a hive's birth graph is the colony author's sovereign
design and is never rejected by it.

See [`cell-types.md`](cell-types.md) § *`hive`: scope marker + logical transit node
(not an actor)*, the `ports` bullet. The `session-keeper` template speaks of sealing a
session generation, which is an unrelated homonym.

### memory hive

The long-term memory as a hive of ordinary cells, owned by a member and not by an agent.
It is that member's source of truth and the agents are lenses on it, so a second agent
wired for the same member inherits what the member already knows, and the hive sits
beside the agents (`<member>/…/memory`) instead of inside one of them.

See [`../templates/memory-hive/README.md`](../templates/memory-hive/README.md), and
[`../templates/cogny/README.md`](../templates/cogny/README.md) for the placement.

### drain

The adapter between a closed session and a central memory: a collector hands its day out
as one write batch, the memory writes one turn at a time, and the drain is that
decomposition, in order and idempotent across replays. It lives outside the memory it
feeds and speaks only the documented write port.

See [`../templates/memory-drain/README.md`](../templates/memory-drain/README.md).
Draining the dead-letter queue is an unrelated homonym.

### episode

The unit of a memory's write path: one turn, one append-only row, no model call, so
nothing waits. Each row carries two timestamps kept apart, `happened_at` for when it was
said and `recorded_at` for when this colony learned it, which is what stops an import of
a year of history from collapsing into the minute you imported it.

See [`../examples/never-forgets/README.md`](../examples/never-forgets/README.md)
§ *The one shape worth reading: one port, two producers*; the canonical source is
[`../templates/memory-hive/README.md`](../templates/memory-hive/README.md).

### dream

The memory hive's nightly consolidation run. It is delta-scoped and idempotent, and it
supersedes instead of editing or removing a written value, so the same recall before
and after a run returns the same candidates.

See [`../templates/memory-hive/README.md`](../templates/memory-hive/README.md), the
nightly consolidation and canonicalisation bullets.

### dead letter (DLQ)

`/colony/dead_letters` is where a message goes when it cannot be routed: an unresolvable
path, an expired TTL, an inactive cell, a hive with no matching out-edge. It is
persistent in `colony.db`, it is colony-wide instead of per-hive, and every reason
carries a canonical `error_code` string that is part of the stable API contract.

See [`meclaw-overview.md`](meclaw-overview.md) § *Behavior on routing errors (cascade)*.

### corridor

An internal engineering discipline for the hot routing paths, which are byte-pinned
against frozen fixtures so they cannot quietly drift, with the pin enforced in CI
instead of by review. It is not a user-facing concept; you meet the word in passing in
the overview and the roadmap, and for a contributor it means that a failing fixture
gate is a real finding, never a flake.

See [`../CONTRIBUTING.md`](../CONTRIBUTING.md) § *Test it*.

## Names of the shipped roles

Every name in the tree is a role. One line each, taken from the template's own
`template.json`.

| name | what it is |
|---|---|
| [`access`](../templates/access/README.md) | the capability broker: an agent asks, and gets a handle, never the credential |
| [`affinity`](../templates/affinity/README.md) | who this colony knows and what holds between them: relations, trust, disclosure, an append-only audit |
| [`argus`](../templates/argus/README.md) | the control loop, named after the many-eyed watchman: it measures its own colony from the ledger, changes one parameter, then keeps the change or reverts it |
| [`builder`](../templates/builder/README.md) | turns a wish into a manifest; it can draft, it can never apply |
| [`cogny`](../templates/cogny/README.md) | the reasoning core: one brain, its own errand, a tool menu it asks for |
| [`collector`](../templates/collector/README.md) | assembles the context window for the turn that is running, out of the record |
| [`door`](../templates/door/README.md) | where `POST /messages` becomes a turn on the ingress lane |
| [`firewall`](../templates/firewall/README.md) | screening by size, sender, forbidden literal and rate; every verdict is a comparison, never a model call |
| [`memory-hive`](../templates/memory-hive/README.md) | a member's long-term memory as a hive of ordinary cells |
| [`operator`](../templates/operator/README.md) | the one front door a person addresses the OS through; it adds identity to a request, and no authentication |
| [`session-keeper`](../templates/session-keeper/README.md) | holds a session the way a phone call is held, opening and ending it by arithmetic |
| [`submit`](../templates/submit/README.md) | the only cell allowed to carry a manifest to the mutation door, after checking the digest you said yes to |
| [`talky`](../templates/talky/README.md) | the conversation surface: fast answers, and it owns the window and the session |
| [`terminal`](../templates/terminal/README.md) | where an undecided lane ends visibly instead of vanishing |
| [`vault`](../templates/vault/README.md) | the secret store with no `get`: `put`, `rotate`, `use`, `revoke`, and no operation that returns a secret |
