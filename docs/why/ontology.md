# Ontology, in the meclaw sense

"Ontology" usually announces either philosophy or a semantic-web diagram. Here it means
something narrower and checkable: **the colony has a typed vocabulary of what can exist,
and building happens against that vocabulary, not around it.**

## The catalogue

The vocabulary is the template library and its declarations: 38 templates
([`templates/README.md`](../../templates/README.md)), each naming its cells, its lanes in
and out, its parameters and what they require. A template is a *class*; instantiating it
copies a subtree into your colony. The closed mutation vocabulary — add nodes, add edges,
register templates, adjust params — is the grammar those classes are composed with.

That pair (typed catalogue + closed grammar) is the whole claim behind
"ontology-grounded".

## Why grounding matters for building

The builder turns a structural wish — *"grow an assistant named scribe under this
member"* — into a **manifest**: an ordered list of mutation declarations. It designs
against the catalogue and is validated by it, rather than emitting free-form JSON
somebody hopes parses. Two lanes:

- A **fast lane** of parameterised recipes renders manifests deterministically, without
  calling a model at all. `grow_level` knows what a new organisation, member, assistant
  or channel gets from its parent — the same edge set every time, pinned byte for byte
  against [`examples/organism`](../../examples/organism/).
- A **design lane** consults the catalogue and asks a model, in a bounded, typed tool
  loop. What the wish does not say, it is asked for — a missing model id comes back as a
  question, not a guess.

The builder never applies anything, and that is a property of the files rather than a
promise: no cell in it has an edge onto the mutation door, and no mutation can create
one. Applying is the submitter's job — it checks the manifest's bytes against the digest
you were shown, takes your identity off the envelope, and asks the capability broker
whether you may. **A yes is a yes to the bytes you read.**

## The ontology learns new words

A closed vocabulary would be a cage if it could not grow. It grows two ways:

- **`add_templates`** registers a new class into a *running* colony and can instantiate
  it in the same diff. A wish the catalogue has no word for is answered by writing the
  word.
- **Apps** are composed words: a set of templates arranged for one task, tagged `app`,
  grown under a member. The first is [`colony-view`](../../templates/colony-view/).

## What it is not

There is no OWL file, no reasoner, no knowledge-graph product hiding here. If you come
from that world: this is an ontology in the software sense — a schema with teeth. The
teeth are the point: the builder cannot invent a cell type, an edge cannot route to an
address that does not exist, and every word the system knows is a file you can read.
