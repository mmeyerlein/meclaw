# Glossary

Seventeen words the other pages assume, two or three sentences each, then the file that defines
the word properly. The seven primitives come first, in the order of [meclaw.md](meclaw.md).

**colony**, a folder, and the sole write authority: the registry, the routing, the templates, the
lifecycle of cells and every mutation. Every cell registers with it directly, so routing is one
lookup however deep the tree goes. [`meclaw-overview.md`](meclaw-overview.md) § Authority model.

**cell**, an actor: one task, one mailbox, one job, single-threaded on the inside. It knows its
contract, its `params` and the message in front of it, never the sender, the receiver or another
cell. [`meclaw-overview.md`](meclaw-overview.md) § Cell model.

**hive**, a directory whose `config.json` says `type: "hive"`: an authority and mutation boundary
with no task and no mailbox. A message aimed at one has the hive's out-edges evaluated instead of
being delivered. [`cell-types.md`](cell-types.md) § `hive`, a scope marker and a transit node.

**edge**, a routing rule between two paths, and where the logic of a colony lives. Its `condition`
is a CEL boolean deciding whether it is responsible, reading the headers and never the body; its
`modifier` is the sole header authority. [`meclaw-overview.md`](meclaw-overview.md) § Edge model.

**hop**, the header compartment that lives for exactly one hop: the output of the cell that just
emitted, refined by the edge the message travelled, replaced wholesale at the next emission. Its
sibling `context` persists, so a value survives only if an edge promotes it.
[`meclaw-overview.md`](meclaw-overview.md) § Headers and body, the write model.

**mutation**, a body POSTed to `/colony/mutations`, carrying a `scope` and a `diff` written in
eight operation keys. The colony validates the whole post-state in one stage, then applies it
while the process keeps running. [`meclaw-overview.md`](meclaw-overview.md) § Mutation format.

**template vs. instance**, cells in `templates/` are classes, cells in the tree are instances.
Instantiation copies the subtree in, mints fresh UUIDs and stamps the provenance, and from then on
the instance has no link back, so editing a template never changes a colony grown from it.
[`../templates/README.md`](../templates/README.md) § What a template is.

**UBF, the universal body format**, the one body shape every cell emits and consumes: three
top-level slots (`system`, `messages[]`, `attachments[]`), at least one of them set. Nothing needs
a format adapter between a chat turn, shell stdout, an HTTP response body and an inference result.
[`meclaw-overview.md`](meclaw-overview.md) § Body format (universal).

**params**, the `config.json` block handed to the cell 1:1: the colony substitutes `${…}`
variables and interprets nothing else; what the keys mean is the cell type's business. The cell
never re-reads the file, updates arrive as a message. [`config.md`](config.md) § `params`.

**port**, a named address a producer writes to, as opposed to an implementation detail that merely
happens to be reachable. Two producers arriving at the same port in the same shape are
indistinguishable to whatever is behind it, and moving one is a breaking change.
[`../templates/collector/README.md`](../templates/collector/README.md) § Ports.

**seal**, a hive that declares `params.ports`. Mutation validation then rejects any new edge that
pairs a non-port node inside it with an endpoint outside it, in either direction, with
`error_code: "hive_port_boundary"`; the birth graph is never rejected.
[`cell-types.md`](cell-types.md) § `hive`, a scope marker and a transit node.

**memory hive**, a member's long-term memory as a hive of ordinary cells, never an agent's. It is
the member's source of truth and the agents are lenses on it, so a second agent inherits what it
knows. [`../templates/memory-hive/README.md`](../templates/memory-hive/README.md).

**drain**, the adapter between a closed session and a memory: a collector hands out its day in one
batch, the memory writes one turn at a time, and the drain is that decomposition, idempotent
across replays. [`../templates/memory-drain/README.md`](../templates/memory-drain/README.md).

**episode**, the unit of a memory's write path: one turn, one append-only row, no model call, so
nothing waits. Each row keeps `happened_at` apart from `recorded_at`, which is what stops an
import of a year of history from collapsing into the minute you imported it.
[`../templates/memory-hive/README.md`](../templates/memory-hive/README.md).

**dream**, the memory hive's nightly consolidation run. It is delta-scoped and idempotent, and it
supersedes instead of editing or removing a written value, so the same recall returns the same
candidates. [`../templates/memory-hive/README.md`](../templates/memory-hive/README.md).

**dead letter (DLQ)**, `/colony/dead_letters`, where a message goes when it cannot be routed: an
unresolvable path, an expired TTL, an inactive cell, a hive with no matching out-edge. Every
reason carries a canonical `error_code` string that is part of the public contract.
[`meclaw-overview.md`](meclaw-overview.md) § Routing errors and the dead-letter queue.

**corridor**, an engineering discipline for the hot routing paths, which are byte-pinned against
frozen fixtures so they cannot quietly drift, with the pin enforced in CI. A failing fixture gate
is a real finding, never a flake. [`../CONTRIBUTING.md`](../CONTRIBUTING.md) § Test it.

## Names of the shipped roles

| name | why it is called that |
|---|---|
| [`access`](../templates/access/README.md) | it grants access without handing over the credential: an agent asks, and gets a handle |
| [`affinity`](../templates/affinity/README.md) | it keeps the affinities, who this colony knows and what holds between them: relations, trust, disclosure, an append-only audit |
| [`argus`](../templates/argus/README.md) | after the many-eyed watchman: it measures its own colony from the ledger, changes one parameter, then keeps the change or reverts it |
| [`builder`](../templates/builder/README.md) | it builds a manifest out of a wish, and only that: it can draft, it can never apply |
| [`cogny`](../templates/cogny/README.md) | the one that thinks: one brain, its own errand, a tool menu it asks for |
| [`collector`](../templates/collector/README.md) | it collects the context window for the turn that is running, out of the record |
| [`door`](../templates/door/README.md) | the door a turn comes in through, where `POST /messages` becomes a message on the ingress lane |
| [`firewall`](../templates/firewall/README.md) | it screens what passes by size, sender, forbidden literal and rate; every verdict is a comparison, never a model call |
| [`memory-hive`](../templates/memory-hive/README.md) | it is a memory and it is a hive: a member's long-term memory built from ordinary cells |
| [`operator`](../templates/operator/README.md) | the operator a person addresses the OS through; it adds identity to a request, and no authentication |
| [`session-keeper`](../templates/session-keeper/README.md) | it keeps a session the way a phone call is held, opening and ending it by arithmetic |
| [`submit`](../templates/submit/README.md) | it submits, and it is the only cell allowed to: it carries a manifest to the mutation door after checking the digest you said yes to |
| [`talky`](../templates/talky/README.md) | the one that talks: fast answers, and it owns the window and the session |
| [`terminal`](../templates/terminal/README.md) | terminal as in end of the line, where an undecided lane ends visibly instead of vanishing |
| [`vault`](../templates/vault/README.md) | a vault has no `get`: `put`, `rotate`, `use`, `revoke`, and no operation that returns a secret |
