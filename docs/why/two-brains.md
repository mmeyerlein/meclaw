# One assistant, two brains

The shipped assistant is two agents standing side by side. `talky` holds the conversation,
`cogny` does the reasoning, each is its own template, and each has exactly one model slot.
The assistant level wires the two together and decides which model goes where.

## What each one is

`templates/talky/template.json` describes a hive that references a session keeper, a
collector and a dispatcher as sub-units, plus one `llm` brain slot, with every internal edge
pre-wired. It owns the session and the window, and it speaks to the person.

`templates/cogny/template.json` describes the same shape without a channel. It has no
session keeper, no summarizer and no proxy, because a core without a channel has no sessions
and no night. Its conversation is the errands the channel voices send it. It has one brain,
it declares its own errand as a tool schema, and it asks the tools hive what the other tools
look like instead of carrying a menu somebody typed.

Both templates require `ctx.model` at instantiation and the assistant level passes a
different value to each, so the surface can run a fast model while the core runs a strong
one. Changing that is an `override_params` entry on the manifest that grows the generation,
which makes it a mutation ([rewiring](../rewiring.md)).

## Why the split sits in the topology

Two cells means two models, two budgets and two prompts, and all three are entries in a
`config.json` that a mutation can change while the colony runs. One model doing both jobs
would put the same decision inside a prompt, where nothing can measure it and no mutation
can move it.

I have no latency measurement for the split itself, because nobody has run the comparison.
What is measured is a failure the split caused while it was wired loosely. A thinking model
puts a sentence beside its tool bundle, the dispatcher used to send that sentence on the
answer lane, and for the core that lane is the advice lane of the voice that asked. On one
colony 11 of 26 answers on that lane were interim sentences, and a single user turn produced
thirteen messages. The repair was one knob: the core now ships with `interim` off, so the
sentence never leaves the dispatcher (`templates/cogny/template.json`,
[GH #539](https://github.com/mmeyerlein/meclaw/issues/539)).

## How the tool menu arrives

A tool is a cell plus a name edge in the tools hive. The declarations are fetched over a
lane pair and written into the brain as durable state, and they are asked for again when
the mutation door leaves a receipt saying the graph moved. Growing a new tool is therefore
a mutation, and no prompt is edited for it.

## Limits

The core once had a second, faster brain and a lookup lane that chose between them, and both
are gone. The right owner of a fast memory question is the surface that already holds the
window, and routing it through an advisor was a round trip somebody measured.

Two brains cost two provider accounts, two failure modes and one more hop on every hard
question. For a colony that only fields small talk that is overhead, and the composites can
be instantiated apart: a `talky` on its own is a complete conversational agent.
