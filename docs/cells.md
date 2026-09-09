# Cell types

Every cell declares a `cell.type` in its `config.json`, and that one word decides what runs behind its mailbox: a model call, a table, a script, a port, a scope marker. Sixteen types have a factory in the binary, and `hive` is the seventeenth entry, a scope marker with none. `ref` is the one further value the key takes, and it never runs: at instantiation it places another template at its position and is gone. The primitives around all of them, colony, edge, hop and mutation, are on [`meclaw.md`](meclaw.md).

The catalog stays small on purpose. A type exists when a job needs its own state model, its own I/O or its own security boundary. Everything else is composition: an `llm` cell has no inner loop, so tool loops, ReAct and plan-and-execute are topology rather than types of their own.

### [`hive`](cell-types.md#hive-a-scope-marker-and-a-transit-node)

A directory whose type is `hive` is a scope marker, not an actor: no task, no mailbox, no `cell.db`. It is the authority and mutation boundary for its path prefix, and in routing it is transit, so a message addressed to it is evaluated against its out-edges instead of delivered into it. An edge that crosses it has the hive as its endpoint, never a cell inside; `params.ports` makes the substrate enforce that. Take one to group cells into a unit a parent can attach without knowing the inside, which is what every composite template does.

### [`llm`](cell-types.md#llm-inference-through-a-provider-adapter)

A bridge to a provider, consuming and emitting the universal body format. Exactly one provider call per inference message, and the incoming `messages[]` is not passed through: whoever wants a thread across several steps builds it as topology, for instance a memory hive in front of the cell. Take it wherever a model has to answer. `talky` and `cogny` hold one each, which is what lets an `assistant` give its two brains different models.

```json
"cell": { "type": "llm" },
"params": {
  "provider": "openai",
  "model": "openai/gpt-4o-mini",
  "api_key": "${OPENROUTER_API_KEY}",
  "base_url": "https://openrouter.ai/api/v1"
}
```

`base_url` is the line that decides the provider: any OpenAI-compatible endpoint fits, and the `${VAR}` in `api_key` is substituted from `.env` by the colony before the cell is handed its params.

### [`store`](cell-types.md#store-typed-persistent-storage)

A CRUD cell with its own `cell.db`. Tables come from `params.schema` and the cell creates them; names pass a syntax gate, and caller text reaches SQL only as bind parameters. One query message is answered by one message, also when it carries several `tool_call` turns. Take it wherever something a colony produces has to outlive its message; `shelf` is the one-cell template, and eighteen shipped templates carry a store inside.

```json
"cell": { "type": "store" },
"params": {
  "schema": { "rows": { "id": "text", "at": "text", "text": "text", "meta": "json" } },
  "query_timeout_ms": 5000
}
```

`schema` is what makes one store template useful twice: an `override_params` at instantiation turns the same `shelf` into a different shelf.

### [`code`](cell-types.md#code-a-programmable-body-constructor)

Runs a script in a declared language (Python today) and builds the outgoing body itself, headers, `messages[]` and its own top-level slots included. Where `bash` emits output, `code` constructs it, which is why dissecting a model answer, extracting tool calls, transform logic and multi-send all live here. It is the only type without a fixed emission mode, because the script decides per run. `scriptlet` ships it blank for an `add_nodes` entry to name, and thirty-two shipped templates carry a `code` cell inside.

```json
"cell": { "type": "code" },
"params": {
  "runner": "python3",
  "script_inline": "import sys, json\nd = json.load(sys.stdin)\n…",
  "external_timeout_ms": 10000
}
```

`script_inline` is the whole interface: the script reads one JSON document of `envelope`, `body` and `params` on stdin and writes the complete content JSON on stdout.

### [`web_fetch`](cell-types.md#web_fetch-an-outbound-http-client)

A pure HTTP tool, stateless and without a `cell.db`. Only `GET` is implemented; the other methods are a roadmap defer. The URL rides on the wire rather than in the config, so one instance serves every caller. Take it to read a document off the network: `fetcher` is the single-cell template, and `daily-digest`, `freeswitch` and `tools` each wire one.

### [`web_search`](cell-types.md#web_search-a-search-provider-client)

A stateless client for an external search provider (Brave, Tavily, SerpAPI), one `tool_result` turn per request. Take it when an agent has to find a page rather than read one it already knows. The usual place is beside `web_fetch` on a tool surface, which is how `tools` wires it.

### [`file`](cell-types.md#file-filesystem-operations)

`read`, `write`, `list` and `stat` inside a mandatory `base_path`, stateless, one `tool_result` per operation. The boundary is checked lexically before anything touches the filesystem, so every escape attempt answers the same whether the target out there exists or not. Take it to give an agent files without giving it a shell; `tools` carries one.

### [`edit`](cell-types.md#edit-file-editing-operations)

`find_replace` and `insert_at_line` behind the same `base_path` boundary, stateless. It exists because replace-ALL with an ambiguous pattern silently patches sites the caller never saw, the highest-risk failure mode while coding, so the optional `expected_matches` turns the match count into a precondition. Take it beside `file` rather than instead of it, the way `tools` does.

### [`bash`](cell-types.md#bash-shell-execution)

Runs shell commands, one-shot only, in a fresh shell per call. A persistent interactive session is deliberately not offered: stateful, fragile, hard to sandbox. Where `cwd` and `env` have to survive several commands, they are persisted and passed per call instead of kept in a living shell. Take it when a command and its stdout are the whole job; anything that has to rework the body is `code`.

### [`proxy`](cell-types.md#proxy-a-bridge-to-an-external-chat-platform)

Long-running, bridging one external chat platform, Telegram or Slack, per instance. It holds the update cursor in its `cell.db`, so a restart does not replay messages already seen. Take it to put a chat in front of an agent: `telegram-connector` is that cell as a whole template, with no persona and no answer of its own.

### [`timer`](cell-types.md#timer-a-periodic-event-emitter)

Long-running, second-accurate, holding its schedules in `cell.db`. The cron format is 6-field Quartz style, and the next occurrence is computed from the running second rather than from a polling grid, so firing does not drift with wake latency. One-off and repeating events both. Take it for real time semantics only, never to ask whether something changed; `clock` is the one-cell template, and seven shipped templates carry a timer inside.

### [`mcp`](cell-types.md#mcp-a-bridge-to-an-mcp-provider)

Long-running, bridging one MCP provider over `http` or `stdio`, with a tool-discovery cache in `cell.db`. Take it when the tools an agent needs already exist as an MCP server, instead of building a cell per tool. No shipped template declares one, so the reference is the shape to copy from.

### [`harness`](cell-types.md#harness-an-agent-harness-as-a-supervised-child-process)

Long-running, operating a whole agent harness, Claude Code today, as a supervised child process. The harness prompts, loops and uses its own tools, because the job here is delegating a coding task rather than a model call. One child per task, with session continuity from the harness's own `--resume`. Parallelism is topology: several cells, each with its own worktree. No shipped template declares one.

### [`subcolony`](cell-types.md#subcolony-a-child-colony-as-one-cell)

Long-running, operating a complete child colony, its own binary, root, `colony.json` and cell tree, over the stdin/stdout bridge. There is no in-process nesting: a colony stays one process with one tree, and nesting happens across the process boundary. From the outside the child is exactly one addressable cell, which is what makes it an opaque composition facade. No shipped template declares one.

### [`vault`](cell-types.md#vault-a-sealed-secret-store-with-no-read)

A secret store whose route surface has no `get`. It has `put`, `rotate`, `use`, `revoke`, `status`, `unlock`, `lock` and `deliver`, and nothing else: a compromised model can ask it to use a secret inside a granted scope or to seal one for a named recipient, but the question "show me" has no name here. That is the difference from a store plus a rule, because a rule is an argument that can be won and a missing operation is not. `vault` is the standalone template, `access/vault` the interior variant inside the broker.

### [`web`](cell-types.md#web-a-port-owning-display-substrate)

Long-running, binding the port named in its own `params` and holding its own `cell.db`, with server-side rendering and LiveView diffs. The cell owns the listener, so a colony opens a second display by instantiating the type again on another port, and the two share nothing but the substrate. Take it wherever something has to be shown instead of told; `web` is the one-cell template, and `display` puts a composer and a store of what is up in front of one.

### [`voice`](cell-types.md#voice-real-time-speech-as-a-channel)

Long-running, built like `web` with audio instead of a page: one WebSocket connection is one client. Audio terminates in the I/O half and no sample ever enters a mailbox, because a message here is a routed, logged JSON body and a stream of samples is not. What travels inwards is text turns, what travels back out is speech. One instance per member is the shape; `voice` is the template, and `freeswitch` puts a telephone in front of it.

Two things on this page are substrate rather than cell type, and both are documented once for all types: the `transfer` slot that every cell with a `cell.db` answers, in [`cell-types.md`](cell-types.md#content-transfer-through-the-transfer-body-slot), and the universal body format they consume and emit, in [`meclaw-overview.md`](meclaw-overview.md#body-format-universal).
