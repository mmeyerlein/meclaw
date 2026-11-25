# Colony architecture

This page fixes the shape every colony is built on, and what must survive when a new kind of
colony is derived from an existing one. [meclaw-os](meclaw-os.md) describes the templates of the
levels. This page states the rules that hold them together.

## The base form

```
/              door  sink
└── os         shell   access  argus  builder  llm-registry  operator
    └── orgs   (container)
```

Every colony has this, and nothing ever leaves it out. The root carries the `door` and the
`sink`. The door belongs to the colony, not to its operation: a turn posted to the colony's HTTP
ingress enters there and gets its first lane. The sink is the documented stop for a lane that has
no consumer yet. Below the root sits exactly one shell with its operating
cells: the broker for those cells, the control loop, the model catalogue, the authoring path and
the operator's front desk. A colony without a shell has no control loop, no model catalogue and
no authoring path, and nobody notices, because nothing reports a cell that was never there.

## The member colony

```
/os/orgs
└── <org>                   organisation   a name and a routing boundary, no cell
    └── members
        └── <member>        access  affinity  memory-hive  firewall  ...
            ├── channels    one node per channel, plus the peer rim
            ├── apps        sealed hives
            ├── assistants
            │   └── <generation>   talky  cogny  tools
            └── <device>    a device, no broker of its own
```

A colony for a person specialises the base form with one organisation and one member. The member
holds the generations of its assistant. The levels are grown in this order, organisation, member,
generation, each into a container that already exists. A colony hosts one member. Growing a
second member into `members` is possible but not a supported deployment: a second member is a
new decision about the colony's shape, not a switch.

## What each level owns

A level owns what its siblings must share. The shell owns what the whole colony must answer
once. The organisation owns a name and a routing boundary and nothing else. The member owns
everything of the person: memory, the record of people and agents, screening, the broker for the
person's credentials, channels, apps and devices. A generation owns its brains and its tool
surface, and nothing that must outlive it.

## Access: two at most

A colony has at most two brokers, and a colony with a member has exactly two. The one in the
shell serves the operating cells, such as the judge of the control loop and the model catalogue.
The one at the member serves everything of the person: assistants, roles, apps, devices and
channels. No other level carries a broker. That excludes an `access` inside an app, a channel, a device, an agent or the organisation.

## The cell boundary

One cell, one database. A cell reads and writes only its own `cell.db` and learns everything else
by message. The colony holds two substrate rights over a cell's data, and the list is closed:
build and seed the staged database before the birth, and move or park a directory without
reading it ([overview](meclaw-overview.md), Database isolation).

## Channels, apps and the screen

Channels and apps hang at the member, never at a generation, so a new generation meets the same
chat, the same telephone and the same apps. A channel is one node in `channels`. An app is a
sealed hive in `apps` that plugs in at the rim and never stands in the way
([templates and apps](templates-and-apps.md)). The screen is secondary and asynchronous. It
shows what is going on, as well as it can tell. It may feed a decision, but it never blocks or
steers a conversation directly.

## Where a service lives

A service is a hive under a member, and its cells get their grants from that member's broker,
cell by cell. A colony that only offers services has a system member for this, the way a system
always has a root user: it stands for the machine, not a person, and gives the services what only
the member level has, a brain, a door and contacts. A service never sits in the shell, where one
hive would split across two levels, and never at the organisation, which has no broker. A support
colony is built as one template with selectable services, instantiated once per host with the
services that host needs.

## The peer rim

Two colonies speak over declared lanes, one `proxy` cell with platform `meclaw` on each side
([overview](meclaw-overview.md), A lane across a colony boundary). The rim is one building block
in the member's `channels` container with a fixed list of nodes, grown and replaced as a whole.
Every peer proxy of the colony lives in the rim, a device's too, so every crossing passes the
rim's admission gate.

## Sealed hives and their upkeep

A sealed hive accepts messages from outside only where it declares them. While it stands, two
kinds of change reach a cell inside it through a declared door: the model package the registry
pushes into a brain, and a params-only update the shell's contact (`/os/operator`) takes and hands
on to the cell it names ([#1353](https://github.com/mmeyerlein/meclaw/issues/1353);
[overview](meclaw-overview.md), Params into a sealed hive). The cell decides what it takes and
answers, and the contact returns that answer to the asker. The store of the cell stays in place.
An edge with both ends inside the hive is drawn by a mutation scoped at the hive. An immutable
parameter, a new cell type and a new script still arrive with a new version of the hive, lifted in
place or born anew ([config](config.md), params; [overview](meclaw-overview.md), Mutation
operations). Only the shell may draw an edge into the member, and only to a door the target hive
declares ([overview](meclaw-overview.md), The hive boundary).

## Deriving a new kind of colony

A new kind of colony is a variant of the base form or of an existing set. It may add the member
level or leave it out, and without a member it carries nothing beyond the base form. It may swap a
level's template for a derived one, and choose parameters and apps. It never touches the base form: the root with door and sink, the shell with its operating
cells and its broker. Where it has a member, the order organisation, member, generation holds.

## Known deviations

Service colonies built before this page hang their service hive directly at the root, with no
shell and a broker of their own. They will be rebuilt into the base form. Until then they keep
their one broker, and nothing new is derived from them. The model catalogue still reaches the
brains of a generation directly; the declared door for it is being built.

## Where to read on

- [meclaw-os](meclaw-os.md), the templates of the levels.
- [Templates and apps](templates-and-apps.md), what an app may and may not do.
- [Security](security.md), the sandbox, the vault and the broker.
- [meclaw-overview.md](meclaw-overview.md), the full spec, which wins on conflict.
