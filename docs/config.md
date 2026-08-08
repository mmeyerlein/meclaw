# `config.json` — Format

Detail-Spec des `config.json`-Formats pro Cell und pro Hive-Scope-Marker. Bei Konflikt zwischen dieser Datei und `meclaw-overview.md` gewinnt die overview — sie ist Single Source of Truth.

## Oberstes Gebot

**Eine Cell weiß nicht, was vor oder hinter ihr passiert.** Sie kennt nur ihren eigenen Vertrag (Input/Output-Schema), ihre Params und die Message, die sie gerade verarbeitet. Sie hat **keine** Kenntnis von Sender-Pfaden, Empfänger-Pfaden, Hop-Historie, Routing-Strategien oder anderen Cells.

Messages sind atomar. Trace-Rekonstruktion lebt im zentralen Message-Log in `colony.db` (filterbar nach Pfad-Präfix), nicht in der Message.

**Envelope-Felder sind aus Cell-Sicht read-only.** `id`, `trace_id`, `parent_message_id`, `correlation_id`, `target`, `reply_to`, `ttl`, `created_at` werden ausschließlich von Colony beim Routing gesetzt — eine Cell kann sie weder in ihrem content-JSON schreiben noch über eine Edge manipulieren (siehe `meclaw-overview.md` Abschnitt „Envelope-Setter-Authority"). Wer ein anderes Reply-Ziel als den Sender will, löst das anwendungs-spezifisch über Header-basiertes Routing.

**Aus Cell-Sicht ist die Welt single-threaded.** Ein `handle()`-Call läuft komplett, bevor der nächste startet — die Cell-Task pulled sequentiell aus der mpsc-Mailbox. Cell-Code enthält daher kein `Mutex`, kein `RwLock`, keine atomics, keine Reentrancy-Defensive. Die Parallelität des Systems liegt außerhalb der Cell — siehe `meclaw-overview.md`, Abschnitt „Nebenläufigkeit & Parallelität".

## Zugriff

- **Authority**: Ausschließlich die Colony liest und schreibt `config.json` — der **einzige Schreiber ist die Instanziierung** (genau einmal). **Read-once:** die laufende Cell-Task liest `config.json` nach dem Start **nie erneut**; `config.json` ist der **Instanziierungs-Snapshot**, kein Live-Dokument.
- **Bei Instanziierung**: Colony kopiert Template, vergibt neue UUID v7, führt `${VAR}`-/`${ctx.*}`-/`${uuid7:*}`-Substitution durch, schreibt das Ergebnis in `config.json` der Instanz. **Die Knoten-Referenz ist der Filesystem-Verzeichnisname** (das Pfad-Segment unter `{root}`), **nicht** ein `cell.name`-Feld — die `config.json` trägt keinen `name`. Bei der Auflösung der Wurzel-Kette gewinnt die `${...}`-Substitution über den `template.json`-Template-Namen. Namens-Kollisionen mit Geschwistern innerhalb desselben Hive-Scopes werden von Colony in der einstufigen Mutations-Validierung rejected, siehe `meclaw-overview.md` Abschnitt „Naming-Kollisionen".
- **Nach Instanziierung**: `config.json` ist semantisch eingefroren — der Bootstrap-Snapshot. Niemand schreibt mehr da rein, weder Colony noch die Cell selbst. **Dynamischer Cell-Zustand** (geänderte Params) lebt ausschließlich in `cell.db`; **Colony-Zustand** (Registry, Edge-Tabelle, `cell_id`, Message-Log, Mutations) lebt in `colony.db` — `config.json` trägt nach dem Snapshot keinen von beiden nach (siehe `meclaw-overview.md` Abschnitt „Lifecycle von `config.json` und `cell.db`"). Der Graph einer Topologie lebt zentral in Colony's Registry und `colony.db` — nicht in der `config.json` des Hive-Scope-Markers (sein `params.graph` ist nur initialer Bootstrap-Hint).
- **Cells lesen `config.json` nicht.** Die Colony übergibt der Cell beim Start den `params`-Block. Param-Updates kommen danach per Message und werden von der Cell in ihrer `cell.db` persistiert (`config.json` divergiert vom Live-Stand — gewollt; Cell-Reset = `cell.db`-Wipe → Cell startet wieder vom Bootstrap-Stand).

## Struktur

```json
{
  "cell":        { ... },
  "params":      { ... },
  "contract":    { ... },
  "description": { ... }
}
```

### Block-Definition (kanonisch)

Zwei der vier Blöcke haben grundverschiedene Authority — diese Trennung ist die Wurzel der ganzen Datei:

- **`cell`-Block = Colony-Substrat.** Diese Felder steuern, **wie die Colony** die Cell instanziiert, registriert und überwacht. Sie werden der Cell **nie** übergeben — die Cell sieht ausschließlich ihren `params`-Block plus die Message, die sie gerade verarbeitet. Erlaubte Schlüssel: `id`, `type`, `timeout`, `restart_limit`, `idle_timeout_ms`, `mailbox_size`, `message_timeout` (Details in der `cell`-Tabelle unten). Ein im `cell`-Block deklarierter, nicht erlaubter Schlüssel ist ein Boot-Fehler.
- **`params`-Block = 1:1 opak an die Cell.** Die Colony reicht ihn nach `${VAR}`-/`${ctx.*}`-/`${uuid7:*}`-Substitution **unverändert** an die Cell durch und interpretiert seinen Inhalt **nicht**. Cell-Type-spezifisch (jeder Cell-Type definiert seine eigene `params`-Struktur, siehe `cell-types.md`). **Einzige Ausnahme:** beim Hive-Scope-Marker liest die Colony `params.graph` als initialen Soll-Graph (der Hive ist kein Aktor, bekommt also keinen `params`-Block „übergeben").

**Unveränderlich sind nur `id` und `type`** — sie identifizieren die Knoten-Instanz und ihren Cell-Type über die gesamte Lebensdauer. **Wirksamkeits-Regel** für alle anderen Felder: Änderungen an `cell`- oder `params`-Feldern (per neuer Instanziierung am Pfad bzw. neuem Template) greifen **beim nächsten Spawn/Wake** der Cell — die laufende Cell-Task liest `config.json` nicht erneut (siehe § Zugriff, „Read-once").

**Spezialfall Hive-Scope-Marker** (`cell.type: "hive"`): nur `cell` und `params` sind relevant. `params` enthält ausschließlich den optionalen `graph`-Block (initialer Soll-Graph, siehe `meclaw-overview.md` Abschnitt „Graph-Schema") — kein `dead_letters`-Override (der `HiveParams`-Deserializer ist `deny_unknown_fields`; die DLQ ist immer `/colony/dead_letters`). Ein `contract`-Block wird nicht ausgewertet, weil Hive-Scope-Marker keine Aktoren sind und nicht am Message-Flow teilnehmen (siehe `cell-types.md` Abschnitt `hive`). Im `cell`-Block sind nur `id` und `type` relevant — `timeout`, `message_timeout`, `idle_timeout_ms` und `mailbox_size` werden ignoriert (kein Aktor, keine Mailbox, kein `handle()`-Call). Eine `description` ist erlaubt, dient aber nur Discovery durch Builder; `emits_meaning` und `consumes_meaning` entfallen.

### `cell`

| Schlüssel | Inhalt |
|---|---|
| `id` | `cell_id` (UUID v7). **Wird beim Kopiervorgang Template → Instanz gesetzt** — das **einzige Mal**, dass sie geschrieben wird. Die Instanziierung liest sie aus der frisch geschriebenen `config.json` und persistiert sie in die **nie löschende `colony.db`**, die ab dann die **autoritative** Quelle der `cell_id` ist (`config.json` ist nur der Bootstrap-Abdruck). Danach **nie neu vergeben** — auch nicht bei Reconnect, Resume oder Reboot. (Der re-dedizierte `swap_nodes`-Graph-Swap schwenkt Edges auf eine andere Implementierung mit **eigener** `id` und lässt die alte Cell mit ihrer `id` disconnected erhalten — er überträgt **keine** `cell_id`, siehe `meclaw-overview.md` § Mutation-Operationen.) |
| `type` | Cell-Type (`hive`, `store`, `llm`, `bash`, `code`, `web_fetch`, `web_search`, `file`, `edit`, `proxy`, `timer`, `mcp`). Zusammen mit `id` der **unveränderliche** Teil des `cell`-Blocks. |
| `restart_limit` | *(optional)* Maximale Restart-Versuche durch den Supervisor, bevor die Cell als `failed` markiert wird. Default `5`. Siehe `meclaw-overview.md` Abschnitt „Restart-Strategie". |
| `timeout` | Hot/Cold-Modus (siehe `meclaw-overview.md` Abschnitt „Hot/Cold-Cell-Modell"): `0` = default (Idle-Timeout-Modell, Awake↔Asleep), `>0` = One-Shot (despawn nach jeder Message), `-1` = persistent (typisch `proxy`/`timer`/`mcp`, nie despawnen). Phase-13-Aktivierung; davor sind alle Cells permanent als Task. |
| `idle_timeout_ms` | *(optional, ab Phase 13)* Idle-Dauer in ms, nach der eine stateful Cell mit `cell.timeout: 0` sich selbst despawnt (Awake→Asleep). Überschreibt Colony-Default aus `colony.json` `idle_timeout_default_ms`. Wird ignoriert, wenn `cell.timeout != 0` (bei `>0` greift One-Shot-Despawn nach jeder Message, bei `-1` ist die Cell persistent und despawnt nie). |
| `message_timeout` | *(optional)* Substrat-Backstop pro `handle()`-Call in ms — siehe `meclaw-overview.md` Abschnitt „Timeouts" (Konzept B). Überschreibt Colony-Default aus `colony.json` `message_timeout_default_ms`. `0` oder `-1` = kein Backstop (für Long-Running-Cells). **Nicht** der primäre Timeout für I/O-Operationen — dafür ist `params.external_timeout_ms` (Konzept A) zuständig. `cell.message_timeout` sollte deutlich großzügiger als `params.external_timeout_ms` sein, sodass normalerweise A zuerst greift. |
| `mailbox_size` | *(optional, ab Phase 5)* Bounded-mpsc-Kapazität; überschreibt den Colony-Default (`colony.json` `mailbox_default_capacity`, Default 1000). Siehe overview Abschnitt „Mailbox-Größe". |

### `params`

**`max_concurrency`** (*optional, nur für stateless Cells, ab Phase 7*) lebt im **`params`**-Block, nicht im `cell`-Block: Maximale Anzahl gleichzeitig laufender Worker-Tasks im Stateless-Cell-Dispatcher (siehe `meclaw-overview.md` Abschnitt „Stateless-Cell-Dispatcher"). Default: hoher Wert (effektiv unbeschränkt für typische Lastpfade). Pro Cell konfigurierbar — z.B. `web_fetch` mit `32` (HTTP-Provider-Rate-Limits), `file` mit `8` (Disk-I/O), `bash` one-shot mit `4` (Process-Resource-Limit). Für stateful und long-running Cells wird der Wert ignoriert.

Cell-Type-spezifisch. Jeder Cell-Type definiert seine eigene `params`-Struktur (siehe `cell-types.md`). Die Colony übergibt diesen Block der Cell beim Start; danach sind Param-Updates per Message möglich (last-write-wins, in `cell.db` persistiert). **Form** (W4b): die Update-Message trägt einen **Top-Level-`params`-Body-Slot** (1:1 dieser `params`-Block, partial) — reiner Cell-Inhalt, kein Header-Gate; die Cell merged + persistiert ihn selbst und replayt das Overlay bei wake/respawn über die Geburts-params (`config.json` bleibt unberührt). Welche Felder laufzeit-änderbar bzw. immutable sind (z.B. Credentials, Security-Boundaries), ist cell-type-spezifisch (siehe `cell-types.md`, z.B. `llm` § Laufzeit-Param-Updates).

`${VAR}`-Substitution aus `.env` erfolgt durch die Colony vor Übergabe an die Cell. `${ctx.<key>}` und `${uuid7:<label>}` werden bei Mutation-Anwendung resolved (siehe overview Abschnitt „Variablen-Substitution").

**Konvention für I/O-Cells**: jede Cell, die I/O-Operationen mit unbestimmter Dauer ausführt (HTTP, DB, Subprozess, Filesystem, MCP-Calls), deklariert ein `params.external_timeout_ms`-Feld (oder einen semantisch passenderen Namen wie `query_timeout_ms` für `store`). Die Cell-Implementation wrappt **jede** solche Operation mit `tokio::time::timeout` und emittiert bei Elapsed eine reguläre Error-Message (`header.finish_reason: "error"`, cell-type-spezifischer `error_code` wie `provider_timeout` / `query_timeout` / `script_timeout`). Das ist Konzept A in `meclaw-overview.md` Abschnitt „Timeouts" — der primäre Schutz, präzise pro Operation gesetzt, vom Operator beherrschbar. **`cell.message_timeout`** (im `cell`-Block) ist der grobe Backstop für Cell-Hänger und liegt deutlich über `external_timeout_ms` — Konzept B im selben Abschnitt.

### `contract`

Die `contract`-Schlüssel gliedern sich nach **Enforcement-Stufe** — nicht alle sind in v0.1.0 substrat-hart erzwungen:

| Schlüssel | Enforcement (v0.1.0) |
|---|---|
| `emits` | **substrat-erzwungen** — am `code`-Typ always-on validiert (P13/D-017); übrige emittierende Cell-Types post-v0.1.0 (siehe § Schema-Format und Validierung + `docs/roadmap.md` § Contract-Validierung). |
| `version`, `settings`, `consumes` | **substrat-erzwungen** — Präsenz + JSON-Typ beim Config-Load (Boot-Hard-Fail; Mutations-Reject `contract_incomplete`). |
| `capabilities` | **discovery-only** — Hint für Builder-Composer/Audit-Tools, **kein Runtime-Check** bis zum Hardening (siehe `capabilities`-Hinweis unten). |

**`version`-Format:** non-empty String, frei wählbar (kein semver-Zwang). **`settings`-Format:** Objekt `{ "<key>": SettingSpec }` (siehe § SettingSpec), leeres Objekt zulässig. **`consumes`-Format:** Objekt (siehe § consumes), leeres Objekt zulässig.

Optional: `tools`, `multi_send_capable`.

**Body folgt Universal-Body-Format**: Top-Level-Slots sind primär `system` und `messages[]` (siehe `meclaw-overview.md` Abschnitt „Body-Format (Universal)"). Cells dürfen eigene Top-Level-Slots deklarieren (`meta`, `delta`, `event` etc.). `emits.body` und `consumes.body` deklarieren die Slots, die diese Cell schreibt bzw. liest — unbekannte Top-Level-Slots in einer eingehenden Message werden vom Konsumenten ignoriert.

#### `emits` — was die Cell in ihre Output-Message schreibt

Aufgeteilt in `body` (eigentlicher Content) und `hop` (der isolierte Cell-Output an Routing-Metadaten). Cells emittieren **nur** `hop` — `context` ist allein Edge-Authority und taucht in `emits` nicht auf (siehe overview Abschnitt „Headers vs. Body — Schreibmodell"). Die Cell produziert content-JSON; Colony interpretiert `content.header` als `hop` und nimmt den Rest als `message.body`.

```json
"emits": {
  "body": {
    "<key>": <EmitSpec>,
    ...
  },
  "hop": {
    "<key>": <EmitSpec>,
    ...
  }
}
```

**`EmitSpec`**:
```json
{
  "type":     "string|number|boolean|object|array|blob_uuid",
  "values":   ["..."],
  "required": true
}
```

- `values` optional, nur für `type: string` sinnvoll (Enum-Whitelist).
- `required` defaultet auf `true`.

#### `consumes` — was die Cell aus der eingehenden Message liest

Aufgeteilt in `body` (Content-Slots) und **die zwei Header-Fächer** `context` (persistent) und `hop` (genau dieser Hop). Cells lesen alle drei read-only; sie haben **keine** Kenntnis darüber, wer den Wert wann gesetzt hat — das ist Topologie-Sache. Die Lebensdauer eines Headers ist **rein strukturell** durch den Fach-Namen bestimmt (`context` = persistent, `hop` = hop-lokal/verfällt) — es gibt **keine** Pro-Key-Lebensdauer-Annotation.

```json
"consumes": {
  "body": {
    "<key>": <ConsumeSpec>,
    ...
  },
  "context": {
    "<key>": <ConsumeSpec>,
    ...
  },
  "hop": {
    "<key>": <ConsumeSpec>,
    ...
  }
}
```

**`ConsumeSpec`**:
```json
{
  "type":     "string|number|boolean|object|array|blob_uuid",
  "required": true
}
```

- Falls erforderlicher Wert fehlt: Cell wird nicht aufgerufen, Fehler-Message an `reply_to` (falls gesetzt), sonst Dead-Letter.
- **Mutations-/Lokalitäts-Validator**: der Build-Zeit-Validator nutzt `emits.hop` (was die Cell produziert) zusammen mit `consumes.context` + `consumes.hop` (was die nachgelagerte Cell erwartet), um Lokalität und Reachability eines Header-Werts statisch zu prüfen — ein `hop`-Wert ist nur am unmittelbar folgenden Hop verfügbar (außer eine Edge befördert ihn per `set_context`), ein `context`-Wert über den ganzen Lebenszyklus. Hive-Transits nehmen an der Fan-in-Schnittmenge teil: eine Edge mit Hive-`from` ist eine Transit-Durchreiche und steuert `set_hop` dieser Edge ∪ die Schnittmenge der Beiträge aller Inbound-Edges der Hive bei (rekursiv über mehrstufige Transits, zyklenfest) — derselbe Key-Walk, den die Runtime beim Transit vollzieht (`hop` verfällt nur an einer Cell-Emission, nicht am Transit). **Teilnahme-/Status-Filter am Boot:** Beim Bootstrap trägt der Lokalitäts-Prüfer Contract-Obligationen **nur für aktive Knoten** — Knoten, die am aktiven Graph teilnehmen. Ein registrierter, aber **disconnecteter/inaktiver** Knoten (persistierter `colony.db`-Status beim Reboot **oder** ab t0 inaktiv abgeleitete Insel beim Erst-Boot) ist reine Buchhaltung: er wird rehydratisiert (stabile `cell_id`), unterliegt am Boot aber **keinem** Contract-Zwang. Die volle Prüfung wohnt am **Mutations-Zeitpunkt**, der ihn anschließt (Teilnahme-Regel + transit-bewusste Schnittmenge). Damit ist die Prüfung über beide Boot-Arten uniform: inaktiv ⇒ keine Boot-Obligation; aktiv-und-verdrahtet ⇒ scharf geprüft.

**Enforcement-Stand:** Der substrat-seitige required-`consumes`-Check läuft an der Delivery-Grenze (vor `handle()`): fehlender/typ-falscher required-Key → Fehler-Message an `reply_to` (`error_code: "consumes_violation"`), sonst Dead-Letter (gleicher Token). **Die Error-Reply wird DIREKT an `reply_to` zugestellt** (Registry-Lookup über `route()`), nicht über die Out-Edges des Konsumenten geroutet — sie ist Feedback an einen bekannten Absender, kein Routing-Ziel (W2b Ruling, ruling 2026-06-12; siehe `meclaw-overview.md` § Routing-Fehler „Outputs-Arm — drei disjunkte Fälle", Fall 2). Eine Catch-all-Out-Edge des Konsumenten leitet die Error-Reply nicht um.

#### Schema-Format und Validierung

- Schemas folgen **JSON-Schema Draft 2020-12** (Rust: `jsonschema`-Crate).
- **`code` = always-on Trust-Boundary (kein Opt-out):** die `emits`-Validierung des `code`-Outputs läuft **unbedingt** (`validate_emits = true`) — unabhängig vom Build-Profil **und** von `colony.json` `strict_validation`. `code` ist der einzige user-skript-getriebene Output, dessen Korrektheit nicht aus Cell-Disziplin folgt; deshalb wird er immer geprüft.
- **Übrige emittierende Cell-Types:** `emits`-Validierung läuft zentral am outputs-Arm der Colony nach dem Debug-on-/`strict_validation`-Modell: im Debug-Build immer aktiv, im Release-Build per `colony.json` `strict_validation: true|false` (Default `false`, Schema siehe `meclaw-overview.md` Abschnitt „`colony.json` — Schema").
- **`strict_validation`-Rolle:** steuert damit **nur noch** die künftige Non-`code`-emits-Validierung im Release-Build — auf den always-on `code`-Pfad hat das Flag **keinen** Einfluss.

**Enforcement-Stand:** `code` always-on (in-cell, Zwei-Pass — unverändert); alle übrigen emittierenden Cell-Types werden **zentral an der Emissions-Grenze der Colony** (outputs-Arm) validiert, flag-gated nach dem Debug-on-/`strict_validation`-Modell. **Asymmetrie by design:** `code` prüft in-cell always-on mit All-or-nothing-Zwei-Pass; der Rest läuft zentral, flag-gated und per-Emission — das ist gewollt und kein Drift. Verletzung: Emission wird verworfen; mit `input_reply_to` Error-Reply (`error_code: "contract_violation"`), sonst Dead-Letter (gleicher Token). **Zwei registrierte Grenzen des zentralen Checks (Debug-Netz, keine Trust-Boundary — ratified 2026-06-10):** (a) Error-Replies an ein `input_reply_to`, das auf einen `/colony/*`-Endpunkt oder einen Hive-Pfad zeigt, werden silent verworfen (nur die Cell-Pfad-Cascade wird verfolgt); (b) eine Cell, die im µs-Fenster zwischen Task-Spawn und Landung ihres `SetNodeContract`-Eintrags emittiert (selbst-emittierende Typen beim Boot), passiert den Check fail-open (absenter Eintrag ⇒ vakuose Prüfung).

#### `capabilities` — feste Liste

| Capability | Bedeutung |
|---|---|
| `network:llm` | darf LLM-Provider kontaktieren |
| `network:http` | darf beliebige HTTP-Calls machen |
| `network:search` | darf Search-Provider kontaktieren |
| `network:mcp` | darf MCP-Provider kontaktieren |
| `network:proxy` | darf Chat-Plattform-Provider kontaktieren |
| `fs:read` | darf Dateisystem lesen (innerhalb Boundary) |
| `fs:write` | darf Dateisystem schreiben |
| `shell:exec` | darf Shell-Befehle ausführen |
| `db:own` | darf eigene `cell.db` lesen/schreiben |
| `mutate-graph` | will Graph-Mutationen auslösen (Discovery-Hint, kein Runtime-Check bis zum Hardening) |

Erweiterbar bei Bedarf, zentral in `meclaw-core` dokumentiert.

**Hinweis zu Permissions bis zum Hardening**: Die Capabilities sind in dieser Phase **Discovery-Hints** für Builder-Composer und Audit-Tools, **kein Runtime-Check**. Das gilt insbesondere für `mutate-graph`: ob eine Cell tatsächlich mutieren _kann_, hängt allein an der Topologie (existiert eine Edge nach `/colony/mutations`?). Post-Roadmap-Hardening kann Capability-Tokens addieren, die zur Laufzeit geprüft werden. Siehe overview Abschnitt „Permissions" im Mutation-Format.

#### `ToolSpec`

Deklariert, welche Tools die Cell ihrem LLM (oder externen Konsumenten) anbietet. **Kein Routing-Endpoint** — wohin Tool-Calls geroutet werden, entscheidet die Topologie.

```json
{
  "name":   "<tool-name>",
  "schema": { ... }
}
```

#### `SettingSpec`

```json
{
  "type":        "string|number|boolean|object|array",
  "secret":      false,
  "default":     "<value>",
  "description": "<text>"
}
```

#### Flags

- `multi_send_capable`: Cell kann mehrere Output-Messages aus einem einzigen Input erzeugen. Aktiviert das Cell-Type-spezifische Multi-Send-Wire-Format — für `code` z.B. das JSON-Array-Format auf stdout (siehe `cell-types.md`). Jede emittierte Message läuft unabhängig durch die ausgehenden Edges; Colony evaluiert pro Message frisch. Der Wert kommt aus `contract.multi_send_capable` (Bool, Default `false`). Die frühere `params.multi_send_capable`-Bridge ist entfernt — ein `params`-Wert wird von der `code`-Factory ignoriert.

### `description`

Sechs Schlüssel — **builder-erzwungen**, nicht substrat-hart: die Struktur greift, sobald der Builder/Composer sie konsumiert (dieselbe Discovery-Vertragsfläche für LLM-Builder, der Edges schreibt, und Reviewer/Operator), nicht als Boot-Validierung im Substrat.

| Slot | Inhalt |
|---|---|
| `purpose` | Warum existiert diese Cell? Welches Problem löst sie? (1–2 Sätze) |
| `use_when` | Wann greift der Composer zu diesem Template? Vorbedingungen, Alternativen. |
| `not_in_scope` | Was tut diese Cell bewusst **nicht**? Hilft dem Builder, die Cell auszuschließen, wenn sie nicht passt. |
| `emits_meaning` | Semantik der `contract.emits`-Einträge — was bedeuten sie über Type-Info hinaus? |
| `consumes_meaning` | Semantik der `contract.consumes`-Einträge. |
| `examples` | Konkrete Input/Output-Beispiele; mindestens eines. |

**Bei Hive-Scope-Markern** (`cell.type: "hive"`): `description` beschreibt den Scope-Zweck (was bündelt dieser Hive? wann benutzt der Builder ihn? was gehört nicht hinein?). `emits_meaning` und `consumes_meaning` entfallen, da Hive-Scope-Marker nicht am Message-Flow teilnehmen.
