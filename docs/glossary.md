# Glossary

Fifteen words you need before the specification reads as prose. Each entry is
two sentences and a pointer at the place that defines it properly. Nothing here
is a new definition — every one is distilled from the document it links to.

Sections named below are headings in the linked file.

---

### colony

A colony is a folder, and it is the sole write authority in the system: it owns
the registry, routing, templates, lifecycle and mutations. Every cell registers
directly with it, so routing is one lookup no matter how deep the directory tree
goes.

→ [`meclaw-overview.md`](meclaw-overview.md) § *Authority model*; the five-bullet
short form is in [`../README.md`](../README.md) § *The whole vocabulary*.

### cell

An actor: one task, one mailbox, one job, single-threaded on the inside. A cell
knows only its own contract, its params and the message in front of it — never
the sender, the receiver, the hop history, or any other cell.

→ [`meclaw-overview.md`](meclaw-overview.md) § *Cell model*.

### hive

A directory whose `config.json` says `type: "hive"`. It is a **scope marker**,
not an actor — no task, no mailbox, no `cell.db`, no registry entry — marking a
path prefix as an authority and mutation boundary, and it doubles as a logical
transit node: a message aimed at a hive path is not delivered but has the hive's
out-edges evaluated instead.

→ [`cell-types.md`](cell-types.md) § *`hive`: scope marker + logical transit node
(not an actor)*.

### template vs. instance

Cells in `templates/` are classes; cells in the colony's directory tree are
instances. Instantiation **copies** the subtree into the colony, mints fresh
UUIDs and stamps the provenance — from that moment the instance has no link back
to the library, so editing a template never changes a colony that already grew
from it.

→ [`../templates/README.md`](../templates/README.md) § *What a template is*;
mechanism in [`meclaw-overview.md`](meclaw-overview.md) § *Instantiation flow
(colony)*.

### edge, condition, modifier

An edge is a routing rule between two paths, and it is where the logic of a
colony lives. Its **condition** is a CEL boolean deciding whether the edge is
responsible — reading only the two header namespaces, never the body — and its
**modifier** is the sole header authority, promoting `context.*` and refining
`hop.*` before forwarding.

→ [`meclaw-overview.md`](meclaw-overview.md) § *Edge model*.

### hop

One of the two header compartments, and the one that lives for exactly one hop:
it is the contract output of the cell that just emitted, refined by the edge the
message travelled, and it is replaced wholesale at the next emission. Its
sibling `context` is the persistent one, so a value survives only if an edge
promotes it with `set_context` — by default the hop expires.

→ [`meclaw-overview.md`](meclaw-overview.md) § *Headers vs. body: write model*.

### UBF — universal body format

The one body shape every cell emits and consumes: three top-level slots
(`system`, `messages[]`, `attachments[]`), at least one of them set. Because it
is universal, nothing in a colony needs a format adapter between a chat turn,
shell stdout, an HTTP response body and an inference result.

→ [`meclaw-overview.md`](meclaw-overview.md) § *Body format (universal)*.

### params

The `config.json` block that is handed to the cell 1:1 and opaque — the colony
substitutes `${…}` variables into it and does not interpret the rest, because
what the keys mean is the cell type's business. The running cell never re-reads
`config.json`: updates arrive as a message and are persisted in the cell's own
`cell.db`, so the file stays the birth snapshot and a `cell.db` wipe is a reset.

→ [`config.md`](config.md) § *`params`*, and § *Access* for the read-once rule.

### port

A named address a producer writes to, as opposed to an implementation detail
that merely happens to be reachable — two different producers arriving at the
same port in the same shape are indistinguishable to whatever is behind it, and
that indistinguishability is what makes it a port rather than a function call.
Moving one is a breaking change for every parent that wired it.

→ [`../examples/never-forgets/README.md`](../examples/never-forgets/README.md)
§ *The one shape worth reading: one port, two producers*; the contract framing is
in [`../templates/collector/README.md`](../templates/collector/README.md)
§ *Ports*.

### seal

A hive that declares `params.ports` is **sealed**: mutation validation rejects
any new edge that pairs a non-port node inside it with an endpoint outside it,
in either direction, with `error_code: "hive_port_boundary"` and before anything
is staged. It is opt-in and it guards runtime mutations only — a hive's birth
graph is the colony author's sovereign design and is never rejected by it.

→ [`cell-types.md`](cell-types.md) § *`hive`: scope marker + logical transit node
(not an actor)*, the `ports` bullet. *(Unrelated homonym: the `session-keeper`
template also speaks of sealing a session generation.)*

### memory hive

The long-term memory as a hive of ordinary cells — and it belongs to a **member**,
not to an agent. It is that member's source of truth; the agents are lenses on it,
so a second agent wired for the same member inherits what the member already
knows. That is why the hive sits beside the agents (`<member>/…/memory`) rather
than inside one of them, and why a `memory-hive` per agent is a misreading of the
shape rather than a stricter one.

→ [`../templates/memory-hive/README.md`](../templates/memory-hive/README.md);
the placement in [`../templates/cogny/README.md`](../templates/cogny/README.md)
§ *One core, N channel voices*.

### drain

The adapter between a closed session and a central memory: a collector hands its
day out as one write batch, the memory writes one turn at a time, and the drain
is that decomposition — in order, and idempotent across replays. It deliberately
lives outside the memory it feeds and speaks only the documented write port.

→ [`../templates/memory-drain/README.md`](../templates/memory-drain/README.md).
*(Unrelated homonym: draining the dead-letter queue.)*

### episode

The unit of a memory's write path: one turn, one append-only row, no model call,
so nothing waits. Each row carries two timestamps kept deliberately apart —
`happened_at`, when it was said, and `recorded_at`, when this colony learned it —
which is what stops an import of a year of history from collapsing into the
minute you imported it.

→ [`../examples/never-forgets/README.md`](../examples/never-forgets/README.md)
§ *The one shape worth reading: one port, two producers*; canonical source
[`../templates/memory-hive/README.md`](../templates/memory-hive/README.md).

### dream

The memory hive's nightly consolidation run: delta-scoped, idempotent, and
superseding rather than deleting — it never edits or removes a written value.
The invariant that buys is the one worth remembering: the same recall before and
after a dream run returns the same candidates.

→ [`../templates/memory-hive/README.md`](../templates/memory-hive/README.md),
the *Nightly consolidation* and *canonicalisation round* bullets.

### dead letter (DLQ)

`/colony/dead_letters` is where a message goes when it cannot be routed — an
unresolvable path, an expired TTL, an inactive cell, a hive with no matching
out-edge. It is persistent in `colony.db`, it is colony-wide rather than
per-hive, and every reason carries a canonical `error_code` string that is part
of the stable API contract.

→ [`meclaw-overview.md`](meclaw-overview.md) § *Behavior on routing errors
(cascade)*.

### corridor

Not a user-facing concept: an internal engineering discipline for the hot
routing paths, which are byte-pinned against frozen fixtures so they cannot
quietly drift, with the pin enforced in CI rather than by review. You will meet
the word in passing in the overview and the roadmap; what it means for you as a
contributor is simply that a failing fixture gate is the point, not a flake.

→ [`../CONTRIBUTING.md`](../CONTRIBUTING.md) § *Test it*.
