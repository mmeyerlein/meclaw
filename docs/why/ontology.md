# Ontology, in the meclaw sense

The word usually promises philosophy or a semantic-web diagram. Here it means one narrow
thing: a colony has a typed vocabulary of what can exist, and everything grown inside it is
composed out of that vocabulary.

## The catalogue

The vocabulary comes in three layers, and each of them is a file you can open.

Cell types are the primitives. `llm`, `store`, `code`, `web_fetch`, `proxy`, `timer`,
`mcp` and the rest are built into the binary and cannot be invented at runtime;
[`cell-types.md`](../cell-types.md) lists each one with its params, its contract and its
failure modes.

Templates are the classes. Every directory under
[`templates/`](../../templates/README.md) carries a `template.json` with its version and a
`config.json` per cell, and `templates/README.md` is the catalogue that names the public
ones. A gate keeps that list honest: the `catalogue` station of `scripts/gate.sh` runs
`scripts/check_catalogue.py`, which compares every listed version against the
`template.json` that ships. A catalogue promising a version nobody can install turns the
gate red.

Contracts are what a cell publishes about itself: the messages it accepts, the ones it
emits, the params it needs. A hive publishes one for its whole subtree, so whoever stands
outside knows what may cross the boundary and nothing about the inside.

## What the builder does with it

The builder turns a structural wish into a mutation manifest, and it designs against the
catalogue. The librarian hive beside it holds a searchable copy of the library and
reconciles that copy against the colony's own registry while the colony runs:
`templates/builder-librarian/catalogue/config.json:7` asks `/colony/templates` what the
registry holds and writes the names the corpus was missing.

At the mutation door the vocabulary is enforced. A manifest naming a template the library
does not hold is refused with `template_missing`
(`crates/meclaw-colony/src/mutation/mod.rs:494`), the refusal names the template, and
nothing of the manifest is applied. An override for a param no cell carries is refused the
same way.

## The vocabulary grows

`add_templates` registers a new class into a running colony and can instantiate it in the
same diff, so a wish the library has no word for is answered by writing the class first
([rewiring](../rewiring.md)).

## What this is not

There is no OWL file here and no reasoner. Nothing infers a new fact from an old one. This
is ontology in the software sense: a schema, and a gate in front of it.
