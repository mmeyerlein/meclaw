# Everything is a file

Where does the harness of a meclaw agent live? In the file system, as directories and
`config.json` files. There is no SDK to import and no plugin API to implement against, so the
tools that read a colony are the ones already on your machine: `ls`, `grep`, `diff`, `git`.

## What the binary reads

A colony is a directory tree. Every directory holding a `config.json` is one node, found by a
recursive walk from `--root` at boot (`walk_cell_directories` in
`crates/meclaw-colony/src/bootstrap.rs`). A node that names a cell type is an actor with a
mailbox of its own; a node that names none groups the nodes below it and is called a hive.
Directed edges between paths are the routes a message may take. Nothing else describes the
running system, so there is no second format beside the tree and no registry to keep in
agreement with it.

A change to the topology is therefore a change to JSON files. What a framework would ship as a
class is topology here: tools are cells, and a tool loop is an edge that routes an answer back
into the cell that asked. Forty templates ship in [`templates/`](../../templates/README.md),
from a one-cell door to the colony shell of meclaw-os, and `find templates -name '*.rs'` prints
nothing.

## The door a change goes through

A running colony changes through one operation. A mutation diff is POSTed to
`/colony/mutations` and carries eight keys and no others: `add_templates`, `add_nodes`,
`remove_nodes`, `swap_nodes`, `move_nodes`, `add_edges`, `remove_edges`, `seed_rows`
(`DIFF_OPERATIONS` in `crates/meclaw-colony/src/mutation/validate.rs`). A key no operation
reads is refused rather than ignored. A committed diff answers 200, a rejected one 422, the
process keeps running either way, and `meclaw --apply` hands the same body to the same door.

Which node may knock is itself a fact about the tree. `submit` carries the edge onto
`/colony/mutations`, the shipped `builder` carries none, and no mutation can draw the missing
edge, because that address is absent from the endpoints a diff may name
(`crates/meclaw-cells/tests/gh425_the_builder_cannot_reach_the_mutation_door.rs`).

## Where the file idea stops

Files describe the harness, and code still runs inside it. The `bash`, `code` and `harness`
cell types start real programs under a kernel sandbox
([why Rust, why Linux](rust-and-linux.md)). The cell types are compiled in, so a seventeenth
one means writing Rust and shipping a new binary. The extension point is templates and edges,
and foreign tools connect through the `mcp` cell type as ordinary cells on ordinary routes.

The primitives this page assumes are on [meclaw](../meclaw.md).
