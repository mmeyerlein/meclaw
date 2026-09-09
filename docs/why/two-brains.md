# One assistant, two brains

Why does the shipped assistant run two models instead of one? Because answering a person and
working a problem are two jobs with two budgets, and two cells put that choice in files a
mutation can change. One model doing both would put the same decision inside a prompt, where
nothing can measure it and no mutation can move it.

## Two templates, two model slots

`templates/talky` holds the conversation surface: a hive referencing a session keeper, a
collector and a dispatcher, plus one `llm` brain, with every internal edge pre-wired. It owns the
session and the window and it speaks to the person.

`templates/cogny` is the same shape without a channel. No session keeper, no summarizer, no
proxy, because a core without a channel has no sessions and no night. Its conversation is the
errands the surface sends it, and it declares that errand as a tool schema of its own.

Both require `ctx.model` at instantiation, and both stand alone elsewhere. The level that
references both is the one place the two models can differ, so `templates/assistant` declares a
strict second key, `ctx.model_surface`, and spends it in the ref marker for the surface
(`templates/assistant/talky/config.json`, trimmed):

```json
{ "cell": { "type": "ref", "template": "talky@5.1.0" },
  "override_params": { "brain": { "model": "${ctx.model_surface}" } } }
```

The ref marker for the core carries no such override, so it reads `ctx.model`. Before that key
existed, one flat ctx reached both occupants, both resolved `model`, and the surface ran the
reasoning model ([GH #516](https://github.com/mmeyerlein/meclaw/issues/516)).

## What the split cost while it was wired loosely

There is no latency measurement for the split itself, because nobody has run the comparison.
What is measured is a failure it caused. A thinking model puts a sentence beside its tool bundle,
the dispatcher used to send that sentence on the answer lane, and for the core that lane is the
advice lane of the voice that asked. On one colony 11 of 26 answers on that lane were interim
sentences, and a single user turn produced thirteen messages. The repair was one knob: the core
ships with `interim` off (`templates/cogny/dispatcher/config.json`,
[GH #539](https://github.com/mmeyerlein/meclaw/issues/539)).

Two brains cost two provider accounts, two failure modes and one hop more on every hard question.
For a colony that fields small talk that is overhead, and a `talky` alone is a whole agent.

The level that wires the two, and the levels above it, are on [meclaw-os](../meclaw-os.md).
