# meclaw — System-Beschreibung

Dateibasiertes, LLM-orientiertes Aktor-Workflow-System für agentische Harnesses. Rust-Binary, Linux. Spezialisiert auf LLM-typische Flows mit stark vereinfachtem Flow-Management — deutlich einfacher als BPMN oder Serverless Workflow, ohne Anspruch auf deren Generalität.

## Was ist meclaw

Ein Workflow-System, dessen Topologie als Verzeichnisbaum im Filesystem lebt. Jeder Knoten ist eine Cell (Aktor), Verzeichnisse mit `type: "hive"` markieren Authority- und Mutations-Boundaries. Die Topologie wird zur Laufzeit von Cells selbst mutiert — typisch durch **Builder-Hives** (mehrstufige Hive-Scopes mit LLM-Cell, Diff-Konstruktor-Hive und Validator-Hive), die natürlichsprachige Aufträge in Graph-Mutationen übersetzen. Cells kommunizieren ausschließlich über atomare Messages mit einem universellen Body-Format. LLM-Inferenz, Tool-Aufrufe, persistente Speicherung, Long-Running-Bridges (Telegram, Timer, MCP) sind alles Cell-Types.

Die DSL ist hierarchisch (Verzeichnisse, Pfade `/main/sub/leaf`), das Aktor-Substrat darunter ist flach (Cells werden direkt in einer zentralen Colony-Registry registriert, Routing ist O(1)-Lookup). Diese Trennung ist Absicht — die DSL bleibt für Menschen und Builder-LLMs natürlich lesbar, die Implementation bleibt mit dem Tokio-Idiom konsistent und konzentriert die Concurrency-Komplexität an genau einer Stelle (Colony).

## Einordnung (Vergleichsachse)

| System | Was meclaw teilt | Was meclaw anders macht |
|---|---|---|
| Erlang/OTP | Aktoren, Mailbox, Supervisor | Topologie als Datei, nicht als Code; LLM-spezialisiert |
| NATS | Subject-basiertes Routing | Compute-Knoten zusätzlich zum Transport |
| Node-RED | Knoten sind dumm, Graph routet | CLI-/Filesystem-first, durable, LLM-spezialisiert |
| LangGraph | Graph für agentische Flows | Sprach-agnostisch, dateibasiert, persistent |
| Temporal | Durable Execution, Message-Log | Leichtgewichtig, dezentral, Filesystem-DSL |
| BPMN, Serverless Workflow | Workflow-Engine mit deklarativer Definition | Stark vereinfacht; deckt nur LLM-Flow-Patterns, kein Anspruch auf Generalität |

---

## Kernprinzipien

- **Filesystem ist Single Source of Truth.** Verzeichnisbaum mit `config.json` pro Knoten = Topologie.
- **Cell weiß nichts über Topologie.** Keine Kenntnis von Sender, Empfänger, Hop-Historie, anderen Cells. Kennt nur ihren Vertrag, ihre Params, die aktuelle Message.
- **Messages sind atomar.** Trace-Rekonstruktion via `parent_message_id` aus dem zentralen Message-Log, nie in der Message.
- **DSL ist die CPU.** Domänen-frei. Kennt nur Routing, Messaging, Cell-Lifecycle. Alles oben drauf ist OS.
- **Cells im `templates/`-Ordner sind Klassen, Cells im Dateibaum sind Instanzen.** Eine Cell ist topologisch neutral — was sie ist, ergibt sich aus ihrem Ort. Lazy-Instantiation kopiert eine Template-Hive in den Tree.
- **Graph entscheidet alles.** Routing, Filterung, Fan-out — ausschließlich über Edges.
- **Hierarchie ist DSL, Substrat ist flach.** Verzeichnis-Verschachtelung und hierarchische Pfade in der DSL — flach implementiert als zentrale Pfad-Registry in der Colony. Routing in einer O(1)-Lookup, keine Hop-by-Hop-Cascade.
- **Hives sind Scope-Marker.** Verzeichnisse mit `type: "hive"` markieren Authority- und Mutations-Boundaries für ihren Subtree. Kein eigener Aktor-Typ, keine eigene Task, keine eigene Mailbox. Wirken zusätzlich als **logischer Transit-Knoten im Routing-Graph**, von Colony ausgewertet — kein Aktor, keine Zustellung.
- **Colony ist die Authority.** Für Lifecycle, Registry, Templates, Routing. Alle Cells werden direkt bei Colony registriert. Schreibt `config.json` **nur bei Instanziierung** (der re-dedizierte `swap_nodes`-Graph-Swap schreibt kein existierendes `config.json` mehr neu, siehe § Mutation-Operationen).
- **Alles ist eine Message — auf der Daten-Ebene** (Zell-zu-Zell-Verkehr, Tool-Calls, einheitliches Body-Format). **Steuer-Befehle an die Colony** (Mutationen, Param-Updates, Supervisor-Events, externe API-Calls) **sind intern typisierte Inbox-Commands** — gleiches UBF-Datenmodell, gleiche sequentielle Colony-Task. Eintritts-Pfade und der heute noch nicht realisierte Cell-Eintritt: siehe § Mutation-Format.
- **Tool-Loops sind Topologie, nicht Cell-Verantwortung.** LLM-Cells haben keine innere Schleife.
- **Schwarm ist selbst-modifizierend.** Builder-Hives (Hive-Scopes mit mehreren spezialisierten Cells) generieren neue Topologie zur Laufzeit über scoped Mutations.
- **Keine leeren Verzeichnisse.** Jedes Verzeichnis im Tree braucht `config.json`, sonst existiert es nicht.
- **UUID v7 überall.** IDs sind zeitsortiert (Messages, Cells, Templates, Blobs, Traces).
- **UTC überall.** Alle Zeitpunkte im System sind UTC — keine lokale Zeitzone, kein TZ-Offset. Das Serialisierungs-Format ist feld-/kontextspezifisch (z.B. Envelope `created_at` als Unix-Sekunden `i64`; menschenlesbare/Log-Timestamps wie Blob-Sidecar `created_at` oder `timer`-Header `scheduled_at`/`fired_at` als ISO-8601/RFC-3339 mit `Z`). Lokale Zeitzonen / `chrono-tz` sind für v0.1 deferred.
- **Agentic-first, human-second.** Diskussionen lösen sich zugunsten der Variante auf, die für agent-getriebene Builder optimal ist.
- **Hochgradig parallel, multithreaded, nebenläufig — by design.** Tokio multi-thread Runtime, jede Cell und die Colony als eigene Task, Sequentialitäts-Garantien nur dort, wo die Architektur sie explizit setzt. Vollständig im Abschnitt „Nebenläufigkeit & Parallelität" — **vor jeder Implementierungs-Entscheidung lesen**.

---

## Nebenläufigkeit & Parallelität

> **Das gesamte System ist hochgradig parallel, multithreaded und nebenläufig.** Das ist nicht optional, keine spätere Optimierung und kein Implementierungs-Detail — es ist Grundannahme jeder einzelnen Architektur-Entscheidung und jeder Cell- oder Colony-Implementierung. Wer diesen Abschnitt nicht gelesen hat, hat das System nicht verstanden.

### Runtime

**Tokio multi-thread Runtime** (work-stealing Scheduler), Default-Flavor `multi_thread`. Anzahl Worker-Threads: Default = Anzahl CPU-Cores (Tokio-Default). Kein `current_thread`-Runtime in Library- oder Binary-Code. Kein `block_on` im Library-Code — Library-APIs sind durchgängig `async`. (Ausnahme für reine Unit-Tests ohne Topologie: siehe „Test-Infrastruktur".)

### Was läuft als eigene Tokio-Task

| Akteur | Tasks pro Instanz | Eingang |
|---|---|---|
| **Stateful Cell** (z.B. `llm`, `store`, `code` mit `cell.db`) | 1 langlebige Task | eigene mpsc-Mailbox |
| **Stateless Cell** (z.B. `web_fetch`, `web_search`, `file`, `edit`, `bash` one-shot) | 1 langlebige Dispatcher-Task + pro Message eine kurzlebige Worker-Task | eigene mpsc-Mailbox |
| **Long-Running Cell** (`proxy`, `timer`, `mcp`) | **2 Work-Tasks** (Handler + I/O) — konzeptuelle Arbeits-Task-Zahl; gekapselt in **einer** äußeren Glue-Supervisions-Task mit genau einem `JoinHandle` (siehe „Long-Running-Cells: Doppel-Task") | externe Mailbox + interner Channel |
| **Colony** | 1 langlebige Task | eigene mpsc-Mailbox (zentrales Routing + `/colony/*`-Endpoints) |
| **HTTP-API** (`axum`) | Tokio-natives Task-pro-Request | übersetzt jeden Request in eine Message und reicht ihn an Colony |

**Hives sind keine Tasks.** Sie sind reine Scope-Marker im Filesystem (`config.json` mit `type: "hive"`) und im Pfad-Schema. Routing, Authority-Boundaries und Mutation-Scoping basieren auf Pfad-Präfixen, nicht auf eigenen Aktoren oder Mailboxen. Wenn ein Hive-Pfad als Message-Target adressiert wird, wirkt er zusätzlich als **logischer Transit-Knoten** in Colonys einer Routing-Schicht (Details siehe „Hive-Pfade als Target — Transit-Auswertung") — kein Aktor, keine Mailbox-Zustellung.

Tokio-Tasks sind ~3 KB Stack, kein OS-Thread. Tausende schlafende Cells haben praktisch keinen Overhead — nur ihre Mailbox-Channels in Colony's Registry (siehe „Hot/Cold-Cell-Modell").

### Sequentialität — was die Architektur garantiert

Diese Sequentialitäts-Inseln gelten **by design**. Cell- und Colony-Code dürfen sich darauf verlassen und müssen sie **nicht selbst herstellen** — keine `Mutex`, keine `RwLock`, keine atomics, keine Defensive gegen Reentrancy:

- **Innerhalb einer Cell**: ein `handle()`-Call läuft vollständig durch, bevor der nächste startet. Folgt aus mpsc-Pull-Semantik der einen Cell-Task. **Cell-State ist aus Cell-Sicht effektiv single-threaded zugreifbar.**
- **Innerhalb der Colony**: Routing-Lookups, Mutations-, Registry- und Templates-Operationen laufen sequentiell durch Colony's einzige Task. Eine Mutation kann atomisch zwischen zwei Routing-Schritten einschneiden — andere Messages pausieren in Colony's Mailbox, bis die Mutation fertig ist. Kein paralleles Schreiben von `config.json`, kein paralleles Anlegen von Staging-Verzeichnissen für dieselbe Mutation.
- **Long-Running-Cells, aus State-Sicht**: aus Sicht der State-haltenden Handler-Task laufen Inbound (Mailbox) und Outbound (Provider-Event) ebenfalls sequentiell — die I/O-Task pufft Events nur in einen internen Channel, der Handler verarbeitet einen nach dem anderen.

### Parallelität — was die Architektur erzwingt

Das Gegenstück: alles, was nicht in obiger Aufzählung steht, läuft **parallel** und nutzt dabei alle Worker-Threads des Tokio-Schedulers:

- **Zwischen Cells**: alle Cell-Tasks sind unabhängig. Während Cell A einen LLM-Call fährt, kann Cell B parallel eine DB-Query laufen lassen, Cell C einen HTTP-Fetch — auf bis zu N Cores gleichzeitig.
- **Fan-out**: emittiert eine Cell eine Message und hat sie mehrere matchende ausgehende Edges in Colony's Edge-Tabelle, dispatcht Colony in einem Routing-Schritt an alle relevanten Handles; die Empfänger-Tasks laufen unabhängig parallel weiter.
- **Stateless Cells**: die Cell-Dispatcher-Task spawnt pro eingehender Message einen kurzlebigen Worker-Task. Worker-Tasks haben keinen Persistent-State und terminieren nach Emit. Hundert parallele Web-Fetches sind hundert Worker-Tasks, die der Scheduler über die Worker-Threads verteilt. Concurrency-Limit pro Cell konfigurierbar via `params.max_concurrency` (siehe „Stateless-Cell-Dispatcher").
- **Long-Running-Cells, aus I/O-Sicht**: die I/O-Task läuft unabhängig vom Handler-Task. Ein 30s-Telegram-Long-Poll blockiert nicht die Verarbeitung einer eingehenden meclaw-Message.
- **Colony und Cells**: Colony verarbeitet Routing-Entscheidungen sequentiell, gibt Messages aber sofort an die Empfänger-Cells weiter — die Empfänger-Verarbeitung läuft parallel über alle Worker-Threads.

### Long-Running-Cells: Doppel-Task

Cells vom Typ `proxy`, `timer` und `mcp` haben einen Cell-internen Doppel-Task-Aufbau. Aus Topologie-Sicht bleibt die Cell eine einzige Adresse mit einer einzigen externen Mailbox — die Doppelstruktur ist Implementations-Detail dieser Cell-Types, aber **für diese Cell-Types vorgeschrieben**, nicht optional.

```
externe Mailbox (mpsc) ──►  [ Handler-Task ]  ◄── interner mpsc ── [ I/O-Task ]
                                  │                                      │
                                  │  hält Cell-State                     │  pollt Provider
                                  │  (cell.db, Schedules,                │  / wartet Timer
                                  │  Cursor, Session-Handles)            │  / liest WebSocket
                                  │                                      │
                                  ▼                                      ▼
                            tokio::select!                       sendet Event-Frames
                            über beide Eingänge                  in den internen mpsc
```

- **I/O-Task**: pollt den Provider (Telegram Long-Poll, MCP-Stream), wartet auf die nächste Cron-Zündung via `tokio::time::sleep_until`, hält eine WebSocket. Bei einem Event: serialisiert es zu einem internen Event-Typ und sendet es per internem mpsc an den Handler-Task. Diese Task **fasst niemals Cell-State direkt an** und **hält keinen `outputs_tx`** — sie pusht ausschließlich in den internen Channel, alle topology-gerichteten Emissionen laufen über den Handler.
- **Handler-Task**: macht `tokio::select!` über die externe Mailbox und den internen Channel vom I/O-Task. Beide Quellen werden zu einem einzigen sequentiellen Stream serialisiert, in dem ein Event nach dem anderen verarbeitet wird. **Cell-State wird ausschließlich in dieser Task verändert** — kein Mutex nötig, keine Lock-Verhandlung. Der Handler hält den `outputs_tx` (einmal beim Spawn geklont) und ruft `cell.handle(...).await` bzw. `cell.handle_event(...).await`, die wiederum `outputs_tx.send` aufrufen.

Damit:
- Inbound aus Topologie und Outbound vom Provider können nie gleichzeitig denselben State anfassen (Race-frei aus Cell-Sicht).
- Ein langer Provider-Poll (30s Long-Poll) blockiert die Mailbox nicht — der Handler ist nicht in I/O gefangen, der I/O-Task wartet eigenständig.
- Ein langer Mailbox-Stau bedeutet nur, dass Provider-Events sich im internen Channel stauen, nicht dass die Provider-Verbindung zusammenbricht.
- **Output-Backpressure-Kaskade**: voll-laufende `outputs`-Mailbox blockiert Handler beim `outputs_tx.send` → interner Channel füllt sich → I/O-Task blockt beim Push in den internen Channel → externes Polling drosselt sich selbst (z.B. Telegram-Long-Poll-Frequenz). Rückwärts-Propagation ohne zusätzliche Mechanismen — liveness-sicher unterhalb der Sättigungsgrenze (§ Backpressure → Liveness-Grenze).

**Restart-Verhalten**: wenn entweder I/O-Task oder Handler-Task panickt, wird die gesamte Cell von Colony's Supervisor neu instanziiert (one_for_one) — beide Sub-Tasks werden gemeinsam neu aufgesetzt.

**`run_io`-Lebensdauer-Vertrag (A1′)**: Die I/O-Task-Funktion (`run_io`) läuft für die **gesamte Lebensdauer der Cell** — sie pollt/wartet endlos und kehrt erst zurück, wenn die Cell als Ganzes endet (Teardown: Handler-Schließung der internen Channels, Disconnect oder Panic). Ein **sauberer, freiwilliger Return aus `run_io` bei noch lebender Cell ist ein Vertragsbruch**: er legte die I/O-Seite still, während der Handler weiterläuft, und eröffnet die latente „io-finish-first"-Verlust-Klasse (das äußere `select!` über beide `JoinHandle`s könnte den I/O-Abschluss gewinnen und das überlebende Handler-Geschwister mitsamt unverarbeiteten Events aborten). Keine reale Cell (`proxy`/`timer`/`mcp`) löst das heute aus — alle I/O-Loops sind endlos (`pending`/Schleife) —; die Invariante verbietet die Klasse für künftige Implementierer explizit.

### Bottleneck und Lösung

Colony's Mailbox ist single-consumer und sequentiell. Bei sehr hohem Routing-Durchsatz wird sie ein potentieller Bottleneck. Für die im Roadmap-Horizont vorgesehene Lastebene (LLM-zentrierte Flows mit Sekunden-skaligen Latenzen pro Step) liegt das weit unter der Grenze. Falls Performance später ein Treiber wird, sind die Lösungs-Pfade additiv: Routing-Tabelle könnte read-mostly werden, Edge-Auswertung könnte aus dem Routing-Pfad herausgezogen werden, oder mehrere Colony-Instanzen können verschiedene Subtrees bedienen (Cross-Colony-Föderation, post-roadmap). Aktuell: keine Optimierung, das System bleibt einfach.

### Backpressure

Bounded mpsc-Mailboxes (Default 1000 pro Cell, ab Phase 1) plus `block` als **einzige** Strategie: wenn eine Mailbox voll ist, wartet der Sender (`send().await`), bis Platz ist. Damit propagiert Backpressure rückwärts durch den Graph — ohne silent message loss **auf dem Live-Backpressure-Pfad** (der Cell-Panic-/Restart-Pfad ist davon ausgenommen und verliert wartende Nachrichten, siehe § Cell-Robustheit), ohne explizite Drop-Logik, ohne Strategie-Wahl pro Cell.

**Zwei Rückwärts-Kaskaden** existieren symmetrisch:

1. **Inbox-Backpressure**: voll-laufende Cell-Mailbox lässt Sender (Colony beim Routing) blockieren → Colony drained ihre Routing-Inbox langsamer → upstream-Cells, die nach Colony schreiben, blockieren ebenfalls.
2. **Output-Backpressure**: jede Cell schreibt ihre Emits in eine zentrale `outputs`-Mailbox (siehe „Output-Pfad"). Voll-laufende Outputs-Mailbox lässt die Cell beim `outputs_tx.send().await` blockieren → die Cell drained ihre eigene Inbox langsamer → siehe (1).

Eine ganz tote Cell wird durch Message-Timeout + `one_for_one`-Restart aufgefangen (siehe „Cell-Robustheit").

**Liveness-Grenze (ehrlicher Stand).** `block`-Backpressure ist liveness-sicher, solange Zufluss ≤ Abfluss bleibt (LLM-zentriertes Zielprofil: Sekunden-skalige Steps, weit unterhalb der Mailbox-Kapazität 1000). Bei **anhaltender Über-Sättigung** über eine **geschlossene Wait-Kette** (Cell A blockiert auf B, B auf … auf A — ein Zyklus aus `send().await`-Wartenden) entsteht ein **Wartekreis-Deadlock**: keiner der Sender kommt frei, der Kreis steht permanent. Besonders exponiert sind backstop-lose Long-Running-Source-Cells (`message_timeout` `0`/`-1`, kein Backstop — § Timeouts) im Zyklus. **TTL fängt das NICHT** — TTL zählt Routing-Hops (Colony dekrementiert pro Routing-Entscheidung); im Deadlock fließt keine Nachricht, also wird TTL nie dekrementiert. Dieser Fall liegt jenseits des Roadmap-Lastprofils und ist als post-MVP-Posten registriert (`docs/roadmap.md` § „Backpressure-Wartekreis-Deadlock unter Über-Sättigung").

Für **stateless Cells** spielt sich Inbox-Backpressure am Dispatcher ab — der Dispatcher bremst sich selbst über die `Semaphore` (siehe „Stateless-Cell-Dispatcher"), Worker-Tasks selbst können nur an Output-Backpressure hängen.

### Was Cell-Implementierer **nicht** beachten müssen

- Kein `Arc<Mutex<...>>` über Cell-State.
- Keine `RwLock`-Verhandlung.
- Keine atomics oder Lock-free-Datenstrukturen.
- Keine Reentrant-Calls in den eigenen `handle()`.
- Keine „was, wenn zwei Messages gleichzeitig …"-Defensive.

**Aus Cell-Sicht ist die Welt single-threaded.** Die Parallelität liegt außerhalb der Cell, im Tokio-Scheduler und in Colony's Routing-Verteilung. Wer `Mutex` in Cell-Code schreibt, hat das Concurrency-Modell nicht verstanden — Code-Review weist das zurück.

### Was Cell-Implementierer **doch** beachten müssen

- **`handle()` ist async, alle I/O via `.await`.** Ein synchroner `std::thread::sleep`, ein blockierender DB-Treiber oder ein synchroner Netzwerk-Call blockiert den Tokio-Worker-Thread und sabotiert die Parallelität für alle anderen Tasks auf diesem Worker. Verboten.
- **Lange CPU-Bursts** (>1 ms ohne `.await`-Punkt): explizit `tokio::task::yield_now().await` einfügen oder per `tokio::task::spawn_blocking` auslagern. Sonst blockiert ein CPU-Burst andere Tasks auf demselben Worker.
- **Kein `block_on`** in Cell- oder Colony-Code. Niemals. Auch nicht in Tests, wenn der Test eine echte Topologie hochfährt.
- **Keine Annahme über Reihenfolge zwischen Cells.** Cell A und Cell B emittieren parallel — welche Message zuerst ankommt, hängt vom Scheduler ab. Wer Reihenfolge braucht, baut sie per Topologie (Collector-Hive mit Korrelations-ID).

Neben Cell- und Colony-Tasks kann das Substrat **prozessweite Infrastruktur-Aktoren** betreiben (P10: der Token-Broker, der den OAuth-Refresh für alle `llm`-Cells serialisiert). Sie folgen derselben Regel: eine Task, State ausschließlich in dieser Task, kein Lock.

---

## Authority-Modell

Die Colony ist die einzige Schreib-Authority im System. Hives sind Scope-Marker im Filesystem, keine eigenen Aktoren — sie definieren Authority- und Mutations-Boundaries für ihren Subtree, ohne selbst eine Mailbox oder Routing-Logik zu besitzen. Sie wirken zusätzlich als logische Transit-Knoten im Routing-Graph; ihre Transit-Auswertung ist **Colony-Authority** (Colony wertet die Hive-Out-Edges aus, kein Hive-eigener Auswertungs-Code).

| Authority | Träger |
|---|---|
| Liest/schreibt `config.json` (nur bei Instanziierung) | Colony |
| Instanziiert Cells (Template-Copy, UUID-Vergabe) | Colony |
| Hält zentrale `HashMap<Path, ActorHandle>`-Registry | Colony |
| Hält Mutations-Log (Audit-Trail) in `colony.db` | Colony |
| Routet Messages (O(1)-Lookup mit vorgeschalteter Pfad-Resolution) | Colony |
| Lifecycle aller Cells (Start/Stop/Restart) | Colony |
| Templates-Registry | Colony |
| `.env`-Substitution beim Bootstrap | Colony |
| Zentrales Message-Log (filterbar nach Pfad-Präfix) | Colony |
| Authority-Scope für Mutations (Pfad-Präfix-basiert) | Hive (Scope-Marker) |

**Hive als Scope-Marker**: ein Verzeichnis mit `config.json` `type: "hive"` markiert einen Pfad-Präfix als Authority-Boundary für scoped Mutations. Es gibt **kein** Hive-Aktor, **keine** Hive-eigene `cell.db`, **keine** Hive-eigene Routing-Tabelle. Die DSL-Wirkung („Verzeichnis-Verschachtelung gruppiert Cells zu einer Einheit") bleibt erhalten, die Implementierung ist flach. Transit-Edges eines Hives (Edges mit `from = <hive-path>`) liegen in **Colonys einer `EdgeTable`** — derselben Datenstruktur wie Cell-Edges, indexiert nach `from`. Colony wertet sie im Rahmen ihrer einen Routing-Schicht aus, keine separate Hive-Routing-Logik und kein Hive-eigener Auswerter.

**Lifecycle von `config.json`**: Colony schreibt `config.json` **ausschließlich bei Instanziierung** (Template-Copy mit UUID-Vergabe und `${VAR}`-Substitution). Nach Instanziierung wird sie nie wieder angefasst — Bootstrap-Snapshot. (Der re-dedizierte `swap_nodes`-Graph-Swap schreibt kein existierendes `config.json` neu — er schwenkt Edges, siehe § Mutation-Operationen.) Live-State einer Cell lebt in ihrer `cell.db`. Eine globale Topologie-Wahrheit lebt in Colony's Registry (in-memory) und in `colony.db` (persistiert).

**Instanziierung & cell_id-Stabilität**: Instanziierung findet **genau dann** statt, wenn am
Zielpfad kein Cell-Verzeichnis existiert. Colony prüft beim Verarbeiten eines Graphen —
`params.graph` beim Bootstrap wie Mutation-Diff zur Laufzeit — pro deklariertem Node, ob das
Verzeichnis im Tree existiert: fehlt es, kopiert Colony das referenzierte Template an den
Zielpfad und vergibt dabei eine frische UUID v7 als `cell_id` (plus `${VAR}`-Substitution);
existiert es, ist die Operation ein **Reconnect/Resume** — keine neue `cell_id`, `config.json`
wird nicht angefasst, `cell.db` wird resumed (M1). Resume setzt Typ-Gleichheit voraus; weicht der
`type` des Bestandsknotens vom Template ab, wird die Mutation mit `resume_type_mismatch` abgelehnt
(kein stilles Resume). **`cell_id` wird genau einmal vergeben und
danach nie geändert oder neu vergeben.** Templates selbst sind id-los — IDs entstehen
ausschließlich beim Kopieren in den Tree. Bei der Instanziierung trägt Colony den Knoten mit
seiner `cell_id` in `colony.db` ein; Einträge dort werden nie gelöscht, nur als inaktiv
markiert (siehe § Konnektivität & Aktivität).

**Flow bei Graph-Mutation (EDA, Verdikt-Reply an `reply_to`)**:

1. Irgendwer (Cell, Builder, externe API) sendet eine Mutation-Message an `/colony/mutations` mit `hop.msg_type == "mutation"` und einem Diff (siehe „Mutation-Format"). Der Diff trägt einen Pfad-Präfix als Scope (typisch der Pfad eines Hive-Scope-Markers). Der Sender erfährt den Ausgang über die Verdikt-Reply an `reply_to` (falls gesetzt, siehe Schritt 4).
2. Colony validiert einstufig: Schema, Match-Patterns gegen aktuelle Registry, Cycle-Check im post_state, Edge-Schema-Compat, Template-Existenz, Filesystem-Vorbereitung, `.env`-Variablen. Bei Fehler: Logging + Antwort an `reply_to` (falls gesetzt). Mit flachem Substrat hat Colony alle nötigen Informationen für eine einstufige Validierung (kein altes Zwei-Stage-Modell mehr).
3. Bei Erfolg: Mutation in `colony.db` als `in_flight` markieren, alle neuen Cell-Verzeichnisse unter `{root}/.staging/<mutation_id>/<cell_name>/` aufbauen (`config.json` mit substituierten Werten und vergebenen UUIDs, ggf. `cell.db` aus Seed), dann sequenziell per `rename(2)` an die finalen Pfade verschieben (auf POSIX atomar pro Verzeichnis) — pro Verzeichnis atomar, aber **NICHT transaktional über alle Verzeichnisse**: scheitert ein `rename(2)` nach bereits erfolgten, stehen die früheren Renames im Live-Tree (Audit-Modell, kein Rollback). Das Substrat behandelt diesen Halb-Zustand laut (Strict-Fail, § Validierung) statt ihn als sauberen Reject zu kaschieren. Registry-Edits werden ausgeführt: neue Cells spawnen und werden unter ihrem Pfad registriert, disconnectete Cells werden inaktiv markiert — Registry-Eintrag und Filesystem bleiben, die Tasks enden graceful (siehe „Konnektivität & Aktivität"). Anschließend in `colony.db` als `committed` markieren. Bei Crash zwischen `in_flight` und `committed`: Recovery-Pass beim nächsten Startup (siehe „Startup-Algorithmus"). Begründung des Staging-Patterns siehe „Filesystem-Layout" → `.staging/`.
4. Cell-Inits laufen asynchron. Bei Init-Failure: Restart one_for_one (N Retries, Default 5), dann `failed`-Status. Symptome via Routing-Cascade (`reply_to` → `/colony/dead_letters`) sichtbar. Das Mutations-Verdikt selbst geht als Reply an `reply_to` (falls gesetzt): `{"mutation":{"id":…,"outcome":"committed"}}` bei Erfolg, `"outcome":"rejected"` plus `error_code`/`details` bei Ablehnung (`build_mutation_reply`, siehe § Dynamik / Builder-Pattern). Das Ack deckt den Mutations-Commit, **nicht** den Erfolg der asynchronen Cell-Inits.

**Cross-Colony-Föderation** (mehrere `meclaw`-Instanzen mit verschiedenen Colonies, die untereinander kommunizieren) ist post-roadmap. Die Architektur — Colony als Authority-Einheit, eindeutige Pfade innerhalb einer Colony — wird das nicht verhindern, implementiert es aber jetzt nicht.

---

## Graph-Schema

Der Graph einer meclaw-Colony ist ein gerichteter Graph aus **Nodes** (= Cells, registriert in Colony's `HashMap<Path, ActorHandle>`) und **Edges** (= Routing-Regeln zwischen Pfaden, gehalten in Colony's Edge-Tabelle). Hive-Scope-Marker sind **keine Cells und keine Aktoren** — keine Mailbox, kein `RegistryEntry`. Sie sind aber **logische Transit-Knoten im Routing-Graph**: ihre Pfade können `from`/`to`-Endpunkt einer Edge sein, und Colony wertet sie als Teil ihrer einen Routing-Schicht aus (siehe „Hive-Pfade als Target — Transit-Auswertung").

Dasselbe Schema beschreibt den Graph in zwei Schreib-Verwendungen:

1. **Bootstrap**: `params.graph` in der `config.json` eines Hive-Scope-Markers liefert den initialen Soll-Stand für seinen Subtree bei Erst-Instanziierung. Colony liest das beim Filesystem-Bootstrap (siehe „Startup-Algorithmus") und registriert die deklarierten Cells.
2. **Runtime Diff**: Builder schickt eine Mutation-Message mit einem Diff an `/colony/mutations` (siehe „Mutation-Format"). Colony berechnet daraus den post_state und führt scoped Registry-Edits aus.

### Schema

```json
{
  "nodes": {
    "<name>": {
      "template": "<template-ref>",
      "override_params": { ... }
    }
  },
  "edges": [
    {
      "from": "<path-relative-to-scope>",
      "to":   "<path-relative-to-scope>",
      "condition": "<CEL-Boolean, default true>",
      "modifier":  { "set_context": { "<key>": "<CEL>" }, "delete_context": ["<key>"], "set_hop": { "<key>": "<CEL>" }, "delete_hop": ["<key>"] }
    }
  ]
}
```

**`nodes`**: Mapping `name → { template, override_params? }`. Der Name ist die Pfad-Komponente und muss innerhalb desselben Hive-Scopes eindeutig sein — Kollisionen werden bei der Mutations-Validierung rejected. `template` ist eine Template-Referenz im Format `<name>` (höchste verfügbare Version) oder `<name>@<version>` (siehe „Auflösung `name@version`"). `override_params` ist optional und überlagert die Default-Params des Templates.

**`edges`**: Liste; Reihenfolge irrelevant. Pflichtfelder: `from`, `to` (Pfade relativ zum Hive-Scope, in dem das Schema deklariert ist). Die Pfade dürfen in **beliebiger Tiefe** innerhalb des Scopes liegen (`./name` ebenso wie `./unit/dispatch`) — eine Tiefen-Restriktion existiert nicht; das gilt symmetrisch im Bootstrap-`params.graph`-Pfad und im Mutations-Diff (R12-Ruling 2026-06-11). Optional: `condition` (CEL-Boolean, default `true` = matcht immer), `modifier` (Operations-Objekt mit `set_context`/`delete_context`/`set_hop`/`delete_hop`, default `null` = Identity — siehe „Edge-Modell" für Schema und Beispiel). Edges operieren strikt auf der Header-Schicht.

**Form-Unterschied Schreib- vs. Lese-Seite**: Im Schreib-Schema (Bootstrap und Mutation-Diff) referenzieren Edges Nodes per Name (`./<name>`). UUIDs entstehen erst zur Laufzeit — Node-UUIDs vergibt Colony bei Instanziierung, Edge-UUIDs vergibt Colony bei Anlage. Das Lese-Schema (`/colony/graph?scope=...` bzw. HTTP `GET /graph?scope=...`, siehe „Visibility / Read-Pfade") zeigt zusätzlich `id`, `path`, `graph_version` etc. — die Read-Seite ist die runtime-projizierte Form derselben Struktur.

---

## Mutation-Format (Builder → Colony)

Eine Mutation ist eine Message an `/colony/mutations` mit `hop.msg_type == "mutation"`, deren Body einen **Diff** plus **Scope** trägt. Colony validiert einstufig, führt aus, antwortet bei Fehler an `reply_to`. Eintritts-Pfade heute: HTTP-Kante (Phase 12, direkte Übersetzung) und interner Bootstrap-Inbox-Command. Eine von einer Cell an `/colony/mutations` emittierte Message wird **direkt dispatcht** (W2b Ruling, ruling 2026-06-12): der outputs-Arm erkennt ein `/colony/*`-Target und routet es VOR der Edge-Auswertung über `route()` zum virtuellen Endpunkt (siehe § Routing-Fehler „Outputs-Arm — drei disjunkte Fälle", Fall 1) — keine Out-Edge nötig/möglich. Cell-emittierte Mutationen/Reads (EDA) sind damit ein erster-Klasse-Zustellweg; ein unbekannter `/colony/<x>`-Endpunkt landet als `colony_endpoint_unimplemented` in der DLQ.

```json
{
  "scope": "/main/agent-pool",
  "diff": {
    "add_nodes":    [ { "name": "...", "template": "...", "override_params": {} } ],
    "remove_nodes": [ { "match": { ... } } ],
    "add_edges":    [ { "from": "...", "to": "...", "condition": "...", "modifier": { "set_context": {}, "delete_context": [], "set_hop": {}, "delete_hop": [] } } ],
    "remove_edges": [ { "match": { ... } } ],
    "swap_nodes":   [ { "match": { ... }, "with": { "template": "..." } } ]
  },
  "ctx": { "key": "value" }
}
```

**`scope`** ist ein absoluter Pfad-Präfix, typisch der Pfad eines Hive-Scope-Markers. Alle relativen Pfade im Diff werden gegen diesen Scope aufgelöst. Mutationen, deren Pfade außerhalb des Scopes liegen würden, werden bei der Validierung rejected.

**`diff`** enthält die Änderungs-Operationen. Reihenfolge irrelevant — Colony berechnet den post_state nach Anwendung aller Operationen und validiert _diesen_, nicht Teil-Zustände. Damit darf eine `add_edges`-Edge auf eine Node zeigen, die im selben Diff per `add_nodes` neu kommt.

**`ctx`** liefert Werte für `${ctx.<key>}`-Substitutionen im Diff (siehe „Variablen-Substitution"). Wird beim Anwenden des Diffs aufgelöst, bevor die Validierung läuft.

### Mutation-Operationen

| Operation | Wirkung |
|---|---|
| `add_nodes` | Neue Cells im Scope instanziieren (Template-Referenz, optional `override_params`). `override_params` gilt nur für Single-Cell-Templates — auf Subtree-Templates wird es rejected (`schema`; R10-Ruling 2026-06-11): eine Sub-Cell-Adressierung existiert nicht, Sub-Cell-Parametrisierung läuft über `${ctx.*}`-Substitution im Template. |
| `remove_nodes` | Entfernt alle Edges, an denen die referenzierten Knoten beteiligt sind → Knoten werden disconnected und inaktiv markiert (inkl. Subtree-Kaskade bei Hives). Registry-Eintrag, Filesystem und `cell_id` bleiben (No-Delete, siehe „Konnektivität & Aktivität"). |
| `add_edges` | Neue Edges in Colony's Edge-Tabelle, scoped |
| `remove_edges` | Edges aus Edge-Tabelle entfernen, scoped |
| `swap_nodes` | **Graph-Swap**: schwenkt **alle externen Edges** einer Implementierung (`match`) atomar auf eine andere (`with`) um — die andere ist entweder frisch aus einem Template instanziiert **oder** eine bereits existierende Cell. Die alte Cell bleibt **disconnected erhalten** (No-Delete-Policy; jederzeit rückschwenkbar, indem die Edges zurückgeschwenkt werden). `swap_nodes` ist damit ein reiner Edge-/Topologie-Diff — **kein** `config.json`-Rewrite einer existierenden Cell, **keine** `cell.db`-Migration, **keine** `cell_id`-Übernahme (die neue Implementierung hat ihre eigene Identität) — und erbt das Atomaritätsmodell der Edge-Mutation. |

**Match-Pattern für `remove_*` und `swap_nodes`**: Pattern referenziert Nodes/Edges per Eigenschaften (`name`, `template`, für Edges `from`/`to`/`condition`/`modifier`), **nicht per UUID**. Pattern muss in der aktuellen Registry mindestens einen Treffer haben, sonst Mutation rejected. Namen sind pro Scope eindeutig (Naming-Kollisions-Reject in der Validierung) — eine UUID-Referenzierung als Disambiguierungs-Fallback ist **nicht** vorgesehen.

### Validierung

Einstufig in Colony. Vor der Anwendung wird der hypothetische post_state berechnet und gegen folgende Kriterien geprüft:

- **Schema**: Diff entspricht JSON-Schema (siehe `docs/config.md` für Details der einzelnen Operationen).
- **Match-Patterns**: jedes Pattern in `remove_*`/`swap_nodes` trifft ≥1 Element im pre_state.
- **Naming-Eindeutigkeit**: keine zwei Nodes haben denselben Namen innerhalb desselben Scopes nach Anwendung des Diffs.
- **Cycle-Freiheit**: post_state-Graph hat keine Zyklen über `from`/`to`-Edges (sofern die Anwendung Zyklen verbietet; meclaw-Core schlägt nicht generell auf Zyklen).
- **Edge-Schema-Compat**: alle Edges referenzieren existierende Nodes im post_state; `condition` parst als gültiges CEL; `modifier` (falls gesetzt) entspricht dem `{set?, delete?}`-Schema, und alle Expressions in `modifier.set.*` parsen als gültiges CEL. Edge-Endpoints lösen relativ zum Mutations-`scope` in **beliebiger Tiefe innerhalb des Scopes** auf (`./name`, `./unit/dispatch`) — gegen den post_state, Diff-neue Nodes inklusive (auch Subtree-Nodes auf Tiefe). Containment bleibt scharf: Endpoints, die außerhalb des Scopes auflösen (`../x`, absolute Pfade), sind `scope_out_of_bounds` (§ Geltungsbereich) — der Parent verdrahtet abwärts in seinen eigenen Subtree, nie hinaus. Ein Tiefen-Pfad auf einen nicht-existenten Node ist `edge_schema`.
- **Template-Existenz**: alle `add_nodes`/`swap_nodes` referenzieren Templates, die in Colony's Templates-Registry existieren.
- **`.env`-Variablen**: alle `${ENV_VAR}` in den `override_params` haben Werte in `.env`.

Bei Fehler **vor** der atomaren rename-Phase (Schema, Match, Cycle, Edge-Schema, Template, `.env`, Staging-Aufbau): gesamter Diff rejected, kein Teilcommit, Live-Tree unberührt. **Ab** der rename-Phase gilt das Audit-Modell: ein Fehler nach dem ersten erfolgten `rename(2)` ist KEIN sauberer Reject mehr (frühere Renames stehen bereits im Live-Tree) — das Substrat strict-failt laut (Panic), und der Halb-Zustand wird beim nächsten Boot als nicht-registrierte Orphan-Dirs sichtbar gemacht (§ Startup-Algorithmus), nie still adoptiert. Fehler-Message an `reply_to` (falls gesetzt) mit:

```json
{
  "error_code": "<code>",
  "details": "<human-readable>",
  "context": { ... }
}
```

`error_code` ist Enum: `schema` | `match_no_hit` | `naming_collision` | `cycle` | `edge_schema` | `template_missing` | `env_var_missing` | `unsupported_substitution` | `ctx_key_missing` | `scope_out_of_bounds` | `unknown_cell_type` | `stop_wiring_unavailable` | `term_timeout` | `resume_requires_stopped_cell` | `subtree_resume_unsupported` | `resume_type_mismatch` | `contract_incomplete`.

Diese Strings sind Teil des stabilen Mutations-API-Vertrags. Anmerkungen zu den Substrat-Codes:

- `ctx_key_missing` — eine `${ctx.<key>}`-Substitution im Diff referenziert einen Key, der im `ctx`-Block der Mutation fehlt (siehe § Variablen-Substitution → `${ctx.<key>}`). Emittiert von `resolve_ctx_token` (`mutation/substitute.rs`).
- `scope_out_of_bounds` — ein Top-Level-Diff-Pfad (`add_nodes[].name`, `*_edges[].from`/`.to`, `match.name`) löst außerhalb des Mutations-`scope` auf (siehe § Geltungsbereich). Scope-Containment-Check vor jeder FS/Registry-Mutation (pre-14-Audit B4); emittiert von `validate_scope_containment` (`mutation/validate.rs`).
- `unknown_cell_type` — `add_nodes`/`swap_nodes` referenziert einen Cell-Type ohne registrierte Factory.
- `stop_wiring_unavailable` — Disconnect/swap einer Cell, deren Stop-Wiring nach einem term_timeout-survivor nicht restaurierbar ist (F5-Guard — permanenter Backstop).
- `term_timeout` — death-ack-Timeout beim Disconnect/swap einer Awake-Cell → voller Rollback + Reject.
- `resume_requires_stopped_cell` — der Resume-Pfad verlangt eine gestoppte Cell.
- `subtree_resume_unsupported` — Subtree-Template am bereits belegten Wurzel-Pfad. Heute kein Producer — der frühere F4-Reject wurde durch Per-Node-Resume abgelöst (paket-5 T12, Commits `d280de4` validate-phase per-node subtree resume + `549422d` Producer-Entfernung); Enum-String bleibt reserviert.
- `resume_type_mismatch` — Resume (single-cell wie subtree) an einem belegten Pfad, dessen Bestands-`type` vom Template abweicht (F2-Ruling, Paket 5).
- `contract_incomplete` — ein zu ladendes `config.json` (Boot-Walk oder Mutations-Staging, non-hive) deklariert die Pflicht-Keys `contract.version`/`settings`/`consumes` nicht oder typ-falsch (`docs/config.md` § contract).

`uuid_provider_exhausted` ist **kein** lebender Code: die Enum-Variante `MutationError::UuidProviderExhausted` war toter Code (`Uuid::now_v7()` ist infallibel, nie konstruiert; zudem als String `"uuid_provider"` statt `"uuid_provider_exhausted"` gemappt) und **wurde mit Paket 7 entfernt** (D-034; verifiziert 2026-06-10: 0 Code-Treffer). Notiz bleibt als Re-Discovery-Schutz.

### Geltungsbereich

Eine Mutation deckt **einen** Scope ab (= ein Pfad-Präfix). Sub-Scopes (= geschachtelte Hive-Marker im Filesystem) werden über eigene Mutation-Messages an `/colony/mutations` mit ihrem eigenen Scope-Pfad mutiert. Eine einzelne Mutation kann nicht mehrere Scopes gleichzeitig adressieren — das hält Mutationen lokal und race-frei.

### Kein Concurrency-Schutz (CAS)

In der jetzigen Phase keine konkurrierenden Builder pro Scope erwartet — kein `expected_version` o.ä. Wenn das später relevant wird (Post-Roadmap), additiv nachrüstbar. Aktuell: Colony's sequentielle Mailbox-Verarbeitung serialisiert konkurrierende Mutations automatisch.

### Permissions

Bis zum Hardening: **keine Permission-Schicht**. Wer routing-technisch eine Mutation-Message an `/colony/mutations` zustellen kann, kann mutieren. Permission ist Topologie-Frage, nicht Identitäts-Check. Die `mutate-graph` Capability im Cell-Vertrag ist ein **Discovery-Hint** (für Builder-Composer und Audit-Tools), kein Runtime-Check. HTTP-API in Phase 12 ohne Auth — Schutz über das `--api <bind>`-Flag (Default: kein Port; opt-in via z.B. `--api 127.0.0.1:7777` für lokal-only).

---

## Architektur-Bausteine

| Begriff | Beschreibung |
|---|---|
| **Colony** | Gesamtsystem und einzige Authority. Hält die zentrale `HashMap<Path, ActorHandle>`-Registry, routet alle Messages, verwaltet Lifecycle, Templates, `config.json`. Hat Pfad `/colony`. Läuft als eigene Tokio-Task mit eigener Mailbox. |
| **Hive** | Verzeichnis mit `config.json` `type: "hive"`. **Scope-Marker** für Authority-Boundary und Mutations-Scope eines Pfad-Präfixes — kein eigener Aktor, keine Mailbox, keine eigene `cell.db`. Wirkt zusätzlich als **logischer Transit-Knoten** im Routing-Graph, von Colony ausgewertet. Hierarchie-Wirkung in der DSL bleibt; Implementierung ist flach. |
| **Cell-Type** | Verhaltens-Klassifikation einer adressierbaren Cell: `llm`, `bash`, `code`, `store`, `web_fetch`, `web_search`, `file`, `edit`, `proxy`, `timer`, `mcp`, `harness`, `subcolony`. Jede Cell-Type bringt eigenes `params`-Schema und Capability-Set. Cells mit einem dieser Werte werden von Colony als Aktoren in der `HashMap<Path, ActorHandle>`-Registry geführt. |
| **Cell** | Verzeichnis mit `config.json` eines bestimmten Cell-Types. Topologisch neutral — Rolle ergibt sich aus dem Ort (Template oder Instanz). |
| **Hive-Scope-Marker** | Verzeichnis mit `config.json` `type: "hive"`. **Kein Aktor**: keine Tokio-Task, keine Mailbox, keine `cell.db`, kein `ActorHandle`-Eintrag in der Cell-Registry. Trotzdem ein **Knotenpunkt im System**: Colony führt eine separate Hive-Scope-Tabelle (Pfad-Präfix, Authority-Boundary, Mutation-Scope, initialer `params.graph`). Beim Filesystem-Bootstrap wird der Hive-Marker erfasst; bei Mutationen wirkt er als Scope-Boundary. **Adressierbar als Transit-Ziel** — Colony leitet anhand der Hive-Out-Edges weiter, stellt nie zu (siehe „Hive-Pfade als Target — Transit-Auswertung" und `cell-types.md` Abschnitt `hive`). |
| **Template** | Eine Cell (oder Cell-Subtree inklusive Hive-Scope-Markern) im `templates/`-Ordner. Rolle: Klasse / Schablone. Wird beim Instanziieren kopiert. |
| **Instanz** | Eine Cell im Dateibaum (Pfad frei wählbar). Rolle: lebendiges Objekt mit Pfad, UUID, eigenem Tokio-Task, ggf. `cell.db`. In Colony's Cell-Registry unter ihrem Pfad eingetragen. |
| **Edge** | Verbindung zwischen Cell-Ausgang und nächstem Eingang. Trägt Bedingung + Modifikator. Lebt im Graph (Colony's Edge-Tabelle). Hat eine UUID v7 (Colony-vergeben). |
| **Graph** | Menge aller Nodes (= Cells in Registry) und Edges. Lebt vollständig in Colony's Registry (in-memory) und `colony.db` (persistiert). Initialer Stand aus dem Filesystem-Bootstrap + `params.graph`-Hints aus Hive-Scope-Markern. Dynamisch über Mutations veränderbar. |
| **Message** | Einheit der Kommunikation. Atomar, klein. Trägt Routing-Daten + Headers + Body-Referenz. |
| **Blob** | Großer Message-Body. Separat im `blobs/`-Verzeichnis, referenziert per UUID v7. |
| **Path** | Adresse einer Instanz. Linux-style: `/`, `.`, `..`. Plus `/colony` als virtueller Endpunkt. Pfad-Resolution ist eine pure String-Operation vor dem Registry-Lookup. |
| **Session** | Anwendungs-Konvention für eine logische Konversations-Klammer (typisch über `session_id`-Header propagiert). Kein Core-Konzept — meclaw-Core kennt keine Sessions, Anwendungen wählen ihre eigene Granularität. |
| **Seed** | JSONL-Datei pro Cell-DB, Schema in Zeile 1, Daten danach. Quelle für DB-Bootstrap. |

---

## Filesystem-Layout

```
{root}/
├── colony.json              # Colony-weite Verhaltens-Defaults (optional)
├── colony.db                # SQLite: Registry, Templates, Mutations-Log, zentrales Message-Log
├── log.jsonl                # Tracing-JSONL
├── .env                     # Secret-Substitution-Quelle
├── .staging/                # Atomic Mutation Staging (siehe unten)
│   └── <mutation_id>/
├── blobs/                   # Blob-Storage
│   └── <uuid7>.json
├── templates/               # Template-Bibliothek (Klassen)
│   └── <template_name>/
│       ├── template.json
│       ├── config.json
│       └── seed/
│           └── <table>.jsonl
└── <root-cell>/             # Wurzel-Cell (üblicherweise ein Hive-Scope-Marker), Pfad `/`
    ├── config.json
    ├── cell.db              # nur wenn Cell stateful ist
    ├── seed/                # optional
    │   └── <table>.jsonl
    └── <sub-cell>/          # weitere Cells im Subtree
        └── ...
```

**Keine vorgeschriebene `main/sessions/archived/`-Trennung.** Pfade werden vom Auslöser der Instanziierung (Builder, CLI, API) bewusst gewählt. Übliche Konventionen entstehen aus der Anwendungslogik, nicht aus dem Core.

**`.staging/`**: temporäres Verzeichnis für Mutationen, die zwischen Validierung und Commit stehen. Colony baut neue Cell-Verzeichnisse hier vollständig auf (mit substituierten `config.json`-Werten und ggf. `cell.db` aus Seed), dann ein einzelner `rename(2)` an den Ziel-Pfad — atomar pro Verzeichnis auf POSIX. Vorteile: zerbrochene Halb-Instanziierungen können nicht im Live-Tree liegen, Recovery beim Startup ist einfach (alles in `.staging/` ohne Commit-Marker → löschen). Verworfen wurde: direktes Schreiben an Ziel-Pfaden mit Backup-Files (löst nicht das Halb-Instanzen-Problem) und ein `.tombstones/`-Verzeichnis für gelöschte Cells (No-Delete-Policy macht das überflüssig).

**Jede Cell-Instanz** hat dieselbe Struktur: Verzeichnis mit `config.json` (Bootstrap-Snapshot), optional `cell.db` (Live-State), optional `seed/`. Sub-Cells als weitere Verzeichnisse darin, sofern der Cell-Type das zulässt (für `hive`-Scope-Marker üblich, für andere Cell-Types nicht).

---

## `colony.json` — Schema

Die Colony-weite Konfigurationsdatei im `{root}`. Enthält ausschließlich **Verhaltens-Defaults für Cells und Colony**, nicht Operations-Konfiguration (Pfade, Logging — die bleiben CLI-Flags, siehe „CLI"). Trennung konsistent mit der nginx-Stil-Philosophie: pro-Run-Operations gehen über Flags, Behavior-Defaults wohnen im File.

```json
{
  "schema_version": 1,

  "mailbox_default_capacity": 1000,
  "message_timeout_default_ms": 60000,
  "idle_timeout_default_ms":   60000,
  "message_default_ttl":          64,
  "restart_max_retries":           5,

  "blob_inline_max_bytes":         65536,
  "blob_max_recursion_depth":      64,

  "strict_validation":          false,

  "log_default_level":          "info"
}
```

**Schlüssel**:

| Schlüssel | Bedeutung |
|---|---|
| `schema_version` | Versions-Marker für Migrations-Verträglichkeit |
| `mailbox_default_capacity` | Default-Kapazität der **regulären Cell-Mailboxen** (bounded mpsc); pro Cell überschreibbar via `cell.mailbox_size`. **Schattet nur die reguläre Cell-Mailbox** (AMBIG-001-Ruling B, 2026-06-06) — die Dead-Letter-Queue- und Disconnect-Mailbox-Kapazitäten sind **feste Konstanten** und werden von diesem Feld **nicht** überschrieben. |
| `message_timeout_default_ms` | Default für den **Substrat-Backstop** pro `handle()`-Call (Konzept B, siehe „Timeouts"). Bei Überschreitung: Cell-Task gekillt, Supervisor restartet. **Nicht** der primäre I/O-Schutz — dafür ist `params.external_timeout_ms` (Konzept A) zuständig. Wert sollte deutlich großzügiger als die längsten erwarteten I/O-Operationen sein. Pro Cell überschreibbar via `cell.message_timeout`. |
| `idle_timeout_default_ms` | Default für die Idle-Dauer pro stateful Cell mit `cell.timeout: 0` — nach dieser Zeit ohne neue Message despawnt sich die Cell selbst (Awake→Asleep, siehe „Hot/Cold-Cell-Modell"). Pro Cell überschreibbar via `cell.idle_timeout_ms` im `config.json`. Wert greift erst ab Phase 13. |
| `message_default_ttl` | Default-TTL für Source-Messages (Schutz-Schranke gegen Routing-Schleifen). Colony dekrementiert pro Routing-Hop; bei `0` geht die Message **direkt** in die Dead-Letter-Queue (`ttl_expired`, direct-to-DLQ — kein Schritt-1-`reply_to`-Reply-Versuch wie bei der Routing-Fehler-Cascade; siehe „Routing-Algorithmus"). Builder können den Wert pro Initial-Message setzen. Empfehlung: 64. |
| `restart_max_retries` | Maximalanzahl der `one_for_one`-Restarts pro Cell, bevor `failed`-Status. **Dieses `colony.json`-Feld ist heute parsed-but-not-applied:** der wirksame Deckel kommt aus der Substrat-Konstante `DEFAULT_RESTART_LIMIT` (5), pro Cell überschreibbar via `config.json` `cell.restart_limit` — die `colony.json`-Verdrahtung dieses Feldes ist post-16. |
| `blob_inline_max_bytes` | Schwelle, ab der ein Body als Blob ausgelagert wird (kleinere Bodies bleiben inline in der Message) |
| `blob_max_recursion_depth` | Hartes Limit für rekursive Blob-Referenz-Auflösung (siehe „Blob-Storage"). **Das `colony.json`-Override dieses Feldes ist heute parsed-but-not-applied** — die rekursive Blob-Auflösung selbst ist Roadmap-Defer D-025 (0 Producer, § Routing-Fehler `blob_recursion_too_deep`); Tiefen-Limit **und** seine `colony.json`-Verdrahtung entstehen mit D-025. |
| `strict_validation` | Release-Build-Default: ob JSON-Schema-Validierung gegen `emits`/`consumes` aktiv ist (Debug-Build: immer `true`) |
| `log_default_level` | Tracing-Default-Level. **Dieses `colony.json`-Feld ist heute parsed-but-not-applied:** der wirksame Default kommt aus dem `--log-level`-Flag bzw. `info` — `colony.json` speist den Log-Level (noch) nicht; die Verdrahtung ist post-16. |

Cells können einzelne Werte über ihre `config.json`-`params` oder ihre `contract.settings` überschreiben — dann gilt der lokale Wert.

**Was bewusst nicht in `colony.json`**: Pfade (`--templates`, `--blobs`, `--env`, `--log`) und Logging-Konfiguration (`--log-level`, `--log-filter`) bleiben CLI-Flags. Begründung: pro-Run-Operations (z.B. Test-Roots, alternative Blob-Pfade, Debug-Logging-Sessions) sollen nicht in die Colony-eigene Konfiguration eindringen. Verworfen wurden: `colony.json` als Pflicht-File (Friction ohne Mehrwert für Quick-Start), Spiegelung aller CLI-Flags auch in `colony.json` (doppelte Konfigurations-Quellen mit Merge- und Konflikt-Auflösungs-Bedarf), Per-Scope-Konfiguration im `colony.json` (das File ist Colony-weit; pro Scope können wir Post-Roadmap nachrüsten, wenn echter Bedarf besteht).

---

## `/colony` als virtueller Endpunkt

`/colony/*` sind nicht im Filesystem-Tree existierende Pfade, sondern **virtuelle Endpunkte**, die Colony selbst behandelt. Sie sind in Colony's Routing-Algorithmus eingebaut: jeder Pfad, der mit `/colony/` beginnt, wird von Colony als interne Operation gelesen, nicht als Registry-Lookup.

**Symmetrie interne API ↔ externe API**: jeder `/colony/<endpoint>` ist gleichzeitig **Message-Target** für interne Sender (Cells, Builder, Routing) und **HTTP-Route** für die externe API (Phase 12, axum-Schicht). Die HTTP-Schicht ist eine **dünne Übersetzungs-Schicht**, die einen HTTP-Request in eine `Message` mit `target = "/colony/<endpoint>"` umwandelt und durch denselben Routing-Pfad schickt (intern: Übersetzung in die typisierte `ColonyMsg::{Mutation, Read*, …}`-Inbox-Variante mit oneshot-ack-Reply; **Symmetrie = gleiche Colony-Task-Sequenz + gleiches UBF-Datenmodell, nicht literal `route()`**; Cell→`/colony/*`-Routing seit W2b implementiert — der outputs-Arm dispatcht `/colony/*`-Targets direkt, siehe § Routing-Fehler „Outputs-Arm — drei disjunkte Fälle"). Damit ist die HTTP-API kein eigenes Sub-System mit eigenen Endpunkten, sondern eine zweite Schreib-/Leseart für die existierenden internen Endpunkte. OpenAPI-Spec (via `utoipa`) und interne Routing-Tabelle teilen sich die Definition.

| Pfad | Zweck | Filter / Query-Parameter | Schreibend? | Phase |
|---|---|---|---|---|
| `/colony/dead_letters` | Dead-Letter-Queue: unauflösbare Routen, abgelaufene TTLs, Routing-Fehler | `?since=<ts>`, `?limit=<N>`, `?error_code=<code>` | beides (Read + Drain) | 2 |
| `/colony/registry` | Cell-Registry lesen (Liste aller registrierten Cells mit Pfaden, IDs, Typen, Status). `?path=` für eine einzelne Cell. Inkl. inaktiver Knoten mit `active`-Feld. | `?path_prefix=<path>`, `?type=<celltype>`, `?path=<exact>`, `?active=true\|false` | nein | 4 |
| `/colony/templates` | Templates-Registry lesen (für Builder-Discovery) | `?type=<celltype>`, `?name=<name>` | nein | 5 |
| `/colony/templates/rescan` | Trigger zum Neu-Einlesen des Templates-Verzeichnis (Ersatz für `--rescan-templates`-Restart) | — | ja | 5 |
| `/colony/mutations` | Mutation-Pipeline; Builder schicken Mutation-Diffs hierhin | — (Diff im Body) | ja | 6 |
| `/colony/graph` | Topologie eines Scopes lesen (Nodes + Edges, runtime-projiziert) | `?scope=<path>` (default Root) | nein | 6 |
| `/colony/trace` | Message-Log lesen, als Baum nach `parent_message_id` aufgebaut wenn `trace_id` gesetzt | `?trace_id=<uuid>`, `?path_prefix=<path>`, `?correlation_id=<uuid>` *(heute inert — `correlation_id` wird nicht originär gesetzt, s. § Envelope-Setter-Authority)*, `?error=true`, `?since=<ts>`, `?limit=<N>` | nein | 11 |
| `/colony/messages` | Message-Log browsen: Liste newest-first mit Filtern + Einzel-Message | `?id=<uuid>`, `?trace_id=<uuid>`, `?parent_message_id=<uuid>`, `?correlation_id=<uuid>`, `?to_path_prefix=<path>`, `?from_path_prefix=<path>`, `?body_kind=inline\|blob`, `?since=<ts>`, `?until=<ts>`, `?before_created_at=<ts>&before_id=<uuid>` (Keyset-Cursor), `?limit=<N>`, `?scan_budget=<N>`, `?resolve_blob=true` | nein | P1 |
| `/colony/events` | Subscribe auf Live-Event-Stream (Routing-Entscheidungen, Mutations-Commits, Restarts, Dead-Letters) | — (subscription-style) | nein | 14 |

`/colony` selbst (ohne Sub-Pfad) ist nicht adressierbar — Anfragen dort hin → Fehler. `/colony/cell` als eigener Endpunkt existiert nicht; einzelne Cells werden über `/colony/registry?path=<path>` gelesen.

**Reply-Body-Form (Reads):** Colony antwortet im Universal-Body-Format mit einem Top-Level-Slot, benannt nach dem Endpunkt: `registry`, `dead_letters`, `templates`, `trace`, `messages`, `mutations` (Audit-Read), `rescan` (Outcome). Analog zum `graph`-Slot (siehe „Visibility").

**Request-Body-Form (Cell-Emissionen / EDA):** Eine Cell, die an einen `/colony/*`-Endpunkt emittiert, trägt den Endpoint-spezifischen Aufruf als Top-Level-Slot im UBF-Body:

- **`/colony/mutations`** — Top-Level `{ scope, diff, ctx }`: der Mutation-Diff plus Geltungs-`scope` plus optionaler `ctx`-Substitutions-Kontext (kanonische Form siehe § „Mutation-Format (Builder → Colony)"). Einziger schreibbare EDA-Endpunkt.
- **`/colony/registry`, `/colony/templates`, `/colony/graph`, `/colony/trace` (Reads)** — Top-Level `{ query: { … } }`: ein `query`-Objekt, dessen Felder den HTTP-Query-Parametern des Endpunkts entsprechen (`registry`: `path`/`path_prefix`/`cell_type`/`active`/`limit`; `templates`: `cell_type`/`name`/`limit`; `trace`: `trace_id`/`path_prefix`/`correlation_id`/`only_error`/`since`/`limit`; `graph`: `scope`). Fehlt `query` oder ein einzelnes Feld, greifen die Defaults (`limit` Default 100, Hard-Cap 1000). Die Read-Reply geht an den Sender-Pfad (Reply-Body-Form oben).
- **`/colony/dead_letters`** — **nicht** EDA-dispatchbar und **nicht** body-operation-gesteuert: Read vs. Drain entscheidet die HTTP-Methode (`GET` = Read / `DELETE` = Drain) bzw. die dedizierte `ColonyMsg::ReadDeadLetters`/`DrainDeadLetters`-Inbox-Variante — **kein** `body.operation`-Feld (Stand W2d/W6d; das frühere `body.operation == "drain"`-Modell ist überholt). Eine Cell-Emission an `/colony/dead_letters` wird hart abgewiesen (siehe Endpunkt-Klassifikation unten).

**Endpunkt-Klassifikation für Cell-Emissionen (EDA, W2d-Ruling the spec owner 2026-06-12):** der outputs-Arm dispatcht ein `/colony/*`-Emissions-Target direkt (§ Routing-Fehler „Outputs-Arm — drei disjunkte Fälle", Fall 1), aber nicht jeder Endpunkt ist von einer Cell aus erreichbar:

- **`/colony/mutations` — EDA-schreibbar.** Eine cell-emittierte Mutation wird ausgeführt (13.5-A6). Einziger schreibbarer Dispatch-Endpunkt.
- **`/colony/registry`, `/colony/templates`, `/colony/graph`, `/colony/trace` — read-only, EDA-lesbar.** Eine Cell darf sie per Emission *lesen* (Reply an ihren Pfad); sie sind nicht beschreibbar.
- **`/colony/dead_letters` — read-only, NICHT EDA-dispatchbar.** Die DLQ wird ausschließlich über die dedizierten Inbox-Varianten `ColonyMsg::ReadDeadLetters`/`DrainDeadLetters` (HTTP `GET`/`DELETE`) gelesen/gedrained, **nie** über einen Routing-Dispatch. Eine Emission an `/colony/dead_letters` ist daher immer ein illegitimer Write an einen READ-Endpunkt: sie wird **hart abgewiesen** (ein `colony_endpoint_unimplemented`-DLQ-Eintrag, Sender-Pass-through, terminal), **niemals** als Read-Reply re-injiziert. Das verhindert den Source-Loop, den der pre-W2d-Hartkodier-Fallback `unwrap_or("/colony/dead_letters")` an den atomar-emittierenden Cell-Typen auslöste (DLQ-Listing-Reply zurück an die emittierende Cell → Re-Emission, ttl-ungedeckelt).
- **`/colony/messages` — read-only, NICHT EDA-dispatchbar (P1-Ruling 2026-08-07, analog `dead_letters`).** Operator-Lesefläche über das Message-Log, ausschließlich über die dedizierte Inbox-Variante `ColonyMsg::ReadMessages` (HTTP `GET`) erreichbar, **nie** über einen Routing-Dispatch. Eine Cell-Emission an `/colony/messages` wird wie bei `dead_letters` hart abgewiesen. Falls Topologien Message-Queries brauchen, ist das ein eigener Design-Pass über den `store`, nicht über Colony-Endpoints.
- **Unbekannter `/colony/<x>`** ⇒ `colony_endpoint_unimplemented` (wie gehabt).

**`?limit=<N>`-Defaults** (für `dead_letters`, `trace`, `messages`, `mutations`-Audit-Read): Default **100**, Hard-Cap **1000**, **kein Config-Knopf** (keine spekulative Spec-Fläche). Cap bremst zugleich den Routing-Loop-Stall DB-schwerer Reads. **`?scan_budget=<N>` (nur `messages`):** Obergrenze der in Stufe 1 gelesenen Rows der Zwei-Stufen-Query (indizierte Prädikate zuerst, residuale Filter `from_path_prefix`/`body_kind`/`correlation_id` danach) — Default **5000**, Hard-Cap **50000**; ein ausgeschöpftes Budget wird in der Antwort als `scan_truncated` ausgewiesen (Ergebnis ggf. unvollständig, nie stillschweigend).

---

## CLI

```
meclaw [options]
```

### Modi

Default-Modus ist **Direct-Mode**: stdin/stdout-Bridge an die Wurzel-Cell, alles auf einer einzigen `meclaw`-Invocation. Für interaktive Sessions, einfache Pipes, Tests. Der Prozess ist **stdin-getrieben** — schließt stdin (EOF, z.B. das Ende einer Pipe), drainet die in-flight-Arbeit und beendet sich mit Exit-Code 0 (Unix-Pipe-Semantik, `cat input.jsonl | meclaw` terminiert wie `grep`). Ein Shutdown-Signal (SIGINT/SIGTERM) wirkt zusätzlich.

`--daemon` (ab Phase 12): **entkoppelt den Prozess-Lifecycle von stdin** — stdin-EOF beendet den Prozess **nicht** mehr; einzige Shutdown-Trigger sind SIGINT/SIGTERM (und der interne Watchdog). Das ist die Bedeutung von „Daemon": ein langlaufender Prozess, der nicht an seiner Eingabe-Pipe hängt. Die stdin/stdout-Bridge wird dabei **nicht abgeschaltet** — der Mechanismus bleibt erhalten, läuft im Daemon-Betrieb aber typisch ins Leere (systemd-`Type=simple` liefert stdin als `/dev/null` → sofortiges EOF ohne Eingabe, stdout ins Journal), analog zu nginx, dessen stdio im Daemon-Modus existiert, aber ungenutzt bleibt. meclaw daemonisiert **nicht** selbst (kein `fork`/`setsid`) — das ist systemd-`Type=simple`-konform; Backgrounding ist Sache der Außenwelt (systemd/nohup). Externe Steuerung läuft via HTTP-API + Web-UI, beide opt-in via `--api`.

`--api <bind>` (ab Phase 12): aktiviert die HTTP-API und das Operator-Web-UI auf der angegebenen Bind-Adresse, z.B. `--api 127.0.0.1:7777` (lokal-only) oder `--api 0.0.0.0:7777` (alle Interfaces). **Ohne `--api` wird kein Port geöffnet** — Default ist API/UI aus. Die HTTP-API ist eine dünne Übersetzungs-Schicht über die `/colony/*`-Endpunkte (siehe „/colony als virtueller Endpunkt"): jeder HTTP-Request wird zu einer `Message` mit `target = "/colony/<endpoint>"` und durch denselben Routing-Pfad geschickt. Web-UI sitzt auf demselben Bind-Port unter `/ui/*` (siehe „Web-UI" unten). `--api` ist unabhängig von `--daemon` setzbar (Direct-Mode + parallele API erlaubt).

`--validate` (ab Phase 12): Dry-Run — Filesystem-Bootstrap, Schema-Checks, Template-Auflösung, Mutations-Replay aus `colony.db`, aber keine Cell-Spawns, kein HTTP-Listen. Exit-Code 0 wenn alles konsistent, sonst Fehler-Liste auf stderr.

`--rescan-templates`: rebuildet die Templates-Registry aus dem Filesystem. Default: Templates werden beim ersten Startup gescannt und in `colony.db` persistiert. Wenn du `templates/` manuell editiert hast (Add/Remove), `--rescan-templates` einmal laufen lassen.

### Flags

**Flags werden phasenweise eingeführt.** `clap` kennt zu jedem Zeitpunkt nur die Flags der bereits abgeschlossenen Phasen — unbekannte Flags werden mit Unknown-Flag-Error rejected. `meclaw --help` zeigt damit den jeweils funktionalen CLI-Surface ohne Misleading-„nimmt-an-tut-nichts"-Flags. Die Phase-Spalte unten gibt an, in welcher Phase ein Flag erstmals in `clap` deklariert und funktional wird.

| Flag | Phase | Default | Bedeutung |
|---|---|---|---|
| `--root <path>` | 0 | `.` | Filesystem-Wurzel der Colony |
| `--log <path>` | 0 | `<root>/log.jsonl` | Tracing-JSONL-Pfad |
| `--log-level <level>` | 0 | `info` (`colony.json log_default_level` wird heute nicht herangezogen) | Tracing-Level |
| `--log-filter <filter>` | 0 | none | `RUST_LOG`-Style Filter |
| `--version` | 0 | — | Version-Info |
| `--help` | 0 | — | Hilfe |
| `--env <path>` | 6 | `<root>/.env` | `.env`-Datei für Variablen-Substitution |
| `--templates <path>` | 11 | `<root>/templates` | Templates-Verzeichnis |
| `--rescan-templates` | 11 | off | Templates-Registry neu aufbauen |
| `--blobs <path>` | 12 | `<root>/blobs` | Blob-Storage-Verzeichnis |
| `--daemon` | 12 | off | Lifecycle von stdin entkoppelt — Shutdown nur per Signal/Watchdog, stdin-EOF beendet nicht (Bridge-Mechanismus bleibt) |
| `--api <bind>` | 12 | off (kein Port) | HTTP-API + Web-UI auf Bind-Adresse; z.B. `127.0.0.1:7777` oder `0.0.0.0:7777` |
| `--validate` | 12 | off | Dry-Run |
| `--stdio-format <text\|json>` | P9 | `text` | Format der Stdin/Stdout-Bridge: `text` = rohes Zeilenformat (Default, unverändert), `json` = Wire-v1-JSONL (Envelope-Durchgriff für `trace_id`/`ttl`/`context`, `ready`-Handshake) |

Bewusst **keine eigenen Subcommands** (`meclaw start`, `meclaw mutate`, etc.). nginx-Stil: ein Binary, viele Flags, ein Mode-Switch (`--daemon`, `--validate`). Operations sind Sache der Außenwelt (systemd, ein Wrapper-Script, ein Builder-LLM).

**Info-only Flags sind side-effect-frei**: `--version` und `--help` geben ihre Information auf stdout aus und exiten mit 0, ohne den Tracing-Subscriber zu initialisieren, ohne Filesystem-Writes (insbesondere kein `log.jsonl`-Anlegen), ohne Subprozess-Spawn. Sie greifen vor dem Subscriber-Setup. Tests für den Subscriber-Setup-Pfad geschehen über direkte Unit-Tests der Setup-Funktion, nicht über CLI-Subprozess-Aufrufe.

### Web-UI (Operator-Inspektion)

`--api <bind>` aktiviert neben der JSON-API auch ein **Operator-Web-UI** auf demselben Port, unter dem Pfad-Präfix `/ui/*`. Root `/` redirected auf `/ui/`. Das Web-UI ist:

- **Server-rendered HTML** via `maud` (siehe Tech-Stack). Kein JavaScript, keine Auto-Refresh, kein CSS-Framework — Browser-`F5` ist der Refresh-Knopf.
- **Read-only**. Keine Mutate-Forms — Mutationen sind Builder-Hive-Sache (siehe „Dynamik / Builder-Pattern"), die wiederum die JSON-API oder interne Routes nutzen.
- **Symmetrisch zu den `/colony/*`-Endpunkten**: dieselben Daten, anderes Rendering. Eine Web-UI-Route ruft intern denselben Read-Endpunkt wie eine API-Route.

| Web-UI-Pfad | Inhalt | Datenquelle |
|---|---|---|
| `/ui/` | Dashboard: Cells-Übersicht, Status-Counts, letzte Errors, letzte Dead-Letters — **kein konsistenter Snapshot (drei unabhängige Reads aus drei Momenten)** | aggregiert aus `/colony/registry` + `/colony/dead_letters` + `/colony/trace?error=true` |
| `/ui/registry` | Tabelle aller Cells mit Filter-Form (Pfad-Präfix, Type) | `/colony/registry` |
| `/ui/graph` | Topologie eines Scopes (Nodes als Liste, Edges als Tabelle), Form: `?scope=` | `/colony/graph` |
| `/ui/dead_letters` | Liste der jüngsten Dead-Letters mit `error_code`, Pfad, Body-Preview, „Original"-Link zur Ursprungs-Message im Message-Browser (sofern im `message_log` vorhanden) | `/colony/dead_letters` |
| `/ui/trace` | Trace-Such-Form (`trace_id`, `path_prefix`, `error`, `since`, `limit`), Ergebnis als Baum-HTML nach `parent_message_id` | `/colony/trace` |
| `/ui/templates` | Template-Übersicht mit Filter `?type=` | `/colony/templates` |
| `/ui/messages` | Message-Liste newest-first mit Filter-Form + Keyset-Paging, Payload trunkiert, Scan-Budget-Ausweis | `/colony/messages` |
| `/ui/message` | Einzel-Message: Headers `hop`/`context` getrennt gerendert, Payload pretty, Blob on demand, Pivots (Trace, Parent-Kette, Correlation, `reply_to`, Dead-Letters) | `/colony/messages?id=` |

Auth (sobald Phase-12-Hardening): einheitliche Middleware vor `axum`'s Router — gilt für JSON-API und Web-UI gleich. Bis dahin: lokale Disziplin (`--api 127.0.0.1:7777` als sicherer Default).

### Stdin/Stdout-Bridge (Direct-Mode)

Im Default-Modus läuft eine Stdin/Stdout-Bridge: stdin wird in Messages an die Wurzel-Cell konvertiert (eine Zeile = eine Message), stdout zeigt Messages, die aus der Wurzel-Cell emittiert werden. **Default-Format ist Text** (grep-/Unix-konform, strukturell identisch zum `proxy`): eine stdin-Zeile roher Text → ein UBF-Body mit genau einem `user`-Turn (`{messages:[{origin:"user",type:"text",text:"<zeile>"}]}`) plus frischer `turn_id`; auf stdout wird der `text` des letzten `assistant`-Turns einer emittierten Message geschrieben (analog zum `proxy`-Inbound). Dieses Textformat ist byte-identisch zu v0.1.0 und **bleibt der Default**. Seit P9 (0.1.9) gibt es zusätzlich das opt-in Flag `--stdio-format json` = **Wire v1**: eine JSON-Zeile pro Message, beim Start ein `ready`-Frame mit dem Protokoll-Integer `v` (strikt assertiert) und der berichteten Release-`version`, danach `message`- und `error`-Frames in beide Richtungen — mit Envelope-Durchgriff für `trace_id` (getragen), `ttl` (dekrementiert) und `context` (explizit gemappt), vgl. HTTP-`POST /messages`.

**Disziplin**: der Bridge ist ein I/O-Detail des `meclaw-cli`-Crates, **kein Cell-Type**. Die Wurzel-Cell bleibt austauschbar (Hive-Scope, LLM-Cell, Builder, …), ohne dass Bridge-Code anzupassen wäre. Verworfen wurden: ein eigener `stdio`-Cell-Type (würde die „Cells kennen keine Topologie"-Disziplin brechen, weil die Cell implizit wüsste, an einem stdin-Endpunkt zu hängen — und außerdem ein unnötiger Eintrag im Cell-Type-Katalog) sowie „interaktive Nutzung erst per `proxy` oder HTTP-API" (widerspricht der Selbstbeschreibung „bringt LLM in die Unix-Shell").

**Topologische Form (Ingress/Egress wie eine `proxy`-Cell)**: die Bridge spielt den (im Substrat nicht existierenden) Parent-Hive der Wurzel — sie schiebt Nachrichten zwischen der stdio-Ebene und der `/`-Ebene, genau wie ein Hive zwischen seinen Ebenen, nur mit JSON↔Message-Übersetzung. Die Wurzel-Cell **muss** daher ein Hive sein (`type: "hive"`): nur der Hive trägt den Graphen, und der Graph ist der Aufbaupunkt der Colony. **Ingress** (stdin → Topologie): die Bridge ist der Geburtspunkt der Nachricht und etabliert die initiale `context` direkt (eine sanktionierte Eingangs-Edge, symmetrisch zum HTTP-Ingress — siehe § Metadata-Aggregation), mit derselben context-Trias wie der `proxy`: eine feste well-known `user_id` (der stdio-User, über alle Läufe identisch), eine `chat_id` pro Prozess-Lauf (Start bis EOF = eine stdio-Session, analog zur `proxy`-`chat_id`) und eine frische `turn_id` pro stdin-Zeile; emittiert mit `sender = @external` an die Wurzel-Cell. **Egress** (Topologie → stdout): eine Message, die zum Wurzel-Hive `/` zurückläuft und dort keine weiterführende Out-Edge matcht, wird nach stdout übersetzt statt als `HiveNoRoute` dead-lettered — das ist „eine Ebene nach oben = nach außen", beim Wurzel-Hive ist diese Ebene stdio. stdio ist damit ein **absoluter Endpunkt** (pure Sink, wie das `proxy`-Inbound-Verhalten); spätere Kopplung mehrerer Colonies über Pipes ist ein Outlook, kein v0.1.0-Scope. Im JSON-Modus (`--stdio-format json`) ist stdio zusätzlich die **Kompositions-Grenze für Sub-Colonies**: eine Eltern-Colony betreibt eine ganze Kind-Colony als **eine** Cell (`cell-types.md` § `subcolony`). Das ist ausdrücklich **Komposition, nicht Föderation** — der Kind-Baum bleibt von außen unadressierbar: kein Pfad-Durchgriff auf Kind-Cells, keine Eltern-Mutation am Kind-Baum.

**Lifecycle**: im Direct-Mode (weder `--daemon` noch `--api`) ist stdin-EOF ein Shutdown-Trigger (drain + Exit 0, siehe § CLI § Modi). `--api` ohne `--daemon` lässt die Bridge weiterlaufen (Direct-Mode + parallele API), EOF beendet ebenfalls. `--daemon` entkoppelt den Lifecycle von stdin: die Bridge bleibt als Mechanismus erhalten, EOF beendet nicht, Shutdown nur per Signal/Watchdog.

---

## Pfad-Adressierung

```
/<hive>/<sub_hive>/<cell>    absolut, ab Colony-Wurzel
/memory/2026-05-16/cache     Beispiel mit anwendungsspezifischer Hierarchie
/colony/registry             virtueller Colony-Endpunkt
./cell_y                     relativ zum Sender-Pfad
../other_sub/cell_z          relativ zum Parent-Verzeichnis des Senders
```

- `{root}` = Filesystem-Startpunkt von meclaw, in Pfad-Notation `/`.
- Pfad-Auflösung ist eine **pure String-Operation** auf dem Sender-Pfad und dem Target-Ausdruck. `.`, `..`, `/`-Prefix werden normalisiert zu einem absoluten Pfad.
- Lookup ist O(1) auf Colony's zentraler `HashMap<Path, ActorHandle>`. Kein Hop-by-Hop, keine Cascade über mehrere Routing-Tabellen.
- Pfade sind ewig stabil (siehe „No-Delete-Policy").

### Routing-Algorithmus (zentral in Colony)

```rust
async fn route(&self, sender_path: &Path, msg: Message) {
    // 1. Pfad-Resolution: target gegen sender_path normalisieren
    let target = resolve_path(sender_path, &msg.target);
    
    // 2. Escape-Hatch für Colony-Pfade
    if target.starts_with("/colony") {
        self.handle_colony_target(target, msg).await;
        return;
    }
    
    // 3. Registry-Lookup
    match self.registry.get(&target) {
        Some(handle) => {
            // Log in zentralem Message-Log (filterbar nach Pfad-Präfix)
            self.log_message(&sender_path, &target, &msg).await;
            let _ = handle.send(msg).await;
        }
        None => {
            // Dead-Letter-Cascade
            self.handle_unresolved(target, msg).await;
        }
    }
}
```

**Pfad-Resolution-Beispiele**:

| Sender-Pfad | `msg.target` | Aufgelöst zu |
|---|---|---|
| `/main/agent` | `./tool` | `/main/agent/tool` |
| `/main/agent` | `../collector` | `/main/collector` |
| `/main/agent` | `/other/cell` | `/other/cell` |
| `/main/agent` | `/colony/templates` | `/colony/templates` |

**Hive-Pfade als Target — Transit-Auswertung**: ein `target`, das auf einen Hive-Scope-Marker zeigt, ist **adressierbar**. Der Hive selbst hat keinen Aktor und damit keine Mailbox — Colony stellt nie zu. Stattdessen wertet sie den Hive als **logischen Transit-Knoten** in derselben Routing-Schicht aus, die auch Cell-Targets bedient: sie nimmt die Out-Edges des Hives (`EdgeTable`-Einträge mit `from = <hive-path>`), prüft ihre CEL-`condition` gegen die Headers (CEL-Standard-Semantik wie überall, siehe „Edge-Modell"), wendet `modifier` an und löst pro Treffer einen **regulären Routing-Hop** auf den jeweiligen `to`-Pfad aus. Die TTL der Message wird **pro Hop** dekrementiert (ein Hive-Transit-Hop zählt genauso wie ein Cell-zu-Cell-Hop). Aus Sender-Sicht ist der Hive damit ein adressierbares Ziel, im Substrat ein Transit-Hop in der einen Routing-Schicht — kein Bypass, keine separate Hive-Routing-Logik.

**Sonderfall — keine Out-Edge matcht** (Edge-Liste leer oder alle CEL-Conditions evaluieren zu `false`): die Message geht an `/colony/dead_letters` mit dem **eigenen `error_code` `hive_no_route`** (kanonischer String, neuer `DeadLetterReason::HiveNoRoute`). Bewusst nicht als `unresolved_path`: der Hive war erreichbar, der Routing-Graph hat ihn aber nicht weitergeleitet — diese Unterscheidung ist Builder-Observability (Hive-Sackgasse vs. Tippfehler-Pfad), kein interner Implementierungs-Detail.

Mutations für einen Hive-Scope gehen weiterhin an `/colony/mutations` mit dem Hive-Pfad als Scope-Feld im Mutation-Body, nicht an den Hive-Pfad als `target`.

**Invariante: `route()` ist rein.** Sämtliches Logging, Sync, Metrik-Auswertung lebt in einem **Wrapper an der Call-Site** (siehe `route_with_log` in [`crates/meclaw-colony/src/colony.rs`](../crates/meclaw-colony/src/colony.rs)) — der Pre-Check + Snapshot vor dem `route()`-Call macht und Log-Send nach dem Return. NICHT in `route()` selbst. *(Baseline-Hinweis: Die ursprüngliche „Body byte-identisch zum **phase-4-done**-Stand"-Formulierung ist seit der **Hive-Transit-Re-Baseline (2026-06-04)** überholt — `route()` trägt seither den `hive_scopes`-Param, eine `RouteAction`-Rückgabe und den HiveTransit-Branch. Das Korridor-Gate läuft gegen den eingefrorenen Fixture `plans/phase-13.5-hive-transit-fixtures/expected_route_body.txt`, nicht mehr gegen `phase-4-done`.)*

**T33-Near-Miss (Phase 5)**: ein Subagent erweiterte unter Borrow-Druck (`&Connection` nicht `Send` über async-Punkte) die `route()`-Signatur um einen `log_tx`-Parameter + einen Log-Send-Block im Body. Im Review gefangen, in Commit `1736e8a` zurückgebaut. Lesson: Subagents tendieren dazu, route() „minimal-invasiv zu erweitern" — Execute-Prompts müssen die Pure-Invariante **explizit pro Task wiederholen**.

**Body-Verify-Commando** (pro `route()`-relevantem Commit auszuführen) — Vergleich gegen den eingefrorenen Hive-Transit-Fixture:

```bash
diff <(git show HEAD:crates/meclaw-colony/src/colony.rs | sed -n '/^async fn route(/,/^}$/p') \
     plans/phase-13.5-hive-transit-fixtures/expected_route_body.txt
```

Leer = Body byte-identisch zum sanktionierten Hive-Transit-Stand. Das `#[rustfmt::skip]`-Attribut über `route()` hält die committete Form gegen dieses zeichenweise Gate eingefroren (`cargo fmt` darf frei über den Workspace laufen). Bei verschachtelten Closures mit `^}$`-Linien, die das `sed`-Pattern zu früh schließen könnten: stattdessen den vollen `git diff <hive-transit-baseline-tag> HEAD -- crates/meclaw-colony/src/colony.rs` prüfen und bestätigen, dass KEIN Hunk in die `fn route`-Definition fällt (`route_with_log`-Hunks sind erlaubt; `route()` proper nicht).

### Pfad-Resolution: Edge-Cases

`Path::resolve(sender, target)` gibt **immer** einen `Path` zurück, **nie** ein `Result`. Begründung: Resolution ist eine pure String-Normalisierung (siehe oben) — sie kann nicht „fehlschlagen". Ob der resultierende Pfad existiert, ist eine separate Frage, die der nachgelagerte Registry-Lookup beantwortet. Damit hat `route()` genau **eine** Fehlerquelle (unbekannter Pfad → Cascade), nicht zwei (Resolution-Fehler vs. Lookup-Miss). Die folgenden Eingaben sind dadurch eindeutig definiert:

| Eingabe | Verhalten | Begründung |
|---|---|---|
| `../` über die Wurzel hinaus (z.B. Sender `/a`, Target `../../x`) | **Clamp auf `/`** (ergibt `/x`) | Linux-Konvention (`cd / && cd ..` bleibt `/`). Kein Fehlerpfad nötig. |
| Leerer Target `""` | → `sender_path` (identisch zu `.`) | Leerer String = „kein Hop". |
| Bare-Name ohne Prefix (z.B. `cell`, kein `/`, `./`, `../`) | → relativ zum Sender (wie `./cell`) | Natürlichste Interpretation für Prefix-losen String; Shell-analog. Bei Nicht-Existenz landet er via regulärem Registry-Miss in `dead_letters` (reason `UnresolvedPath`) — kein Sonder-Fehler nötig. |
| Trailing-Slash (`/a/b/`) | → normalisiert (`/a/b`) | Konsistente Key-Form. |

### Verhalten bei Routing-Fehlern (Cascade)

Wenn ein aufgelöster Pfad nicht in Colony's Registry existiert (Cell entfernt durch Mutation, Subtree nie instanziiert, Tippfehler):

1. Falls `reply_to` gesetzt: Fehler-Message zurück an `reply_to`.
2. Falls `reply_to == None`: an `/colony/dead_letters` schicken.
3. Colony loggt mit `trace_id`, aufgelösten Pfad, Original-Target, Grund.

**Phasen-Abgrenzung (Phase 2 vs. ab Phase 3)**: Schritt 1 (`reply_to`-Reply) und der `trace_id`-Teil von Schritt 3 setzen den UBF-Header voraus, der erst **Phase 3** entsteht. In **Phase 2** ist die Message trivial (`{ target, payload }`, kein `reply_to`, kein `trace_id`). Die Phase-2-Cascade ist daher **reduziert**: ein unauflösbarer Pfad geht **immer direkt** an `/colony/dead_letters` (Schritt 2), mit Logging von aufgelöstem Pfad, Original-Target und Grund — die `reply_to`-Verzweigung und das `trace_id`-Logging werden in Phase 3 nachgezogen, wenn die Header-Felder existieren. Die `reply_to`-Verzweigung in Phase 2 vorzubauen ist Phasen-Vorgriff.

**Dead-Letter-Queue-Eigenschaften**: `/colony/dead_letters` ist ein Colony-internes Konstrukt, **kein** Eintrag in `HashMap<Path, ActorHandle>` — `/colony/*`-Pfade werden im Routing als virtuelle Endpunkte abgefangen, vor dem Registry-Lookup. **Persistenz (Phase-16 W6d / Audit A6):** die DLQ ist **persistent in `colony.db`** (Tabelle `dead_letters`, `schema_version` 4) — nicht mehr ein flüchtiger In-Memory-`VecDeque`. Sie ist die letzte verbliebene Diagnose-Wahrheit nach dem Verlust der Nachrichten-Persistenz und überlebt jetzt Colony-Shutdown/Crash. Die DB ist die **einzige** Wahrheit: Read und Drain query'n die Tabelle direkt; ein in-memory `VecDeque` dient nur noch als transienter Hand-off-Puffer, den die Single-Owner-`colony_task` nach jedem Event in die DB flusht (nie ein zweiter Spiegel). **Kein drop-oldest** mehr (das bounded Ring-Buffer-Verdrängen entfällt) — die Diagnose-Einträge werden erhalten, nicht verdrängt; ein fire-and-forget-Write hält Backpressure vom Routing fern. Jede Zeile trägt die sechs Lokalisierungs-Felder (`DeadLetterDto`; seit P1 zusätzlich `message_id` — aus `message_json` geparst, `None` bei Alt-Zeilen) plus den vollen serialisierten Message-Envelope (`message_json`), sodass der Drain die vollständige `DeadLetter` rekonstruiert. Unimplementierte `/colony/<x>`-Pfade (alle außer `/colony/dead_letters` in Phase 2) sowie `/colony` ohne Subpfad landen ebenfalls in der Queue, mit reason `ColonyEndpointUnimplemented` bzw. `ColonyEndpointInvalid` — beobachtbar statt crashy. **Read/Drain-Symmetrie**: Spec-konform ist `/colony/dead_letters` ein Message-Target (lesbar via `reply_to`-Roundtrip, ab Phase 3 / HTTP-API Phase 12). In Phase 2 — ohne `reply_to` — erfolgt das Auslesen über einen dedizierten internen Test-Hook (`ColonyMsg::DrainDeadLetters` mit `oneshot`-Reply); das ist ein Phase-2-Provisorium, nicht das finale symmetrische Design.

**Kanonische `error_code`-Strings**: Jeder Dead-Letter-Grund (intern eine `DeadLetterReason`-Enum-Variante) hat eine kanonische String-Repräsentation, die in der Dead-Letter-Queue als `error_code`-Feld exponiert wird (relevant für `?error_code=`-Filter der Phase-12-API): `unresolved_path`, `hive_no_route`, `no_route`, `cell_inactive`, `ttl_expired` (ab Phase 3, wenn TTL existiert), `colony_endpoint_unimplemented`, `colony_endpoint_invalid`, `blob_unavailable`, `blob_recursion_too_deep`, `invalid_ubf_body`, `consumes_violation`, `contract_violation`. Diese Strings sind Teil des stabilen API-Vertrags — neue Gründe ergänzen die Liste, bestehende ändern ihre String-Form nicht.

Anmerkungen zu den Delivery-Grenze-Codes:

- `blob_unavailable` — Blob-Resolution-Failure an der Delivery-Grenze (`Body::Blob`-uuid nicht auffindbar); live seit A8.
- `blob_recursion_too_deep` — Überschreitung des Tiefen-Limits bei rekursiver Blob-Referenz-Auflösung (siehe § Blob-Referenzen). Der String ist Teil des stabilen Vertrags, **aber der Code ist heute nicht produziert** (0 Producer für `messages_id`/`text_id`-Pointer) — die rekursive Blob-Auflösung selbst ist ein Roadmap-Defer (D-025, siehe `docs/roadmap.md` § Body / Blob-Auflösung).
- **`invalid_ubf_body` — Debug-vs-Release-Vertrag**: Dieser Code wird **nur im Debug-Build** konstruiert. Der UBF-Strukturvalidator im `colony_task`-`outputs_rx`-Arm läuft unter `#[cfg(debug_assertions)]` und DLQ't malformierte Cell-Emissions als `invalid_ubf_body`. Im **Release-Build** ist diese Struktur-Validierung der Cell-Outputs **inaktiv** — der String bleibt kanonisch und stabil, aber sein Auftreten ist build-profil-abhängig (D-033). (Nicht zu verwechseln mit der `contract.emits`/`consumes`-Schema-Validierung, die über `colony.json` `strict_validation` gesteuert wird und ein eigenes Sicherheitsnetz ist.)
- `consumes_violation` — eine Message verfehlte den substrat-seitigen required-`consumes`-Check an der Delivery-Grenze; die Cell wurde nicht aufgerufen (`docs/config.md` § consumes).
- `contract_violation` — eine Nicht-`code`-Emission verletzte ihr `contract.emits` am zentralen Check des outputs-Arms (flag-gated); die Emission wurde verworfen. Mit `input_reply_to` wird stattdessen eine Error-Reply geroutet (kein DLQ-Eintrag). Gleicher kanonischer Token wie die `code`-in-cell-Reply (cell-types.md).
- `no_route` — eine **Cell-Emission**, die keine Out-Edge ihres Senders matcht (Edge-Liste leer oder alle CEL-Conditions `false`), landet in der DLQ (Cell-Analogon zu `hive_no_route`). Kein impliziter Identity-Fallback mehr (Ruling A1): Default-Routing ist eine **setzbare Catch-all-Out-Edge** (eine bedingungslose Edge vom Sender = die „Default-Edge"). Der Eintrag ist selbst-lokalisierend (vier Felder, siehe unten).

**Outputs-Arm — drei disjunkte Fälle für eine Cell-Emission** (Ruling A1 + the spec owner 2026-06-12). Beim Verarbeiten einer Cell-Emission im outputs-Arm gilt genau einer von drei Pfaden, in dieser Reihenfolge:

1. **`em.target` ist ein `/colony/*`-Endpunkt** ⇒ **direkter ColonyDispatch** (Registry-/Virtual-Endpoint-Lookup über `route()`), VOR der Edge-Auswertung. `/colony/*` sind virtuelle Service-Endpunkte (siehe „/colony als virtueller Endpunkt"), keine Topologie-Knoten — eine Out-Edge ist dort weder nötig noch möglich; die A1-no_route-Regel greift nicht. Unbekannter `/colony/<x>`-Endpunkt ⇒ `colony_endpoint_unimplemented`. Das ist der Zustellweg für cell-emittierte Mutationen/Reads (EDA).
2. **Substrat-generierte Error-Reply an einen bekannten Absender** (`consumes_violation`, `message_timeout`-Backstop, `contract_violation`) ⇒ **direkt an `reply_to`** (Registry-Lookup via `route()`), NICHT über Out-Edges. Es ist Feedback an einen bekannten Absender, kein Routing-Ziel — eine fehlende Out-Edge darf es weder umleiten noch in `no_route` verwandeln. Unauflösbares `reply_to` ⇒ DLQ (Cascade-One-Shot).
3. **Normale Emission ohne matchende Out-Edge** ⇒ `no_route`-DLQ (siehe oben). **A1 regiert ausschließlich Fall 3.**

`cell_inactive` = Ziel-Pfad existiert (Cell oder Hive), ist aber disconnected/inaktiv (siehe
§ Konnektivität & Aktivität); gilt auch für Mailbox-Restbestand beim Disconnect.

**Cascade ist One-Shot, nicht rekursiv**: Fehler-Replies (Schritt 1) setzen selbst kein `reply_to` — sie sind terminal. Wenn eine Fehler-Reply selbst nicht zustellbar ist, greift automatisch Schritt 2 (Dead-Letter), ohne weiteren Cascade-Versuch. Maximale Cascade-Tiefe damit zwei Hops: originaler Fehler → Reply-Versuch → Dead-Letter. Verworfen wurden: header-basierte Loop-Detection (`is_cascade`-Flag — überflüssiger Envelope-State), TTL als Cascade-Backstop (vermischt Hop-Distance mit Cascade-Tiefe), mehrstufige konfigurierbare Cascade-Tiefe (Über-Engineering für ein Pathologie-Szenario).

### Mutation-Race-Sicherheit

Colony verarbeitet ihre Mailbox sequentiell. Mutationen sind normale Messages an `/colony/mutations`. Während eine Mutation läuft (inkl. Filesystem-Staging und Registry-Edits), pausieren andere Messages in Colony's Mailbox. Nach Mutation-Abschluss verarbeitet Colony die nächsten Messages mit dem neuen Registry-Stand. Falls eine Message auf eine zwischenzeitlich entfernte Cell zielt: Cascade oben.

Zwischen Cells und Colony gibt es keine Race — alle Routing-Entscheidungen laufen durch Colony's sequentielle Mailbox. Parallelität entsteht erst bei der Auslieferung an die Empfänger-Cells, die alle ihre eigenen Tokio-Tasks haben.

### Wildcards

Keine. Fan-out wird auf Edge-Ebene gelöst (1 Output → mehrere Edges). Pub/Sub-Pattern können später bei Bedarf separat diskutiert werden, sind aktuell nicht geplant.

### Routing-Symmetrie

Cell → andere Cell und Cell → Colony laufen beide durch denselben Routing-Pfad (Pfad-Resolution + Registry-Lookup). `/colony/*`-Pfade sind virtuelle Endpunkte in derselben Registry — keine Asymmetrie, kein Escape-Hatch nötig.

---

## Cell-Modell

- **Verzeichnis** mit `config.json` (von Colony **nur bei Instanziierung** geschrieben, danach Bootstrap-Snapshot).
- **Optional**: `cell.db` (SQLite, persistente Parameter und State — Cell-Authority, `db:own` Capability).
- **Optional**: `seed/<table>.jsonl` (Bootstrap-Daten + Export-Ziel).
- **Uniformes Aktor-Konzept**: jede Cell wird in Colony's `HashMap<Path, ActorHandle>`-Registry mit **einem** `ActorHandle` registriert — uniform für alle Cell-Klassen, kein Sum-Type. Der Handle ist im Kern ein `mpsc::Sender<Message>` (plus Pfad- und Cell-Type-Metadata). Colony's Routing-Code ist damit für alle Cells identisch: `handle.send(msg).await`.
- **Drei Spawn-Strategien** hinter diesem uniformen Handle, je nach Cell-Klasse:
  - **Stateful**: nicht reentrant → 1 langlebige `cell_task`-Tokio-Task, die in einer Loop die Mailbox pulled und `cell.handle()` direkt aufruft. Cell-State ist aus Cell-Sicht single-threaded zugreifbar (siehe „Nebenläufigkeit & Parallelität").
  - **Stateless**: reentrant → 1 langlebige `stateless_dispatcher`-Tokio-Task pulled die Mailbox und spawnt pro Message eine kurzlebige Worker-Task, die `factory.invoke()` ausführt und terminiert. Concurrency-Limit pro Cell via `tokio::sync::Semaphore` im Dispatcher konfigurierbar (`params.max_concurrency`, siehe unten).
  - **Long-Running** (`proxy`/`timer`/`mcp`): Doppel-Task-Pattern (Handler-Task + I/O-Task, siehe „Long-Running-Cells: Doppel-Task"). Beide Sub-Tasks gemeinsam unter einer logischen Cell-Identität.
- **Keine innere Loop in Cell-Code**: Cells warten nur auf Eingangs-Messages (oder externe Events bei `proxy`/`timer`/`mcp`). Iteration ist Topologie-Sache.
- **Vertrag**: jede Cell deklariert `contract.emits`, `contract.consumes`, `contract.settings`, `contract.capabilities` (siehe `config.md`).
- **Wissen ist beschränkt**: Cell kennt nur Message + Params. Nicht: Sender-Pfad, Empfänger-Pfad, andere Cells. Envelope-Felder (`id`, `trace_id`, `parent_message_id`, `correlation_id`, `target`, `reply_to`, `ttl`, `created_at`) sind aus Cell-Sicht **read-only** — sie werden ausschließlich von Colony beim Routing gesetzt (siehe „Envelope-Setter-Authority" im Message-Modell).

### Output-Pfad

Cells emittieren Outputs über einen geklonten `outputs_tx`, der zu Colony's zentraler `outputs`-Mailbox geht. Die Cell-Trait-Signatur ist uniform für alle Cell-Klassen:

```rust
trait Cell: Send {
    fn handle(
        &mut self,
        msg: Message,
        outputs: &mpsc::Sender<OutputEnvelope>,
    ) -> impl Future<Output = ()> + Send;
}
```

**Warum `impl Future<Output = ()> + Send` statt `async fn`**: native AFIT (`async fn` in Trait, stable seit Rust 1.75) bindet keine `Send`-Garantie an den zurückgegebenen Future. In generischen Kontexten wie `cell_task<C: Cell>(…)` oder `ColonyHandle::spawn<C, F>(…)` weiß der Compiler dann nicht, dass `cell.handle(…).await` zu einem Future führt, der über `tokio::spawn` an einen Worker-Thread reisen darf — Multi-Thread-Tokio braucht aber genau das (siehe „Nebenläufigkeit & Parallelität"). Return Type Notation (`C: Cell<handle(..): Send>`) wäre die elegantere Lösung, ist aber zum Spec-Stand noch nicht stable. Das explizite `impl Future + Send` im Trait-Return ist der idiomatische stable-Rust-Workaround; jede Cell-Implementation schreibt entweder `fn handle(...) -> impl Future<Output = ()> + Send { async move { ... } }` oder, häufiger, behält `async fn` mit zusätzlicher `Send`-Bound via `where`-Klausel. Der `Cell: Send`-Supertrait ist analog zwingend, weil Cells über Channel-Messages an `cell_task`-Spawns gereicht werden.

**Was eine Cell emittiert (`CellOutput`)**: Eine Cell emittiert **niemals** eine fertige `Message` — sie kennt die Envelope-Felder nicht (siehe „Envelope-Setter-Authority"). Sie pusht `CellOutput`-Werte über `outputs_tx`:

```rust
struct CellOutput {
    target: Path,                  // in Phase 3 von der Cell direkt gesetzt; ab Phase 4 typischerweise aus Edge-Auswertung
    content: serde_json::Value,    // content-JSON mit optionaler "header"-Sektion; Colony extrahiert header → message.headers, Rest → body
}
```

In Phase 3 (vor den Edges) setzt die Cell `target` direkt im `CellOutput`; ab Phase 4 überlagert die Edge-Auswertung dieses `target`. Das `content`-JSON wird von Colony zerlegt: `content.header` → `message.headers`, der Rest → `body: Body::Inline(...)`.

**Wer den Parent-Kontext beilegt — `cell_task`, nicht Colony**: Eine Cell läuft in ihrer **eigenen** `cell_task` (das Aktor-Substrat aus Phase 1). Colony ruft `cell.handle()` **nicht** direkt auf — täte sie es, liefe die Cell in Colony's Task und würde sie für die Dauer des `handle()`-Calls blockieren, was das „eine Task pro Aktor"-Modell bricht. Stattdessen: `cell_task` hält die konsumierte Eingangs-`Message` als lokale Stack-Variable und reichert jeden gepushten `CellOutput` mit dem Kontext an, den nur `cell_task` kennt — `parent_message_id` (= `id` der konsumierten Message), `trace_id` (von der konsumierten Message kopiert) und den eigenen `sender_path`. Dieses angereicherte Paket geht an Colony's `outputs`-Mailbox. Colony setzt die restlichen Envelope-Felder (`id`, `reply_to` = `sender_path`, `ttl` dekrementiert, `created_at`) und routet. Kein Shared-State, kein Lock — der Parent-Kontext ist lokaler Task-State, konsistent mit dem Concurrency-Modell.

**Wo `outputs_tx` lebt**:

| Cell-Klasse | `outputs_tx` lebt | Wer ruft `outputs_tx.send().await` |
|---|---|---|
| stateful | im `cell_task`-Lokalen (einmal beim Spawn geklont) | `cell.handle()` |
| stateless | beim Worker-Spawn als Parameter durchgereicht | `factory.invoke()` im Worker |
| long-running Handler-Task | im Handler-Lokalen (einmal beim Spawn geklont) | `cell.handle()` |
| long-running I/O-Task | hat **keinen** `outputs_tx` | — (I/O-Task pusht nur intern an den Handler) |

**`outputs`-Mailbox-Konsumption**: Colony's Hauptschleife läuft als `tokio::select!` über (a) ihre eigene Routing-Inbox (eingehende Messages von HTTP-API, Mutations, weitergerouteten Messages) und (b) die `outputs`-Mailbox (Cell-Emissions). Beide Wege landen in derselben Routing-Logik: Edge-Auswertung, Header-Modifikation, Target-Resolution, anschließend entweder `handle.send` (Target ist Cell) oder Hive-Transit-Auswertung (Target ist Hive-Scope-Marker — siehe „Hive-Pfade als Target — Transit-Auswertung"). Damit gibt es genau **eine** Routing-Schicht — keine Bypass-Pfade, keine Asymmetrie zwischen „intern emittiert" und „extern eingespeist", und Hive-Transit ist ein Zweig dieser einen Schicht, kein paralleler Pfad.

**Emit-Frequenz pro `handle()`-Call**: atomisch-emittierende Cells rufen `outputs.send` einmal, stream-fortpflanzende `code`-Cells können mehrmals senden (Multi-Send). Backpressure greift bei jedem `send`-Call gleich (siehe „Backpressure"-Abschnitt).

### Stateless-Cell-Dispatcher

Die Dispatcher-Task einer statelessen Cell läuft als:

```rust
async fn stateless_dispatcher<F: StatelessCell + 'static>(
    own_path: Path,
    mut mailbox: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<CellEmission>,
    cell: Arc<F>,
    max_concurrency: usize,
) {
    let sem = Arc::new(Semaphore::new(max_concurrency));
    while let Some(msg) = mailbox.recv().await {
        let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
        let outputs = outputs_tx.clone();
        let cell = cell.clone();
        let path = own_path.clone();
        tokio::spawn(async move {
            let sink = OutputSink::new(
                outputs, path, msg.id, msg.trace_id, msg.ttl, msg.headers.clone(),
            );
            cell.handle(msg, &sink).await;
            drop(permit);
        });
    }
}
```

**Weg A: generisch `<F: StatelessCell + 'static>` statt `Arc<dyn StatelessFactory>`**: `StatelessCell` nutzt RPITIT (`impl Future` als Return-Type im Trait), was den Trait nicht object-safe macht — `Arc<dyn StatelessCell>` kompiliert nicht. Stattdessen Monomorphisierung pro Cell-Type: `stateless_dispatcher::<FileCell>`, `stateless_dispatcher::<BashCell>` usw. Jede Cell-Instanz bekommt ihren eigenen monomorphisierten Dispatcher-Einstiegspunkt.

**Per-Message `OutputSink`**: der Worker (nicht der Dispatcher) baut den `OutputSink` aus den Message-Metadaten (`msg.id`, `msg.trace_id`, `msg.ttl`, `msg.headers`). Der Sink kapselt `outputs_tx` + den eigenen Pfad und liefert das `emit()`-Interface für `cell.handle()`.

**Permit-Drop am Worker-Ende**: `drop(permit)` am Ende des Worker-Closures gibt den Semaphor-Slot erst frei, wenn `cell.handle()` abgeschlossen ist. Damit ist `max_concurrency` ein harter Cap auf tatsächlich gleichzeitig laufende `handle()`-Calls, nicht nur auf gespawnte Tasks.

`max_concurrency` ist ein optionaler Cell-Param (`params.max_concurrency`, siehe `config.md`). **Default-Werte pro Cell-Type** (Phase 7):

| Cell | Default `max_concurrency` | Begründung |
|---|---|---|
| `file` | 8 | Disk-I/O — OS-I/O-Queue sättigt früh |
| `bash` | 4 | Subprozesse, resource-intensiv (Memory, FDs, Scheduler) |
| `edit` | 8 | Disk-I/O wie `file` |
| `web_fetch` | 32 | HTTP-Provider-Rate-Limits typisch tolerant, Connection-Pool begrenzt ohnehin |
| `web_search` | 8 | Search-APIs strikter rate-limited als einfache HTTP-GETs |

Die Dispatcher-Task ist ein **echter Concurrency-Wächter**, kein bloßer Spawn-Loop: durch `acquire_owned().await` bremst sie sich selbst, bevor sie weiter aus der Mailbox pulled — damit füllt sich bei Überlast die Mailbox, Sender (Colony beim Routing) blockieren, Backpressure propagiert sauber rückwärts.

### Long-Running-Cells: Doppel-Task

Cell-Types, die kontinuierlich externe Ereignisse aufnehmen (`proxy`/`timer`/`mcp`), nutzen statt einer einzelnen Cell-Task ein **Doppel-Task-Pattern pro Instanz**. Beide Sub-Tasks gehören zur selben logischen Cell, kommunizieren über einen internen `mpsc::channel`, und teilen sich in Colony's Registry eine einzige `ActorHandle`-Adresse mit einer externen Mailbox.

**Motivation**: ein 30-Sekunden-Long-Poll an Telegram, ein `tokio::time::sleep_until` bis zur nächsten Schedule-Zündung oder ein blockierender MCP-SSE-Read darf niemals die Annahme neuer Messages aus der Topologie blockieren — und umgekehrt darf eine volle externe Mailbox nicht das Polling stocken lassen. Eine einzige Task könnte das nur über `tokio::select!` zwischen einem unbegrenzt langen Future und `mailbox.recv()` lösen — mit dem Risiko, dass die Cancellation des Future bei jedem neuen Mailbox-Item Provider-State verliert (z.B. Telegram-Update-Cursor halb fortgeschritten). Das Doppel-Task-Pattern entkoppelt Polling- und Mailbox-Frequenz vollständig.

**Struktur**:

- **Handler-Task**: hält den gesamten Cell-State (z.B. Cursor in `cell.db`, Session-Maps, in-flight Korrelations-Tabellen, Schedule-Liste). Macht `tokio::select!` über (a) die **externe Mailbox** aus Colony's Routing und (b) einen **internen mpsc**, in den der I/O-Task Provider-Events pusht. Verarbeitet beide Quellen sequentiell — aus State-Sicht damit single-threaded, kein `Mutex`. Setzt allein Reihenfolge und State-Mutationen, hält den `outputs_tx` und ist der einzige der beiden Sub-Tasks, der in Richtung Topologie emittiert.
- **I/O-Task**: hält **keinen** Cell-State, hat **keinen** `outputs_tx`, hat **keinen** direkten `cell.db`-Zugriff. Macht die unbegrenzt lange I/O-Operation (Long-Poll, Sleep, SSE-Read), serialisiert eingehende Ereignisse zu Event-Frames, schiebt sie in den internen mpsc. Empfängt vom Handler bei Bedarf Reconfigure-Hints (z.B. „Schedule wurde geändert, dein nächster Sleep-Punkt gilt nicht mehr") über einen zweiten internen Channel.

**Skelett** (generisch, ohne Cell-Type-Spezifika):

```rust
async fn long_running_cell_spawn(
    mailbox: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<OutputEnvelope>,
    cell: Box<dyn LongRunningCell>,
) {
    let (events_tx, events_rx) = mpsc::channel(64);
    let (reconfig_tx, reconfig_rx) = mpsc::channel(8);

    tokio::spawn(io_task(cell.io_state(), events_tx, reconfig_rx));
    tokio::spawn(handler_task(mailbox, outputs_tx, events_rx, reconfig_tx, cell));
}
```

**Wichtig — äußere Glue-Supervisions-Task (AUDIT-PRE14-001):** das Skelett oben ist bewusst vereinfacht. Ein reines fire-and-forget-`tokio::spawn` beider Sub-Tasks würde Sub-Task-**Paniken verschlucken** — eine panickende Handler- oder I/O-Task bliebe vom Supervisor unbeobachtet, die Cell wäre still tot ohne Restart. Der reale Spawn (`cell_task_long_running`) ist daher selbst eine **äußere Task mit genau einem `JoinHandle`**, das der Supervisor beobachtet (die `RespawnFn`-Signatur bleibt damit byte-identisch zum Single-Task-Pattern). Diese äußere Task `tokio::select!`t über die `JoinHandle`s beider Sub-Tasks, **abortet das überlebende Geschwister** beim ersten Abschluss, **awaitet beide Ergebnisse** (nicht nur das gewinnende `select!`-Arm — ein Handler-Panik schließt über den gedroppten `reconfig_tx` auch `run_io`, und umgekehrt) und re-raised eine Panik via `std::panic::resume_unwind` → Supervisor sieht `was_panic=true` (`one_for_one`-Restart). Die Panik-Propagation ist damit **order-unabhängig** vom `select!`-Ausgang. (Der B-Backstop `cell.message_timeout` bleibt für Long-Running deferred, siehe Phase-7.5/9-Limitationen.)

**Backpressure-Verhalten**: der interne mpsc vom I/O-Task zum Handler ist bounded. Wenn der Handler überlastet ist (z.B. Topologie nimmt Tool-Results langsamer ab als der Provider sie liefert), blockiert der I/O-Task beim Push — die externe Polling-Frequenz drosselt sich von selbst, TCP-Buffer am Provider reguliert sich, kein Drop-Mechanismus nötig. Konsistent mit der system-weiten `block`-only Backpressure-Strategie (siehe „Backpressure-Strategie").

**Hot/Cold-Modell**: Long-Running-Cells sind **permanent awake** — Idle-Despawn macht hier per Definition keinen Sinn, weil die Daseinsberechtigung das kontinuierliche externe Polling ist. `cell.timeout: -1` ist die typische Konfiguration (siehe „Hot/Cold-Cell-Modell").

**Message-Timeout-Backstop**: Long-Running-Handler haben typisch `cell.message_timeout: 0` oder `-1` (kein Backstop), weil ein einzelner `handle()`-Call hier definitionsgemäß lang sein kann (z.B. ein langlaufender MCP-Tool-Call). Operation-Timeouts (Konzept A, `params.external_timeout_ms`) bleiben Pflicht für jede I/O-Operation im Handler — siehe „Timeouts".

**Cell-Type-spezifische Ausprägung**: die konkrete Rollenbelegung (was genau pollt der I/O-Task, was hält der Handler) ist Cell-Type-Sache und in `cell-types.md` pro Type beschrieben — siehe `proxy`, `timer`, `mcp`.

**Verworfen** wurden: (a) eine einzige Task mit `tokio::select!` über Mailbox und I/O-Future — Cancellation der I/O-Future bei jedem neuen Mailbox-Item verliert Provider-State. (b) `tokio::spawn` einer kurzlebigen I/O-Task pro Mailbox-Message — passt nicht zum kontinuierlichen Polling-Charakter (Long-Poll/Sleep/Stream). (c) Long-Running-Cell als zwei separate Cells unter einem Hive zusammenklemmen — würde Provider-State über zwei `cell.db`s splitten, die Atomarität des internen Channels verlieren und die „eine Adresse pro Cell"-Disziplin in der Registry brechen.

### Lifecycle von `config.json` und `cell.db`

| Datei | Wer schreibt | Wann |
|---|---|---|
| `config.json` (Cell) | Colony | **nur** bei Instanziierung (Template-Copy, UUID-Vergabe, `${VAR}`-Substitution) — der `swap_nodes`-Graph-Swap schreibt kein existierendes `config.json` neu (siehe § Mutation-Operationen) |
| `cell.db` (Cell) | die Cell selbst | nach Param-Updates per Message |
| `colony.db` | Colony | bei Instanziierung + Mutations-Commit + Templates-Scan + Message-Log-Write |

Nach Instanziierung ist `config.json` ein eingefrorener Bootstrap-Snapshot. Live-State lebt ausschließlich in `cell.db`. Param-Updates, die per Message kommen, persistiert die Cell in ihrer `cell.db` — `config.json` wird **nicht** mit-geschrieben. Beim Cell-Reset (z.B. Wipe der `cell.db`) startet die Cell mit ihrem Bootstrap-Stand aus `config.json`.

**Hive-Scope-Marker** haben eine `config.json` mit `type: "hive"` und ein `params.graph`-Feld (initialer Soll-Graph für ihren Subtree). Sie haben **keine** `cell.db` — der laufende Graph lebt in Colony's Registry und `colony.db`.

**Connection-Ownership-Modell (Phase 6.5)**: Die `cell.db`-Connection lebt im
`cell_task_stateful`-Stack-Frame, nicht im Cell-Field. Cells implementieren
`StatefulCell` (in `meclaw-colony`, nicht `meclaw-core` — Schicht-Trennung wie
bei `CellFactory`) und bekommen `&mut DbConn` als handle-Param (Phase-9-Update,
vorher `&mut rusqlite::Connection`). `cell_task_stateful` ist die einzige
Authority über cell.db-Lifecycle — es öffnet beim Spawn via
`open_or_create_cell_db` (M1 Resume-mit-State), reopens beim Restart über die
Factory-RespawnFn-Closure, schließt beim Mailbox-Disconnect oder
Cell-Task-Panic via Drop. Cell-Impls sind agnostisch.

Damit retired ist die E3-Variante-3-Pragma (Snapshot-vor-Output) aus Phase 5:
Cells können State NACH oder zwischen Output-Emits mutieren, weil `&mut DbConn`
über `.await` gehalten werden kann (im handle()-async-Block direkt).

**`DbConn`-Substrat-Pass (Phase 9)**: `DbConn` kapselt `rusqlite::Connection`
+ `rusqlite::InterruptHandle` (Send, Single-Move in Timer-Task — kein `Clone`,
keine Mutex-Ausnahme). rusqlite-Calls werden über `DbConn::call(|c| { ... }).await`
auf `tokio::task::spawn_blocking` ausgelagert; ein echter `query_timeout`
unterbricht hängende Queries via `InterruptHandle`. Closure ist
`Send + 'static`, owned-Input/owned-Output. `DbConn::wrap(conn, query_timeout)`
sitzt zwischen `open_or_create_cell_db` und `tokio::spawn(cell_task_stateful)`
— sync, kein `.await`, RespawnFn-Korridor bleibt unverletzt.
`QueryTimeout` ist vollwertiger `thiserror`-Error. Der Pass ist
verhaltensneutral: Phase-5/6.5/8-Demos blieben grün ohne Assert-Anpassung.

**State-Identity-Modell (M1 Resume-mit-State, Phase 6.5)**: Ein erneutes
`add_nodes` an einem Pfad mit existierender `cell.db` öffnet diese DB
wieder (resume — alle Rows bleiben). Schema-Migration ist `CellFactory`-
Verantwortung beim Spawn: Factory prüft `schema_version` und migriert oder
returnt Err (Mutation rejected). (`swap_nodes` migriert **keine** `cell.db`
mehr — der re-dedizierte Graph-Swap instanziiert bzw. verwendet eine eigene
Implementierung mit eigener `cell.db`, siehe § Mutation-Operationen.) Wipe-Pfad ist
deferred — kein Mutation-Op, Operator-Action außerhalb des Mutation-Flows.
Konsistent mit § No-Delete-Policy "lassen sich jederzeit über erneute
add_nodes (mit demselben Pfad/Namen) wieder anschließen".

**`OpenStatus`-Diskriminator (Phase 9)**:
`open_or_create_cell_db_with_status` liefert `(Connection, OpenStatus)` mit
`OpenStatus::Created | Resumed`. `OpenStatus::Created` ist der Seed-Trigger
für `store` (Factory ruft `load_seed_if_present` ausschließlich bei
`Created`, sonst doppelte Rows). `Resumed` bedeutet existierende `cell.db`
wurde wiedergeöffnet — keine Re-Initialisierung.

**Open-Failure-Story**: `open_or_create_cell_db` panickt bei FS-IO-Fehler,
DB-Korruption oder Permissions. Factory-Closure macht `.expect(...)` →
Initial-Spawn: Bootstrap-/Mutation-Fehler. Restart: Supervisor-Loop bis
`restart_limit` (Default 5) → Cell `failed`. Gleicher Modus wie
deterministisch panickende Cell (siehe § Restart-Strategie).

**Kanonische Reihenfolge bei Panic-Hooks / Backstop-Cancellation** (Phase-5-Test-Mocks; Phase-6+ Anwendung für echte Cells mit `cell.message_timeout`-Cancellation analog):

1. `counter += 1` (oder Per-Call-State-Update, sync).
2. `write_snapshot_with(...)` — sync, persistiert Pre-Panic/Pre-Cancel-State.
3. Cancel-/Panic-Check — sync, VOR async Output.
4. Output emit (async).

**Begründung**: Panic/Cancel VOR Output sorgt dafür, dass eine abgebrochene Cell KEINEN Output emittiert (sonst läuft die Cascade weiter UND der Trace bekäme einen extra Hop, der nicht zur Cell-State-Realität passt). Snapshot VOR Panic/Cancel sorgt dafür, dass der Restart-Overlay den korrekten Pre-Abort-State sieht.

---

## Cell-Types (Übersicht)

Übersichts-Tabelle (Type · Aufgabe · Aktor-Art · Bauart · Phase) ist kanonisch in [`cell-types.md` § Übersicht](cell-types.md#übersicht). Detail-Spec pro Built-in Cell-Type ebenfalls in `cell-types.md`. Status pro Cell-Type / pro Phase → `PROGRESS.md`.

---

## Edge-Modell

- Verbindet 1 Output → 1 Input (1 Output kann mehrere Edges haben → Fan-out → parallel).
- **`condition`** (CEL boolean) entscheidet, ob die Edge zuständig ist. Liest **ausschließlich** die zwei Header-Fächer der Source-Cell-Emission über die Namensräume `context.*` (persistent) und `hop.*` (genau dieser Hop, = isolierter Cell-Output ∘ Edge-Modifier; siehe „Headers vs. Body — Schreibmodell").
- **`modifier`** (Operations-Objekt mit CEL-Expressions als Werte) ist die **alleinige Header-Authority**: er befördert/berechnet `context.*` und verfeinert `hop.*` vor Weiterleitung. Schema:

  ```json
  "modifier": {
    "set_context":    { "<key>": "<CEL über context.* + hop.*>" },
    "delete_context": [ "<key>" ],
    "set_hop":        { "<key>": "<CEL über context.* + hop.*>" },
    "delete_hop":     [ "<key>" ]
  }
  ```

  - `set_context` / `set_hop`: Map von Keys auf CEL-Expressions. Jede Expression hat Lesezugriff auf **beide** Fächer (`context.*` und `hop.*`, read-only Maps) und liefert den neuen Wert. Existiert der Key im Ziel-Fach bereits, wird er überschrieben; existiert er nicht, wird er angelegt. Damit deckt `set_*` die zwei Operationen „neuen Wert setzen" und „bestehenden Wert modifizieren" ab — pro Fach getrennt.
  - `set_context` ist CEL-wertig und deckt damit beides ab: **befördern** eines Hop-Werts nach context (`"set_context": { "turn_id": "hop.turn_id" }`) UND **berechnen** (`"set_context": { "iter": "int(context.iter) + 1" }` — der `int()`-Cast ist nötig, weil CEL einen JSON-Integer als `uint` deserialisiert und `uint + int` nicht definiert ist).
  - `delete_context` / `delete_hop`: Liste von Keys, die aus dem jeweiligen Fach entfernt werden.
  - Alle vier Felder optional. Fehlender oder leerer Modifier = Identity (`context` unverändert durchgereicht, `hop` unverändert weitergegeben).
  - **Auswertungs-Semantik**: alle `set_*`-Expressions lesen den **eingehenden** (vor-Modifier) Zustand beider Fächer als fixen Kontext. Reihenfolge pro Fach: zuerst `set_*`, dann `delete_*` — damit kann ein gesetzter Wert durch dasselbe Modifier-Stück nicht versehentlich wieder gelöscht werden (wer das doch will, schreibt zwei Edges).
  - **Begründung der Schema-Wahl**: CEL ist eine reine Expression-Language ohne Side-Effects — eine CEL-Auswertung liefert *einen* Wert, sie mutiert keine Inputs. Damit ist „Header setzen/löschen" nicht direkt in CEL ausdrückbar. Verworfen: (a) Modifier als CEL-Script, das eine komplette Headers-Map zurückgibt — zwingt Edges, alle durchlaufenden Werte explizit aufzulisten, sonst werden sie implizit gelöscht; (b) Modifier als Patch-Map mit `null`-Sentinel für Löschen — kollidiert mit `null` als legitimem Wert. Die hier gewählte Variante macht alle Operationen explizit, ohne Sentinel-Werte, trennt sie nach Fach (`context` vs. `hop`) und bleibt für AI-Builder trivial generierbar.

  Beispiel:
  ```json
  "modifier": {
    "set_hop": {
      "msg_type": "hop.finish_reason == 'tool_calls' ? 'tool_call' : 'final_response'",
      "tier":     "hop.priority == 'high' ? 'gold' : 'standard'"
    },
    "delete_hop": ["internal_debug_marker"]
  }
  ```
  Antwort-Metadaten (`finish_reason`, `priority` etc.) leben im `hop`-Fach. `context`-Werte (`session_id`, `turn_id`, `user_id` etc.) laufen unverändert durch, weil sie weder in `set_context` noch in `delete_context` erwähnt sind.

- **Fan-out und die Fächer**: bei Fan-out (1 Output → N Edges) kopiert Colony den `context` **identisch** in jede der N erzeugten Nachrichten; die Cell fasst `context` nie an. Zweig-Spezifisches lebt im `hop` (von der Cell geschrieben) oder wird per-Zweig über den jeweiligen Edge-Modifier gesetzt.

- **Edges operieren strikt auf der Header-Schicht.** Body-Slots und Envelope-Felder (`target`, `reply_to`, `ttl`, Trace-IDs etc.) sind außerhalb des Edge-Scope. Wer Body-Transform oder Envelope-Logik braucht, baut eine `code`-Cell (siehe Cell-Type-Beschreibung). Content-bewusstes Routing geht über „Cell setzt Header (`hop`), Edge konditioniert darauf". Begründung: Symmetrie zwischen Condition und Modifier (beide lesen `context.*` + `hop.*`, einfaches Mental-Model), Routing-Performance (Edge-Auswertung muss nie Blob-referenzierte Body-Felder auflösen), Selbst-Dokumentation (der Graph zeigt direkt, welche Header-Werte zu welchen Targets führen), Body-Stabilität (Cells können Body-Slot-Schemata weiterentwickeln, ohne dass Edges brechen). Verworfen wurden: Modifier darf Body modifizieren (würde Blob-Auflösung im Routing-Pfad erzwingen und überlappt mit `code`-Cells), Condition darf Body lesen (gleiche Performance-Bedenken, plus nimmt Druck weg, Header-disziplinierte Outputs zu emittieren), Modifier darf `target`/`reply_to` umschreiben (macht den Graph nicht-deklarativ, schwächt Read-API und Audit).
- **Edge-Identität**: jede Edge hat eine UUID v7, von Colony bei Anlage vergeben. Sichtbar in der Read-API und im Mutations-Log. Im Mutation-Surface (Builder-Diff) werden Edges aber üblicherweise per **Match-Pattern** über ihre Eigenschaften (`from`, `to`, `condition`, `modifier`) referenziert; UUID ist Fallback für Disambiguierung im Pathologie-Fall.
- **Edge-Tabelle**: Edges leben zentral in Colony's Edge-Tabelle, indiziert nach `from`-Pfad für schnellen Fan-out-Lookup. Cells kennen ihre Edges nicht — Colony evaluiert sie nach Cell-Emission.

---

## Message-Modell

```rust
struct Message {
    id: Uuid,                          // v7, zeitsortiert, von Colony gesetzt
    trace_id: Uuid,                    // Root-Message-ID, konstant über Trace
    parent_message_id: Option<Uuid>,   // None bei Source, sonst von Colony automatisch
    correlation_id: Option<Uuid>,      // optional, für req/resp-Paarung

    target: Path,
    reply_to: Option<Path>,
    ttl: u32,                          // routing-step-based, dekrementiert bei jeder Colony-Routing-Entscheidung

    headers: serde_json::Map<String, Value>,  // Routing-Metadaten
    body: Body,                               // Inhalt (Inline oder Blob)

    created_at: i64,                   // Unix-Sekunden (SystemTime → as_secs() as i64), nicht Millisekunden
}

enum Body {
    Inline(serde_json::Value),    // < blob_inline_max_bytes (Default 64 KB, Colony-konfigurierbar)
    Blob(Uuid),                   // ≥ Schwelle, in blobs/<uuid>.<ext> + blobs/<uuid>.<ext>.meta.json (siehe „Blob-Storage")
}
```

**TTL-Semantik (flach)**: `ttl` ist eine Schutz-Schranke gegen unkontrollierte Routing-Schleifen. Colony dekrementiert bei jeder Routing-Entscheidung, also einmal pro Cell-zu-Cell-Hop. Bei `ttl == 0` geht die Message **direkt** in die Dead-Letter-Queue (`ttl_expired`, direct-to-DLQ — **nicht** über die Routing-Fehler-Cascade mit ihrem Schritt-1-`reply_to`-Reply-Versuch; eine abgelaufene TTL ist terminal, siehe „Routing-Algorithmus"). Default in `colony.json` per `message_default_ttl` (Empfehlung: 64). Builder können den Wert pro Initial-Message setzen (`ttl`-Feld in `POST /messages`, nur positive Integer, sonst `422 invalid_ttl`). **Hierarchie**: explizites `ttl`-Feld der Initial-Message > `colony.json` `message_default_ttl` > Const-Seed `MESSAGE_DEFAULT_TTL` (=64); Cells setzen `ttl` nie (Envelope-Setter-Authority). Nicht zu verwechseln mit `message_timeout_default_ms`, das die maximale Bearbeitungszeit _innerhalb_ einer Cell adressiert.

### Envelope-Setter-Authority

Envelope-Felder (`id`, `trace_id`, `parent_message_id`, `correlation_id`, `reply_to`, `ttl`, `target`, `created_at`) werden **ausschließlich von Colony beim Routing gesetzt**. Cells können sie nicht schreiben — das content-JSON, das eine Cell emittiert, hat keinen Mechanismus für Envelope-Felder, und Edge-Modifier operieren strikt auf Headers (siehe „Edge-Modell"). Konkret:

| Feld | Wer setzt | Wann |
|---|---|---|
| `id` | Colony | bei jeder neuen Message (UUID v7) |
| `trace_id` | Colony | bei Source-Message neu; sonst aus parent kopiert |
| `parent_message_id` | Colony | aus der konsumierten Eingangs-Message übernommen, `None` bei Source-Messages |
| `correlation_id` | **heute kein originärer Producer** — reserviertes Envelope-Feld für künftige req/resp-Paarung. Korrelation läuft aktuell über die **context-Header-Konvention** (z.B. `turn_id`, s. § Metadata-Aggregation), **nicht** über `correlation_id` | — (Feld reserviert, kein originärer Producer; `?correlation_id=`-Filter auf `/colony/trace` daher heute inert) |
| `target` | Auslöser-Schicht (Cell-Output bestimmt durch Edges, HTTP-API durch Endpoint) | beim Routen |
| `reply_to` | **Colony**, automatisch auf den absoluten Pfad des Senders | bei jeder Routing-Entscheidung |
| `ttl` | Colony — bei Source-Messages neu gestempelt aus `colony.json` `message_default_ttl` (Seed: Const `MESSAGE_DEFAULT_TTL`, =64); der HTTP-Ingress nimmt ein explizites `ttl`-Request-Feld pro Initial-Message als Override (Hierarchie s. „TTL-Semantik"); dekrementiert pro Hop | bei Source-Message neu, danach dekrementiert |
| `created_at` | Colony | bei Message-Anlage |

**`reply_to`-Spezialfall**: bei Messages, die über die HTTP-API eingespeist werden (`POST /messages`), setzt Colony `reply_to` auf einen virtuellen API-Request-Pfad oder belässt `None` — die HTTP-Response wird über den Request-Channel zurückgegeben, nicht über Routing. Cells, die ein anderes Reply-Ziel als ihren eigenen Pfad wollen (z.B. „Tool-Result soll zur Collector-Hive, nicht zurück zur LLM"), lösen das **anwendungs-spezifisch über Header-basiertes Routing** — z.B. ein Header `reply_target`, gesetzt vom ursprünglichen Sender, und eine Edge, die darauf konditioniert. Damit bleibt das Substrat minimal: keine reservierte Envelope-Slot-Konvention im Cell-Output, kein Envelope-Schreib-Modifier, keine Aufweichung der Edge-Modell-Spec.

**`reply_to == None` — Terminal-Kette ohne matchende Out-Edge (IST-Verhalten, W2-revidiert)**: der JSON-Ingress setzt heute kein `reply_to` (belässt `None`). Eine dadurch ausgelöste Cell-Emission, die anschließend **keine** Out-Edge matcht, durchläuft seit Phase-16 W2 (Rulings A1 + W2d) folgende Kette:

1. Die Built-in-Cell-Typen setzen das Target ihrer Op-/Fehler-Replies auf das inbound `reply_to`; bei `None` fallbacken sie seit **W2d** auf das eigene `msg.target` (den Pfad, an den die Cell adressiert war) — **nicht** mehr auf den `/colony/dead_letters`-READ-Endpunkt. Der vorgelagerte Hartkodier-Fallback `unwrap_or("/colony/dead_letters")` an den atomar-emittierenden Cell-Typen ist entfernt.
2. outputs-Arm (A1, drei disjunkte Fälle): die Emission matcht **keine** Out-Edge — es gibt **keine** Identity-Decision mehr. Sie dead-lettert als `no_route` (`DeadLetterReason::NoRoute`, Cell-Analogon zu `hive_no_route`): selbst-lokalisierend mit `sender → resolved_target`, `trace_id`, `created_at`. Default-Routing ist eine **setzbare Catch-all-Out-Edge**. **Terminal**, kein Re-Inject, kein Loop.
3. (Sonderfall) Emittiert die Cell explizit an ein `/colony/*`-Target: `/colony/mutations` wird ausgeführt; `/colony/dead_letters` und die übrigen Read-Endpunkte werden hart abgewiesen bzw. gelesen (§ „Endpunkt-Klassifikation für Cell-Emissionen"). Der pre-W2d-Source-Loop (DLQ-Listing-Reply zurück an die Cell → Re-Emission, ttl-ungedeckelt) ist damit an der Wurzel beseitigt.

Eine `no_route`-DLQ-Signatur nach einem `reply_to`-losen Ingress-Probe (z.B. store-/sink-Op-Reply ohne verdrahtete Out-Edge) ist damit erwartetes IST-Verhalten dieser Kette, kein Routing-Bug — die Op-Reply erreicht den Sender bewusst nicht mehr (Routing ist explizit, nicht implizit-Identity). Das distinkte No-Match-Diagnose-Signal für Cell-Emissions (`no_route`) existiert seit W2a.

### Headers vs. Body — Schreibmodell

Header leben in **zwei strukturell getrennten Fächern** mit unterschiedlicher Lebensdauer und Schreib-Authority:

- **`context`** — persistent, reist über den **gesamten** Message-Lebenszyklus. **Alleinige Schreib-/Lösch-Authority: die Edge.** Trägt Korrelation/Langlebiges (`turn_id`, `session_id`, `iter`).
- **`hop`** — genau **ein** Hop. Ist der isolierte Contract-Output der unmittelbar vorausgehenden Cell, verfeinert durch den durchlaufenen Edge-Modifier. Wird bei der **nächsten** Cell-Emission **komplett ersetzt** (verfällt). Trägt Cell-Produkt/Routing-Kontrolle/Antwort-Metadaten (`operation`, `finish_reason`, `msg_type`, `route`, `agent_target`, `rows_affected`, `error_code`).

Daraus folgt das Schreibmodell:

- **Cells schreiben content als JSON** mit optionaler `"header"`-Sektion. Colony interpretiert diese Sektion **als `hop`** — sie ist der isolierte Cell-Output. **Cells schreiben nie `context`** (das ist allein Edge-Authority), und Cells erben **nicht** den `hop` ihres Vorgängers.
- **Cells lesen alles** read-only: `context`, `hop`, Body-Slots und die Envelope-Felder. Sie schreiben **ausschließlich** ihren isolierten Output → dieser wird zum neuen `hop`. (Lese-Deklaration in `contract.consumes.context.<key>` / `contract.consumes.hop.<key>`, Body-Werte in `contract.consumes.body.<key>` — Aufteilung siehe `config.md`.)
- **Lösch-Authority ist Edge-Sache**: Cells haben keinen Lösch-Mechanismus. Eine Edge entfernt Werte per `delete_hop` (Hop-Fach) bzw. `delete_context` (Context-Fach).
- **Edges (Conditions und Modifier) sind die alleinige Header-Authority**: Conditions lesen `context.*` + `hop.*` (read-only, CEL-Boolean), der Modifier schreibt `context` (`set_context`/`delete_context`) und verfeinert den `hop` (`set_hop`/`delete_hop`) über das explizite Operations-Schema (siehe „Edge-Modell"). Body-Slots und Envelope-Felder sind außerhalb des Edge-Scope.
- **Default: der `hop` verfällt.** Ein Wert lebt genau **einen** Hop, außer eine Edge befördert ihn per `set_context` nach `context` (fail-loud: Befördern vergessen → der Wert verschwindet bei der nächsten Cell-Emission).
- **Konflikt-Regel**: innerhalb eines Fachs Replace (last-write-wins); die zwei Fächer überschneiden sich nicht. Audit-Trail lebt im zentralen Message-Log via `parent_message_id`-Chain, nicht in den Headern.
- **Komposition entlang des Routing-Pfads (R3-Ruling, K-H7):** Durchläuft eine Message mehrere Edges nacheinander (mehrstufige Hive-Transits, oder Cell-Emission → Transit-Edge → weiterer Hive-Transit), komponieren same-key-Modifier **left-to-right entlang des Pfads**: jede Edge wendet ihren `set_context`/`set_hop` auf den von der vorhergehenden Edge **bereits transformierten** Header-Stand an. Setzen mehrere Edges denselben Key, **gewinnt die spätere — konsumentennahe, „innere" — Edge** (last-write-wins entlang des Pfads). Das ist dieselbe Replace-Semantik wie innerhalb eines Fachs, nur über die Hop-/Transit-Sequenz gezogen (`hop` verfällt am Pfad-Ende bei der nächsten Cell-Emission, `context` bleibt persistent). **Pin:** jeder Transit-Hop schreibt eine eigene `message_log`-Transit-Row, sodass der komponierte Endzustand über die `parent_message_id`-Chain nachvollziehbar bleibt.

### Standard-Header-Konvention (Anwendungs-Ebene)

meclaw-Core kennt diese Keys nicht semantisch — sie sind Konventionen für Anwendungen:

| Key | Bedeutung |
|---|---|
| `turn_id` | Eine User-Interaktion (z.B. ein Chat-Turn von proxy-cell) |
| `session_id` | Eine logische Session-Klammer |
| `user_id` / `chat_id` | externe Identifikatoren (Proxy-Plattform) |
| `locale` | Sprache/Locale der Anfrage |

**`chat_id` ist plattformabhängig typisiert** — numerisch bei Telegram, ein String bei Slack (Composite-Form `"<channel>"` bzw. `"<channel>:<thread_ts>"`, siehe `cell-types.md` § `proxy`). meclaw-Core validiert den Typ nicht; er ist eine Konvention der jeweiligen Source-Cell. Die Invariante darunter bleibt davon unberührt: `context` wird ausschließlich von Edges und vom Ingress geschrieben, nie von Cells.

Diese Standard-Keys leben im `context`-Fach (persistent über den Lebenszyklus). **Invariante: `context` wird ausschließlich von Edges und vom Ingress-at-birth geschrieben, nie von Cells.** Es gibt damit genau zwei Eintritts-Pfade, über die ein Key initial nach `context` gelangt:

1. **Source-Cells** (`proxy`, `timer`): emittieren ihre Werte als `hop`; ihre **Aus-Edge** befördert per `modifier.set_context` nach `context` — in-graph, sichtbar, kein implizites Substrat-Verhalten. Das Source-/Proxy-Template-Pattern backt diese Eingangs-Promotion ein (default-da, aber als reale Edge sichtbar).
2. **HTTP-Ingress**: ist der Geburtspunkt der Nachricht und **etabliert die initiale `context` direkt** — eine sanktionierte, eng begrenzte context-Quelle (der Ingress *ist* die Eingangs-Edge). Die vom Ingress nach `context` gehobenen Keys sind **deklariert** (welche HTTP-Header → `context`: `turn_id`, `session_id`, `user_id`, `chat_id`, `locale`), sodass sie auditierbar sind und der Mutations-Validator den Ingress als **Reachability-Wurzel** für `consumes.context` behandelt (sonst meldete er `turn_id` fälschlich als unerreichbar).

Diese Ingress-Ausnahme ist **strikt auf den Geburtspunkt begrenzt** — kein Cell-Schlupfloch: **Cells können `context` nicht direkt schreiben**, sie emittieren ausschließlich `hop`. **Weitere Header-Konventionen sind frei wählbar** — Anwendungen, Builder oder Pattern-Templates können beliebige eigene Keys definieren (z.B. `msg_type`, `priority`, `tool_call_id`); ob ein Key persistent (`context`) oder hop-lokal (`hop`) ist, ergibt sich rein strukturell aus dem Fach. meclaw-Core unterscheidet nicht zwischen "Standard"- und "Anwendungs"-Headers — alle sind generische Map-Einträge im jeweiligen Fach.

### Edge-Expression-Sprache

CEL (Common Expression Language) via die `cel`-Crate (GitHub-Projekt `cel-rust`; der Crate-Name auf crates.io ist `cel`). Eigenständiger Google-Standard, sicher (nicht Turing-complete), ausdrucksstark genug für alle bekannten Routing-Patterns.

**Funktionsumfang (empirisch verifiziert gegen `evaluate_condition`, cel 0.13.0, 2026-06-11)**: das CEL-Standard-Makro `has()` ist in Edge-Conditions verfügbar — `has(hop.priority)` evaluiert bei fehlendem Key zu `false` (kein Eval-Fehler); gleichwertig für Key-Existenz-Checks ist der `in`-Operator (`'priority' in hop`). Daneben belegen die Substrat-Tests Vergleiche, Ternary, String-Methoden (`contains`) und numerische Casts (`int()` — nötig für Arithmetik auf JSON-Integern, siehe „Edge-Modell" zum `uint`-Deserialisierungs-Quirk).

### Atomarität

Messages sind atomar — **zwei feste Header-Slots** (`context` und `hop`), je ein Wert pro Name pro Fach, **keine wachsende Hop-Liste**, keine Versionierung und keine Annotation-History im Envelope. Die Zwei-Fächer-Trennung (siehe „Headers vs. Body — Schreibmodell") ist strukturell, nicht historisch: `hop` wird bei jeder Cell-Emission komplett ersetzt, `context` per Replace überschrieben — in beiden Fällen bleibt es bei genau einem Slot pro Name. Echte Historie (kumulierte Tokens, Iterations-Trail, erwartete Tool-Call-IDs) gehört in eine `store`-Cell bzw. einen Aggregator, **nicht** in den Header (siehe § „Metadata-Aggregation ist Topologie"). Trace-Rekonstruktion läuft über `parent_message_id`-Chain im zentralen Message-Log in `colony.db`.

---

## Body-Format (Universal)

Alle Cells emittieren und konsumieren denselben Body. Damit braucht keine Cell einen Format-Adapter zwischen Inputs unterschiedlicher Quelltypen — `proxy` User-Turns, `bash` stdout, `web_fetch` Response-Body, `file` Read-Content, `code` Skript-Output, `llm` Inferenz-Result: alle in derselben Struktur.

### Top-Level-Slots

Drei zentrale Slots — `system`, `messages` **oder** `attachments` —, von denen **mindestens einer** gesetzt sein muss (Schema-`anyOf` über genau diese drei `required`-Zweige). Ein reiner Datei-Upload (nur `attachments`, ohne `system`/`messages`) ist eine legitime Message-Form.

| Slot | Bedeutung |
|---|---|
| `system` | Optionaler System-Kontext: Identity (Persona), Tools-Schema, Bootstrap-Instructions, Facts, Session-State. Nested über Sub-Slots wie `system.identity.soul.text`. Vollständiger Slot-Pfad ist Pflicht; abgekürzte Notation verboten. |
| `messages[]` | Chronologische Liste von Konversations-Turns. |
| `attachments[]` | Liste blob-referenzierter Datei-Anhänge (ab Phase 12 aktiv konsumiert, Slot ab Phase 3 namensreserviert — siehe „Reservierte Slot-Namen" und das `attachments[]`-Schema). Als `anyOf`-Zweig bereits heute ein gültiges Top-Level-Shape. |

**Reservierte Slot-Namen** (ab Phase 3 namensreserviert, Implementierung wie angegeben):

| Slot | Phase aktiv | Bedeutung |
|---|---|---|
| `attachments[]` | 12 | Liste von Blob-referenzierten Datei-Anhängen (PDF, TXT, Bilder, Audio etc.) mit typisierten Metadaten — siehe „`attachments[]`-Schema" unten. Slot-Name ist ab Phase 3 reserviert: keine Cell darf ihn anders verwenden, auch wenn das Substrat das Slot noch nicht aktiv konsumiert. |

Cells können **eigene Top-Level-Slots** anlegen (z.B. `meta`, `delta`, `event`, `graph`), solange sie nicht mit reservierten Slot-Namen kollidieren. Konsumenten ignorieren unbekannte Top-Level-Slots und unbekannte `system`-Sub-Slots.

**Schema-Validierung — Zeitpunkt und Scope**: Die UBF-Body-Validierung gegen das JSON-Schema läuft an Stellen unterschiedlicher Schärfe — **Rand-Validierung always-on, Interior-Korrektheit als Debug-Netz** (kein trusted-Exemption-Carve-out):

- **Rand (Trust-Boundary) — always-on, auch im Release:** Jeder über die HTTP-API eingespeiste Source-Body (`POST /messages`, JSON- wie Multipart-Pfad) wird **vor** dem Routing gegen das Schema validiert; ein Verstoß wird mit `422` (`invalid_ubf_body`) abgewiesen, statt einen malformierten Body in die Routing-Schicht zu lassen. Das beim Multipart-Upload synthetisierte attachments-only-Shape ist dabei ein gültiger `anyOf`-Zweig. Auf dem Multipart-Pfad gibt es keinen client-verfassten UBF-Body: der Client lädt Dateien (`multipart/form-data`), das Substrat synthetisiert daraus einen `attachments[]`-Body, der per Konstruktion schema-gültig ist (auch der datei-lose Fall `{"attachments":[]}`). Die always-on-Validierung läuft dort als Defense-in-Depth, kann aber konstruktionsbedingt nicht fehlschlagen — ein client-erreichbares `422` entsteht nur auf dem JSON-Pfad.
- **`code`-Output — always-on (kein Opt-out):** Die `code`-Cell ist die einzige user-skript-getriebene Output-Quelle und damit selbst eine Trust-Boundary; ihre `contract.emits`-Validierung läuft **unbedingt** (`validate_emits = true`, unabhängig von Build-Profil und `colony.json` `strict_validation` — siehe `docs/config.md` § Schema-Format und Validierung).
- **Built-in-Cell-Output — Debug-Netz:** Die UBF-Struktur-Validierung der von Substrat-Cells emittierten Bodies läuft unter `#[cfg(debug_assertions)]` — Dev-/Test-Builds fangen Schema-Verstöße hart und DLQ'en sie (`invalid_ubf_body`), Release-Builds haben null Validierungs-Overhead. Das ist ein Korrektheits-Sicherheitsnetz für die vertrauenswürdigen Substrat-Cells, **kein** Trust-Boundary-Carve-out: die untrusted Ränder (HTTP-Ingress, `code`) sind oben always-on gedeckt. Zusätzlich läuft dort (outputs-Arm) die zentrale `contract.emits`-Validierung der Nicht-`code`-Typen, flag-gated (`resolve_validate_emits`): Verletzung → Error-Reply an das `input_reply_to` (`error_code: "contract_violation"`), sonst DLQ (gleicher Token); `code` bleibt davon ausgenommen (in-cell always-on).

Das Schema liegt als statisches Dokument in `meclaw-core` (via `include_str!`, einmalig zu einem Validator kompiliert) und kennt `attachments`-only als valides Shape. Das `attachments[]`-Slot ist mit seinem korrekten (Phase-12-)Schema verankert, auch wenn keine Phase-3-Cell ihn befüllt — wer ihn mit Fremdinhalt füllte, schlüge gegen die Validierung an. So ist die Namensreservierung syntaktisch erzwungen, ohne Resolver-Code.

### `messages[]`-Schema

Jeder Eintrag ist entweder ein **Turn-Objekt**, ein **Turn-Pointer** oder ein **Bulk-Pointer**:

```json
// Turn-Objekt — inline
{ "origin": "user|assistant|tool|system",
  "type":   "text|tool_call|tool_result|image|audio",
  "text":   "<inline-string>",
  "id":     "<bei tool_call/tool_result Pflicht>" }

// Turn-Pointer — eine einzelne Turn-Content im Blob
{ "text_id": "<UUIDv7>" }

// Bulk-Pointer — Verweis auf ein Body-Dokument im Blob; dessen messages[] wird inline expandiert
{ "messages_id": "<UUIDv7>" }
```

- `origin` (Pflicht, Enum): wer den Turn gesprochen hat.
- `type` (Pflicht): bestimmt das semantische Format. `image`/`audio` reserviert für Multi-Modal.
- `text` (inline) **oder** `text_id` (Pointer) — exklusiv pro Slot.
- `id` ist bei `tool_call`/`tool_result` Pflicht und ist der Korrelations-Anker für die Collector-Aggregation. Werte sind Pass-Through vom Provider (`tool_call_id`).
- **Das Turn-Objekt ist geschlossen** (`additionalProperties: false` in `crates/meclaw-core/schemas/ubf-body.json` § `$defs.TurnObject`): erlaubt sind genau `origin`, `type`, `text`, `id`. Ein zusätzliches Feld — etwa ein Tool-Name neben `type: "tool_call"` — macht **den gesamten Body** `invalid_ubf_body`. Strukturelle Zusatzinformation gehört deshalb in den `header`-Slot, nicht in den Turn.

### `attachments[]`-Schema (Slot-Name ab Phase 3 reserviert, aktiv ab Phase 12)

Liste von typisierten Datei-Anhängen, die als Blobs im `blobs/`-Verzeichnis liegen (siehe „Blob-Storage"). Jeder Eintrag ist ein Objekt mit:

```json
{
  "blob_id":    "<UUIDv7>",
  "mime_type":  "application/pdf",
  "filename":   "report.pdf",
  "size_bytes": 124573,
  "sha256":     "abc..."   // optional; in Phase 12 weggelassen (kein Consumer)
}
```

- `blob_id` (Pflicht): UUID v7, referenziert die zwei Blob-Dateien `blobs/<blob_id>.<ext>` + `blobs/<blob_id>.<ext>.meta.json`.
- `mime_type` (Pflicht): MIME-Type des Inhalts. Authoritativ im Sidecar; hier dupliziert für schnellen Read ohne Sidecar-Fetch.
- `filename` (optional): ursprünglicher Filename bei Upload via HTTP-API; `null` bei system-generierten Anhängen.
- `size_bytes` (Pflicht), `sha256` (optional): dupliziert aus Sidecar, gleicher Grund. `sha256` ist in Phase 12 nicht verpflichtend (siehe Sidecar-Schema-Note). **Schema-Drift-Hinweis (D-027):** Das UBF-JSON-Schema (`ubf-body.json`) listet `sha256` heute als **required** in `attachments[]` — strenger als diese Spec. Latent (attachments werden erst in Phase 12 aktiv); Angleichung des Schemas an „optional" steht beim attachments-Aktivierungs-Slice an.

Cells, die Anhänge konsumieren, deklarieren `consumes.body.attachments` im Vertrag (siehe `config.md`). Deklarieren ist dabei bindend: jeder in `consumes.body` deklarierte Key ist Pflicht, es gibt kein optionales `consumes.body`-Feld (siehe `config.md` § contract). Cells, die das nicht tun, ignorieren das Slot. Damit ist Anhangs-Verarbeitung eine **Cell-Capability**, kein Substrat-Detail — eine `llm`-Cell mit Vision-Modell deklariert `consumes.body.attachments` und lädt Bilder via Storage-Abstraktion, eine `text`-only-LLM-Cell sieht den Slot nicht.

Anhänge sind **separat von `messages[]`**: eine Konversation bleibt rein textuell, Anhänge hängen als parallele Liste am Body. Damit kollidieren PDF-Anhänge nicht mit der `messages[]`-Turn-Semantik (`origin`, `type: tool_call|...`), und LLM-Provider-Adapter können sie cell-type-spezifisch in ihren API-Call einbauen (z.B. bei OpenAI als `image_url`-Content-Block).

### `system`-Sub-Slot-Struktur

`system` ist ein anwendungs-definiertes Tree. Leaves sind `{text}`- oder `{text_id}`-Container, analog zu Turn-Objekten/-Pointern. Beispiel:

```json
"system": {
  "identity": {
    "soul": { "text": "Du bist ein..." },
    "body": { "text_id": "01HXY..." }
  },
  "facts": {
    "user_name": { "text": "Alice" }
  },
  "tools": {
    "web_fetch":  { "text": "{\"description\":\"...\",\"parameters\":{...}}" },
    "calculator": { "text_id": "01HXZ..." }
  }
}
```

**Tool-Definitionen sind Spezialfall**: ihre `text`-Werte sind JSON-Strings mit der Tool-Definition (Name, Description, JSON-Schema-Parameters). meclaw-Core kennt das nicht — der LLM-Provider-Adapter parsed das Format. Damit bleibt die `{text|text_id}`-Leaf-Disziplin universal.

**Konkatenation zum Provider-System-String**: bei Inferenz baut der LLM-Adapter aus dem Tree einen einzelnen String, joined mit `\n\n` zwischen Sub-Slots. Reihenfolge: erst die in `params.system_order` gelisteten Sub-Slots (in der dort angegebenen Reihenfolge), danach alle übrigen alphabetisch. Innerhalb von Sub-Trees wird alphabetisch DFS gewalkt. `system.tools.*` ist von dieser Konkatenation **ausgenommen** — Tools werden separat als provider-natives Tool-Set rausgezogen.

### Replace-Semantik

Jeder Slot wird atomar gesetzt — nicht additiv:

- `system.X.Y.Z` updaten: dieser Pfad wird ersetzt, andere Pfade bleiben unverändert (akkumulative-replace pro Pfad)
- `messages[]` updaten: das gesamte Array wird ersetzt
- Wer einen Turn anhängen will, schickt die volle gewünschte Liste

History-Management ist damit **Anwendungs-Sache** (z.B. via dedizierter Memory-Hive-Topologie), nicht Core-Feature. Eine `llm`-Cell, die direkt von einer Proxy gefüttert wird, sieht genau das, was die Proxy sendet — typischerweise einen einzelnen Turn, und antwortet genau auf diesen.

### Blob-Referenzen sind universal

Jeder Blob ist selbst ein vollständiges Body-Dokument. `text_id` referenziert "den Text-Inhalt eines Body-Slots", `messages_id` referenziert "ein Body-Dokument; nimm dessen `messages[]` und expandiere inline". Beide treffen denselben Cache (`blob_uuid → parsed body`) — eine Cell, die einmal einen Blob aufgelöst hat, hält ihn in-memory bis zum Restart. Cache-Invalidierung gibt's nicht, weil Blob-UUIDs immutable sind.

**Rekursion ist erlaubt, aber hart limitiert**: ein Blob's `messages[]` kann selbst `messages_id`-Pointer enthalten, die werden weiter aufgelöst. **Self-Cycles** (ein Blob referenziert sich selbst) sind durch UUID-Immutability ausgeschlossen — die UUID eines Blobs existiert erst nach Inhalts-Fixierung, kann also nicht im eigenen Inhalt stehen. **Wechselseitige Zyklen** (A→B→A über zwei Blobs) sind dadurch **nicht** ausgeschlossen; sie werden — wie pathologische Tiefe generell — durch das harte Tiefen-Limit gefangen. Pathologische Tiefe wird durch ein **hartes Tiefen-Limit** gefangen — Default **64**; das `colony.json`-Feld `blob_max_recursion_depth` ist als Override **vorgesehen, heute aber parsed-but-not-applied** (siehe Abschnitt „`colony.json` — Schema"). Das Tiefen-Limit selbst **und** seine `colony.json`-Verdrahtung entstehen mit dem Roadmap-Defer D-025 (die rekursive Blob-Auflösung hat heute 0 Producer — § Routing-Fehler `blob_recursion_too_deep`, konsistent mit der dortigen „Code heute nicht produziert"-Feststellung). Bei Überschreitung: Resolution-Failure mit `error_code: "blob_recursion_too_deep"`, gehandhabt über die existierende Routing-Cascade (zurück an `reply_to` bzw. nach `/colony/dead_letters`). Resolution-Failures durch nicht-auffindbare Blobs gehen denselben Weg. Verworfen wurden: alleiniger `message_timeout`-Backstop (schlechte Diagnostik — Timeout heißt „zu langsam", nicht „zu tief" — und ressourcen-intensiv vor Erkennung), unbegrenzte Tiefe mit Stack-Overflow als implizitem Backstop (würde die auflösende Cell-Task crashen).

### Cell-Bauarten bzgl. `messages[]`

Cells fallen in zwei Klassen, je nachdem wie sie mit dem eingehenden `messages[]` umgehen — plus einem Sonderfall:

| Bauart | Verhalten | Beispiele |
|---|---|---|
| **Stream-fortpflanzend** | Eingangs-`messages[]` wird durchgereicht + eigener Beitrag (typisch 1 Turn) angehängt. Konversations-Faden bleibt entlang der Kette. | Guardrail/Transform-Hive (modifizieren durchlaufende Turns), Aggregator-Hive |
| **Atomisch-emittierend** | Cell emittiert eine frische `messages[]` mit nur ihrem eigenen Beitrag — kein Pass-Through. Typisch für Quellen (externe Events), Tool-Endpoints und LLM-Inferenz. | `llm` (assistant-Turn allein); `bash`/`web_fetch`/`web_search`/`file`/`edit` (tool_result-Turn); `proxy` (User-Turn aus externem Chat); `timer` (Schedule-konfigurierter Body); `store`/`mcp` (atomare Query/Tool-Response) |
| **Vom Skript bestimmt** *(Sonderfall)* | Die Cell-Bauart entsteht pro Execution aus dem, was das Skript schreibt — kann atomisch sein oder stream-fortpflanzend. Einzige Cell-Type in dieser Klasse. | `code` (programmierbarer Body-Konstruktor, siehe `cell-types.md`) |

Welche Bauart eine Cell ist, gehört zu ihrer Job-Description und steht in `cell-types.md`. Die Entscheidung ergibt sich aus dem Cell-Type, nicht aus der Topologie.

**Konsequenz**: Stream-Ketten sind effektiv **append-only** bezüglich `messages[]`. Atomisch-emittierende Cells brechen die Kette absichtlich — wer den Konversations-Kontext zurück zum LLM bringen will, baut eine **Collector-Topologie**, die Tool-Result-Messages aggregiert und mit dem Konversations-Faden joint (Anwendungs-Pattern, kein Core).

**Multi-Send-Wire-Format**: Cells, die `contract.multi_send_capable: true` deklarieren, dürfen mehrere Output-Messages pro Input emittieren. Das konkrete Wire-Format ist Cell-Type-spezifisch; für `code` siehe Cell-Type-Beschreibung in `cell-types.md`. Jede emittierte Message läuft unabhängig durch die ausgehenden Edges der Cell — Colony evaluiert pro emittierter Message frisch, Routing kann pro Message divergieren.

### Output: Was die Cell emittiert, was sie persistiert

Zwei verschiedene Dinge:

| | In Output-Message? | In `cell.db`? |
|---|---|---|
| Eingangs-`messages[]` (mit Blob-Refs unaufgelöst) | ja, durchgereicht | ja, last-received as-is |
| Eigener neuer Turn (z.B. assistant-Turn aus LLM-Call) | ja, angehängt | **nein** — Output wird nicht zurück in den Cell-State persistiert |
| `system.*` | **nein** — privater Cell-State | ja, akkumulativ-replace pro Pfad |
| `meta`-Slot (Cell-spezifische Metadata) | ja | nein, jeder Call setzt eigene |
| Blob-Cache | nein (in-memory) | nein (re-fetchable beim Restart) |

Damit driftet der Cell-State nicht eigenmächtig: was die Cell hält, kam von außen. Sie schreibt sich keine eigene "Wahrheit" über die Konversation.

### Metadata-Aggregation ist Topologie

Antwort-Metadaten (`tokens_prompt`, `tokens_completion`, `model`) leben im `hop`-Fach und **verfallen** bei der nächsten Cell-Emission — eine `llm`-Cell, die mehrfach im Tool-Loop läuft, produziert bei jedem Call einen frischen `hop`. Es gibt keine kumulierte Header-Sicht über den Loop; jeder Hop trägt nur die Metadaten seines eigenen Calls.

Wer Totals will (kumulierte Tokens, USD-Cost, Latenz-Summe): **separate Aggregator-Hive** in der Pipeline, korreliert über eine Anwendungs-Konvention im `context`-Fach (z.B. `turn_id`), hält Totals in eigener `cell.db` und ergänzt sie. Aggregation ist nie Cell-Type-Verantwortung — sie ist Anwendungs-Topologie. Echte Historie gehört in den `store`, nicht in den Header (siehe § „Atomarität").

---

## Iteration ist Topologie (kein Core-Feature)

LLM-Cells haben **keine innere Schleife**. Sie machen einen Provider-Call, geben das Response als eine Message raus, fertig. Jegliche Iteration (Tool-Loop, ReAct, Plan-and-Execute, etc.) entsteht durch Graph-Topologie und ist **Anwendungslogik**, nicht meclaw-Core.

meclaw bringt **keine** vorgefertigten Tool-Loop-Topologien, Dispatcher-Hive oder Collector-Hive mit. Der Builder (Mensch oder AI) komponiert solche Patterns aus den Grundbausteinen (Hive-Scopes als Gruppierung, `code`-Cells, `store`-Cells, `llm`-Cells). Was meclaw-Core garantiert: die topologische Komposition ist möglich, weil Cells dumm sind und Edges entscheiden.

**Beispiel-Topologie für einen Tool-Loop** (zur Illustration, nicht als Vorschrift):

```
[llm] ──► [dispatcher (code-cell)] ──► fan-out via Edge-Bedingungen
                                  ├─► [proxy]            (intermediate user message)
                                  ├─► [tool-A]           ┐
                                  ├─► [tool-B]           ├─► [collector (code-cell)] ─► [llm]
                                  ├─► [tool-C]           ┘    (gleiche turn_id, nächste Iteration)
                                  └─► [collector]        (expect-Notification)
```

Dispatcher-Hive zerlegt LLM-Output in mehrere typisierte Messages (z.B. Tool-Calls, intermediate Responses, expect-Notifications). Collector-Hive sammelt Tool-Antworten und sendet aggregiert an LLM zurück. Beide leben üblicherweise unter einem `hive`-Scope-Marker, der die Tool-Loop-Sub-Topologie als Einheit gruppiert (z.B. `/main/tool-loop/`).

**Loop-Zähler und Korrelation im Zwei-Fächer-Modell:**

- Der Loop-Zähler (`iter`) lebt als **`context`** (persistent über die Iterationen) und wird auf der **Loopback-Edge** per `modifier.set_context: { "iter": "int(context.iter) + 1" }` hochgezählt (der `int()`-Cast ist nötig, weil CEL einen JSON-Integer als `uint` deserialisiert und `uint + int` nicht definiert ist). Cells fassen `iter` nie an — Hochzählen ist Edge-Authority.
- Die **Collector-Korrelation matcht über Tool-Call-IDs** (Set-Differenz: erwartete IDs ⊆ erhaltene IDs), **nicht über Zählung** — damit ist sie idempotent sowie out-of-order- und duplikat-fest. Die **erwartete ID-Menge lebt im `store`** (echte Historie → `store`), nicht im Header; der Header trägt pro Tool-Result nur dessen `tool_call_id` (hop-lokal).

**Routing-Bedingungen** auf den Edges nutzen reguläre CEL-Expressions auf `context.*`/`hop.*`-Keys (z.B. `hop.msg_type == "tool_call"`). Solche Header-Konventionen sind **Anwendungs-Konventionen** — meclaw-Core kennt sie nicht als Spezialfall.

---

## Template-System

### Template-Definition

- Templates sind Cells (oder ganze Subtrees inklusive Hive-Scope-Markern) unter `templates/`. Ihre Rolle: Klasse / Schablone.
- **Verzeichnisstruktur innerhalb `templates/` ist frei wählbar** — Sub-Ordner, Gruppen, Namespaces sind erlaubt. Der Scanner findet Templates anhand der `template.json`-Datei.
- **Identifikation per Name**, weil Template-interne Graphen stabile Namens-Referenzen brauchen (UUIDs sind erst nach Instanziierung vergeben).
- **Versionierung optional**: Verzeichnis-Name `<name>@<version>` (z.B. `llm-openai@2.1.0/`) oder einfach `<name>/` (gilt als unversioniert).

### `template.json` — Template-Index

Jedes Template hat im Wurzel-Verzeichnis eine `template.json`, die das Template als Klasse beschreibt (getrennt von `config.json`, die die zu instanziierende Cell beschreibt).

```json
{
  "name": "llm-openai",
  "version": "2.1.0",
  "description": {
    "purpose": "...",
    "use_when": "...",
    "not_in_scope": "...",
    "examples": [...]
  },
  "tags": ["llm", "openai", "completion"],
  "author": "@author",
  "license": "MIT",
  "homepage": "..."
}
```

`template.json` beschreibt ausschließlich das Template selbst (Metadaten für Discovery), keine Aussage über inneren Cell-Type-Aufbau.

Die `description` in `template.json` hat genau **vier Slots** (`purpose`, `use_when`, `not_in_scope`, `examples`); die **Sechs-Slot-Form** (zusätzlich `emits_meaning`/`consumes_meaning`) gilt für Cell-`config`-descriptions, siehe `config.md` § `description`. (Ruling 2026-06-10.)

### Templates-Registry (in `colony.db`)

Colony hält eine persistente Registry. Schema:

| Spalte | Inhalt |
|---|---|
| `template_id` | UUID v7 (intern, Primary Key) |
| `name` | aus `template.json` |
| `version` | aus `template.json` oder `NULL` |
| `filesystem_path` | wo das Template liegt |
| `description_json` | gecachter description-Block |
| `tags_json` | gecachte Tags |
| `author` | optional |
| `scanned_at` | Timestamp |
| `embedding` | später, für semantische Suche |

### Scan-Strategie

- **Beim Start**: Registry wird aus `colony.db` geladen, **kein Filesystem-Scan**. Schneller Start.
- **Erstmaliger Start (leere Registry)**: automatischer Scan.
- **Manueller Rescan** via CLI-Flag `meclaw --rescan-templates` oder API `POST /colony/templates/rescan`.
- **Rekursiv, ohne Ausschluss**: `scan_templates_dir` (`crates/meclaw-colony/src/templates/scanner.rs`) walkt den ganzen Baum unter `templates/` — **jedes** Verzeichnis mit einer `template.json` wird registriert, unabhängig von Tiefe und Elternnamen. `templates/drafts/<name>/` ist damit kein Entwurfsraum, sondern voll instanziierbar (gelistet **und** per `add_nodes` zu einer aktiven Cell instanziierbar). Entwurfs- und Staging-Material gehört deshalb **nicht** unter `templates/`; Builder-Staging liegt in `<root>/staging/` und wird per `rename(2)` promotet.

### Auflösung `name@version`

Im Graph referenziert per:

```json
"template": "llm-openai"           // → höchste verfügbare Version
"template": "llm-openai@2.1.0"     // → exakt diese Version
```

- Ohne Version: höchste SemVer-Version. Unversionierte Templates gelten als kleiner als alle versionierten.
- SemVer-Ranges (`^`, `~`) erst Post-Roadmap (Marktplatz-Relevanz).

### Verhalten bei Fehlern

- **Template referenziert, aber nicht in Registry**: Instanziierung schlägt fehl, Fehler-Message an `reply_to` (falls gesetzt), Mutation wird rejected. Colony loggt. Batch-Mutation: gesamter Batch rejected.
- **Registry-Eintrag vorhanden, aber Verzeichnis weg** (z.B. manuelles `rm -rf`):
  - Lazy-Check beim Instanziierungs-Versuch → Fehler + automatisches Entfernen aus Registry.
  - Bei `--rescan-templates`: alle Registry-Einträge ohne Verzeichnis werden gelöscht.
  - Existierende Instanzen, die das Template referenziert hatten, laufen weiter (sie haben ihre eigene Filesystem-Kopie).

### Instanziierungs-Ablauf (Colony)

1. Colony empfängt eine Mutation-Message an `/colony/mutations`, in der ein `add_nodes`-Eintrag eine zu instanziierende Cell beschreibt (Felder: `name`, `template`, optional `override_params`).
2. Lookup in Registry: `template_ref → filesystem_path`.
3. Kopiert `templates/<path>/` rekursiv ins Staging-Verzeichnis (`.staging/<mutation_id>/<name>/`).
4. Generiert neue UUID v7 für alle kopierten Cells und Edges.
5. Patcht `config.json` mit den neuen UUIDs. **Name bleibt wie im Template (bzw. wie in `override_params` angegeben)** — bei Kollision mit Geschwister-Namen innerhalb desselben Scopes wird die Mutation rejected, siehe „Naming-Kollisionen" unten.
6. Führt `${VAR}`-, `${ctx.*}`- und `${uuid7:*}`-Substitution durch (siehe „Variablen-Substitution").
7. Initialisiert `cell.db` aus `seed/`, falls vorhanden.
8. Atomarer `rename(2)` aus Staging an Ziel-Pfad.
9. Registriert Instanz in Colony's `HashMap<Path, ActorHandle>` und spawnt die Aktor-Task: bei **stateful** Cells die `cell_task`-Loop, bei **stateless** Cells die `stateless_dispatcher`-Loop (mit `Semaphore` aus `params.max_concurrency`), bei **long-running** Cells das Doppel-Task-Pattern (Handler + I/O). In allen drei Fällen wird die Mailbox als bounded mpsc allokiert (Default-Kapazität 1000, überschreibbar via `cell.mailbox_size` ab Phase 5).
10. Übergibt `params` an die Instanz beim Start.

### Discovery (für AI-Builder)

`GET /colony/templates` liefert Template-Liste aus der Registry mit:
- `name`, `version`, `template_id`
- Voller `description`-Block
- `tags`
- (später) Vector-Embedding für semantische Suche

Phase 11: plain text matching + tag-Filter. Post-Roadmap: Embedding-Index, Vector-Search via API.

### Lifecycle von Templates

- **Templates sind read-only Klassen und werden nie automatisch entfernt** — sie sind Bibliotheks-Vorlagen und stehen für zukünftige Instanziierungen bereit, auch wenn aktuell keine Instanz sie referenziert.
- **Manuelle Entfernung** ausschließlich über das Filesystem: Template-Verzeichnis in `templates/` löschen, danach `--rescan-templates` (oder `POST /colony/templates/rescan`), damit die Registry den Stand übernimmt.
- **Kein dediziertes Tooling** (kein CLI-Subcommand, keine API-Endpunkte für Cleanup) — Templates sind Bibliotheks-Material, die FS-Disziplin reicht.

---

## Seed-Konzept (JSONL-Format)

DBs werden **nie als binäre Dateien** in Templates abgelegt — stattdessen versions-sicheres JSONL.

```
<cell>/seed/<table>.jsonl
```

Format:
```
{"schema": {"col1": "text", "col2": "int", "col3": "json"}}
{"col1": "value", "col2": 42, "col3": {...}}
{"col1": "value2", "col2": 43, "col3": {...}}
```

- **Zeile 1**: Schema-Deklaration.
- **Zeilen 2+**: Datensätze.
- **Bei fresh-`cell.db`-Creation** (`OpenStatus::Created`, siehe § Lifecycle): Colony liest Seed, baut `cell.db` neu. Beim Re-Open einer existierenden `cell.db` (`OpenStatus::Resumed`) wird **nicht** re-seeded — sonst doppelte Rows.
- **Export**: Cell empfängt `EXPORT`-Message, schreibt aktuellen DB-Stand als JSONL in `seed/`. Dateiname: UUIDv7 oder `YYYY-MM-DD_<counter>.jsonl`.
- **Vorteil**: kein binärer DB-Schema-Drift, grep-bar, append-friendly.

---

## Variablen-Substitution

meclaw kennt **drei Substitutions-Quellen**, alle mit `${...}`-Syntax. Wo welche zulässig sind und wer substituiert:

| Token | Quelle | Wer substituiert | Wann |
|---|---|---|---|
| `${ENV_VAR}` | aus `.env` im Root | Colony | beim Lesen von `config.json` (Instanziierung) und in Mutation-Diffs (Mutation-Validation) |
| `${ctx.<key>}` | aus Header/Body der **Mutation-Message** selbst | Colony | bei Mutation-Anwendung |
| `${uuid7:label}` | frisch generiert pro Label | Colony | bei Mutation-Anwendung |

Alle drei Quellen werden ausschließlich von Colony substituiert — das flache Substrat hat keine zwischengeschaltete Schicht, die ihre eigenen Tokens hätte.

### `${ENV_VAR}` aus `.env`

- `.env`-Datei im Root: klassisches Key=Value-Format.
- Substitution durch die Colony, bevor `params` an die Cell übergeben werden. Cell sieht nur den substituierten Wert.
- **POSIX-Style-Default** unterstützt: `${VAR:-fallback}` liefert `fallback`, wenn `VAR` leer oder ungesetzt ist. `${VAR}` ohne Default ist strict — fehlt die Variable, gibt's einen Fehler (siehe Fehler-Verhalten unten).
- **Escape:** `$${...}` escaped zu literalem `${...}`; Substitution läuft ausschließlich bei der Instanziierung.
- Strict-Variante `${VAR:?error_msg}` (bash-style) **nicht** unterstützt; jede andere `${VAR<op>...}`-Form außer `${VAR}` und `${VAR:-fallback}` wird mit `unsupported_substitution` abgelehnt (kein stilles Durchreichen).

### `${ctx.<key>}` aus Mutation-Kontext

- Nur in Mutation-Diffs zulässig (nicht in `config.json` auf der Filesystem-Seite).
- Zugriff auf den `ctx`-Block der Mutation-Message: `${ctx.user_id}` → Wert des `ctx.user_id`-Feldes. Die Resolution ist **strict** aus dem `ctx`-Block der Mutation — kein Fallback auf andere Quellen; fehlender Key → Reject mit `ctx_key_missing` (siehe `error_code`-Enum).
- Erlaubt dem Auftraggeber, anwendungs-eigene Identifier (`user_id`, `session_id`, `turn_id`) in Namen und `override_params` zu injizieren — er legt sie dafür **explizit in den `ctx`-Block** (kein automatisches Lesen aus dem `headers.context`-Fach der Mutation-Message).

### `${uuid7:label}` frische UUIDs

- Generiert eine UUID v7 beim ersten Vorkommen eines Labels in einer Mutation. Alle weiteren Vorkommen **desselben Labels** im gleichen Diff bekommen denselben Wert.
- Verschiedene Labels → verschiedene UUIDs.
- Labels sind frei wählbar (`sess`, `s1`, `worker_a`, ...) und nur innerhalb der einen Mutation-Message gültig — nach Mutation-Abschluss vergessen.
- **Form ohne Label (`${uuid7}` plain) existiert nicht** — explizite Labels sind Pflicht für eindeutige Semantik (verhindert Foot-Gun, wo jedes Vorkommen ungewollt eine neue UUID wäre).

Beispiel mit allen drei Quellen kombiniert:

```json
{
  "scope": "/main",
  "diff": {
    "add_nodes": [
      {
        "name":     "session_${uuid7:s}",
        "template": "session-scope@1.0.0",
        "override_params": {
          "user_id": "${ctx.user_id}",
          "api_key": "${OPENAI_KEY}"
        }
      },
      { "name": "worker_${uuid7:w}", "template": "worker@1.0.0" }
    ],
    "add_edges": [
      { "from": "./dispatcher",            "to": "./session_${uuid7:s}" },
      { "from": "./session_${uuid7:s}",    "to": "./worker_${uuid7:w}"   },
      { "from": "./worker_${uuid7:w}",     "to": "./collector"           }
    ]
  },
  "ctx": { "user_id": "alice" }
}
```

### Fehler-Verhalten

| Fehler | Wann gefangen | Reaktion |
|---|---|---|
| Fehlende `${ENV_VAR}` ohne Default beim initialen Colony-Bootstrap | vor Pipeline-Start | Daemon failed-to-start, Exit-Code != 0, Error auf stderr/log |
| Fehlende `${ENV_VAR}` ohne Default bei Mutation-Validation | Mutation-Validation | Fehler-Reply an `reply_to` (falls gesetzt), Mutation rejected |
| Fehlende `${ctx.<key>}` | Mutation-Validation | Fehler-Reply an `reply_to`, Mutation rejected |
| Cell-Init-Folgefehler durch invaliden substituierten Wert (z.B. ungültiger API-Key) | Cell-Init nach Commit | Restart one_for_one, nach N Retries `failed`-Status |
| `${uuid7:label}` | nie fehlend (immer generiert) | — |
| Name-Kollision im `post_state` nach Substitution | Mutation-Validation | Fehler-Reply an `reply_to`, Mutation rejected (siehe „Naming-Kollisionen") |

### Naming-Kollisionen

Strict-Default: wenn eine Mutation einen Node-Namen erzeugt, der im `post_state` innerhalb desselben Scopes doppelt vorkommt, wird die ganze Mutation rejected (`error_code: "naming_collision"` in der Fehler-Reply). Kein Auto-Suffix, kein Pfad-Magic. Wer Bulk-Instanziierung mit Eindeutigkeits-Garantie braucht, nutzt `${uuid7:label}` oder `${ctx.<key>}` mit anwendungs-stabilen Tokens.

**Auftraggeber-Discovery** nach der Mutation: falls der Auftraggeber den resolved Namen kennen muss und kein anwendungs-stabiler Token zur Verfügung steht, gibt es zwei Wege:
1. Selbst die UUID generieren (außerhalb der Mutation) und als Literal einsetzen — Auftraggeber kennt den Namen vorher.
2. Nach der Mutation `/colony/registry` (per HTTP `GET /instances?path=...` oder per Message an `/colony/registry`) abfragen — Registry liefert für jede Instanz `id`, `name`, `path`, `type`, `status`. UUIDs sind zeitsortiert (v7), neueste Cells finden sich am Listenende.

---

## Blob-Storage

### Layout

Blobs leben im `blobs/`-Verzeichnis (Default `{root}/blobs/`, CLI-überschreibbar via `--blobs`). Jeder Blob besteht aus **zwei Dateien**:

```
blobs/<uuid-v7>.<ext>            # Blob-Inhalt
blobs/<uuid-v7>.<ext>.meta.json  # Sidecar mit authoritativen Metadaten
```

- **`<uuid-v7>`** ist die Blob-ID, zeitsortiert.
- **`<ext>`** ist die native Datei-Extension, abgeleitet aus dem MIME-Type. Aktuell (Phase 3+): `.json` für ausgelagerte UBF-Bodies. Ab Phase 12+: `.pdf`, `.txt`, `.png`, `.jpg`, weitere — je nach `attachments[]`-Slot-Konvention (siehe „Body-Format (Universal)").

**Sidecar-Schema** (`.meta.json`):

```json
{
  "schema_version": 1,
  "mime_type":      "application/json",
  "size_bytes":     123456,
  "sha256":         "abc...",   // optional; in Phase 12 weggelassen (kein Consumer)
  "created_at":     "2026-05-19T10:23:45Z",
  "filename":       null
}
```

- `mime_type`: authoritative MIME-Info. Konsumenten lesen das Sidecar, nicht die Extension (Extension ist nur Operator-Convenience für `ls blobs/`).
- `filename`: ursprünglicher Filename bei Upload via HTTP-API (z.B. `"report.pdf"`); `null` bei system-generierten Blobs.
- `sha256` (optional): Content-Hash für Integritäts-Checks und Dedup-Potential (beide post-Roadmap). **In Phase 12 nicht berechnet** (kein Consumer) — das Feld darf fehlen; ein Recompute-Pass kommt konditional, falls je ein Dedup-Pfad landet.
- `schema_version`: für künftige Sidecar-Erweiterungen ohne Migration-Bruch.

### Verhalten

- **Schwelle** (Default 64 KB) für Auslagern konfigurierbar via `blob_inline_max_bytes` in `colony.json`.
- **Beim Schreiben**: UBF-Body ≥ Schwelle → wird als `blobs/<uuid>.json` ausgelagert, Sidecar wird mit-geschrieben, in der Message bleibt nur `Blob(uuid)`. (Der `==`-Grenzfall ist **inklusiv** — der `Body`-Enum implementiert kanonisch `≥`; die Prosa ist hier daran angeglichen.)
- **Bei Anhängen** (ab Phase 12+): über die HTTP-API hochgeladene Files (`multipart/form-data`) werden als `blobs/<uuid>.<ext>` mit echtem MIME-Type abgelegt. Die zugehörige Message trägt einen `attachments[]`-Slot-Eintrag mit `{blob_id, mime_type, filename, size_bytes, sha256}` (`sha256` optional). Schreib-Reihenfolge: erst die Blob-Datei (`tmp` → `rename(2)`), dann das Sidecar (`.meta.json`) als **Commit-Marker** — ebenfalls per atomarem `rename(2)`. Reader-Konvention: ein Blob gilt als vollständig genau dann, wenn sein Sidecar existiert; Blobs ohne Sidecar werden ignoriert. (Phase-13-Reader hängen an diesem Vertrag.)
- **Beim Lesen durch Cell**: Cell konsumiert ein `attachments[]`-Element oder einen `Blob(uuid)`-Body und ruft eine Storage-Abstraktion auf, die das Sidecar mitlädt und Inhalt + MIME-Info zurückgibt. JSON-Bodies werden für die Cell weiterhin transparent als `serde_json::Value` deserialisiert.
- **Kein automatisches GC** — Blobs fallen unter die No-Delete-Policy wie der Rest von `{root}/`. Disk-Space-Management ist Operations-Sache (externes Archivieren via rsync, tarball, S3 etc.).

### Phasen-Anbindung

| Phase | Was |
|---|---|
| 3 | UBF-Body-Auslagerung als `blobs/<uuid>.json` + Sidecar (MIME `application/json`). Layout ist zukunftsfähig, aber praktisch nur JSON-Blobs |
| 12 | Echte Anhänge: `multipart/form-data`-Upload via HTTP-API, native Extensions (`.pdf`, `.txt`, `.png`, `.jpg`), Operator-Web-UI zeigt Anhänge im Trace-View, `attachments[]`-Body-Slot aktiv |
| 13+ | Cell-Type-spezifische Konsumenten (LLM mit Vision, `code`-Cell mit File-Processing, `store`-Cell mit File-Indexierung) |

---

## No-Delete-Policy (Event-Sourcing auf Filesystem-Ebene)

- **Keine Datei in `{root}` wird je gelöscht oder verschoben.** Es entstehen nur neue Dateien/Verzeichnisse.
- **Instanzen sind unsterblich**: Eine einmal instanziierte Cell bleibt für immer auf dem Filesystem, behält ihre `cell.db`, ist via UUID auffindbar.
- **Disconnect statt Delete**: Nicht mehr gebrauchte Cells verlieren ihre Edges
  (`remove_edges`/`remove_nodes`) und werden dadurch inaktiv — nicht mehr geroutet, keine
  Tasks. Sie existieren weiter auf dem Filesystem und in `colony.db` und lassen sich jederzeit
  über `add_edges` (oder erneutes `add_nodes` am selben Pfad) wieder anschließen — mit
  derselben `cell_id` und resumter `cell.db` (siehe „Konnektivität & Aktivität").
- **Pfade sind ewig stabil**: zentraler Vorteil für die „Cells kennen keine Topologie, aber Pfade sind verlässlich"-Disziplin.
- **Hierarchie als Builder-Disziplin**: Auslöser der Instanziierung (Builder, CLI, API) wählt Pfad und Name bewusst, um Root-Verzeichnis-Pollution zu vermeiden (z.B. `memory/2026-05-16_user_xyz/`).
- **Audit-Trail eingebaut**: jeder je gelaufene State, jedes Message-Log ist erhalten.
- **Backup-Strategie trivial**: das ganze `{root}` ist ein Snapshot, Git-fähig.
- **Operations-Sorge, nicht Core**: Sehr alte Verzeichnisse können extern archiviert werden (rsync, tarball, S3 etc.) — meclaw selbst beteiligt sich daran nicht.
- **Carve-out (Spawn-Reject-Residue)**: No-Delete gilt absolut für **registrierte** Cells. Die **einzige** Ausnahme ist das Aufräumen **frischer, nie-registrierter** Verzeichnisse beim Spawn-Reject (`sweep_reject_residue`, `crates/meclaw-colony/src/colony.rs`): eine gerade aus dem Staging in den Live-Tree umbenannte `add_nodes`/`swap`-Dir, deren Spawn fehlschlägt, wird entfernt — sie war nie eine lebende, registrierte Cell. Adoptions-Targets (`adopt` — eine vom Builder vor-platzierte Dir mit eigener `cell.db`) sind durch den `preexisting_target`-Guard geschützt und werden **nie** gelöscht.

---

## Startup-Algorithmus

1. Colony startet mit `{root}` (default CWD oder `--root`).
2. **Mutation-Recovery**: Colony scannt `colony.db` nach Mutations-Einträgen mit Status `in_flight` (in-flight beim letzten Crash unterbrochen). Pro Eintrag: Staging-Verzeichnis `{root}/.staging/<mutation_id>/` löschen (falls vorhanden), Mutation als `failed` markieren mit `failure_reason: "crash_during_commit"`. Bereits an ihre finalen Pfade renamete Cell-Verzeichnisse bleiben als Orphans im Live-Tree (No-Delete-Policy gilt) — Colony berücksichtigt sie beim Filesystem-Bootstrap (Schritt 4) anhand der `config.json`. Zusätzlich werden `.staging/<mutation_id>/`-Verzeichnisse ohne zugehörigen `colony.db`-Eintrag mit aufgeräumt. Mechanismus und Trade-offs: siehe Abschnitt „Filesystem-Layout" → `.staging/`.

   **Bootstrap-Recovery (Erst-Apply)**: Der Erst-Apply schreibt VOR dem ersten Cell-Spawn einen durablen `bootstrap_in_flight`-Marker in die `meta`-Tabelle der `colony.db`; die Löschung läuft atomar in derselben Transaktion wie das `InitialApply`-Bundle (edges + hive_scopes) am Apply-Ende. Findet die Boot-State-Klassifikation den Marker vor, war der letzte Erst-Apply unterbrochen (Crash zwischen den per-Cell-Registry-Upserts und dem Bundle): der Boot wird als **FirstBoot** klassifiziert und der Apply läuft als idempotentes Resume erneut — deterministischer Rebuild aus dem Filesystem (das FS ist die Quelle; Registry-Upserts sind `cell_id`-stabil via Identity-Overlay, das Bundle ist `INSERT OR IGNORE`). Kein Operator-Eingriff, kein „DB löschen". Ein Tabellen-Mischzustand (z.B. registry non-empty, edges/hive_scopes leer) **ohne** Marker bleibt `Inconsistent` (externe Korruption, Strict-Fail-Boot-Panic).
3. **Templates-Registry**: Colony liest die Templates-Registry aus `colony.db`. Falls leer oder `--rescan-templates`: Scan von `templates/`.
4. **Registry-Rehydration + Filesystem-Validierung**: Colony rehydratisiert die Registry aus
   `colony.db` — bekannte Pfade behalten ihre persistierte `cell_id` und ihren
   Aktiv-/Inaktiv-Status. Der rekursive Tree-Walk validiert den Filesystem-Stand gegen den
   persistierten Stand. **Registrierung erfolgt ausschließlich durch Instanziierung/Mutation,
   nie durch Boot-Entdeckung (A5b):** Beim **ersten Bootstrap** ist der Walk die Quelle — jeder
   unbekannte `config.json`-Knoten wird als Neueintrag erfasst. Bei einem **Reboot** dagegen wird
   ein unbekannter (im persistierten Registry-Stand fehlender) `config.json`-Knoten — etwa ein
   manuell angelegtes Verzeichnis — **nicht adoptiert, sondern nur gemeldet** (Konsistenz-Sicht:
   WARN im ops-log; in `--validate` als Warnung gelistet, Exit 0, mit `--strict` als Fehler). Ein
   solcher Knoten wird zum registrierten Teil des Graphen erst durch eine Mutation auf seinen Pfad
   (Adoptions-Pfad „2b", siehe § Mutation-Format). Für keinen bereits bekannten Pfad
   wird eine neue `cell_id` vergeben. Hive-Scope-Marker: `params.graph`-Hint lesen und als
   deklarative Edges für den Scope eintragen (sofern noch nicht in `colony.db` persistiert).
   **Abgeleitete Aktivität ab dem ersten Bootstrap (die-eine-Regel):** Der erste Bootstrap wendet
   dieselbe Aktivierungs-Regel an wie der Mutations-Recompute (§ Konnektivität & Aktivität): die
   Berechnung wird aus den `params.graph`-Edges geseedet (wie eine Mutation aus ihrem
   `involved`-Set), und **nur die davon erreichten** neu erfassten Knoten werden auf ihren
   edge-abgeleiteten Zustand gebracht — **Inseln** (Sub-Hives, deren interne Edges ihren eigenen
   Scope seeden) booten so **inaktiv**, ihre Dauerläufer spawnen nicht. Ein **nicht erreichter**
   Knoten (eine edge-lose Single-Cell) behält seine Instanziierungs-Aktivität (**Grace**, aktiv) —
   symmetrisch zur Mutationszeit, kein pauschales initial-active und keine Boot-only-Sonderregel.
   Die Wurzel `/` ist per Definition immer aktiv; ein bereits bekannter Knoten behält seinen
   persistierten Status (Reboot).
5. **Edge-Tabelle hydratisieren**: Colony liest die persistierte Edge-Tabelle aus `colony.db`. Bei Konflikten zwischen `params.graph`-Hints und persistierten Edges gewinnt der persistierte Stand (Hints sind nur initialer Soll-Stand bei Erst-Instanziierung).
6. **Long-Running-Cells spawnen**: für jeden **aktiven** `proxy`/`timer`/`mcp` direkt das
   Doppel-Task-Pattern starten (kein Lazy-Wake). Inaktive Long-Running-Cells werden nicht
   gestartet.
7. **Mailbox-Pumpen starten**: Colony startet ihre eigene Routing-Loop (Mailbox-Consume). HTTP-API und Web-UI binden auf der `--api <bind>`-Adresse, falls das Flag gesetzt ist; sonst ist die HTTP-Schicht inaktiv.
8. Colony hochgefahren.

**Boot-Endpoint-Existenz-Check:** Vor dem ersten Cell-Spawn prüft die Colony, dass jeder
`params.graph`-Edge-Endpoint **auflösbar** ist — gegen den Filesystem-Plan (Cells + Hives), die
**bereits laufende Registry** (zur Laufzeit vor dem Bootstrap registrierte Cells) **oder** einen
`/colony/*`-Endpunkt. Ein Endpoint, der auf nichts davon zeigt, ist eine tote/vertippte Kante und
führt zu einem **lauten Boot-Fail** (präzise Meldung: Edge-Id + fehlender Pfad) statt einer still
ignorierten toten Edge. **`--validate`** (statischer Dry-Run ohne laufende Colony) kann
laufzeit-gespawnte Cells prinzipiell nicht sehen und meldet einen nicht-auflösbaren Endpoint
daher als **Warnung** (Exit 0, nginx-`-t`-Rolle); das **`--strict`**-Flag hebt diese Warnungen auf
**Fehler** an (Exit ≠ 0) — der Operator entscheidet, nicht die interne Logik.

Da alle Instanzen persistent sind, ist dieser Start sehr schnell: Cells werden nicht neu erzeugt, sondern aus dem existierenden Filesystem wiederhochgefahren (`cell.db` wird gelesen, Tasks ggf. gespawnt — siehe Hot/Cold-Cell-Modell).

---

## Konnektivität & Aktivität (Active/Inactive)

Jeder Knoten des Graphen — Cell wie Hive — ist zu jedem Zeitpunkt **aktiv** oder **inaktiv**.
Der Zustand ist **vollständig aus der Edge-Tabelle abgeleitet**; es gibt keinen eigenen
Aktivierungs- oder Deaktivierungs-Befehl im Mutation-Surface.

**Konnektivitäts-Regel**: Ein Knoten ist **verbunden**, wenn er auf seiner Ebene — also im
umschließenden Scope — an mindestens einer Edge beteiligt ist, als `from` **oder** als `to`.
Eine einzige ein- oder ausgehende Edge genügt; Quellen wie `timer`/`proxy` (per Design ohne
eingehende Edges) sind über ihre ausgehenden Edges verbunden.

**Hive-Schärfung:** Für die Konnektivität eines **Hives** zählen **ausschließlich externe
Edges** — in zwei Formen: (a) Edges der Eltern-Ebene, die den **Hive-Pfad** als `from` oder
`to` referenzieren, und (b) Edges, die die **Scope-Grenze des Hives kreuzen** — genau ein
Endpoint liegt strikt innerhalb des Hive-Subtrees, der andere außerhalb (Tiefen-Port-
Verdrahtung, z. B. `/anchor → /unit/dispatch`; R12-Ruling 2026-06-11). Die **interne**
Verkabelung des Hive-Subtrees (beide Endpoints innerhalb) ist für die Hive-Konnektivität
**bedeutungslos**: ein Hive mit noch so reich verdrahtetem Innenleben, aber ohne eine einzige
externe Edge, ist **unverbunden** (und damit inaktiv, samt seinem gesamten Subtree). Umgekehrt
hält schon eine einzige externe Edge — referenzierend oder kreuzend — den Hive verbunden,
unabhängig davon, ob intern etwas verkabelt ist.

**Aktivitäts-Regel (rekursiv)**: Ein Knoten ist **aktiv** genau dann, wenn er selbst verbunden
ist **und** sein Eltern-Hive aktiv ist. Die Wurzel ist per Definition immer aktiv. Damit gilt:
ein disconnecteter Hive deaktiviert seinen **gesamten Subtree**, unabhängig von dessen
interner Verkabelung.

**Invariante (Task ⇔ aktiv)**: Tokio-Tasks laufen ausschließlich für aktive Cells.
Long-Running-Cells (`proxy`/`timer`/`mcp`) laufen genau dann, wenn sie aktiv sind. Stateful
Cells folgen innerhalb von „aktiv" zusätzlich dem Hot/Cold-Modell (lazy Wake) — die beiden
Achsen sind orthogonal (siehe § Hot/Cold-Cell-Modell).

**Die-eine-Aktivierungs-Regel (event-getriebene Ableitung, Boot wie Mutation):** Die Aktivität
eines Knotens ist das **Ergebnis der letzten Konnektivitäts-Berechnung, die ihn erreicht hat**;
ein nie erreichter Knoten behält seine **Instanziierungs-Aktivität**. Diese **eine Regel** gilt
identisch in beiden Kontexten — das ist der Punkt: der **erste Bootstrap** seedet die Berechnung
aus den `params.graph`-Edges (wie eine Mutation aus ihrem `involved`-Set) und rechnet **nur die
davon erreichten** Knoten neu; eine **Mutation** seedet aus den Diff-Edge-Endpunkten. Greift, sobald
ein Konnektivitäts-Recompute den Scope eines Knotens erreicht. **Frisch instanziierte Knoten
starten aktiv** und werden vom ersten Recompute, der ihren Scope berührt, auf ihren
edge-abgeleiteten Zustand gebracht: bei einem **Subtree**-`add_nodes` (oder einer **Insel** beim
Boot) seeden die internen Edges den Recompute über den eigenen Scope, sodass inaktiv-abgeleitete
Subtree-/Insel-Knoten gar nicht erst eager spawnen. Bei reinem **Single-Cell**-`add_nodes` **ohne**
Edge — und symmetrisch bei einer **edge-losen Single-Cell beim Boot** — fehlt der Recompute-Trigger;
der Knoten bleibt mangels Auslöser **aktiv** (Grace). Das ist gewollt: ein edge-loser Knoten erzeugt
**keinen** transienten Spawn-then-Stop, weil der Recompute ihn nie erreicht. **Randfall (bewusst
symmetrisch):** eine edge-lose Single-Cell **innerhalb** eines unverbundenen Sub-Hive behält
ebenfalls die Grace (kein Edge-Seed in ihrem Scope) — sollte diese Rest-Grace je stören, wird sie
auf **beiden** Pfaden gleichzeitig geändert, **nie** boot-only. (Der Randfall —
Single-Cell-`add_nodes` einer Long-Running-Cell, deren Diff-Edges sie inaktiv ableiten — ist
behoben: das Aktivitäts-Gate vor dem Eager-Spawn wertet die POST-STATE-Edge-Sicht aus und
registriert die Cell inaktiv ohne Task-Spawn (Paket-3 P3-C1).)

**Disconnect** (die letzte Edge eines Knotens wird entfernt, typisch via `remove_edges` oder
`remove_nodes`):

- Colony berechnet nach jeder Mutation die Konnektivität des betroffenen Scopes neu und
  markiert disconnectete Knoten — bei Hives einschließlich des gesamten Subtrees — als
  **inaktiv**. Die Markierung wird in `colony.db` persistiert.
- Laufende Tasks enden graceful: ein laufender `handle()`-Call läuft zu Ende, danach endet die
  Task. Bei Long-Running-Cells werden Handler- und I/O-Task gestoppt (externes Polling endet).
  Blockiert die Cell während des Disconnects auf vollem `outputs`, gilt der `term_timeout`-Reject
  (atomarer Rollback); Drain-Unterstützung während des Disconnect-Fensters ist post-v0.1.0.
- Restbestand in der Mailbox einer deaktivierten Cell läuft in die Dead-Letter-Queue mit
  `error_code: "cell_inactive"`.
- Registry-Eintrag, Filesystem, `cell.db` und `cell_id` bleiben vollständig erhalten —
  Disconnect ist Stilllegung, keine Löschung (No-Delete-Policy).
- Inaktive Knoten nehmen nicht am Routing teil: jede Routing-Entscheidung auf einen inaktiven
  Pfad geht in die Dead-Letter-Queue mit `error_code: "cell_inactive"`. Bewusst **nicht**
  `unresolved_path`: der Pfad existiert, ist nur stillgelegt — die Unterscheidung ist
  Builder-Observability (stillgelegter Knoten vs. Tippfehler-Pfad).

**Reconnect** (ein Knoten erhält wieder eine Edge, typisch via `add_edges` oder erneutes
`add_nodes` am existierenden Pfad):

- Der Knoten — und rekursiv sein Subtree, soweit dieser intern verbunden ist — wird wieder
  als aktiv markiert.
- Long-Running-Cells des reaktivierten Subtrees werden **sofort** gestartet (wie beim
  Colony-Startup, Schritt „Long-Running-Cells spawnen").
- **Stateful**-Cells starten **lazy** beim ersten Message-Empfang (Hot/Cold-Modell,
  Wake-on-Message). **Stateless**-Cells starten — wie Long-Running — **eager** (sofort
  beim Reconnect): sie haben keinen Wake-Pfad, „lazy stateless" ist nicht repräsentierbar.
- Jede `cell.db` wird resumed (M1 Resume-mit-State) — keine Re-Initialisierung, `cell_id`
  unverändert, `config.json` wird nicht neu geschrieben.

**Insel-Aktivierung (offizieller Weg).** Eine **Insel** — ein Subtree/Sub-Hive, der beim Boot
mangels externer Edge inaktiv abgeleitet wurde (§ Hive-Schärfung; intern beliebig verkabelt,
aber unverbunden zur Eltern-Ebene) — wird ausschließlich über eine **`add_edges`-Mutation
aktiviert, die eine die Scope-Grenze kreuzende Edge in die Insel einführt** (eine *Crossing-In-Edge*:
genau ein Endpoint liegt innerhalb des Insel-Subtrees, der andere außerhalb — ein Intermediate-Hive
braucht diese kreuzende Eingangs-Edge, um überhaupt verbunden zu werden, eine rein interne Edge
genügt nicht). Diese Mutation seedet den Konnektivitäts-Recompute über den Insel-Scope; die
Aktivierung kaskadiert von dort rekursiv durch den intern verbundenen Subtree (K-H5-bewiesen).
Das ist der **einzige** sanktionierte Aktivierungs-Pfad; der frühere Runbook-Trick „materialisierten
Instanz-Subtree per Re-Root booten" (daily-digest) ist damit **ersetzt** — eine Topologie aktiviert
Inseln durch Verdrahtung, nicht durch Re-Rooting.

**Sichtbarkeit**: `/colony/registry` zeigt inaktive Knoten weiterhin an (Feld
`active: true|false` pro Eintrag); optionaler Filter `?active=true|false`.

**Begründung des abgeleiteten Zustands**: Aktiv/Inaktiv aus der Edge-Tabelle abzuleiten hält
das Mutation-Surface minimal (keine neuen Ops), macht den Zustand jederzeit aus dem Graph
rekonstruierbar und verhindert Drift zwischen „deklariert deaktiviert" und „tatsächlich
unverkabelt". Verworfen wurden: ein expliziter `deactivate`-Mutation-Op (zweite Wahrheit neben
der Edge-Tabelle) und Erreichbarkeits-Berechnung per Graph-Traversierung von der Wurzel
(teurer und strenger als nötig — die lokale Edge-Beteiligung plus Eltern-Kette genügt und ist
O(1) pro Knoten prüfbar).

---

## Hot/Cold-Cell-Modell (Skalierung)

Bei tausenden persistenten Instanzen aber nur wenigen aktiven gleichzeitig: dynamisches Spawnen/Despawnen von Tokio-Tasks.

**Abgrenzung zu Aktiv/Inaktiv**: Das Hot/Cold-Modell gilt nur für **aktive** stateful Cells.
Aktiv/Inaktiv (§ Konnektivität & Aktivität) ist eine orthogonale, edge-abgeleitete und in
`colony.db` persistierte Achse; `NotYetSpawned`/`Awake`/`Asleep` ist der in-memory
Lifecycle-Status innerhalb von „aktiv". Inaktive Cells haben keinen Lifecycle-Status — sie
haben keine Task und werden nicht geroutet.

**Drei Zustände** pro Cell in Colony's Registry-Bookkeeping (gilt für **stateful** Cells; stateless und long-running siehe Klarstellungen unten):

| Status | Bedeutung | Ressourcen |
|---|---|---|
| `NotYetSpawned` | Cell existiert auf FS, nie gespawnt seit Colony-Start | Mailbox-Channel allokiert, kein Task |
| `Awake(JoinHandle)` | Cell läuft als Tokio-Task | Task + Mailbox + cell.db-Connection |
| `Asleep` | Cell hat sich nach Idle-Timeout selbst despawnt | Mailbox-Channel allokiert, kein Task |

**Lifecycle**:
```
NotYetSpawned ──[erste Message]──→ Awake ──[idle timeout]──→ Asleep
                                     ↑                          │
                                     └──[neue Message]──────────┘
```

**Cell-Task-Pattern** (stateful, mit Idle-Timeout + Message-Timeout-Backstop):
```rust
async fn cell_task(
    mut mailbox: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<OutputEnvelope>,
    cell: Box<dyn Cell>,
    message_timeout: Option<Duration>,  // Backstop B, siehe "Timeouts"
) {
    loop {
        tokio::select! {
            Some(msg) = mailbox.recv() => {
                let result = match message_timeout {
                    Some(t) => tokio::time::timeout(t, cell.handle(msg, &outputs_tx)).await,
                    None    => { cell.handle(msg, &outputs_tx).await; Ok(()) }
                };
                if result.is_err() {
                    emit_backstop_timeout(&outputs_tx, /*...*/).await;
                    break;  // Task endet, Supervisor restartet
                }
            }
            _ = tokio::time::sleep(IDLE_TIMEOUT) => {
                if mailbox.is_empty() {
                    cell.shutdown().await;
                    break;
                }
            }
        }
    }
}
```

Operation-Timeouts (A) für I/O leben **innerhalb** von `cell.handle()` — siehe „Timeouts" für die saubere Trennung.

**`IDLE_TIMEOUT`** im Pattern ist konfigurierbar: globaler Default in `colony.json` `idle_timeout_default_ms` (Empfehlung: 60000), pro Cell überschreibbar via `cell.idle_timeout_ms` in `config.json`. Greift nur bei `cell.timeout: 0`.

**`cell.timeout` aus `config.json`** steuert das Verhalten stateful Cells:
- `0` (default): Idle-Timeout-Modell (Awake → Asleep)
- `> 0`: One-Shot (despawn nach jeder Message)
- `-1`: persistent (proxy, timer, mcp — nie despawnen)

**Stateless Cells haben dieses Drei-Zustände-Modell nicht.** Die Dispatcher-Task (siehe Cell-Modell, „Stateless-Cell-Dispatcher") ist permanent awake — sie hält keinen Persistent-State, hat fast keinen Idle-Cost (eine `mailbox.recv().await`-blockierte Task, ~3 KB Stack), und ein Despawn-Wiederspawn-Zyklus würde nichts einsparen außer dem Mailbox-Channel selbst. Der Sleep-/Wake-Mechanismus ist konzeptuell stateful-only, weil er den State-Erhalt zwischen `Asleep` und `Awake` über die `cell.db` voraussetzt.

**Long-Running-Cells** (Doppel-Task) sind permanent awake. Ihre Existenz ist die Daseinsberechtigung — der I/O-Task macht externes Polling, der Handler-Task wartet auf eingehende Events. Idle-Asleep widerspräche dem Zweck.

Tokio-Tasks sind ~3 KB Stack, kein OS-Thread. Tausende schlafende Cells haben praktisch keinen Overhead — nur Mailbox-Channels in Colony's Registry. Nur Awake-stateful-Cells haben relevante Kosten.

**Phasen**: dieses Modell wird in Phase 13 aktiviert. Bis dahin sind alle Cells permanent als Tasks (siehe „Roadmap").

---

## Cell-Robustheit (Supervision, Backpressure, Timeouts)

### Restart-Strategie

- **`one_for_one`** als einzige Strategie, supervised durch Colony. Wenn eine Cell panickt, wird genau diese Cell von Colony's Supervisor neu instanziiert.
- Begründung: Cells sind per Design entkoppelt (kennen keine Topologie), die OTP-Strategien `one_for_all`/`rest_for_one` lösen Probleme, die meclaw architektonisch gar nicht hat.
- State-Erhalt bei Restart: `cell.db` wird neu geladen, in-memory State ist verloren.
- **Restart-Limit**: Default 5 Versuche pro Cell (überschreibbar via `cell.restart_limit` ab Phase 5). Sofortiger Restart ohne Backoff, kein Sliding-Window — eine deterministisch panickende Cell scheitert nach 5 Versuchen schnell und wird in der Registry als `failed` markiert. Routing an eine `failed` Cell läuft in die Dead-Letter-Cascade (Phase 2+). Eine `failed` Cell kehrt über die normale Reconnect-Semantik (Z.1404) zurück, wenn eine Mutation sie direkt adressiert (Edge-Endpunkt oder Resume); beiläufige Recomputes reaktivieren sie nicht. Die Reaktivierung setzt den Restart-Zähler zurück. Verworfen wurden: Exponential-Backoff (löst keine bekannte Pathologie in Phase 1, fügt Test-Determinismus-Komplexität hinzu), OTP-Stil-Sliding-Window (`max_restarts in max_seconds`, gleicher Trade-off). Härtere Garantien (transaktionale In-Flight-Erhaltung) sind Phase-6-Mutations-Thema.
- **Channel-Mechanik bei Restart**: das `mpsc::channel`-Paar einer panickenden Cell überlebt den Panic nicht — der `Receiver` wird beim Stack-Unwind des `cell_task` gedroppt, der bisherige `Sender` in der Registry zeigt damit auf einen geschlossenen Channel (`SendError`). Supervisor erzeugt auf Restart ein frisches `mpsc::channel(1000)`-Paar, ruft die Respawn-Closure mit dem neuen `Receiver` auf (neuer `cell_task`, frischer `JoinHandle`), ersetzt den `Sender` in der Registry atomar. In-flight-Messages auf dem alten Channel — `id=1` (der Panic-Auslöser) **plus alle wartenden Mailbox-Nachrichten `id=2..N`** — gehen mit dem Receiver-Drop **verloren**; die respawnte Cell bekommt eine frische, leere Mailbox. Heutiger Stand, akzeptiert: **der Panic-/Restart-Pfad hat keinen silent-loss-Schutz** (anders als der Live-Backpressure-Pfad, § Backpressure). Die künftige Survival-Garantie — `id=1` bleibt verloren, `id=2..N` **müssen** überleben und in Reihenfolge zugestellt werden — ist als PRE-MVP-Pflicht in `docs/roadmap.md` § „stateful-Cell-Panic: Überleben wartender Nachrichten (id=2..N)" registriert (**nicht** mehr ein Phase-6-Mutations-Replay — der frühere Forward-Ref ist damit aufgelöst).

- **Tripwire (Phase 5+): `handle_cell_died` ist await-frei zwischen RespawnFn-Aufruf und Registry-Sender-Swap.** In der Implementierung ([`crates/meclaw-colony/src/colony.rs`](../crates/meclaw-colony/src/colony.rs), `handle_cell_died`) liegt zwischen `(entry.respawn)()` (RespawnFn-Closure, sync — enthält `build_cell_with_open_db` und `tokio::spawn(cell_task)`) und `entry.handle = ActorHandle::new(...)` (Sender-Swap) **kein `.await`-Punkt**. Die `tokio::select!`-Loop in `colony_task` verarbeitet eine `ColonyMsg::CellDied`-Event-Iteration komplett, bevor sie zur nächsten `inbox.recv()` zurückkehrt — **die serielle Loop ist dadurch die Restart-Ordering-Barriere**. Phase-5-Quieszenz-Tests (Q8/Q9, Counter-Restore-Pflicht) hängen an dieser Barriere: Test wartet auf `spawn_count`-Increment, dann sendet er die nächste Message — sie landet in der Inbox und wird in der nächsten Loop-Iteration empfangen, zu welchem Zeitpunkt der Sender-Swap garantiert erfolgt ist.

  **Phase-6+ Konsequenz**: jede Status-Persistenz (`failed`/`disconnected` in registry-Tabelle, Mutations-Replay-Pfad), die einen `colony_db.send_op(...).await`-artigen Punkt **zwischen RespawnFn und Sender-Swap** einzieht, BRICHT die Restart-Race-Sicherheit — Q8/Q9 + jede vergleichbare Test-Sync-Mechanik flakert. Falls ein await zwingend nötig wird: Restart-Barriere neu bewerten (in_flight-Counter, Cell-Completion-Signal, Test-Harness-Hook), **NICHT durchwinken**.

  Die Inaktiv-Markierung aus § Konnektivität & Aktivität berührt diesen Korridor nicht — sie
  wird ausschließlich im Mutations-Pfad (`handle_mutation`) gesetzt, nie in der
  Restart-Behandlung.

### Mailbox-Größe

- **Phase 1+**: bounded mit Default 1000.
- **Phase 5+**: pro Cell überschreibbar via `cell.mailbox_size` in `config.json`.

Begründung der Phase-1-Wahl: bounded Mailboxes sind eine **Concurrency-Eigenschaft** des Substrats. Sie nachträglich einzuziehen würde gegen die „Concurrency-first"-Architektur-Leitlinie der Roadmap verstoßen (Phase 2–4 würde gegen unbounded Channels entwickeln, mit anderem Race- und Timing-Verhalten als das spätere Production-Substrat). `mpsc::channel(1000)` statt `mpsc::unbounded_channel()` ist eine Code-Zeile Unterschied — kein Bootstrap-Aufwand.

### Backpressure-Strategie

- **`block` ist die einzige Strategie** im gesamten System — keine Cell-, Colony- oder Pfad-spezifischen Overrides. Wenn eine Mailbox voll ist, blockiert der Sender (`mpsc::Sender::send().await`), bis Platz frei wird. Damit propagiert Backpressure rückwärts durch den Graph, **ohne silent message loss auf diesem Live-Backpressure-Pfad** (blockierender Sender statt Drop) — **dieser No-Loss-Scope gilt nicht für den Cell-Panic-/Restart-Pfad**, der wartende Mailbox-Nachrichten verliert (§ Restart-Strategie, „Channel-Mechanik bei Restart"). Kein Drop-Logik, keine Strategie-Auswertung pro Routing-Schritt.
- **Implementierung**: `ActorHandle` ist ein trivialer Wrapper um `mpsc::Sender<Message>`; `handle.send(msg).await` ist eine Zeile. Kein `try_send`-Pfad, keine Branching-Logik, kein Wrapper-Crate.
- **Konsequenz für hängende Cells**: eine ganz tote Cell wird durch den **Message-Timeout** (siehe unten) erkannt — der `handle()`-Call wird abgebrochen, die Cell als crashed markiert, `one_for_one`-Restart greift; die respawnte Cell startet mit einer **frischen, leeren** Mailbox (die noch wartenden Nachrichten der alten Mailbox gehen dabei **verloren** — derselbe Panic-/Restart-Verlust wie unter „Channel-Mechanik bei Restart", künftige Survival-Garantie als roadmap-Posten registriert; **nicht** als „drained" missverstehen). `tracing`-Warn-Log bei `send`-Operationen, die > Schwellwert blockieren, gibt frühe Diagnostik.
- **Konsequenz für Long-Running-Cells**: der I/O-Task im Doppel-Task-Pattern (`proxy`/`timer`/`mcp`) blockiert beim Push in den internen mpsc, sobald der Handler überlastet ist. Das drosselt die externe Polling-Frequenz von selbst — gewünschtes Verhalten, TCP-Buffer am Provider reguliert sich.
- **Verworfen wurden** (vor der Festlegung auf `block`-only): `drop_newest` (silent loss, agentic-LLM-Reliability bricht), `drop_oldest` (nicht Tokio-mpsc-natürlich, braucht Custom-Wrapper, der die „eine Task pro Akteur"-Spec verletzt oder ein zusätzliches Crate erzwingt), `deadletter` (silent loss mit Audit-Trail, semantisch verwirrend gegenüber der existierenden Routing-Cascade nach `/colony/dead_letters` bei Routing-Fehlern). Wer eine andere Strategie braucht (z.B. „neueste Daten priorisieren"), baut sie **anwendungs-spezifisch über eine `code`-Cell als Priority-Filter** — konsistent mit „Iteration ist Topologie".

### Timeouts — zwei Konzepte, sauber getrennt

meclaw hat **zwei verschiedene Timeout-Mechanismen** mit unterschiedlichen Zwecken. Sie heißen in der Spec **Operation-Timeout** (A) und **Message-Timeout** (B). Wer beide vermischt, bekommt entweder falsche Restarts (Cell-Hänger-Backstop zu eng) oder unentdeckte Hänger (kein Backstop) — siehe Faustregel weiter unten.

#### A. Operation-Timeout (Cell-Disziplin, `params.external_timeout_ms`)

**Zweck**: jede I/O-Operation, die unbestimmt lange dauern kann, bekommt im Cell-Code einen `tokio::time::timeout`-Wrapper. Gilt für HTTP-Calls (`web_fetch`, `llm`), DB-Queries (`store`), Subprozesse (`bash`), Filesystem-Operationen (`file`, `edit`), MCP-Tool-Calls.

**Verhalten bei Elapsed**: Cell fängt das `Err(Elapsed)`-Result, baut eine **reguläre Error-Message** (`header.finish_reason: "error"`, `header.error_code` cell-type-spezifisch wie `provider_timeout`/`query_timeout`/`script_timeout`), emittiert sie via `outputs_tx`, **`handle()`-Call beendet sich regulär**. **Kein** Cell-Restart, **kein** Task-Killing.

**Konfiguration**: pro Cell via `params.external_timeout_ms` (Konvention; einzelne Cell-Types können semantisch passendere Namen wählen, z.B. `params.query_timeout_ms` für `store`). Cell-Type-Default in `cell-types.md`. Vollständig in Operator-Hand — der weiß, dass sein self-hosted LLM 90s braucht und der Cloud-Provider 5s antwortet.

**Cell-Code-Beispiel**:
```rust
match tokio::time::timeout(params.external_timeout, http_client.post(url).send()).await {
    Ok(Ok(response))  => /* normal processing */,
    Ok(Err(http_err)) => emit_error("provider_error", http_err, &outputs).await,
    Err(_elapsed)     => emit_error("provider_timeout", /*...*/, &outputs).await,
}
```

#### B. Message-Timeout (Substrat-Backstop, `cell.message_timeout`)

**Zweck**: Backstop für Pathologie-Fälle, in denen die Cell aus unbekanntem Grund hängt — Cell-Code-Bug ohne sauberen Operation-Timeout, Tokenizer-Loop, JSON-Parsing-Pathologie, intern verklemmter State. **Nicht** der primäre Timeout für I/O.

**Verhalten bei Elapsed**: `tokio::time::timeout`-Wrapper um den **gesamten** `handle()`-Call beendet diesen mit `Err(Elapsed)`. Die Cell-Task wird daraufhin terminiert (`break` aus dem `cell_task`-Loop), Supervisor erkennt das, Restart greift (`one_for_one`). Colony emittiert eine generic Timeout-Error-Message an `reply_to` (`header.finish_reason: "error"`, `header.error_code: "message_timeout"`). Trait-Object-State der Cell ist verloren, `cell.db` wird beim Neu-Spawn neu geladen.

**Konfiguration**: globaler Default in `colony.json` `message_timeout_default_ms` (Empfehlung: 60000), pro Cell überschreibbar via `cell.message_timeout` im `config.json`. Wert `0` oder `-1` = kein Backstop (typisch `proxy`/`timer`/`mcp`, die definitionsgemäß lang laufen).

**Cell-Task-Pattern** (stateful, mit Backstop):
```rust
async fn cell_task(
    mut mailbox: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<OutputEnvelope>,
    cell: Box<dyn Cell>,
    message_timeout: Option<Duration>,
) {
    while let Some(msg) = mailbox.recv().await {
        let result = match message_timeout {
            Some(t) => tokio::time::timeout(t, cell.handle(msg, &outputs_tx)).await,
            None    => { cell.handle(msg, &outputs_tx).await; Ok(()) }
        };
        if result.is_err() {
            emit_backstop_timeout(&outputs_tx, /*...*/).await;
            break;  // Task endet, Supervisor restartet
        }
    }
}
```

Stateless-Dispatcher und Long-Running-Handler nutzen denselben Wrapper-Pattern um ihren jeweiligen `handle`-Call.

**Stateless-Spezifikum (Worker vs. Dispatcher):** die pro Message gespawnte **Worker-Task ist ephemer** — sie wird vom Supervisor **nicht** beobachtet und **nicht restartet**; ein Worker-Panic endet still mit seiner Message (Disziplin: `handle()` ist panik-frei, alle I/O zu Error-Messages konvertiert). Die **supervidierte Einheit ist allein die langlebige Dispatcher-Task**: stirbt sie, restartet der Supervisor sie (und damit die Fähigkeit, neue Worker zu spawnen) — nicht die einzelnen Worker.

#### Faustregel für die Konfiguration

**B großzügig, A präzise.** Operation-Timeouts (A) sind die eigentliche Schutzschicht für I/O — die werden cell-type-spezifisch eng gesetzt. Der Message-Timeout (B) als Backstop liegt **deutlich darüber**, sodass normalerweise immer A zuerst greift und eine saubere Error-Message produziert. Nur wenn die Cell wirklich aus unbekanntem Grund hängt, springt B ein.

Beispiel `store`-Cell mit komplexen Queries:

```json
"cell":   { "type": "store", "message_timeout": 300000 }   // 5 Min Backstop
"params": { "external_timeout_ms":          60000 }        // 60s Query-Schutz
```

Verhalten:
- Query dauert 5s → normal.
- Query dauert 70s → A feuert nach 60s, Cell emittiert `query_timeout`-Error-Message, läuft weiter.
- Cell-Code hat einen SQLite-Deadlock-Bug → B feuert nach 5 Min, Task gekillt, Restart.

#### Cell-Type-Defaults (Phase 7/8 final, vorläufige Empfehlungen)

| Cell-Type | `params.external_timeout_ms` (A) | `cell.message_timeout` (B, Default) |
|---|---|---|
| `llm` | 110000 (110s) | 120000 (120s) |
| `web_fetch` | 25000 (25s) | 30000 (30s) |
| `web_search` | 25000 (25s) | 30000 (30s) |
| `bash` one-shot | 60000 (60s) | 90000 (90s) |
| `file` / `edit` | 10000 (10s) | 15000 (15s) |
| `store` | 60000 (60s) | 300000 (5 Min) |
| `code` (stateful/stateless) | 60000 (60s) | 90000 (90s), Operator-Sache |
| `proxy` / `timer` / `mcp` / `harness` / `subcolony` | — (cell-type-intern, Handler-spezifisch) | `0` oder `-1` (kein Backstop, definitionsgemäß lang) |

Diese Defaults werden in Phase 7/8 final gesetzt, wenn Cell-Type-Implementierung real wird. Operator überschreibt jederzeit pro Instanz.

Für `harness` liegt A auf `startup_timeout_ms` und den stdin-Writes; die **Task-Laufzeit ist bewusst unbegrenzt** (ein arbeitender Coding-Agent darf Minuten brauchen) — der Stopp-Hebel ist die `cancel`-Message (siehe `cell-types.md` § `harness`).

#### Foot-gun: CPU-Loop ohne `.await`

`tokio::time::timeout` ist **kooperativ** — es bricht ein Future nur am nächsten `.await`-Punkt ab. Eine reine CPU-Schleife ohne `.await` (Pathologie-Fall) bleibt am Worker-Thread festkrallen, und weder A noch B greift. Gegenmaßnahmen sind Code-Disziplin (siehe CLAUDE.md Regel 13: `tokio::task::yield_now().await` in langen CPU-Loops, `tokio::task::spawn_blocking` für echte blocking Operationen) und Beobachtung via `tokio-console` ab Phase 1 (siehe „Tech-Stack").

#### Andere Fehler-Pfade (zur Abgrenzung)

| Auslöser | Verhalten |
|---|---|
| Externer Call returnt sauberen Fehler (HTTP 500, DB-Error) | Cell baut reguläre Error-Message, kein Timeout |
| Cell-Code panickt (`unwrap`, OOB) | Tokio fängt Panic ab, Supervisor erkennt via `JoinError::is_panic()`, Restart; der `message_timeout`-Backstop (Konzept B) löst denselben Restart aus (Watcher klassifiziert ihn als Backstop-Death-Kind, Panic hat Vorrang). |
| Cell durch Mutation entfernt während laufendem `handle()` | **Graceful**: laufender Call läuft fertig (Drop des Mailbox-Receivers schließt nur Inbox für neue Messages), dann Task-Ende. **Kein** `abort()`, das würde Drop-Cleanup verlieren. |
| Eingabe-Validierungs-Fehler (fehlender Body-Slot) | Cell baut reguläre Error-Message, kein Timeout |
| LLM antwortet `finish_reason: error` | Cell-Type-spezifischer Error-Pfad, kein Timeout |

Der `llm`-A-Timeout umschließt den gesamten Provider-Roundtrip **inklusive des vollständigen Empfangs eines gestreamten Response-Bodies** — ein offener SSE-Stream ist damit ebenso gedeckt wie ein einzelner Request/Response. Der OAuth-Token-Refresh (P10) trägt einen eigenen, konstanten 30-s-Timeout; er ist bewusst **kein** Param, weil er im geteilten Token-Broker läuft und diesen nicht länger als nötig blockieren darf.

### Hänger-Erkennung

- **Kein expliziter Heartbeat.** Der Message-Timeout deckt das implizit ab — eine Cell, die zu lange in `handle()` ist, wird abgebrochen.
- Watchdog-Mechanismen erst Post-Roadmap, falls dann nötig.

---

## API (HTTP)

- **Implementation**: Modul in der Colony (`meclaw-api`-Crate, im Binary), nicht als Cell-Type. Colony übersetzt HTTP-Requests in typisierte `ColonyMsg`-Inbox-Commands (oneshot-ack-Reply); die Sequenzialität der Colony-Loop ist die Symmetrie-Garantie. Die „Alles ist eine Message"-Disziplin bleibt auf der **Daten-Ebene** gewahrt.
- **Stack**: `axum` (Tokio-nativ, async).
- **Surface**: REST. gRPC bei Bedarf später als zweite Surface.
- **OpenAPI-Spec**: generiert via `utoipa`.
- **Auth**: keine in Phase 12. Lokal nutzbar. Post-Roadmap-Hardening (Bearer-Token via `${API_TOKEN}`, später Capability-Tokens).
- **WebSocket** (`/events`): Live-Topologie-Events ab Phase 14 für Visualisierungs-Tools.
- Aktiv im Daemon-Mode (Direct-Mode optional via Flag).
- **Symmetrie zu internem Message-Routing**: jeder HTTP-Endpunkt ist ein dünner Wrapper, der den HTTP-Request in eine reguläre Message übersetzt und an die passende Authority (typisch Colony) routet. Cells innerhalb eines Builder-Hive erreichen dieselben Daten via direkter Messages — z.B. ein Graph-Read kommt per Message an `/colony/graph?scope=...` oder per HTTP `GET /colony/graph?scope=...` mit identischem Antwort-Schema.

### Visibility / Read-Pfade

Die Visibility-Schicht ist eine **Operator-orientierte Sicht** auf die kanonische `/colony/*`-Endpunkt-Liste (siehe „/colony als virtueller Endpunkt"). Sie sagt: „wenn ich Operation X tun will, welchen Endpunkt benutze ich?". Sie definiert keine neuen Endpunkte — alle hier genannten Pfade sind in der kanonischen Tabelle bereits aufgeführt.

| Operator-Frage | Endpunkt (intern + HTTP) | Filter |
|---|---|---|
| Welche Cells laufen gerade? | `/colony/registry` | `?path_prefix=`, `?type=` |
| Status einer einzelnen Cell? | `/colony/registry` | `?path=<exact>` |
| Wie sieht die Topologie eines Subtrees aus? | `/colony/graph` | `?scope=<path>` |
| Welche Templates gibt es? | `/colony/templates` | `?type=` *(heute silent no-op — `cell_type` liegt im FS, nicht in `colony.db`; aktiv ab Phase 14)*, `?name=` |
| Was steckt in der Dead-Letter-Queue? | `/colony/dead_letters` | `?since=` *(funktional — filtert per `WHERE created_at >= ?` auf den `created_at`-Timestamp der dead-lettered Message, seit W2a/W2d)*, `?limit=`, `?error_code=` |
| Was ist in Trace X passiert? | `/colony/trace` | `?trace_id=<uuid>` |
| Welche Fehler gab es zuletzt? | `/colony/trace` | `?error=true&limit=20` |
| Welche Mutationen sind committed? | `/colony/mutations` | `?since=` (Read auf Mutation-Log in `colony.db`) |
| Live-Stream der Routing-Entscheidungen? | `/colony/events` | (subscription) |

HTTP-Routen sind 1:1 die internen Pfade (siehe „/colony als virtueller Endpunkt", Symmetrie-Aussage). Operator-Web-UI rendert dieselben Daten als HTML unter `/ui/*` (siehe „Web-UI").

**Graph-Query-Response-Schema**:

Colony antwortet im Universal-Body-Format mit einem Top-Level-Slot `graph`. Konsumenten lesen `body.graph.*`:

```json
{
  "graph": {
    "scope": "/main/router",
    "graph_version": 42,
    "nodes": [
      { "name": "...", "id": "01HXY...", "type": "...", "template_ref": "...", "path": "..." }
    ],
    "edges": [
      { "id": "01HXZ...", "from": "...", "to": "...", "condition": "...", "modifier": null }
    ]
  }
}
```

- **Slot-Wrapper** (`graph`): gruppiert die zusammengehörigen Felder unter einem benannten Top-Level-Slot, konsistent mit der Universal-Body-Disziplin „Cells dürfen eigene Top-Level-Slots anlegen".
- **`graph_version`** ist heute **konstant `0`**; der monoton pro Scope wachsende Counter (zählt bei jeder erfolgreichen Mutation für diesen Scope hoch, hilft beim Polling-Diff) ist **ab Phase 14** vorgesehen.
- **Granularität: nur shallow** — eine Ebene pro Read. Sub-Scopes werden über separate Graph-Queries mit deren Pfad als Scope gelesen.
- **Edge-UUIDs sichtbar**: Builder kann sie nutzen, wenn er sie für Disambiguierung in Pathologie-Fällen braucht (`remove_edges` mit `id`-Feld).

**Push vs. Pull**: Pull (`GET /colony/registry?path_prefix=...` mit `graph_version`-Vergleich für Cache-Invalidation) ab Phase 12; Push (`GET /colony/events`, WebSocket-Subscribe) ab **Phase 14** — Grund: der Event-Broadcast müsste aus Routing-Loop, `handle_cell_died` und `handle_mutation` gefeuert werden, das berührt den await-freien `handle_cell_died`-Korridor (byte-identisch-Gate) und braucht einen eigenen Design-Pass (Broadcast-Mechanik, Slow-Consumer-Drop-Policy, Event-Schema). Pull ist ab Phase 12 verfügbar, weil das Web-UI selbst sie nicht braucht (kein JS, kein Auto-Refresh), externe Clients (Observability-Tools, Live-Graph-Viewer) aber davon profitieren. Cell-zu-Cell-Subscriptions als Pattern später möglich, aber kein Core-Feature.

### HTTP-Endpunkte

HTTP-Endpunkte sind 1:1 die `/colony/*`-Pfade (siehe „/colony als virtueller Endpunkt", Symmetrie-Aussage). axum nimmt einen HTTP-Request, baut eine Message mit `target = "/colony/<endpoint>"`, schickt sie durch denselben Routing-Pfad wie eine interne Message. Damit ist diese Tabelle redundant zur kanonischen Endpunkt-Tabelle — sie wiederholt sie nur in HTTP-Routen-Form:

| HTTP-Route | Methode | entspricht intern |
|---|---|---|
| `/messages` | POST | allgemeiner Message-Einlass: HTTP-Body → `Message` mit beliebigem `target` (z.B. `/main/agent/llm` oder `/colony/...`); axum übersetzt und gibt an Colony's Routing |
| `/colony/dead_letters` | GET/DELETE | `/colony/dead_letters` (Read + Drain) |
| `/colony/registry` | GET | `/colony/registry` (mit Filter-Query) |
| `/colony/templates` | GET | `/colony/templates` |
| `/colony/templates/rescan` | POST | `/colony/templates/rescan` |
| `/colony/mutations` | GET/POST | `/colony/mutations` (POST: neue Mutation, GET: Mutation-Log Audit) |
| `/colony/graph` | GET | `/colony/graph?scope=...` |
| `/colony/trace` | GET | `/colony/trace?trace_id=...&...` |
| `/colony/events` | GET (WS-Upgrade) | `/colony/events` (Subscribe) |
| `/ui/*` | GET | (HTML-Render-Schicht über denselben Daten — siehe „Web-UI") |
| `/health` | GET | Health-Check der HTTP-Schicht selbst (kein Routing durch Colony) |

`POST /messages` ist der einzige HTTP-Endpunkt, der eine Message mit beliebigem Target einschleusen kann. Alle anderen Routen sind 1:1 ihre internen `/colony/*`-Pfade (Symmetrie-Aussage im Abschnitt „/colony als virtueller Endpunkt").

`POST /messages` ist in Phase 12 fire-and-forget: Antwort **202 Accepted** mit `{message_id}`; eine etwaige Cell-Antwort läuft über die Routing-Cascade, nicht über HTTP zurück. Synchroner Request/Response-Roundtrip (ephemerer Reply-Sink) ist deferred auf Phase 13+. Der JSON-Request-Body ist `{target, body, headers?, ttl?}`: das optionale `ttl`-Feld setzt die TTL der Initial-Message (nur positive Integer ≤ `u32::MAX`; jeder andere Wert → `422 invalid_ttl`); ohne Feld gilt `colony.json` `message_default_ttl`. Der Multipart-Pfad hat kein `ttl`-Formfeld (Uploads, keine Conversation-Turns) — dort gilt immer der `colony.json`-Default.

HTTP-Status `/colony/mutations` (POST): **200** bei `Committed`, **422 Unprocessable Entity** bei `Rejected` — die volle `MutationOutcome::Rejected`-Detail bleibt im `mutation`-Slot des Bodies. (Status-Code ist Teil des HTTP-Datenmodells; 422 ist treue Übersetzung des Reject-Outcomes, kein Symmetrie-Bruch.)

**Bewusst nicht über API**: Template-Upload. Templates kommen nur via Filesystem oder CLI (Sicherheit + No-Delete-Disziplin).

---

## Persistenz

- Pro Cell: `cell.db` (SQLite) im Cell-Verzeichnis für dynamischen State — Cell-Authority. (Keine eigene Param-/Config-History-Tabelle: `CELL_DB_DDL` führt nur `system`/`last_input`/`meta`; `last_input` ist Forensik, keine History.)
- `colony.db`: zentrale Datenbank mit Registry (Pfad → Cell-ID + Status + Template), Templates-Registry, Mutations-Log, zentralem Message-Log und Edge-Tabelle. Colony schreibt; Cells lesen nicht direkt.
- Trace-Rekonstruktion via `parent_message_id`-Verkettung im zentralen Message-Log (flacher `SELECT`, der Eltern-Kind-Baum wird client-/UI-seitig aufgebaut; Index `idx_msglog_parent`).
- Blobs separat als JSON-Dateien.
- Operations-Log: `{root}/log.jsonl` (siehe Abschnitt „Logging").

---

## Logging

**Default**: `{root}/log.jsonl` (JSON-Lines, append-only, von Colony bei Start angelegt falls nicht vorhanden).

**Engine**: `tracing` + `tracing-subscriber` mit JSON-Formatter. Jedes Subsystem (Colony-Routing, Cell-Tasks, HTTP-API) schreibt via `tracing::*`-Macros in denselben Stream.

**Override**: CLI-Flag `--log <path>` (Default `{root}/log.jsonl`); `--log-level <level>` (Default `info`); `--log-filter <expr>` (per-Modul-Filter, z.B. `meclaw_core=debug,meclaw_colony=info`).

**Rotate**: nicht im Core. Operations-Sache via externes `logrotate` o.ä.

**Format pro Zeile**:
```json
{"ts":"2026-05-17T14:32:15.123Z","level":"error","event":"mutation_failed","error_code":"template_missing","scope":"/main","mutation_id":"01HXY...","correlation_id":"01HXZ...","details":{"template":"llm-anthropic@2.1.0"}}
```

**Verhältnis zu Mutations-Log und Message-Log in `colony.db`**: drei Logs koexistieren, komplementär.

| Log | Pfad | Zweck |
|---|---|---|
| Operations-Log | `{root}/log.jsonl` | tracing-Stream aller Subsysteme, für Operator/Debug, grep-/jq-freundlich |
| Mutations-Log | Tabelle in `colony.db` | strukturierter Audit-Trail nur der Mutationen, queryable via API `GET /mutations` |
| Message-Log | Tabelle in `colony.db` | jede geroutete Message mit `trace_id`, `parent_message_id`, `from_path`, `to_path` — filterbar nach Pfad-Präfix für scoped Tracing |

**Tracing & Metrics**: nicht Core-Architektur. Das `tracing`-Crate ist OTel-bridgeable (Crate `tracing-opentelemetry`), Metrics-Exposure ist über externe Tools / Sidecars / API-Erweiterungen lösbar. Wer Distributed-Tracing oder Prometheus-Scraping braucht, hängt es extern an — kein Crate oder Endpoint im Core-Stack vorgesehen.

---

## Dynamik / Builder-Pattern

- Jede Cell (oder externe API-Client) darf Mutation-Messages an `/colony/mutations` schicken — Permission ist Topologie-Frage, nicht Identitäts-Check.
- Mutation-Format ist ein **Diff** mit fünf optionalen Operationen: `add_nodes`, `add_edges`, `remove_nodes`, `remove_edges`, `swap_nodes` (siehe Abschnitt „Mutation-Format" oben), plus `scope`-Feld (Pfad-Präfix für die Mutation) und `ctx`-Block (für `${ctx.*}`-Substitution).
- Colony validiert einstufig, führt Staging + atomarer Filesystem-Rename + Registry-Edits aus, schließt Mutation in `colony.db` ab.
- **Builder-Hive** = ein **Hive-Scope** (kein einzelner Aktor), der mehrere spezialisierte Cells unter einem Pfad-Präfix bündelt — typisch eine `llm`-Cell für Natural-Language-Auftrag-Verständnis und Diff-Generierung, eine `code`-Cell für Mutation-Diff-Konstruktion und Validierung, optional eine `code`-Cell für Template-Discovery-Aggregation (Reads auf `/colony/templates`) und eine Collector- oder Memory-Hive für Multi-Step-Builder-Konversationen. Die finale Mutation-Diff wird von der äußersten Cell des Builder-Hive (oder einer dedizierten Output-Hive) an `/colony/mutations` emittiert. Begründung für Hive statt Single-Cell: die Builder-Aufgabe ist mehrstufig (Verstehen → Discovery → Diff-Konstruktion → Validierung), jede Stufe profitiert von einer eigenen Cell mit klarem Vertrag, und Hive bündelt sie als Authority- und Mutations-Boundary. Lebt üblicherweise unter `/main/builder/` o.ä. — Outputs laufen über die normale Edge-Topologie zu `/colony/mutations`.
- Konsistent mit No-Delete-Policy: Cells werden nie gelöscht — sie werden durch Edge-Entzug inaktiv (Registry-Eintrag bleibt, als inaktiv markiert; Filesystem und `cell_id` bleiben) und lassen sich über `add_edges` oder erneutes `add_nodes` am selben Pfad wieder aktivieren; `swap_nodes` schwenkt für Template-Upgrades die externen Edges auf eine neue oder andere Implementierung um (Graph-Swap — die alte Cell bleibt disconnected erhalten, siehe „Konnektivität & Aktivität" und § Mutation-Operationen).
- **EDA — Erfolgs-Ack an den Builder.** `/colony/mutations` beantwortet jede Mutation über `build_mutation_reply` (`crates/meclaw-colony/src/colony_dispatch.rs`) an `reply_to`: bei Erfolg `{"mutation":{"id":…,"outcome":"committed"}}`, bei Ablehnung analog mit `"outcome":"rejected"` plus `error_code`/`details`. Ohne gesetztes `reply_to` bleibt nur das Logging. Zweiphasige Builder (Mutation raus, Verdikt zurück, Receipt daraus) bauen darauf auf.

---

## DSL

- **Eigenes meclaw-Schema**, JSON-only, optimiert für die agentic-first Architektur.
- Validierung gegen JSON-Schema Draft 2020-12.
- Übernommene eigenständige Standards: **CEL** (Edge-Expressions), **SemVer** (Template-Versionen), **HTTP/OpenAPI-Konventionen** (für Auth, Retry, Timeout — eigen definiert, an etablierte Praxis angelehnt).
- Keine Compliance mit Workflow-Standards wie CNCF Serverless Workflow — die fundamental andere Architektur (Filesystem-DSL, Aktor-Substrat mit zentraler Routing-Authority, selbst-modifizierend statt deklariert) macht eine Anlehnung nicht sinnvoll.

---

## Tech-Stack

| Bereich | Wahl |
|---|---|
| Sprache | Rust (Edition 2024, `rust-toolchain.toml` mit `channel = "stable"` — rustup-Update der Workstation reicht, kein expliziter Versions-Pin/Bump; Build-Bruch durch neue stable wird als Spec-Konflikt-Frage behandelt) |
| Workspace-Resolver | `resolver = "3"` im Workspace-`Cargo.toml` (Default für Edition 2024, im Workspace-Manifest aber explizit zu setzen) |
| Async-Runtime | `tokio` (multi-thread Flavor, work-stealing Scheduler — siehe „Nebenläufigkeit & Parallelität") |
| Async-Observability (ab Phase 1) | `console-subscriber` für `tokio-console`-Bridge; aktiviert via `--cfg tokio_unstable` in `.cargo/config.toml` (Phase 0) |
| CLI | `clap` |
| Logging | `tracing` + `tracing-subscriber` |
| Non-Blocking-Log-Writer (ab Phase 1) | `tracing-appender` (Writer-Wrapper mit `WorkerGuard` für Flush; ergänzt `tracing-subscriber`'s synchronen Writer, sobald async-Cells loggen) |
| Serialisierung | `serde`, `serde_json` |
| DB | `rusqlite` (in Phase 5 entschieden; `sqlx` verworfen — `rusqlite="0.39"` in vier Crates; seit P4 mit Feature `functions` in `meclaw-cells` für registrierte Scalar-Functions wie `hamming()`) |
| Graph (Datenstruktur) | `petgraph` |
| Edge-Expressions | `cel` (Crate; GitHub-Projekt `cel-rust`) |
| HTTP-API | `axum` |
| HTTP-Client (ab Phase 7) | `reqwest` mit `rustls`-Feature (async, hyper-basiert, native Tokio-Runtime-Nutzung, statisches Binary möglich) |
| HTML-Templating (Operator-Web-UI, ab Phase 12) | `maud` (inline HTML in Rust-Macros, kein externes Template-Verzeichnis) |
| OpenAPI-Generation (ab Phase 12) | `utoipa` |
| UUID | `uuid` mit Feature `v7` |
| Cron-Parser (ab Phase 10) | `croner` (6-Feld-Quartz-Stil mit Sekunden, `find_next_occurrence`; nur als Parser genutzt, **kein** Scheduler-Crate) |
| Datum/Zeit (ab Phase 10) | `chrono` (Fremd-Dep von `croner`; zugleich Quelle für UTC-ISO-8601-Timestamps; `chrono-tz` / lokale Zeitzonen deferred) |
| Errors | `thiserror` (Library-Errors) + `anyhow` (Binary-Errors) |
| JSON-Schema | `jsonschema` (Draft 2020-12) |
| Test-Tmp-Verzeichnisse (dev-deps, ab Phase 0) | `tempfile` |
| File-Watcher | nicht im Scope |
| Prozessgruppen-Signale (ab P8) | `libc` 0.2 — nur `killpg`/`SIGTERM`/`SIGKILL`/`pid_t`, unix-only, ein Modul (Marcus-Sanktion 2026-08-08) |

---

## Repo-Struktur

```
MeClaw-core/
├── README.md                  # Landing-Page (Pitch + Pointer, keine Roadmap-Tabelle)
├── CLAUDE.md                  # Instruktionen, Disziplinen und Tripwires für den Agenten
├── PROGRESS.md                # alleiniger Status-Owner (Phasen, Sub-Phasen, letzter Tag)
├── docs/                      # autoritative Spezifikation (kanonisch)
│   ├── meclaw-overview.md
│   ├── cell-types.md
│   └── config.md
├── plans/                     # Phase-für-Phase-Pläne (abgeschlossene → plans/archive/)
├── archive/                   # archivierte DSL-Doku: PROGRESS-Volllog + project-state-Snapshot; Referenz für etwaige DSL-Weiterentwicklung
├── Cargo.toml                 # Workspace-Manifest
├── crates/
│   ├── meclaw-core/           # Aktor-Trait, ActorHandle, Message-Struct, Pfad-Resolution, CEL-Wrapper
│   ├── meclaw-colony/         # Colony-Task, Registry, Lifecycle, Templates, Routing, Mutations
│   ├── meclaw-cli/            # Binary, clap, Daemon, stdin/stdout-Bridge
│   ├── meclaw-cells/          # Built-in Cell-Typen
│   ├── meclaw-api/            # HTTP-API (axum)
│   └── meclaw-testing/        # Test-Fixtures
├── examples/                  # Beispiel-Colonies
└── rust-toolchain.toml        # gepinnte Rust-Version
```

**Inter-Crate-Dependencies werden phasenweise eingeführt.** Das Workspace-Manifest und die einzelnen `Cargo.toml`s halten in Phase 0 nur die externen Dependencies fest, die für das jeweils aktuelle Phasen-Ziel tatsächlich gebraucht werden. Eine Crate erhält erst dann eine `path = "../<other-crate>"`-Dep, wenn die aktuelle Phase ein konkretes Symbol aus der anderen Crate konsumiert. Layering ist Konsequenz der Phasen-Importe, kein Vorgriff — die finale Topologie (`meclaw-colony` → `meclaw-core`, `meclaw-cells` → `meclaw-core`, `meclaw-api` → `meclaw-colony`, `meclaw-cli` → `meclaw-colony` + `meclaw-cells` + `meclaw-api`) entsteht über die Phasen 1–4 organisch.

**`meclaw-testing` ist immer `[dev-dependencies]`**, in jedem Crate, das sie konsumiert. Sie ist per Spec nie eine Runtime-Dependency (siehe Abschnitt „Test-Infrastruktur (`meclaw-testing`)").

---

## Test-Infrastruktur (`meclaw-testing`)

Die `meclaw-testing`-Crate liefert Test-Fixtures und Helpers für Unit-, Integration- und Phase-Demo-Tests aller anderen Crates. Konsumenten sind ausschließlich `#[cfg(test)]`-Module und `tests/`-Targets — die Crate ist nie eine Runtime-Dependency.

**Was die Crate liefert**:

- **`TestRoot`**: RAII-Wrapper um ein tmp-Verzeichnis als `{root}`. Bei Drop wird das tmp-Verzeichnis aufgeräumt. Dies ist die **einzige erlaubte Ausnahme von der No-Delete-Policy**, weil tmp-Pfade nicht Teil des echten Live-Trees sind. Implementation über das `tempfile`-Crate; jeder Test bekommt einen eindeutigen Pfad.
- **`ColonyHandle`**: async Test-Wrapper um eine laufende Colony. Methoden: `send_message`, `wait_for_response`, `wait_for_dead_letter`, `query_registry`, `shutdown`. Nutzt durchgängig `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]`-Runtime, kein `block_on` an keiner Stelle.
- **`MessageBuilder`**: kleine Builder-API für Test-Messages, eliminiert UUID/Timestamp-Boilerplate.
- **`MockCell`-Set** unter `meclaw_testing::mocks`: vorgefertigte Cells fürs Testen — Echo, Header-Capture, Delay, Fail-On-Demand, Counter. Decken die in Phase 1–6 benötigten Test-Patterns ab.
- **Topology-Fixtures** unter `meclaw_testing::topologies::phase_N`: Helper-Funktionen, die die Demo-Topologie der jeweiligen Phase aus der Roadmap aufbauen. Jeder Phase-Demo-Test ruft seinen passenden Fixture.

**Was bewusst nicht in der Crate**:

- Keine **Production-Cells** — die kommen aus `meclaw-cells`.
- Keine **externen Provider-Mocks** (LLM-Provider, Telegram-Bot, MCP-Server) — diese wohnen als Test-Module direkt in den jeweiligen Cell-Crates, weil sie provider-spezifisch sind und nur dort gebraucht werden.
- Keine **Helpers für Operator-Workflows** — das deckt die HTTP-API ab Phase 12 ab.

**Konventionen**:

- Alle Helpers `async`, kein `block_on` irgendwo im Test- oder Helper-Code.
- Eindeutige tmp-Pfade pro Test via `tempfile`-Crate.
- `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` für jeden Test, der eine echte Topologie hochfährt (Cell-Spawns, Colony-Task). `worker_threads = 4` ist Konvention — deterministisch, schnell genug, reproduzierbar in CI. `current_thread`-Flavor (`#[tokio::test]` ohne Args) nur in reinen Unit-Tests ohne Topologie zulässig (pure-function-Tests, Schema-Validierung, Pfad-Resolution etc.).

**Phase 1** baut zuerst das `meclaw-testing`-Grundgerüst auf (`TestRoot`, `ColonyHandle`, `MessageBuilder`, eine Echo-`MockCell`, `topologies::phase_1`) und nutzt es ab dann konsequent. Verworfen wurden: keine eigene Crate sondern Test-Helpers pro Crate (Duplizierung, inkonsistente Patterns), Test-Helpers in `meclaw-core` unter `#[cfg(feature = "testing")]` (zwingt Konsumenten zu Feature-Toggles, vermischt Test-Code mit Library-Code), Phase-Demo-Topologien in den jeweiligen Crates (Demos kombinieren mehrere Crates, zentrale Stelle ist sauberer).

**Phase 0** hat das `meclaw-testing`-Crate nur als leere Schale (siehe Roadmap Phase 0). Tests in Phase 0 nutzen ausschließlich `std` und `tempfile`:

- **CLI-Integration-Tests** unter `crates/meclaw-cli/tests/*.rs` via `std::process::Command::new(env!("CARGO_BIN_EXE_meclaw"))`. Assertions auf Exit-Code, stdout, stderr und auf Side-Effect-Freiheit (kein `log.jsonl` im cwd nach `--version`/`--help`).
- **Unit-Tests** für die clap-Definition (`Cli::parse_from(...)`-Roundtrip) und für Subscriber-Setup-Funktionen direkt in den Modulen, mit `tempfile::tempdir()` für isolierte Log-Pfade. Kein Subprozess nötig.
- `tempfile` ist ab Phase 0 als Dev-Dependency erlaubt (siehe Tech-Stack-Tabelle).

---

## Roadmap (Phasen)

Concurrency-first: das Aktor-Substrat steht in Phase 1, alles andere baut darauf auf. Jede Phase liefert eine beobachtbare Demo.

| Phase | Inhalt | Demo |
|---|---|---|
| 0 | Cargo-Workspace, Crate-Skelette (alle 6 Crates: leere `Cargo.toml`s + minimaler `src/lib.rs` bzw. `src/main.rs`; Inhalt entsteht phasenweise), CLI-Skeleton (`--version`/`--help`/Flag-Parsing), Logging (`tracing` → `log.jsonl`), `rust-toolchain.toml`-Pin, `.cargo/config.toml` mit `--cfg tokio_unstable` (für `console-subscriber` ab Phase 1) | `meclaw --version` |
| 1 | **Aktor-Substrat**: Actor-Trait, ActorHandle, Colony als zentrale `HashMap<Path, ActorHandle>`, Supervisor (one_for_one), bounded mpsc (1000) mit Backpressure-`block`, `meclaw-testing`-Grundgerüst | 2 Echo-Hive + Supervisor-Restart, beobachtbar via tokio-console |
| 2 | **Pfad-Resolution**: pure Funktionen für `/`, `.`, `..`, `/colony/...`; Dead-Letter-Cascade als String-Op; Colony-Routing-Loop | Echo-Aktor unter `/a/b/c` via absolute (`/a/b/c`) und relative (`../`, `./`) Pfade ansprechbar; Message an `/missing` landet in `/colony/dead_letters` |
| 3 | **Universal-Body-Format**: `system + messages[] + slots`, JSON-Schema-Validation, `parent_message_id`, content.header-Extraktion | Echo-Hive mit Universal-Body, Header-Propagation sichtbar |
| 4 | **Filesystem-Bootstrap**: Colony scannt Tree, liest `config.json`, registriert Cells, liest `params.graph`-Hints aus Hive-Scope-Markern | Colony startet aus `examples/`-Tree, Registry zeigt alle Cells |
| 5 | **cell.db + State-Persistenz**: SQLite per Cell, `colony.db` als Registry- und Message-Log-Persistenz, Trace-Rekonstruktion via `parent_message_id` | Replay einer Trace nach Restart |
| 6 | **Mutations**: Diff-Ops (`add_nodes`/`add_edges`/`remove_nodes`/`remove_edges`/`swap_nodes`), Variablen-Substitution (`${ENV_VAR}`, `${ctx.*}`, `${uuid7:label}`) in Mutation-Diffs, einstufige Validierung, scoped Registry-Edits, `.staging/` + atomarer Rename, Mutations-Log | Live-Mutation einer laufenden Topologie, Recovery nach Crash mit `in_flight` |
| 7 | **Tool-Cells (atomic-emittierend, ohne `cell.db`)**: `bash`, `file`, `edit`, `web_fetch`, `web_search` | Tool-Chain via Messages |
| 8 | **`llm`-Cell (atomic-emittierend, mit `cell.db`)**: Provider-Translate **nur OpenAI** (Anthropic deferred, kein fester Phasen-Bezug), `system.*`-Slot-Akkumulation, Tool-Definition-Extraction nach `system.tools.*`, Error-Modell (`finish_reason: "error"`) | LLM-Cell antwortet via OpenAI-Provider, Tokens/Cost in Header |
| 9 | **Tool-Cells (mit `cell.db`)**: `store` (Schema + Seed + dynamische Tabellen + CRUD), `code` (Python-Runner first, programmierbarer Body-Konstruktor, optional Multi-Send) | `code`-Skript schreibt in `store`, queryt zurück |
| 10 | **Long-Running-Cells (Doppel-Task)**: `proxy` (Telegram first), `timer` (cron-like, sekundengenau, einmalig + repetierend), `mcp` (MCP-Bridge mit Discovery) | Telegram-Message triggert Topology, Timer-Schedule emittiert nach `n` Sekunden |
| 11 | **Templates**: `templates/`-Scanner, `template.json`, Templates-Registry in `colony.db`, `name@version`-Auflösung, Seed-JSONL-Bootstrap, `--rescan-templates`, Instanziierungs-Flow (Copy + UUID v7 + `${ENV_VAR}`-Substitution) | Instanziieren aus Template via Mutation |
| 12 | **HTTP-API + Web-UI + Blob-Storage**: Blob (`text_id`/`messages_id`, 64-KB-Default), Blob-Cache pro Cell, Daemon-Mode (`--daemon`), HTTP-API mit `axum` als dünne Übersetzungs-Schicht über `/colony/*` (opt-in via `--api <bind>`), OpenAPI via `utoipa`, Operator-Web-UI via `maud` unter `/ui/*`, `--validate`-Mode | `meclaw --api 127.0.0.1:7777`; Web-UI zeigt Cell-Übersicht, Trace, Dead-Letters |
| 13 | **Hot/Cold-Cell-Modell (stateful)**: Zustände `NotYetSpawned`/`Awake`/`Asleep`, `cell.timeout`-Semantik (`0`/`>0`/`-1`), Idle-Despawn, Wake-on-Message. Gilt nur für stateful Cells; Stateless- und Long-Running-Cells haben dieses Modell nicht | Tausende stateful Instanzen, wenige Awake |
| 14 | **Beispiel-Topologien**: Tool-Loop (Dispatcher + Collector als `code`-Cells unter Hive-Scope), RAG, einfaches Multi-Agent | Tool-Loop läuft end-to-end, im Web-UI als Trace inspizierbar |
| 15 | **Builder-Hive + AI-Builder**: Builder-Hive als mehrstufiger Hive-Scope (LLM-Cell für NL-Verständnis + `code`-Cell für Diff-Konstruktion und Validierung + optional Template-Discovery-Aggregator), nimmt Natural-Language-Aufträge, emittiert Mutation-Diffs an `/colony/mutations`, nutzt Template-Discovery (`/colony/templates`) | Builder-Hive baut Sub-Topologie aus Prompt |
| 16 | **Schema-Freeze + Audit**: Schema-Final-Review, Doku-Audit, Cross-Reference-Check, Lizenz-Entscheidung | Dokumentations-stabiler Tag |

**Sub-Phasen** (emergente Substrat-Zwischenpässe wie 6.5 / 7.5 oder Doku-Konsolidierungen wie 9.5) sind kein Roadmap-Bestandteil. Status pro Phase und Sub-Phase, inkl. aktueller Phase und letztem Git-Tag, lebt ausschließlich in `PROGRESS.md`.

---

## Was meclaw _nicht_ tun wird (Scope-Disziplin)

- Kein verteiltes Cluster-Setup (bei Bedarf später: NATS als Transport drunter, Cross-Colony-Föderation als additive Erweiterung).
- Keine GUI / kein Editor (VSCode + Filesystem reichen; Visualisierung über API von externen Tools).
- Keine Nicht-Agentic-Workflows abdecken (kein Airflow-Ersatz, kein BPMN-Ersatz).
- Keine eigene LLM-Inference (Cells rufen externe Provider).
- Keine Cell-zu-Cell-Topologie-Kenntnis (Cells bleiben dumm).
- Keine Compliance mit fremden Workflow-Standards — eigenes Schema, JSON-first.
