# Configure your model

An `llm` cell reaches its provider through three params: `model`, `api_key` and `base_url`.
`provider` names the wire protocol and not the vendor, so `"openai"` means the OpenAI-compatible
HTTP API and `base_url` decides who answers it. The shipped templates write those params as
`${VAR}` tokens and read the values from the colony's `.env`.

A `${VAR}` binds at every read, not at instantiation. The token stays literal in the tree, the
secret stays in one file, and another model is an `.env` line and a restart, not a config edit.

## Set it

`.env` sits next to the colony root; `--env <path>` points at another file.

```
OPENROUTER_API_KEY=sk-...
MODEL_BRAIN=openai/gpt-5.6-luna
```

`OPENROUTER_API_KEY` binds `api_key` in [`templates/talky/brain/config.json`](../templates/talky/brain/config.json).
It has no default, so the grow step needs the variable to exist: a missing one is a hard reject,
`env_var_missing`, naming it. `MODEL_BRAIN` is read by [`examples/meclaw-os/grow.json`](../examples/meclaw-os/grow.json)
as `"ctx": {"model": "${MODEL_BRAIN}"}` and arrives in the brain as `${ctx.model}`. The assistant
has two brains, and `grow-cogny.json` passes `MODEL_CORE` the same way, so the surface can run a
fast model while the core runs a slow one ([why/two-brains.md](why/two-brains.md)).

## Another endpoint

Any endpoint on the same wire works, and OpenRouter is only the default `base_url` the templates
carry. Some examples declare it as a token (`OPENROUTER_BASE_URL` in
[`examples/telegram-research`](../examples/telegram-research/)); `talky` and `cogny` name the
endpoint outright, so the manifest that grows the cell overrides it. An empty `api_key` is no
credential, and the cell then sends no `Authorization` header at all, which is what a local server
on loopback wants.

```json
{"name": "talky", "template": "talky",
 "override_params": {"brain": {"base_url": "http://127.0.0.1:11434/v1", "api_key": ""}}}
```

## Read on

[cell-types.md](cell-types.md) has every `llm` param and [config.md](config.md) the substitution
rules. [costs.md](costs.md) reads what a colony spent per model out of `colony.db`.
