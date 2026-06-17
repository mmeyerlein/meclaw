# examples/14a-tool-loop — Tool-Loop-Demo-Topologie (Phase 14-A)

Eingecheckter Baum der **finalen End-to-End-Topologie** der Phase-14-A-Tool-Loop-Mechanik
(store-frei, Single-Iteration). Die Phase-14-A-Integrationstests kopieren `main/` in einen
`TempDir` und booten genau diesen Baum — der eingecheckte Baum bleibt sauber, der Laufzeit-State
(`cell.db`/`colony.db`) landet ausschließlich im `TempDir`.

## Topologie

```
user → /llm → /tool-loop (Hive-Transit) → dispatcher → tool-a → collector → /sink
```

- **`/` (Root-Hive)** — Out-Edge `./llm → ./tool-loop`, CEL-Condition `hop.finish_reason == 'tool_calls'`.
- **`/llm`** — `llm`-Cell (OpenAI-Provider). `params.base_url` ist hier ein **bewusst
  nicht-funktionaler Placeholder** (`http://127.0.0.1:1/v1`). Der Test **überschreibt `base_url`
  nach dem Copy** auf den deterministischen `MockOpenAI` (keine Real-Keys). `base_url`
  ist der echte Override-Punkt — am Code geerdet über `LlmCellFactory`/`LlmParams`.
- **`/tool-loop` (Hive)** — adressierbarer Transit-Knoten. Out-Edges:
  `. → dispatcher` (`finish_reason == 'tool_calls'`), `dispatcher → tool-a`
  (`msg_type == 'tool_call'`), `tool-a → collector`, `collector → /sink`.
- **`dispatcher`** — `code`-Cell (`multi_send_capable`), zerlegt den llm-`tool_call`-Output in
  typisierte `tool_call`-Messages.
- **`tool-a`** — `code`-Cell, deterministisches Tool-Endpoint-Surrogat (emittiert `tool_result`
  `"42"`, keine externe I/O).
- **`collector`** — `code`-Cell, store-frei: reicht die `tool_result`-Turns an `/sink` weiter
  (`msg_type = "collected"`). KEIN Thread-Rebuild, KEIN store, KEIN zweiter llm-Call — das ist 14-B.

## `/sink`

`/sink` ist **nicht** Teil des Baums. Der Test registriert es als `CaptureCell` via `h.spawn(...)`
**vor** dem Bootstrap (positives Receipt als Beweis; Anti-Cascade, Phase-6.5-Lesson).

## Topologie-Bild

[`topology.svg`](topology.svg) — gerendert aus dem **live gebooteten** Graph des
End-to-End-Tests (`/colony/graph` je Scope, gemerged). Hive = Transit-Form (Raute, gestrichelt),
Cell = Box, Edge-Label = CEL-Condition, `/sink` als Test-Sonde markiert.
