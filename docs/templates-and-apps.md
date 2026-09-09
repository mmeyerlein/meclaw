# Templates and apps

## What a template is

A template is a physical subtree on disk: a `template.json` at its root with the name, the
version and the description a colony serves over `/colony/templates`, then one `config.json`
per cell, with sub-cells as nested directories. There is no Rust line in it and no plugin API.
Its role is that of a class. `templates/` holds the shapes a mutation can name, from one blank
`code` cell (`scriptlet`) up to the colony shell of meclaw-os, and every composition level of
meclaw-os in between is one of them.

## Why they exist

A colony is built by mutation, and a mutation that spells out forty cells and their edges by
hand gets written once and never again. A template holds that composition in files, so it can
be reviewed as a diff and instantiated by name. Reach for one when the shape you want already
exists, and write one when a shape is worth repeating.

Instantiation copies. The colony materialises the subtree into your colony root, substitutes
`${uuid7:*}` on disk and keeps `${VAR}` as a token that binds again at every read. From that
moment the instance is yours and has no link back to the library: a booted colony never reads
`templates/`, and deleting the directory leaves every colony grown from it running.

That copy is also the versioning rule. A reference is either `name` or
`name@major.minor.patch`, and a bump reaches no colony that is already running, so an upgrade
means instantiating the new version beside the old one and moving the edges.

## What an app is

An app is a sealed hive at the rim of a member, instantiated by an ordinary mutation into that
member's `./apps` container. It has no port, no secret and no channel of its own. There are
exactly three ways it plugs in: it may observe what the conversation carries, offer a tool to
the member's assistant, and write to a screen. It is never an interception. Every edge it gets
is an additional one, so a path that existed before the app fires exactly as it did before,
which is what makes an app installable and removable without re-reading the member.

`./apps` stands beside `./assistants` and `./channels` for the reason those two are separate:
a picture worth code, or a tool only this person needs, does not belong inside one assistant,
because a generation gets swapped and the app would go with it.

An app is a template. What makes it one is the tag `app` in its `template.json`, a fixed place
in the tree and a fixed set of edge classes. `colony-view` is the one the library ships: a
committed mutation triggers a topology snapshot, a `code` cell turns it into one component
tree, and the view leaves the hive towards a display.

## Installing one

One mutation, scope `<member>`, shortened to the three edges that carry the shape:

```json
{"scope": "<member>", "diff": {
  "add_nodes": [{"name": "apps/<app>", "template": "<app>@<version>"}],
  "add_edges": [
    {"from": "./firewall", "to": "./apps",
     "condition": "has(hop.route) && hop.route == 'pass' && has(context.channel_node) && context.channel_node != ''",
     "modifier": {"set_hop": {"route": "'turn'"}}},
    {"from": "./apps", "to": "./apps/<app>",
     "condition": "has(hop.route) && hop.route == 'turn'"},
    {"from": "./apps/<app>", "to": "./apps",
     "condition": "has(hop.route) && hop.route == 'view'",
     "modifier": {"set_context": {"channel_node": "'<screen>'"}}}
  ]}}
```

The `add_nodes` entry names the container in the path, so the instance is born as
`apps/<app>`. The first edge is the observer, and the member template does not ship it:
whoever listens orders the lane, so a member with no app carries no edge into an empty
container. Its guard on the channel is load-bearing, because a turn injected at the member's
own door has to keep leaving through the member's guarded default exit. The second edge binds
the container to the instance. The third is the way out, and the screen is named here rather
than in the app, because an app is display-blind. If the app is one you wrote, `add_templates`
in the same diff registers the class before it is instantiated.

## Where to read on

- [`../templates/README.md`](../templates/README.md) is the library, one row per template plus
  the rules for writing one.
- [meclaw-os](meclaw-os.md) shows the levels these templates compose into.
- [rewiring](rewiring.md) is the mutation vocabulary in full.
- [why/ontology](why/ontology.md) explains why a running colony can be taught a new class.
