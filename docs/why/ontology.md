# Ontology, in the meclaw sense

What does ontology mean in this repository? A colony has a typed vocabulary of what can
exist, and everything grown inside it is composed out of that vocabulary. That is the
software meaning of the word, a schema with a gate in front of it, and not the philosophical
one: no OWL file, no reasoner, nothing that infers a new fact from an old one.

## The catalogue

The vocabulary has three layers, each of them a file you can open. Cell types are the
primitives. `llm`, `store`, `code`, `web_fetch`, `proxy`, `timer`, `mcp` and the rest are
built into the binary and cannot be invented at runtime, and
[`cell-types.md`](../cell-types.md) gives each one its params, contract and failure modes.

Templates are the classes. Every directory under [`templates/`](../../templates/README.md)
carries a `template.json` with a version, then one `config.json` per cell. A gate keeps the
published list honest: `scripts/check_catalogue.py` compares every version named in
`templates/README.md` against the `template.json` that ships, so a catalogue promising a
version nobody can install turns the gate red.

Contracts are what a cell publishes about itself, which messages it accepts, which it emits
and which params it needs. A hive publishes one for its whole subtree, so whoever stands
outside knows what may cross that boundary and nothing about the inside.

## The builder designs against it

The builder turns a structural wish into a mutation manifest and composes it out of the
catalogue. The librarian hive beside it asks `/colony/templates` what the registry holds
while the colony runs, and writes in the names its corpus was missing
(`templates/builder-librarian/catalogue/config.json`).

At the mutation door the vocabulary is enforced rather than assumed. A manifest naming a
template the library does not hold is refused with `template_missing`, the refusal names the
template, and nothing of it is applied (`crates/meclaw-colony/src/mutation/mod.rs`). An
override for a param no cell carries is refused the same way.

## The vocabulary grows

`add_templates` registers a new class into a running colony and can instantiate it in the
same diff, so a wish the library has no word for is answered by writing the class first.

## Where to read on

- [templates and apps](../templates-and-apps.md) for what a class is made of
- [`rewiring.md`](../rewiring.md) for the eight diff operations, `add_templates` among them
