# examples/14a-tool-loop — tool-loop demo topology (phase 14-A)

A checked-in tree of the **final end-to-end topology** of the phase-14-A tool-loop mechanics
(store-free, single iteration). The phase-14-A integration tests copy `main/` into a
`TempDir` and boot exactly this tree — the checked-in tree stays clean, the runtime state
(`cell.db`/`colony.db`) lands exclusively in the `TempDir`.

## Topology

```
user → /llm → /tool-loop (Hive-Transit) → dispatcher → tool-a → collector → /sink
```

- **`/` (root hive)** — out-edge `./llm → ./tool-loop`, CEL condition `hop.finish_reason == 'tool_calls'`.
- **`/llm`** — `llm` cell (OpenAI provider). `params.base_url` is a **deliberately
  non-functional placeholder** here (`http://127.0.0.1:1/v1`). The test **overrides `base_url`
  after the copy** with the deterministic `MockOpenAI` (no real keys). `base_url`
  is the real override point — grounded in the code via `LlmCellFactory`/`LlmParams`.
- **`/tool-loop` (hive)** — an addressable transit node. Out-edges:
  `. → dispatcher` (`finish_reason == 'tool_calls'`), `dispatcher → tool-a`
  (`msg_type == 'tool_call'`), `tool-a → collector`, `collector → /sink`.
- **`dispatcher`** — `code` cell (`multi_send_capable`), splits the llm `tool_call` output into
  typed `tool_call` messages.
- **`tool-a`** — `code` cell, a deterministic tool-endpoint surrogate (emits a `tool_result`
  `"42"`, no external I/O).
- **`collector`** — `code` cell, store-free: passes the `tool_result` turns on to `/sink`
  (`msg_type = "collected"`). NO thread rebuild, NO store, NO second llm call — that is 14-B.

## `/sink`

`/sink` is **not** part of the tree. The test registers it as a `CaptureCell` via `h.spawn(...)`
**before** the bootstrap (a positive receipt as proof; anti-cascade, phase-6.5 lesson).

## Topology picture

[`topology.svg`](topology.svg) — rendered from the **live booted** graph of the
end-to-end test (`/colony/graph` per scope, merged). Hive = transit shape (diamond, dashed),
cell = box, edge label = CEL condition, `/sink` marked as a test probe.
