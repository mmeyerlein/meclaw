# examples/telegram-research

A research bot you talk to on Telegram. It fans out tool calls, gathers the results, and
answers in the chat. The whole agent is a directory tree, and the loop is an edge.

This is the big one. Same idea as `examples/swarm`, scaled up to a real multi-tool agent with a
real surface. You message a Telegram bot, an `llm` decides which tools to call (and can call
several in one turn), the tools run, a store-backed collector fans the results back in, and the
loop re-enters the `llm` until it has an answer. The answer goes back to your chat.

Nothing here is a new cell type. `dispatch` and `collector` are plain `code` cells composed from
the same primitives as `examples/swarm`. The fan-in state lives in a `store` cell, because `code`
cells hold no state of their own. That is the point: the orchestration is topology, not a
framework feature.

## Heads up: this one needs credentials

Unlike `hello` and `swarm`, this colony talks to the outside world (Telegram, an LLM, a search
endpoint). It is not part of the default test suite and is not the quickstart target. It
validates clean without any keys (`meclaw --root ./examples/telegram-research --validate`), but to
actually run it you need the `.env` below.

## The topology

```
/proxy       proxy       the Telegram bridge. inbound chat -> user turn, final answer -> chat
/prep        code        mints a turn id, persists the user turn, hands the llm persona + tools
/planner     llm         one call per turn. emits tool_calls, or a final answer
/dispatch    code        fans every tool_call out to the right tool, in parallel
/searcher    web_search  tool: queries a search endpoint
/reader      web_fetch   tool: GETs a URL
/memory      store        the thread store. holds the fan-in rows per turn
/collector   code        waits until every tool result is in, rebuilds the thread, fires it back
/archive     code        bridges the final answer into a store insert
/drain       code        in-tree error sink (unknown tools, llm errors)
```

The loop, in `main/config.json`, is the same trick as `swarm`, just with a real fan-in:

```
proxy     -> prep                                  (carry chat_id into context)
prep      -> planner
planner   -> dispatch     when finish_reason == 'tool_calls'
planner   -> proxy        when finish_reason == 'stop'      (answer back to the chat)
dispatch  -> searcher     when tool_name == 'web_search'
dispatch  -> reader       when tool_name == 'web_fetch'
searcher  -> collector
reader    -> collector
collector -> planner      when the round is complete        <-- THE LOOP. it is an edge.
```

The `planner` can emit several `web_search` calls in one turn. `dispatch` sends each to the
searcher in parallel, the `collector` holds them in `memory` until every result is back, then
fires the rebuilt thread back to the `planner` over one edge. Multi-tool fan-out, fan-in, and
loopback, all drawn in the tree.

For the row-by-row protocol, including the expected-ID check and atomic fired-once guard, read
the [store-backed tool-loop walkthrough](../../docs/store-backed-tool-loop.md).

## Set up the Telegram bot

1. In Telegram, open a chat with **@BotFather**.
2. Send `/newbot`. Pick a display name, then a username ending in `bot` (for example
   `meclaw_research_bot`).
3. BotFather replies with a token that looks like `123456789:AAExxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx`.
   That is your `TELEGRAM_BOT_TOKEN`.
4. Open a chat with your new bot and send it a message so it has someone to talk to.

## Configure `.env`

Create `examples/telegram-research/.env`:

```
# Telegram bot token from BotFather
TELEGRAM_BOT_TOKEN=123456789:AAExxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx

# Any OpenAI-compatible endpoint. OpenRouter shown; swap for OpenAI, vLLM, Ollama, LiteLLM.
OPENROUTER_API_KEY=sk-...
OPENROUTER_BASE_URL=https://openrouter.ai/api/v1

# A search endpoint that answers GET <endpoint>?q=<query> with {"results":[{title,url,snippet}]}.
# A local SearXNG instance with format=json works, or front your provider (Brave, etc.) with a
# small shim that returns that shape.
SEARCH_ENDPOINT=http://127.0.0.1:8888/search?format=json
SEARCH_API_KEY=
```

The `web_fetch` tool (`/reader`) needs no key. The search tool (`/searcher`) speaks a generic
`{results:[...]}` shape on purpose, so it is provider-agnostic. Point it at something that
returns that shape.

## Run it

```bash
./target/release/meclaw --root ./examples/telegram-research --daemon --api 127.0.0.1:7777 \
  --env ./examples/telegram-research/.env
# watch it in the UI: http://127.0.0.1:7777/ui/
```

Now message your bot something like "what is the actor model, with sources". Watch the trace in
the UI light up: `proxy -> prep -> planner -> dispatch -> searcher -> collector -> planner ->
proxy`, the loopback edge firing once per tool round, and the answer landing back in your chat.

## What this demonstrates, honestly

- A real multi-tool agent is still just a tree. Tools are cells, the loop is an edge.
- Parallel tool calls fan out and fan back in, with the thread state in a `store`, not in code.
- The same `code`-cell dispatcher and collector pattern from `swarm`, scaled to real tools and a
  real chat surface.

This is a proof of concept on a frozen v0.1.0 schema. The `bash`-style tools have real access to
the network, so run it somewhere you trust the topology.
