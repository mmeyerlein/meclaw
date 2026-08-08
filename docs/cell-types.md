# Cell-Types

Detail-Spec der Built-in Cell-Types. Bei Konflikt zwischen dieser Datei und `meclaw-overview.md` gewinnt die overview — sie ist Single Source of Truth.

> **Concurrency-Hinweis**: jede Cell ist in Colony's Registry mit einem uniformen `ActorHandle` (`mpsc::Sender<Message>`) registriert; was hinter der Mailbox läuft, hängt von der Cell-Klasse ab. Stateful Cells: **eine** langlebige `cell_task` mit direktem `handle()`-Aufruf. Stateless Cells: **eine** langlebige `stateless_dispatcher`-Task, die pro Message eine kurzlebige Worker-Task spawnt (Concurrency-Limit pro Cell via `params.max_concurrency`, default unbeschränkt). Long-Running-Cells (`proxy`, `timer`, `mcp`): **zwei** Tokio-Tasks (Handler + I/O), kommunizierend über internen mpsc — siehe `meclaw-overview.md`, Abschnitte „Cell-Modell", „Stateless-Cell-Dispatcher" und „Long-Running-Cells: Doppel-Task". Cell-State ist aus Sicht des jeweiligen Handler-Tasks immer single-threaded zugreifbar; `Mutex`/`RwLock`/atomics in Cell-Code sind verboten.

> **Timeout-Disziplin**: jede I/O-Operation in Cell-Code (HTTP, DB, Subprozess, Filesystem, MCP) wird mit einem eigenen `tokio::time::timeout` umschlossen (Konzept A, „Operation-Timeout"). Bei Elapsed: Cell emittiert eine reguläre Error-Message und beendet `handle()` regulär — kein Restart. Konfiguration pro Cell-Instanz via `params.external_timeout_ms` (oder semantisch passender Name, z.B. `query_timeout_ms` für `store`). Zusätzlich greift der Substrat-Backstop `cell.message_timeout` (Konzept B) als grober Schutz für Cell-Hänger aus unbekanntem Grund. Details und empfohlene Defaults pro Cell-Type: `meclaw-overview.md` Abschnitt „Timeouts".

**Cell-Bauarten** (Detail in `meclaw-overview.md` Abschnitt „Cell-Bauarten bzgl. `messages[]`"):

- **atomisch-emittierend**: Cell emittiert eine frische `messages[]` mit nur ihrem eigenen Beitrag, kein Pass-Through. Default für alle Tool-Endpoints, Quellen und LLM-Inferenz.
- **stream-fortpflanzend**: Eingangs-`messages[]` wird durchgereicht und um den eigenen Beitrag ergänzt. Im Built-in-Set nicht vertreten — anwendungsspezifisch über `code`-Cells baubar.
- **vom Skript bestimmt** *(Sonderfall)*: Bauart entsteht pro Execution aus dem Skript-Output — nur `code`.

## Übersicht

| Type | Aufgabe | Aktor? | Bauart | Phase |
|---|---|---|---|---|
| `hive` | **Scope-Marker** (Authority- und Mutations-Boundary für Pfad-Präfix) + **logischer Transit-Knoten** im Routing-Graph | **nein** — kein Aktor, keine Mailbox, keine `cell.db` | — (Transit, keine Zustellung) | 4 |
| `store` | typisiertes SQLite-Storage mit Schema + Seed | ja, stateful | atomisch-emittierend | 9 |
| `llm` | LLM-Inferenz, hält System-State + Blob-Cache | ja, stateful | atomisch-emittierend | 8 |
| `bash` | Shell-Ausführung (one-shot only) | stateless | atomisch-emittierend | 7 |
| `code` | programmierbarer Body-Konstruktor (Python first) | ja, **stateless** (Stateless-Dispatcher) — Phasen-Limitation, stateful-`code` mit `cell.db` deferred | vom Skript bestimmt | 9 |
| `web_fetch` | HTTP-Client | stateless | atomisch-emittierend | 7 |
| `web_search` | Search-Provider-Client | stateless | atomisch-emittierend | 7 |
| `file` | Filesystem-CRUD mit Security-Boundary | stateless | atomisch-emittierend | 7 |
| `edit` | File-Editing-Operationen | stateless | atomisch-emittierend | 7 |
| `proxy` | External-Chat-Bridge (Telegram first), Doppel-Task | ja, long-running | atomisch-emittierend (User-Turn pro externer Message) | 10 |
| `timer` | periodischer Event-Emitter, sekundengenau, Doppel-Task | ja, long-running | atomisch-emittierend (Schedule-Body) | 10 |
| `mcp` | MCP-Provider-Bridge, Doppel-Task | ja, long-running | atomisch-emittierend | 10 |

**Status pro Cell-Type / pro Phase** (welche Cell ist heute live, welche deferred) → `PROGRESS.md` § Status.

---

## `hive` — Scope-Marker + logischer Transit-Knoten (kein Aktor)

**Kein Cell-Type im klassischen Sinne**, sondern ein Scope-Marker mit zusätzlicher Transit-Rolle im Routing-Graph. Ein Verzeichnis mit `config.json` `type: "hive"` markiert einen Pfad-Präfix als Authority- und Mutations-Boundary für seinen Subtree. Es gibt **keine** Hive-Task, **keine** Hive-Mailbox, **keine** Hive-eigene `cell.db`, **keinen** `ActorHandle`-Eintrag in Colony's Registry. Routing, Lifecycle, Mutations-Validierung und UUID-Vergabe laufen zentral über die Colony — siehe `meclaw-overview.md` Abschnitte „Authority-Modell" und „Nebenläufigkeit & Parallelität".

**Wirkung in der DSL**: Verzeichnis-Verschachtelung gruppiert Cells zu einer logischen Einheit (z.B. `/main/tool-loop/dispatcher`, `/main/tool-loop/collector`). Mutationen können den Hive-Pfad als Scope-Feld nutzen — alle Diff-Operationen darin werden relativ zu diesem Pfad-Präfix aufgelöst, und Colony lehnt Mutationen ab, deren Pfade außerhalb des Scopes liegen würden.

**Wirkung im Routing — Transit, keine Zustellung**: ein Hive ist aus Sender-Sicht ein **adressierbares Ziel**, im Substrat ein **Transit-Hop**. Trifft eine Message mit `target = <hive-path>` ein, stellt Colony **nicht** in eine Mailbox zu (es gibt keine) — sie wertet stattdessen die Out-Edges des Hives (`EdgeTable`-Einträge mit `from = <hive-path>`) als Teil ihrer einen Routing-Schicht aus: CEL-`condition` gegen Headers, `modifier` anwenden, regulärer Routing-Hop pro Treffer auf den jeweiligen `to`-Pfad, TTL pro Hop dekrementiert. Kein Hive-eigener Auswerter, keine separate Routing-Logik — siehe `meclaw-overview.md` Abschnitt „Hive-Pfade als Target — Transit-Auswertung". Bei keiner matchenden Out-Edge: Dead-Letter mit `error_code = "hive_no_route"`. Graph-Reads für einen Hive-Scope laufen über `/colony/graph?scope=<hive_path>` (siehe `meclaw-overview.md` Abschnitt „Visibility / Read-Pfade").

**Konnektivität des Hives**: Ob ein Hive aktiv ist, entscheiden ausschließlich die Edges der
Eltern-Ebene, die seinen Pfad referenzieren — seine interne Verkabelung zählt nicht (siehe
`meclaw-overview.md` § Konnektivität & Aktivität). Ein disconnecteter Hive deaktiviert seinen
gesamten Subtree. Genau das macht Hives zum Anschlusspunkt für komplexe Templates: ein
instanziiertes Subtree-Template wird über Edges an seinen Hive-Pfad angeschlossen — die
interne Struktur muss der Anschließende nicht kennen.

**`params`** — **ausschließlich `graph`** (der `HiveParams`-Deserializer ist `deny_unknown_fields`; jeder andere Schlüssel ist ein Boot-Fehler):
- `graph` (optional): initialer Soll-Graph für den Subtree (Format siehe `meclaw-overview.md` Abschnitt „Graph-Schema"). Colony liest das beim Filesystem-Bootstrap und trägt die deklarierten Cells in die Registry und die Edges in `colony.db` ein. Nach dem ersten Bootstrap ist die persistierte Edge-Tabelle in `colony.db` die Wahrheit — `params.graph` ist nur initialer Hint.

Kein scope-eigener `dead_letters`-Override: die Dead-Letter-Queue ist immer `/colony/dead_letters` (Hive = Authority-/Mutations-Boundary, **nicht** DLQ-Boundary). Sonst keine Hive-Type-eigenen Felder. Insbesondere keine Routing-Konfiguration, keine Mailbox-Größe, keine eigene Bauart-Aussage — Hives haben keinen Aktor und keine Mailbox; ihre Routing-Rolle ist passive Transit-Auswertung durch Colony über die `params.graph`-Edges.

---

## `store` — typisiertes persistentes Storage

**Aufgabe**: CRUD-Cell mit eigener `cell.db`. Schema und Spalten-Typen können in `params.schema` definiert werden; die Cell legt die Tabellen daraus an. Dynamisch kann sie auch per Message eine neue Tabelle anlegen. Tabellen- und Spaltennamen unterliegen einem Syntax-Gate (P3, 2026-08-08): `[A-Za-z_][A-Za-z0-9_]{0,62}`, kein `sqlite_`-Präfix, kein `_fts`-Suffix. In SQL formatiert wird ausschließlich, was der SQLite-Katalog (`sqlite_master`/`pragma_table_info`) selbst zurückgibt oder aus einem internen Enum stammt — Caller-Text erreicht Statements nur als Bind-Parameter.

**Bauart**: atomisch-emittierend. Pro Query-Message eine Response-Message mit dem Resultat als Turn.

**Eingabe-Format** (Phase-9 Brainstorm E7, analog `bash`): strukturierte JSON-Args im `tool_call`-Turn. Pflichtfeld `operation` (`"insert"`/`"select"`/`"update"`/`"delete"`/`"create_table"`/`"search"`/`"traverse"`/`"similar"`) + `table`, plus operationsspezifische Felder:

- `insert`: `row` (Objekt `{ "<column>": <value> }`).
- `select`: `columns` (**Pflicht** — Array von Spalten-Namen mit mindestens einem Eintrag; die Projektion) + optional `where`, `order_by` (Array von `{ "col": "<column>", "dir": "asc"|"desc" }`, multi-column) und `limit` (Integer ≥ 1, **kein** impliziter Default, kein Cap — der Runaway-Guard ist `query_timeout_ms`). Es gibt **keinen** projektionslosen `SELECT *`: fehlt `columns` oder ist es leer, antwortet die Cell mit `finish_reason: "error"` und `error_code: "invalid_input"` (kein Cell-Crash; Doku-an-Code-Korrektur, ruling 2026-08-08). Das Resultat ist ein Array von Zeilen-Objekten, projiziert auf die angeforderten Spalten.
- `update`: `set` (Objekt) + optional `where`.
- `delete`: optional `where`.
- `create_table`: `columns` als **2-Stufen-Map** `{ "<column>": "<type>" }` (Typen `text`/`int`/`json`) — **nicht** `schema`.
- `search` (P3): `match` (**Pflicht** — FTS5-Query-Syntax) + `columns` (**Pflicht**, wie `select`) + optional `where`/`order_by`/`limit`. Nur auf Tabellen mit `params.fts`-Deklaration (sonst `invalid_input`). Jede Ergebnis-Zeile trägt zusätzlich die Spalte `rank` (bm25, kleiner = besser); ohne `order_by` ist `rank` die Default-Ordnung.
- `traverse` (P4): Multi-Hop über eine Edge-Tabelle per rekursiver CTE, **gerichtet** `src`→`dst`. Args: `table` + Spalten-Rollen `src`/`dst` (optional `kind`/`weight` — alle katalog-validiert), `start` (Bind-Wert), optional `where` (voller Operatorensatz, gilt pro Kante) und `columns` (zusätzliche Edge-Spalten in den Pfad-Zeilen), Guards `max_depth` (Default 2, Cap 5) und `max_nodes` (Default 200, Cap 5000) — Werte über dem Cap ⇒ **Reject** (`invalid_input`), kein stilles Clampen. Zyklen-Eliminierung pro Pfad einschließlich des Start-Knotens (eine Kante zurück zum Ursprung wird geprunt). Ergebnis ist ein **Objekt-Payload** `{ paths, truncated, max_depth, max_nodes }`; jede Pfad-Zeile trägt Endknoten, Tiefe, Pfad-Array, Edge-Attribute und akkumuliertes Gewicht. **Kein** `order_by` (BFS-artige Expansion; die Reihenfolge innerhalb einer Tiefe ist nicht Teil des Kontrakts); `truncated: true` macht das Abschneiden durch `max_nodes` sichtbar.
- `similar` (P4): Ähnlichkeits-Ranking über eine Vektor-Spalte via registrierter `hamming()`-Scalar-Function. Args: `table`, Vektor-Spalte, Query-Vektor (Bind), optional `where`/`order_by`/`limit`, `columns` (darf `distance` **nicht** enthalten). Jede Ergebnis-Zeile trägt `distance` (kleiner = besser); Default-Ordnung `distance` aufsteigend mit `rowid`-Tiebreaker. Vektoren sind **Base64-TEXT** (primär; echte BLOBs werden zusätzlich akzeptiert — ein nativer Blob-Schreibpfad ist roadmap-Defer), striktes Base64 (Reject bei Alphabet-, Padding- und Längenfehlern), `NULL` → `NULL`; **Längen-Mismatch zweier Vektoren ⇒ lauter `sql_error`** (ein Mismatch ist praktisch immer ein Bruch der Embedding-Generationen-Disziplin, kein stiller Skip). Die Op ergänzt **immer** implizit `<vektor-spalte> IS NOT NULL` — `NULL`-Embeddings (Backfill-Queue) würden sonst an Platz 1 ranken. Known limits: keine erzwungene Modell-Gleichheit (der Caller filtert `model_id` selbst), kein ANN-Index — Full-Scan über die gefilterte Menge.

`columns` hat damit je nach Operation eine andere Form: bei `select` ein **Array von Spalten-Namen** (Projektion), bei `create_table` eine **2-Stufen-Typ-Map**. `where`: pro Spalte entweder ein nackter Wert (Kurzform für `eq`) oder ein Operator-Objekt mit genau einem Schlüssel aus `eq`, `neq`, `lt`, `lte`, `gt`, `gte`, `in` (Array), `is_null` (bool), `or_null` (umschließt genau einen Vergleichsoperator, Tiefe 1). Ein Objekt mit unbekanntem Schlüssel ⇒ `invalid_input`. Die Operator-Formen gelten einheitlich für `select`/`search`/`update`/`delete` (ein gemeinsamer `build_where`-Pfad). `schema` ist ausschließlich der `params`-Block (Bootstrap-Tabellen). Phase 9 akzeptiert nur `tool_call`-Turns; direkte Verwendung mit `user`/`system`-Origin (s.u.) ist Phase-9-Limitation.

**Body-Format der Response**: `messages[]` mit einem einzelnen Turn. Bei Tool-Loop-Verwendung typisch `{ origin: "tool", type: "tool_result", text: "<json-serialisiertes Ergebnis>", id: "<tool_call_id>" }`. Bei direkter Verwendung außerhalb eines Tool-Loops kann der `origin` je nach Anwendungs-Konvention auch `user` oder `system` sein; `id` entfällt dann.

**Output-Header** (`hop`-Fach — verfallen bei der nächsten Cell-Emission): `operation`, `rows_affected`, `duration_ms`, optional `error_code`.

**Failure-Klassifikation** (Phase-9 Brainstorm E5, analog `bash`): SQL-Errors (Constraint-Verletzung, Type-Mismatch, unknown table/column) sind **reguläre `tool_result`-Turns** mit `header.error_code` (`"sql_error"` / `"unknown_table"` / `"unknown_column"` / `"type_mismatch"` / `"constraint_violation"` / `"query_timeout"` / `"invalid_input"` bei malformierten Args bzw. unbekannter Operation) — **nicht** `finish_reason: "error"`. Begründung: LLM/Caller liest den Code und entscheidet (Retry, Schema-Korrektur, andere Operation). Nur interne Fehler (DB-Korruption, Spawn-Fehler) lösen Cell-Crash + Restart aus. `unknown_column` deckt seit P3 auch `select`/`where`/`order_by` ab (vorher nur den `insert`-Pfad über den SQLite-Fehlertext). Die `traverse`-/`similar`-Fehlerfälle (P4) bilden auf die bestehenden Codes ab — `invalid_input` für Guard-/Arg-Verstöße, `unknown_table`/`unknown_column` über den Katalog, `sql_error` für Vektor-Mismatch, `query_timeout` — **kein neuer Code**.

**`params`**:

- `schema` (Phase-9 Brainstorm E6): 2-Stufen-Map `{ "<table>": { "<column>": "<type>" } }` mit Typen `text` / `int` / `json`. Constraints (PK / NOT NULL / UNIQUE / Default / Index) sind in Phase 9 **deferred** — eigener Design-Pass nötig.
- `fts` (P3): Map `{ "<table>": ["<column>", …] }` — aktiviert einen FTS5-Volltextindex (external-content-Tabelle + Trigger) über die genannten Spalten. Nur Tabellen aus `params.schema`, nur `text`-/`json`-Spalten; **kein FTS für per `create_table` angelegte Tabellen** (Known limit P3). Immutable wie `schema`. Bestands-`cell.db`s bauen den Index beim nächsten Spawn einmalig auf — auch für vor der Deklaration geschriebene Zeilen; weicht eine deklarierte Spalte vom Live-Schema ab ⇒ lauter Spawn-Fehler.
- `query_timeout_ms` (Konzept A, siehe overview § Timeouts): pro Query erzwungener Timeout via `DbConn`'s `InterruptHandle`; unterbricht nachweislich auch eine laufende rekursive CTE (`traverse`).
- Optional Seed-Daten (Convention-Pfad `seed/<table>.jsonl`). Seed greift nur bei `OpenStatus::Created` der `cell.db` — siehe overview § Seed-Konzept.

**Laufzeit-Param-Updates (β, `config.md` § Zugriff Z.20):** wie `llm` (siehe dort) — Top-Level-`params`-Body-Slot, partial, last-write-wins, in der `cell.db` persistiert, bei wake/respawn über die Geburts-params replayt. **Mutabel:** `query_timeout_ms` — wirkt **sofort live** (der laufende `DbConn` übernimmt den neuen A-Timeout für den nächsten Query, ohne wake/respawn). **Immutable je `store`:** `schema` (bootstrap-only — beim Spawn per DDL in die `cell.db` gebacken; eine Laufzeit-Änderung würde die Live-Tabellen von der deklarierten Schema desynchronisieren). Update-Versuch auf `schema` oder ein unbekannter Key ⇒ lauter Reject (`error_code: "invalid_input"`), kein Teil-Apply.

---

## `llm` — LLM-Inferenz via Provider-Adapter

**Aufgabe**: Bridge zu einem LLM-Provider. Konsumiert und emittiert Universal-Body-Format (siehe `meclaw-overview.md` Abschnitt „Body-Format (Universal)"). **Keine innere Loop** — pro Inferenz-Message genau ein Provider-Call. Iteration (Tool-Loops, ReAct, Plan-and-Execute, …) entsteht durch Topologie.

**Bauart**: atomisch-emittierend. Pro Inferenz-Call emittiert die `llm`-Cell genau einen neuen assistant-Turn — Eingangs-`messages[]` wird **nicht** durchgereicht. Wer den Konversations-Faden über mehrere Schritte zusammenhalten will, baut das per Topologie (z.B. eine Memory-Hive vor der `llm`-Cell, die History aggregiert und dem nächsten Call mitgibt). Konsistent mit der „Messages sind atomar"-Disziplin und der Cell-Bauarten-Tabelle in `meclaw-overview.md`.

**Inferenz-Trigger**: ausschließlich `messages[]`. System-Updates (Pfade unter `system.*`) akkumulieren in `cell.db` ohne Provider-Call.

**State in `cell.db`**:
- `system.*` — akkumulativ-replace pro Pfad. Bootstrap-Kontext (Persona, Tool-Schemas, Facts). Updates kommen per Message von beliebigen Cells; Sender kennt die Struktur nicht.
- `messages[]` — last-received as-is (Blob-Refs unaufgelöst, keine appended Turns).
- **Nicht in cell.db**: appended assistant-Turn (Output), Blob-Cache (nur in-memory).

**`params`**:
```json
{
  "provider":    "openai",
  "model":       "gpt-4o",
  "api_key":     "${OPENAI_KEY}",
  "base_url":    null,
  "temperature": 0.7,
  "max_tokens":  4096,

  "external_timeout_ms": 110000,

  "system_order":   ["identity", "facts", "instructions", "tools"],
  "provider_extra": { },

  "http_referer": "${OPENROUTER_HTTP_REFERER}",
  "x_title":      "${OPENROUTER_X_TITLE}"
}
```

- `external_timeout_ms` (Konzept A, siehe overview § Timeouts): A-Timeout um den Provider-HTTP-Call (`tokio::time::timeout`), Default `110000` (110 s). Bei Elapsed: reguläre Error-Message mit `finish_reason: "error"`, `error_code: "timeout"`.

- `provider` (Phase 8): **nur `"openai"`** (inkl. OpenAI-kompatible Endpoints via `base_url`). Der Wert ist als Enum angelegt, aber Phase 8 implementiert ausschließlich den OpenAI-Translate. Weitere Provider (insb. `"anthropic"`, Messages-API nativ) sind **deferred** — kein fester Phasen-Bezug (siehe „Multi-Provider" unten). Ein nicht-`openai`-Wert ist in Phase 8 ein `model_not_found`/`invalid_input`-äquivalenter Konfigurationsfehler beim Spawn.
- `base_url` überschreibt Provider-Default (nützlich für lokale/proxied Endpoints wie LiteLLM, Ollama, vllm — alle über den OpenAI-kompatiblen Wire).
- `system_order`: optionale Reihenfolge der `system.*`-Sub-Slots bei Konkatenation zum Provider-System-String. Nicht gelistete Sub-Slots kommen danach in alphabetischer Reihenfolge.
- `provider_extra`: freier JSON-Block für Provider-spezifische Knobs (Phase 8: z.B. OpenAI `seed`). Overlay über Common-Params bei Konflikten. Provider-fremde Knobs (z.B. Anthropic `cache_control`) sind erst mit dem jeweiligen Provider-Translate aktiv.
- `http_referer` / `x_title`: optionale Provider-Attribution (OpenRouter `HTTP-Referer` / `X-Title`). **Reguläre params** (Audit-Ruling A4, params-uniform): in `config.json` gesetzt, via `${VAR}` aus `.env` substituiert wie jeder andere param — **kein** Code-Pfad liest `.env` direkt, **keine** Sonder-Header-Mechanik. Unset (`null`/weggelassen) ⇒ der Header wird **nicht** gesendet. Das Wire-Ziel (HTTP-Request-Header statt Request-Body) entscheidet die Translate-Grenze (siehe „Provider-Translate" unten).

**Laufzeit-Param-Updates (W4b, `config.md` § Zugriff Z.20):** params sind Cell-**Inhalt**, kein Topologie-Zustand — sie ändern sich per **Message**, nicht per Mutation. Die Form ist ein **Top-Level-`params`-Body-Slot** (1:1 der `config.json`-`params`-Block), partial, **last-write-wins pro Key**:

```json
{ "params": { "model": "gpt-4o-mini", "temperature": 0.4 } }
```

Reihenfolge in einer Message: der `params`-Slot wird **zuerst** gemerged + in der `cell.db` persistiert, **dann** läuft eine ggf. mitgesendete `system`/`messages`-Inferenz mit den **aktualisierten** params (derselbe Call nutzt schon das neue Modell / die neue Attribution). Eine **params-only**-Message (Slot ohne `system`/`messages`) persistiert und schweigt (kein Emit, analog system-only). `config.json` divergiert dabei vom Live-Stand — **gewollt**; bei wake/respawn replayt die Cell ihr `cell.db`-Overlay über die Geburts-params (`config.json` bleibt der Instanziierungs-Snapshot). **Reset = `cell.db`-Wipe ⇒ Bootstrap-params** zurück.

**Immutable je llm** (Update-Versuch ⇒ **lauter Reject**, `error_code: "invalid_input"`, **kein** Teil-Apply): `api_key` (Credential, Secret-Hygiene — Spiegel des A4-`Authorization`-Rulings) und `provider` (Phase-8-Identität). **Unbekannte** Param-Keys ⇒ ebenfalls lauter Reject (kein stiller No-op). Ein malformter Wert (falscher Typ) ⇒ Reject (All-or-nothing). Die Reject-Detail nennt nur den Key/die Regel, **nie** einen Param-Wert.

**Tool-Definitionen**: leben in `system.tools.<tool_name>.text` als JSON-Strings. Der Adapter parsed sie beim Provider-Call und baut das provider-native Tool-Set. Tools werden **nicht** in den System-Prompt-String konkateniert — separat extrahiert. Tool-Calls und Tool-Results sind eigene `messages[]`-Turn-Types (`type: "tool_call"` / `"tool_result"` mit `id` als Korrelations-Anker, Pass-Through-Wert vom Provider).

**Output-Body**:
- `messages[]` = nur der neue assistant-Turn (kein Pass-Through des Eingangs-`messages[]`)
- `system.*` wird **nicht** emittiert (privater Cell-State)
- `meta` (Cell-spezifischer Top-Level-Slot): `{ provider, model, response_id, latency_ms, started_at, tokens_cache_read?, tokens_cache_creation?, … }`

**Output-Header** (`hop`-Fach — verfallen bei der nächsten Cell-Emission):

| Header | Inhalt |
|---|---|
| `finish_reason` | `"stop"` \| `"length"` \| `"tool_calls"` \| `"content_filter"` \| `"error"` — Pflicht |
| `tokens_prompt` | Input-Token-Count |
| `tokens_completion` | Output-Token-Count |
| `model` | Modell, das der Provider tatsächlich benutzt hat |
| `error_code` | nur bei `finish_reason == "error"`: `"rate_limit"` \| `"auth"` \| `"timeout"` \| `"model_not_found"` \| `"provider_error"` \| `"invalid_input"` (W4b: params-Update-Reject — immutable/unbekannter/malformter Key) |

**Aggregation über Loops** (Total-Cost, kumulierte Tokens): **kein Cell-Feature**. Separate Aggregator-Hive in der Topologie gruppiert über `correlation_id` und ergänzt Pass-Through-Header (`cost_total_usd`, `tokens_total`). Begründung in `meclaw-overview.md` Abschnitt „Metadata-Aggregation ist Topologie".

**Error-Modell**: Provider-Fehler (Rate-Limit, Auth, Timeout etc.) sind **reguläre Output-Messages** mit `finish_reason: "error"` + `error_code`, `messages[]` unverändert (kein Turn angehängt), `meta.error` mit Detail-Info. Topologie kann via Edge-Bedingung Failover machen. Nur interne Fehler (Panic, Bad-Params) lösen Cell-Crash + Restart aus.

**Streaming**: Phase 8 nicht unterstützt (Single-Message-Output). Post-Roadmap.

**Multi-Provider**: Phase 8 implementiert **ausschließlich den OpenAI-Translate** (ein Provider pro Instanz). **Anthropic ist deferred — kein fester Phasen-Bezug.** Die Cell-Logik (UBF-Konsum, `system.*`-Akkumulation in `cell.db`, Tool-Definition-Extraction, atomic-emit, Error-Modell) ist provider-agnostisch; provider-spezifisch ist allein der Translate (siehe „Provider-Translate" unten). Failover/A-B-Test über mehrere Provider läuft via Topologie (zwei `llm`-Cells + Dispatcher-Hive unter einem Hive-Scope), nicht cell-intern. Post-Roadmap zusätzlich denkbar: Cell-interne Provider-Liste für robuste Provider-Anbindung (Cell sichert „Kommunikation zum Provider klappt" über Retries/Failover).

**Provider-Translate (Übersetzungs-Grenze)**: Die `llm`-Cell ist **provider-agnostisch**. Sie konsumiert ausschließlich Universal-Body-Format, akkumuliert `system.*` als UBF in ihrer `cell.db` (UBF ist damit auch ihr internes/persistentes Format) und emittiert genau einen assistant-Turn als UBF. Die gesamte Provider-Kenntnis lebt in einer Übersetzungs-Funktion (hier „Translate", synonym zum in `meclaw-overview.md` genannten „LLM-Provider-Adapter"), die zwei Richtungen kennt: **UBF → provider-natives Request** (System-Konkatenation, `messages[]`-Mapping, `system.tools.*` → provider-natives Tool-Set) und **provider-natives Response → UBF** (assistant-Turn inkl. ggf. `type: "tool_call"`-Turns, Header wie `finish_reason`/Tokens, `meta`-Slot). Konsequenzen, die jeder Phase-8-Implementer einhalten muss:
- **Keine Loop.** Pro Inferenz-Message genau ein Provider-Call, dann Emit. Iteration ist Topologie (siehe `meclaw-overview.md` „Iteration ist Topologie").
- **Kein Composing/Decomposing von Tool-Calls.** Die Cell baut keine Tool-Aufrufe zusammen und löst keine auf. `tool_call`/`tool_result` sind reine UBF-`messages[]`-Turn-Types mit `id` als Pass-Through-Korrelations-Anker (Wert vom Provider). Tool-Schemas werden vom Translate aus `system.tools.*` ins provider-native Tool-Set übersetzt — das ist Format-Übersetzung, kein Tool-Loop.
- **Wire-Merge konsekutiver `tool_call`-Turns (Request-Bau, Ruling 2026-06-11).** Beim UBF→Request-Mapping fasst der Translate **konsekutive** assistant-`tool_call`-Turns zu **einer** provider-nativen assistant-Message mit `tool_calls[]` zusammen — der OpenAI-Wire-Vertrag verlangt, dass auf eine assistant-Message mit `tool_calls` unmittelbar `tool`-Messages zu jeder `tool_call_id` folgen (Run-4b-Wire-Befund: Ein-Call-Messages vor gesammelten Results → 400). Das ist reine Wire-Format-Übersetzung innerhalb der Translate-Grenze, kein Composing auf UBF-Ebene: UBF bleibt unverändert (ein Turn = ein Call = eine `id`), `id`s bleiben Pass-Through. Der Response-Rückweg bleibt unverändert (jedes Provider-`tool_calls[i]` → ein eigener UBF-Turn).
- **Provider-natives JSON verlässt nie die Translate-Grenze.** Der Cell-Core sieht ausschließlich UBF; provider-spezifische Strukturen existieren nur innerhalb des Translate.
- **Param → Wire-Ziel-Mapping (Audit-Ruling A4).** Die Translate-Grenze entscheidet je param das Wire-Ziel — Request-Body-JSON vs. HTTP-Request-Header. Provider-Wissen wohnt damit ausschließlich im Translate. Die explizite Tabelle:

  | param | Wire-Ziel |
  |---|---|
  | `model`, `temperature`, `max_tokens`, `provider_extra` (Overlay) | Request-Body-JSON |
  | `http_referer` | HTTP-Header `HTTP-Referer` |
  | `x_title` | HTTP-Header `X-Title` |

  Die Header-Tabelle ist eine **geschlossene Allow-List**: `Authorization` ist **kein** params-steuerbarer Header — er ist der `api_key`-Bearer und wird allein von der Wire-Schicht gesetzt; ein params-Versuch, ihn zu überschreiben, wird ignoriert (Secret-Hygiene). Nur gesetzte (`Some`) Attribution-params erzeugen einen Header; unset ⇒ kein Header.

Daraus folgt direkt die Deferral-Sauberkeit: ein weiterer Provider (z.B. Anthropic) ist allein ein zweiter Translate plus Enum-Wert — die Cell-Logik, `cell.db`-Semantik und das Error-Modell bleiben unverändert.

---

## `bash` — Shell-Ausführung

**Aufgabe**: führt Shell-Befehle aus — **one-shot only** (`cell.timeout > 0`, Cell terminiert nach jeder Message). Ein persistent-Mode (`cell.timeout: -1`, langlebige interaktive Shell-Session) wird **by-design nicht eingeführt** (Architektur-Ruling 2026-06-08, Design-Record in `archive/roadmap-resolved.md`): zustandsbehaftet, fragil, schwer sandboxbar. `cwd`/`env`-Kontinuität über mehrere Befehle — falls gebraucht — läuft über Persistieren von `cwd`/`env` in der `bash`-`cell.db` + Mitgabe pro one-shot-Call, nicht über eine lebende Shell. Für Programm-Logik, Body-Manipulation oder Multi-Send siehe `code` — die Wahl-Heuristik steht am Anfang der `code`-Sektion.

**State-Modell**: `bash` ist **stateless** im klassischen Sinne (Stateless-Dispatcher, kurzlebige Worker-Tasks) und hat **keine `cell.db`** — konsistent mit der Phase-7-Disziplin „Tool-Cells ohne `cell.db`". Shell-State (cwd, env-Vars, History, offene Prozesse) wird nicht über Calls hinweg gehalten; jeder Call startet eine frische Shell.

**Bauart**: atomisch-emittierend. Pro ausgeführtem Kommando ein `tool_result`-Turn.

**Body-Format der Response**: `messages[]` mit einem Turn `{ origin: "tool", type: "tool_result", text: "<stdout-plus-ggf-stderr>", id: "<tool_call_id falls vorhanden>" }`.

**stderr-Konvention**: stderr lebt **nicht** in einem eigenen Header oder Body-Slot, sondern wird in `text` hinter den stdout-Anteil angehängt, abgegrenzt durch klare Sentinel-Marker (nur eingefügt, wenn stderr nicht-leer):

```
<stdout-content>

##meclaw-stderr-start##
<stderr-content>
##meclaw-stderr-end##
```

Damit liest ein LLM-Konsument den vollen Tool-Output natürlich (stdout zuerst, stderr explizit markiert), und Edges können vor dem `text`-Parse über `header.had_stderr` schnell routen. Verworfen wurden: stderr als eigener Header-String (würde bei großen Compiler-Outputs / Stack-Traces die „Headers = klein"-Disziplin sprengen), stderr als eigener Top-Level-Body-Slot (bricht das natürliche LLM-Konsumenten-Modell „Tool-Output lesen heißt `text` lesen" und erhöht Slot-Inflation), und stderr immer als JSON-Struct in `text` (`{stdout, stderr, exit_code}`, nicht direkt LLM-lesbar ohne Parse-Schritt).

**Output-Header** (`hop`-Fach — verfallen bei der nächsten Cell-Emission): `operation` (= `"bash"`), `exit_code`, `duration_ms`, `had_stderr` (Pflicht, immer gesetzt), `bytes` (Länge des `text`), optional `truncated` (bei langem stdout).

**`params`**: typisch das auszuführende Kommando bzw. die Skript-Pfad-Konvention.

**Phase-7-Konventionen** (Slice-2-Entscheidungen):
- **`exit ≠ 0` ist NORMAL tool_result**: `exit_code` immer im Header (auch =0). LLM/Caller liest den Code und entscheidet. Konsistent mit Claude Code's Bash-Tool.
- **Nur Spawn-Failure, Timeout + ungültiger Input = Error**: `error_code: "io_error"` (spawn) bzw. `"timeout"` (external_timeout elapsed) bzw. `"invalid_input"` (fehlendes/ungültiges `command`-Feld).
- **`exit_code = -1`** bei signal-killed/abnormal-Termination (plattform-unspezifische Convention). Bei Timeout zusätzlich `error_code: "timeout"`.
- **stderr-Sentinel-Format** (nur einfügen wenn stderr non-empty):
  ```
  <stdout>

  ##meclaw-stderr-start##
  <stderr>
  ##meclaw-stderr-end##
  ```
- **`had_stderr: bool`** Header IMMER gesetzt (true/false).
- **Kein Security-Boundary**: bash hat Vollzugriff aufs FS via Shell. Trust-Modell — bash-Cell läuft nur in vertrauenswürdigen Topologien. Sandbox-Ausbau ist Post-Roadmap.
- **Shell**: `/bin/sh -c <command>`. `cwd`/`shell` als params deferred (Operator setzt via `cd /x && cmd` inline).
- **Kein Persistent-bash** (`cell.timeout: -1`): by-design gestrichen (Architektur-Ruling 2026-06-08) — `bash` ist one-shot only, keine deferred Option.
- **Input minimal**: `{"command": "..."}`.
- **Defaults**: `max_concurrency: 4`, `external_timeout_ms: 60000`.

---

## `code` — programmierbarer Body-Konstruktor

**Wahl `bash` vs `code`** (für AI-Builder und Template-Authors):

- Brauchst du nur „Kommando absetzen, stdout/stderr als `tool_result`-Turn emittieren"? → **`bash`** (immer one-shot, auch für Befehlssequenzen — `cwd`/`env`-Kontinuität falls gebraucht via `bash`-`cell.db` pro Call, siehe § `bash`).
- Brauchst du Programm-Logik, die den Body manipuliert, mehrere Messages aus einer macht (Multi-Send), Headers gezielt setzt, oder eingehende `messages[]` umarbeitet? → **`code`**.

**Aufgabe**: führt user-suppliertes Programm in einer deklarierten Sprache aus (Python first; Node und weitere später). Im Unterschied zu `bash` ist `code` ein **Body-Konstruktor**: das Skript bekommt die eingehende Message als JSON, baut komplett selbst die ausgehende Content-JSON — Headers, `messages[]`, eigene Top-Level-Slots, Routing-relevante Headers für Edges. Damit ist `code` das Schweizer Taschenmesser für anwendungs-spezifische Logik: LLM-Outputs zerpflücken, Tool-Calls extrahieren, Transform-Logik, Multi-Send-Dispatcher.

**Begründung dieser Rolle**: ein simpler Subprozess-Wrapper analog `bash` würde diese Aufgabenfläche nicht abdecken — Body-Manipulation, Multi-Send und Header-Routing brauchen Programm-Logik, nicht bloß stdout-zu-Text. Verworfen wurden: (a) `code` als bash-artiger Wrapper mit „Skalar-Lift" (die Cell extrahiert nur skalare Header-Werte aus stdout — deckt die echte Anwendungsfläche nicht ab, lässt den Body unberührt), (b) separate Transform-Cells für jede dieser Aufgaben (würde den Cell-Type-Katalog ohne Mehrwert vergrößern), (c) `bash` und `code` formgleich machen (würde `bash` unnötig schwer machen). Mit dem Body-Konstruktor-Modell bleibt der Katalog schlank, ohne neue Cell-Types erfinden zu müssen. Trade-off: `code` und `bash` sind formal nicht symmetrisch — das ist gewollt und über die Wahl-Heuristik oben für AI-Builder explizit aufgelöst.

**Bauart**: **vom Skript bestimmt** — atomisch-emittierend oder stream-fortpflanzend, je nachdem ob das Skript die eingehenden `messages[]` durchreicht oder neu baut. `code` ist die einzige Cell-Type ohne fixe Bauart.

**Skript-Schnittstelle**:
- **stdin**: JSON-serialisierte eingehende Message — alles, was die Cell laut `contract.consumes` und Standard-Message-Konvention liest (`header`, Body-Slots, plus die Envelope-Felder `target`, `reply_to`, `trace_id`, `parent_message_id`, `correlation_id`, `ttl`).
- **stdout**: vollständige Content-JSON in genau der Form, die jede andere Cell auch produziert — `header`-Sektion (optional) plus Top-Level-Slots. Das **Wire-Format ist unverändert**: das Skript schreibt weiterhin eine `header`-Sektion. Colony interpretiert diese als `hop` (der isolierte Cell-Output — verfällt bei der nächsten Cell-Emission), der Rest wird `message.body`. Das Skript schreibt **nicht** `context` (das ist allein Edge-Authority).

**Multi-Send**: wenn `multi_send_capable: true` (Quelle ist `contract.multi_send_capable` aus der `config.json` der Cell; die frühere Phase-9-`params.multi_send_capable`-Bridge ist **entfernt**), darf das Skript statt eines einzelnen Content-JSONs ein **JSON-Array** von Content-JSONs auf stdout schreiben. Die Cell discriminiert anhand des JSON-Wurzel-Typs:

- **JSON-Object** → eine ausgehende Message (Standard-Fall).
- **JSON-Array** → N ausgehende Messages, eine pro Element. Reihenfolge: Array-Reihenfolge.

Wenn `multi_send_capable: false` und das Skript ein Array schreibt → Contract-Violation, Error-Message mit `error_code: "multi_send_not_declared"`. Wenn `multi_send_capable: true` und das Skript ein Object schreibt → erlaubt, behandelt wie ein Array der Länge 1.

Jede emittierte Message läuft **unabhängig** durch die ausgehenden Edges der Cell — Colony evaluiert pro emittierter Message frisch alle Edge-Conditions; eine Message kann an Edge A landen, die nächste an Edge B.

Wire-Beispiel:

```json
[
  { "header": { "msg_type": "tool_call" },
    "messages": [{ "origin": "assistant", "type": "tool_call", "id": "call_a", "text": "..." }] },
  { "header": { "msg_type": "tool_call" },
    "messages": [{ "origin": "assistant", "type": "tool_call", "id": "call_b", "text": "..." }] },
  { "header": { "msg_type": "user_visible" },
    "messages": [{ "origin": "assistant", "type": "text", "text": "Drei Tools werden parallel angefragt." }] }
]
```

Verworfen wurden: Multi-Send über NDJSON (line-delimited JSON — bringt keinen Vorteil, weil die Cell auf Skript-Ende wartet, kein Streaming-Bedarf), Multi-Send mit explizitem Wrapper (`{ "messages": [...] }` als Wrapper für das Array — unnötig, JSON-Type-Discrimination reicht).

**Cell-Standard-Header** (gesetzt von der Cell selbst nach Skript-Ende, **überlagern** Skript-Output für diese Keys):
- `exit_code` (number)
- `duration_ms` (number)
- `had_stderr` (bool)

Das Skript kann diese Keys nicht hijacken — Process-Metadaten gehören der Cell.

**stderr** bei erfolgreichem Skript-Run (Exit 0): wird **nicht** in den Skript-Output injiziert (Body-Konstruktion des Skripts bleibt sauber). `header.had_stderr` wird gesetzt, stderr-Inhalt landet in `log.jsonl` mit Warn-Level. Bei Skript-Fehler (Exit ≠ 0, siehe Failure-Modell) emittiert die Cell stattdessen eine Error-Message mit stderr in der `bash`-Konvention.

**Failure-Modell** (vollständige `error_code`-Liste):
- stdin kein valides JSON (eingehende Message unparsbar) → Error mit `error_code: "invalid_input"`, **kein** DB-Write.
- Skript-Spawn schlägt fehl (Runner nicht startbar) → Error mit `error_code: "io_error"`.
- `external_timeout_ms` elapsed (Skript-Lauf zu lang) → Error mit `error_code: "script_timeout"`.
- Skript-Exit ≠ 0 → Cell verwirft den Skript-Output und emittiert Error-Message mit `header.finish_reason: "error"`, `header.error_code: "script_failed"`, `header.exit_code`, `header.had_stderr`. Body: `tool_result`-Turn mit stderr in der `bash`-Sentinel-Marker-Form (stdout, dann abgegrenzter stderr-Block).
- Skript-stdout kein valides JSON → Error mit `error_code: "invalid_json"`.
- Skript schreibt JSON-Array ohne `multi_send_capable` → Error mit `error_code: "multi_send_not_declared"`.
- Skript-stdout valide, aber `contract.emits` verletzt → Error mit `error_code: "contract_violation"`. Diese `code`-Validierung läuft **always-on** (unbedingt, unabhängig von Build-Profil und `colony.json` `strict_validation` — `code` ist die einzige user-skript-getriebene Trust-Boundary; siehe `meclaw-overview.md` § „Schema-Validierung — Zeitpunkt und Scope" und `docs/config.md` § Schema-Format und Validierung).

**`params`**: typisch `runner` (kanonisch `"python3"` in Phase 9 — `CodeParams::parse` rejected andere Werte mit `'params.runner: only "python3" is supported in Phase 9'`. Hintergrund: auf den Zielplattformen Ubuntu 24 / Python 3.12 ist `/usr/bin/python3` der reale Binary, `python` existiert dort bewusst nicht), Skript-Pfad bzw. inline-Code, `external_timeout_ms` (Konzept A, siehe overview § Timeouts; Default `60000`). **`multi_send_capable` liegt nicht (mehr) in `params`** — es kommt aus `contract.multi_send_capable` (siehe Multi-Send oben).

**`cell.db` für `code`** (Phase-9 Brainstorm E9): in Phase 9 **deferred**. DB-Zugriff aus Skript-Logik läuft über Topologie (`code` → Multi-Send → `store`), nicht in-process. Wer einen Collector-/State-Pattern in `code` braucht, hebt das in einen eigenen Design-Pass.

---

## `web_fetch` — Outbound HTTP-Client

**Aufgabe**: reines HTTP-Tool. Stateless (kein `cell.db`). **Implementiert ist nur `GET`** (Phase-7-Slice-3, siehe Phase-7-Konventionen unten); `POST`/`PUT`/`PATCH`/`DELETE` samt `method`/`headers`/`body` sind ein Roadmap-Defer (siehe `docs/roadmap.md` § Cell-Type-Feature-Erweiterungen, „`web_fetch` POST/headers/body").

**Bauart**: atomisch-emittierend. Pro HTTP-Call ein `tool_result`-Turn.

**Body-Format der Response**: `messages[]` mit einem Turn `{ origin: "tool", type: "tool_result", text: "<response body>", id: "<tool_call_id>" }`. Bei großem Body wird (ab Phase 12) die **gesamte** Output-Message als `Body::Blob` ausgelagert — **Ganzkörper-Offload** an der Delivery-Grenze (`blob_inline_max_bytes`-Schwelle, `resolve_blob_for_delivery`), **nicht** ein In-Message-`text_id`-Pointer. In-Message-Pointer (`text_id`/`messages_id`) haben heute **keinen Producer** (D-025 deferred, siehe `docs/roadmap.md` § Body / Blob-Auflösung und CLAUDE.md Regel 14).

**Output-Header**: `operation` (= `"web_fetch"`), `http_status`, `content_type`, `duration_ms`, `bytes`, optional `truncated`.

**`params`**: typisch `base_url`, Default-`headers`, optional Auth-Konfiguration.

**Phase-7-Konventionen** (Slice-3-Entscheidungen):
- **Nur GET** in Slice 3. `method`/`headers`/`body` deferred.
- **Input minimal**: `{"url": "..."}`.
- **non-2xx HTTP status = NORMAL tool_result** mit `http_status`-Header. LLM/Caller liest den Status. Nur DNS/connect/timeout/ungültiger Input produzieren Error-Messages (`io_error` / `timeout` / `invalid_input` bei fehlender/ungültiger `url`).
- **TLS**: rustls (`rustls-tls`-Feature von reqwest); kein OpenSSL/native-tls im Tree.
- **Header**: `operation: "web_fetch"`, `http_status: u16` (Pflicht), `content_type: String`, `duration_ms`, `bytes`.
- **Truncation/Blob**: deferred (Phase 12) — große Bodies inline in `text`.
- **`reqwest::Client` pro Cell-Instanz** (intern Arc, kein Mutex). Build-Fehler beim Spawn → spawn-Error. RespawnFn cloned den initial-gebauten Client.
- **Defaults**: `max_concurrency: 32`, `external_timeout_ms: 30000`.

---

## `web_search` — Web-Search-Client

**Aufgabe**: reines Search-Tool, spricht einen externen Search-Provider an (z.B. Brave, Tavily, SerpAPI). Stateless (kein `cell.db`).

**Bauart**: atomisch-emittierend. Pro Such-Anfrage ein `tool_result`-Turn.

**Body-Format der Response**: `messages[]` mit einem `tool_result`-Turn, dessen `text` die Suchergebnisse als JSON-Liste enthält (Titel, URL, Snippet pro Treffer). Bei großen Ergebnis-Listen (ab Phase 12) Ganzkörper-Offload der gesamten Message als `Body::Blob` an der Delivery-Grenze, **nicht** via In-Message-`text_id`-Pointer (D-025 deferred).

**Output-Header**: `operation` (= `"web_search"`), `result_count`, `duration_ms`, `bytes`.

**error_codes**: `io_error` (DNS/connect-Fehler), `timeout` (external_timeout elapsed), `invalid_input` (fehlende/ungültige `query`). Eine bloß nicht-konforme Provider-Response ist **kein** Error (siehe Phase-7-Konventionen — `result_count=0`, Body durchgereicht).

**`params`**: typisch Provider `base_url` und API-Token (via `${VAR}`-Substitution).

**Phase-7-Konventionen** (Slice-3-Entscheidungen):
- **Generischer JSON-Wrapper**: Cell macht GET `<params.endpoint>?q=<query>` mit optionalem `params.api_key` als Bearer-Token. Erwartet Response `{"results":[{"title","url","snippet"}]}`.
- **Provider-spezifische Adapter** (Brave, Tavily, SerpAPI, …) sind **deferred** — Anwendungs-Topologie via `code`-Cell (Phase 9) oder Builder-Hive normalisiert.
- **Input**: `{"query": "..."}`.
- **Graceful bei nicht-konformer Response**: `result_count=0` wenn `results`-Key fehlt oder kein Array. Body wird IMMER in `text` durchgereicht — **kein Hart-Error**.
- **Header**: `operation: "web_search"`, `result_count: u64`, `duration_ms`, `bytes`. (`http_status`-Header ist hier deferred — Parität mit web_fetch wäre konsistenter, ist aber Post-Slice-3.)
- **Truncation/Blob**: deferred (Phase 12).
- **`reqwest::Client` pro Cell-Instanz** (analog web_fetch). Build-Fehler beim Spawn → spawn-Error. RespawnFn cloned den Client.
- **Defaults**: `max_concurrency: 8`, `external_timeout_ms: 15000`.

---

## `file` — Filesystem-Operationen

**Aufgabe**: CRUD für Dateien innerhalb einer Security-Boundary. Pfad-Traversal außerhalb der Boundary wird abgewiesen. Stateless.

**Bauart**: atomisch-emittierend. Pro Operation (`read`/`write`/`list`/`stat`) ein `tool_result`-Turn.

**Body-Format der Response**: `messages[]` mit einem `tool_result`-Turn. Bei `read` enthält `text` den Datei-Inhalt (bei großen Dateien ab Phase 12 Ganzkörper-Offload der gesamten Message als `Body::Blob` an der Delivery-Grenze, **nicht** via In-Message-`text_id`-Pointer — D-025 deferred). Bei `write`/`list`/`stat` enthält `text` einen JSON-strukturierten Status (Bytes geschrieben, Datei-Liste, Stat-Info).

**Output-Header**: `operation` (`"read"`/`"write"`/`"list"`/`"stat"`), `bytes`, `duration_ms`.

**`params`**: `base_path` (Pflicht; Security-Boundary).

**Phase-7-Konventionen** (Slice-1-Entscheidungen):
- **`target = reply_to`**: FileCell emittiert an `msg.reply_to`; Fallback `/colony/dead_letters` falls `reply_to` fehlt. Edges in der Topologie können das Target überlagern.
- **`tool_call.text` ist JSON-Args**: `{"op": "read"|"write"|"list"|"stat", "path": "<rel>", "content"?: "<str für write>"}`.
- **`write` ohne auto-mkdir**: Parent-Dir MUSS existieren. Fehlender Parent → `io_error`. Symlink-safe via Parent-canonicalize.
- **Security-Boundary**: alle Pfade gegen `base_path` canonicalisiert (Symlinks aufgelöst); Traversal/absolute-rel/symlink-escape → `path_outside_boundary` bzw. `invalid_input`.
- **Default `max_concurrency`**: 8.
- **error_codes**: `invalid_input`, `path_outside_boundary`, `not_found`, `not_a_directory`, `not_a_file`, `io_error`.

---

## `edit` — File-Editing-Operationen

**Aufgabe**: editiert Dateien innerhalb einer Security-Boundary (typisch: Find/Replace, Insert-At-Line, Patch). Stateless.

**Bauart**: atomisch-emittierend. Pro Edit-Operation ein `tool_result`-Turn.

**Body-Format der Response**: `messages[]` mit einem `tool_result`-Turn. `text` enthält Status der Edit-Operation (z.B. „3 Stellen ersetzt" oder ein Diff-Snippet). Bei Fehler (Datei nicht gefunden, Pattern matcht nicht) wird der Fehler im `text` strukturiert beschrieben; `header.error_code` markiert die Klasse.

**Output-Header**: `operation`, `matches_changed`, `bytes`, `duration_ms`, optional `error_code`.

**`params`**: `base_path` (Pflicht; Security-Boundary).

**Phase-7-Konventionen** (Slice-2-Entscheidungen):
- **Ops in Slice 2**: `find_replace` + `insert_at_line`. **Patch ist deferred** (eigener Diff-Format-Design-Pass nötig).
- **`find_replace` = replace ALL**: alle Vorkommen werden ersetzt. `matches_changed`-Header gibt die Anzahl.
- **0 matches → `ERR_PATTERN_NOT_FOUND`**: Caller wollte ersetzen, Pattern war nicht da → Error (kein normaler tool_result mit `matches_changed: 0`).
- **`insert_at_line` ist 1-based und insert-VOR**: `line = 1` → ganz am Anfang; `line = file_lines + 1` → ganz am Ende. `line < 1` oder `line > file_lines + 1` → `invalid_input`.
- **Teilt FileCells Security-Boundary**: gleiche `base_path`-Logik (extrahiert in `meclaw-cells/src/boundary.rs`).
- **Nicht atomar**: read-modify-write ohne tempfile+rename (konsistent mit FileCell::write). Crash-mitten = OS-Level-Problem. Atomare Edits sind Post-Roadmap.
- **Concurrent-edit auf dieselbe Datei**: race-condition möglich (kein Lock in Phase 7). Caller-Topologie serialisiert wenn nötig.
- **Input**:
  - `{"op": "find_replace", "path": "<rel>", "find": "<str>", "replace": "<str>"}`
  - `{"op": "insert_at_line", "path": "<rel>", "line": <u32>, "content": "<str>"}`
- **Default `max_concurrency`**: 8.
- **error_codes**: reuse aus file + neu `pattern_not_found`.

---

## `proxy` — External-Chat-Plattform-Bridge

**Aufgabe**: Long-Running. Bridged zu einem externen Chat-Plattform-Anbieter (Telegram first; weitere Plattformen folgen). Hält in `cell.db` einen Cursor für Update-Offsets, damit Restarts keine Nachrichten doppelt verarbeiten.

**Concurrency-Aufbau**: **zwei Tokio-Tasks pro Instanz** (Handler + I/O), kommunizierend über internen mpsc — siehe `meclaw-overview.md`, Abschnitt „Long-Running-Cells: Doppel-Task". Aus Topologie-Sicht bleibt die Cell eine einzige Adresse mit einer einzigen externen Mailbox; die Doppelstruktur ist intern und für diesen Cell-Type vorgeschrieben.

- **Handler-Task**: macht `tokio::select!` über externe Mailbox (Inbound aus Topologie) und internen Channel (Provider-Events vom I/O-Task). Hält den gesamten Cell-State (Cursor in `cell.db`, in-memory Session-Maps). Setzt allein Reihenfolge und State-Mutationen — kein Mutex.
- **I/O-Task**: pollt Telegram (Long-Poll bzw. Webhook-Reader), serialisiert eingehende User-Messages zu Event-Frames und schiebt sie in den internen mpsc. Hält keinen Cell-State, kein direkter `cell.db`-Zugriff.

Damit blockiert ein 30s-Long-Poll niemals eine Inbound-Message aus der Topologie und umgekehrt.

**Bauart**: atomisch-emittierend (Richtung Topologie). Eine externe Chat-Message vom User → eine emittierte meclaw-Message mit genau einem User-Origin-Turn. Die Proxy ist **Quelle** des Konversations-Fadens, nicht Mid-Stream — sie hat kein Eingangs-`messages[]` zum Durchreichen.

**Body-Format der Outbound-Message** (Telegram → Topologie):
```json
{
  "messages": [
    { "origin": "user", "type": "text", "text": "<vom User getippt>" }
  ]
}
```

Plus Header mit Plattform-Metadaten: `chat_id`, `user_id`, `platform: "telegram"`, optional `message_id` (Plattform-eigene ID, Pass-Through für spätere Replies).

**Inbound-Verhalten** (Topologie → Telegram): Proxy konsumiert eingehende meclaw-Messages, extrahiert den letzten assistant-Turn aus `messages[]` und sendet dessen `text` an die Chat-Plattform. Emittiert dabei **nichts** zurück in die Topologie — pure Sink. Routing zur richtigen Chat-Konversation läuft über `chat_id` aus den Headers.

**Inbound-Fehlerpfade**: Ist der Inbound-Body nicht inline-lesbar (kein Inline-UBF), emittiert die Proxy `error_code: "invalid_body"`. Fehlt der `chat_id`-Header, `error_code: "missing_chat_id"` (Fallback `/colony/dead_letters`). Enthält `messages[]` keinen sendbaren assistant-Turn, `error_code: "missing_assistant_turn"`. Schlägt der Versand an die Chat-Plattform fehl (Network-Fehler, Telegram-API-Fehler, ungültige `chat_id`), `error_code: "send_failed"`. Alle Error-Replies gehen an `msg.reply_to` (Fallback `/colony/dead_letters`) und tragen einen Nicht-Konversations-Origin (kein `user`/`assistant`-Turn) und zählen nicht als Konversations-Emission — die Pure-Sink-Disziplin („emittiert nichts in den Konversations-Fluss") bleibt gewahrt.

**`params`**: typisch Plattform-Credentials (Bot-Token via `${VAR}`) und Polling-Konfiguration (Long-Poll-Intervall, Timeout). Optional `query_timeout_ms` (A-Timeout für `cell.db`-Ops via `DbConn::call_with_timeout`, z.B. Cursor-Persist).

**Laufzeit-Param-Updates (β, `config.md` § Zugriff Z.20):** wie `llm` (siehe dort) — Top-Level-`params`-Body-Slot, in der `cell.db` persistiert, bei wake/respawn replayt. **Mutabel über alle drei Propagations-Wege:** `send_timeout_ms` (Weg A, handle-seitig — der nächste `sendMessage` nutzt es), `long_poll_timeout_ms`/`long_poll_request_secs`/`base_url` (Weg B — der Handler signalisiert der I/O-Task via internem Reconfig-Channel, der nächste Poll nutzt sie; bei `base_url`-Wechsel bauen Handler und I/O-Task ihren `TelegramClient` live neu (`with_base_url`) und **rehalten den immutablen `bot_token` aus dem bestehenden State** — der Token quert die params-Fläche nie; die W7-Tripwire `long_poll_timeout_ms > long_poll_request_secs*1000` wird beim Merge erneut erzwungen), `query_timeout_ms` (Weg C — der laufende `DbConn`). **Immutable je `proxy`:** `bot_token` + `emit_to` (Credential/Routing-Identität). `base_url` ist eine Konfig-URL (wie `llm.base_url`), **kein** Credential → mutabel. Update-Versuch auf ein Immutable oder ein unbekannter Key bzw. eine W7-Verletzung ⇒ lauter Reject (`error_code: "invalid_input"`), kein Teil-Apply. Eine params-only-Message persistiert und schweigt.

---

## `timer` — periodischer Event-Emitter

**Aufgabe**: Long-Running. Cron-artige Scheduling-Cell. Hält in `cell.db` die aktive Schedule-Liste. **Cron-Format ist 6-Feld-Quartz-Stil** (`Second Minute Hour DayOfMonth Month DayOfWeek`), damit Sekunden-Granularität nativ ausdrückbar ist. Scheduler-Auflösung ist entsprechend sekundengenau — die Zündung erfolgt exakt zur konfigurierten Sekunde, kein Polling-Grid. Kann einmalige wie auch repetierende Events senden.

**Concurrency-Aufbau**: **zwei Tokio-Tasks pro Instanz** (Handler + I/O), kommunizierend über internen mpsc — siehe `meclaw-overview.md`, Abschnitt „Long-Running-Cells: Doppel-Task". Für diesen Cell-Type vorgeschrieben, nicht optional.

- **Handler-Task**: macht `tokio::select!` über externe Mailbox (Schedule-Anlage/Modifikation/Löschung) und internen Channel (Timer-Zündungen vom I/O-Task). Hält die in-memory Schedule-Liste und persistiert sie nach `cell.db`. Setzt allein Reihenfolge und State-Mutationen.
- **I/O-Task**: berechnet den nächstfälligen Schedule-Eintrag, wartet mit `tokio::time::sleep_until` darauf, schiebt einen Zündungs-Event-Frame in den internen mpsc, berechnet den nächsten Wartepunkt. Bei Schedule-Änderungen (Add/Modify/Remove) schickt der Handler-Task einen Reconfigure-Hint an den I/O-Task, der seine Sleep-Berechnung neu macht. Hält keinen Cell-State, kein direkter `cell.db`-Zugriff.

Damit ist der Timer sekundengenau, ohne dass die Mailbox-Verarbeitung das Sleep-Timing stören kann und umgekehrt.

**Bauart**: atomisch-emittierend. Timer **erzeugt keine eigenen Inhalte** — er versendet, was bei der Schedule-Anlage als Body-Template mitgegeben wurde, zum konfigurierten Zeitpunkt.

**Schedule-Identität**: jede Schedule hat eine **`schedule_id` (UUID v7) als eindeutigen
Schlüssel** — vom Aufrufer in der Anlage-Nachricht vergeben (bzw. im `params.schedules`-
Eintrag bei Instanziierung). `schedule_name` ist demgegenüber ein **nicht-eindeutiges
menschenlesbares Label** (darf mehrfach vorkommen) und dient nur der Lesbarkeit + dem
Fire-Header. Modifikation und Löschung adressieren **immer über `schedule_id`**, nie über
`schedule_name`.

**Operation per Nachricht** über das Pflichtfeld `op: "add" | "modify" | "remove"`
(Default `add`, falls weggelassen):

```json
{
  "op":            "add",
  "schedule_id":   "0190a3f2-...-v7",
  "schedule_name": "daily-standup",
  "cron":          "0 0 9 * * *",
  "emit_to":       "/main/standup_hive",
  "emit_body":     { "messages": [{ "origin": "user", "type": "text", "text": "..." }] },
  "emit_headers":  { "msg_type": "standup_trigger" }
}
```

```json
{ "op": "remove", "schedule_id": "0190a3f2-...-v7" }
```

`modify` trägt `schedule_id` plus die zu ändernden Felder (z.B. neues `cron`).

**Semantik** (strikt, keine Heuristik):
- `add` = INSERT; vorhandene `schedule_id` → Fehler (kein impliziter Upsert).
- `modify` = UPDATE der getragenen Felder; unbekannte `schedule_id` → Fehler.
- `remove` = Schedule deaktivieren (Status-Update in `cell.db`, **No-Delete-konform** — keine
  Row-Löschung); unbekannte `schedule_id` → Fehler.

**Validierung & Fehler-Surfacing**: bei `add`/`modify` wird ein `cron`-Ausdruck gegen den 6-Feld-Quartz-Parser validiert — ungültige Ausdrücke werden abgelehnt (es entsteht kein still gespeicherter, nie zündender Schedule). Alle Op-Fehler werden als Message an die `reply_to` der Op-Nachricht emittiert (`parent_message_id` = die konsumierte Op-Nachricht), mit `header.error_code` für: `invalid_body` (Body nicht inline-lesbar), `parse_error` (Op-Nachricht jenseits des Cron-Checks unparsbar), `schedule_id_exists` (add auf vorhandene `schedule_id`), `schedule_not_found` (modify/remove auf unbekannte `schedule_id`), `kind_mismatch` (modify-Typ-Wechsel once↔repeating), `invalid_cron` (ungültiger Cron-Ausdruck). Erfolgreiche Ops werden nicht geackt.

**Einmalig vs. repetierend**: eine repetierende Schedule trägt `cron` (6-Feld-Quartz). Eine einmalige trägt stattdessen `at` (RFC-3339-Z, UTC) und **kein** `cron` — die Felder sind exklusiv (genau eines pro Schedule). `iteration_n` wird nur bei repetierenden Schedules emittiert (bei once weggelassen). `modify` darf den Typ nicht wechseln (once↔repeating) — dafür `remove` + `add`.

```json
{ "op": "add", "schedule_id": "0190a3f2-...-v7", "schedule_name": "one-shot-reminder", "at": "2026-06-01T09:00:00Z", "emit_to": "/main/x", "emit_body": { "messages": [] } }
```

**Vergangene Zündungen werden verworfen** (POC-Verhalten): der Timer plant ausschließlich die
nächste Zündung *nach jetzt* (`find_next_occurrence`). Eine einmalige Schedule, deren Zeitpunkt
bereits in der Vergangenheit liegt (zur Anlage- oder Restart-Zeit), wird nicht eingeplant und
nur geloggt. Repetierende Schedules holen verpasste Zündungen nicht nach — sie zünden ab der
nächsten Zukunfts-Occurrence. Begründung: der Timer hat keine Relevanz-/Prioritäts-
Klassifikation und kann nicht entscheiden, ob ein verpasstes Event noch zuzustellen ist.

Der Body kann beliebige Universal-Body-Slots enthalten — `messages[]`, eigene Top-Level-Slots, oder auch leer (nur Header-Trigger).

**Bei Schedule-Auslösung emittierte Header** (Timer-automatisch, zusätzlich zu `emit_headers`):

| Header | Inhalt |
|---|---|
| `event_id` | UUID v7 dieses einzelnen Events |
| `schedule_id` | eindeutiger UUID-v7-Schlüssel der auslösenden Schedule |
| `schedule_name` | menschenlesbares Label der Schedule |
| `scheduled_at` | geplanter Zeitpunkt (RFC-3339-Z, UTC) |
| `fired_at` | tatsächlicher Feuer-Zeitpunkt (RFC-3339-Z, UTC) |
| `iteration_n` | bei repetierenden Schedules: 0, 1, 2, … |

**Contract-Quirk**: `emits.body` ist wildcard-mäßig (was die Schedule definiert), `emits.header` strikt das fixe Set oben (plus was die Schedule unter `emit_headers` mitgibt).

**`params`**: typisch keine — Schedules werden zur Laufzeit per Message erstellt (oder optional initial via `params.schedules`). `params.schedules`-Einträge tragen dasselbe Schema (jeweils `schedule_id` als UUID v7), und der Initial-Seed greift nur bei frischer `cell.db` (`OpenStatus::Created`-Gate, analog zum Phase-9-`store`-Seed) — sonst re-seedet jeder Restart die Config-Schedules zu Duplikaten. Optional `query_timeout_ms` (Default 5000) setzt den A-Timeout für `cell.db`-Zugriffe (rusqlite-`InterruptHandle` via `DbConn`) — er gilt für **alle** cell.db-Ops der Cell (`add`/`modify`/`remove` + die Fire-seitigen Reads/Writes), die über `DbConn::call_with_timeout` laufen.

**Laufzeit-Param-Updates (β, `config.md` § Zugriff Z.20):** wie `llm` (siehe dort) — Top-Level-`params`-Body-Slot, in der `cell.db` persistiert, bei wake/respawn replayt. Das **einzige** overlay-fähige Feld ist `query_timeout_ms` — es wirkt **sofort live** (der laufende `DbConn` übernimmt den neuen A-Timeout für die nächste cell.db-Op, ohne wake/respawn). `schedules` sind **nicht** overlay-fähig: sie ändern sich ausschließlich über die `add`/`modify`/`remove`-Ops (sie tragen Live-Zustand `status`/`iteration_n` in der `cell.db`). Immutable-Set ist **leer**; ein Update auf `schedules` oder ein unbekannter Key ⇒ lauter Reject (`error_code: "invalid_input"`). Eine params-only-Message persistiert und schweigt.

---

## `mcp` — MCP-Plattform-Bridge

**Aufgabe**: Long-Running. Bridged zu einem externen MCP-Anbieter (Model Context Protocol). Hält in `cell.db` ggf. Zustände (z.B. Tool-Discovery-Cache, Session-Handles). **v0.1.0-Scope: minimaler HTTP + JSON-RPC-POC** (`initialize` / `tools/list` / `tools/call`). Streaming-Transporte (SSE/stdio), server-pushed Notifications und Auto-Reconnect sind Roadmap-Defer (siehe `docs/roadmap.md` § Provider-Erweiterungen, „MCP SSE/stdio/Server-Push").

**Concurrency-Aufbau**: **zwei Tokio-Tasks pro Instanz** (Handler + I/O), kommunizierend über internen mpsc — siehe `meclaw-overview.md`, Abschnitt „Long-Running-Cells: Doppel-Task". Für diesen Cell-Type vorgeschrieben, nicht optional.

- **Handler-Task**: macht `tokio::select!` über externe Mailbox (Tool-Call-Requests aus der Topologie, Discovery-Anfragen) und internen Channel (server-pushed Events oder Tool-Responses vom I/O-Task). Hält den gesamten Cell-State (Discovery-Cache, Session-Handles, In-Flight-Map korrelierter Tool-Calls).
- **I/O-Task**: spricht den MCP-Provider in v0.1.0 über **HTTP + JSON-RPC** an (kein persistenter Stream — die Streaming-Transporte SSE/stdio sind Roadmap-Defer), serialisiert Responses zu Event-Frames und schiebt sie in den internen mpsc. Hält keinen Cell-State, kein direkter `cell.db`-Zugriff. (Die Doppel-Task-Struktur ist der vorgeschriebene Long-Running-Aufbau und trägt ab dem SSE/stdio-Ausbau die dann langlaufenden Stream-Reads.)

Damit blockiert ein langlaufender Provider-Call niemals die Annahme neuer Tool-Call-Requests aus der Topologie.

**Post-Init-Backend-Tod (ehrlicher v0.1.0-Stand)**: Über den HTTP+JSON-RPC-Transport hält die Cell **keine** persistente Verbindung — **jeder** Tool-Call verbindet neu. Stirbt das MCP-Backend transient *nach* der Discovery, erholt sich die Cell daher **automatisch beim nächsten Tool-Call** (der frische Connect gelingt wieder); ein dauerhaft toter Backend äußert sich pro Call als `provider_timeout` bzw. `mcp_error`. Eine **persistente Tod-Erkennung mit aktivem Reconnect** — die Cell bemerkt einen Backend-Tod *zwischen* Calls und signalisiert/restartet — existiert **erst mit dem SSE/stdio-Ausbau** (ein langlaufender Stream-Read trüge das Liveness-Signal); Roadmap-Defer, konsistent mit dem SSE/stdio-Defer oben. Bis dahin pendet `run_io` nach der Discovery ohne eigene Liveness-Probe: ein Post-Init-Backend-Tod löst **kein** `CellDied`/Restart und kein Diagnose-Signal aus (registriert in `docs/roadmap.md` § Provider-Erweiterungen / Cell-Factory-Robustness, „mcp — Post-Init-Subprozess-Tod unbemerkt").

**Bauart**: atomisch-emittierend. Pro MCP-Tool-Call eine Response-Message mit dem Resultat als Turn.

**Body-Format der Response**: `messages[]` mit einem `tool_result`-Turn, `text` enthält die MCP-Tool-Antwort (typisch JSON-strukturiert). Bei großen Antworten (ab Phase 12) Ganzkörper-Offload der gesamten Message als `Body::Blob` an der Delivery-Grenze, **nicht** via In-Message-`text_id`-Pointer (D-025 deferred).

**Discovery**: MCP-Tools, die dieser Provider anbietet, werden via Discovery-Message verfügbar gemacht — die Cell kann ihre `system.tools.*`-Slots an eine `llm`-Cell ausspielen, damit diese die Tools dem LLM präsentiert. Genauer Mechanismus ist Phase-10-Detail.

**Output-Header**: `mcp_tool` (Name des aufgerufenen Tools), `duration_ms`, optional `error_code`. Kanonische `mcp`-`error_code`-Werte: `"mcp_error"` (JSON-RPC-/Protokoll-Fehler des Providers, z.B. `tools/call`-Fehlerantwort) und `"provider_timeout"` (`external_timeout_ms`-Elapsed beim HTTP+JSON-RPC-Call).

**`params`**: typisch Provider-Endpoint (in v0.1.0 die HTTP-URL für JSON-RPC; SSE-URL/Stdio-Command sind Roadmap-Defer), Auth-Credentials (via `${VAR}`), Discovery-Konfiguration, optional `external_timeout_ms` (A-Timeout, `error_code: "provider_timeout"`) sowie `query_timeout_ms` (A-Timeout für `cell.db`-Ops via `DbConn::call_with_timeout`).

**Laufzeit-Param-Updates (β, `config.md` § Zugriff Z.20):** wie `llm` (siehe dort) — Top-Level-`params`-Body-Slot, in der `cell.db` persistiert, bei wake/respawn replayt. **Mutabel:** `external_timeout_ms` — wirkt **sofort live** (Weg A, der nächste `call_tool` nutzt es; der I/O-Task hat post-Discovery **keinen** live-nachzulesenden Wert, daher rein handle-seitig) — und `query_timeout_ms` (Weg C, der laufende `DbConn` übernimmt den neuen A-Timeout für die nächste cell.db-Op). **Immutable je `mcp`:** `endpoint` + `auth` (Bearer) — Credential/Identität. Update-Versuch darauf oder ein unbekannter Key ⇒ lauter Reject (`error_code: "invalid_input"`), kein Teil-Apply. Eine params-only-Message persistiert und schweigt.
