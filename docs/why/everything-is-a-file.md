# Everything is a file

The harness of a meclaw agent lives in the file system. meclaw ships no SDK to import and no
plugin API to implement against. You point one binary at a directory, and the tools that
read that directory are the ones already on your machine: `ls`, `grep`, `diff`, `git`.

## What the binary reads

A colony is a directory tree. Every directory that holds a `config.json` is one node, found
by a recursive walk from `--root` at boot (`walk_cell_directories` in
`crates/meclaw-colony/src/bootstrap.rs`). A node that names a cell type is an actor with a
mailbox and an async task of its own. A node that names none groups the nodes below it and
is called a hive; it has no mailbox and runs no task. Directed edges between paths are the
routes a message may take.

Sixteen cell types have a factory in the binary, and `hive` is a seventeenth catalogue entry
with none. The list is in [cell types](../cell-types.md), the shape of a `config.json` is in
[config](../config.md), the vocabulary in the [glossary](../glossary.md). Nothing else
describes the running system: no second configuration format beside the tree, no registry
to keep in agreement with it.

## What that buys

A change to the topology is a change to JSON files in a directory, so you read one with `ls`
and `cat`, search it with `grep`, review a change to it with `diff` and version it with
`git`. What a framework would ship as a class is topology here: tools are cells, and a tool
loop is an edge that routes an answer back into the cell that asked. An `llm` cell makes one
provider call and emits one message, so whatever loops in your system is on disk as edges,
traced hop by hop in
[the store-backed tool loop](../store-backed-tool-loop.md). Forty templates ship in
[`templates/`](../../templates/README.md), from a one-cell door to a whole operating shell,
and `find templates -name '*.rs'` prints nothing. Instantiating a template copies its
subtree into your colony, and from that moment the copy is yours with no link back.

## The door a change goes through

A running colony changes through one operation. A mutation diff is POSTed to
`/colony/mutations` and carries eight keys and no others: `add_templates`, `add_nodes`,
`remove_nodes`, `swap_nodes`, `move_nodes`, `add_edges`, `remove_edges`, `seed_rows`
(`DIFF_OPERATIONS` in `crates/meclaw-colony/src/mutation/validate.rs`). A key no operation
reads is refused instead of ignored. A committed diff answers 200, a rejected one 422, the
process keeps running either way, and `--apply` hands the same body to the same door.

An agent gets no second door, and which node may knock on this one is itself a fact about
the tree. `submit` carries the edge onto `/colony/mutations`, the shipped `builder` carries
none, and no mutation can draw the missing edge, because that address is absent from the
endpoints a diff may name
(`crates/meclaw-cells/tests/gh425_the_builder_cannot_reach_the_mutation_door.rs`).

## Where the file idea stops

Files describe the harness, and code still runs inside it: the `bash`, `code` and `harness`
cell types start real programs under a kernel sandbox
([why Rust, why Linux](rust-and-linux.md)). The cell types themselves are compiled in, so
adding a seventeenth one means writing Rust and shipping a new binary. The extension point
is templates and edges, and foreign tools connect through the `mcp` cell type as ordinary
cells on ordinary routes.

A template fixed here does not reach the trees already grown from it; lifting an instance to
a newer template is a mutation somebody writes ([rewiring](../rewiring.md)).
