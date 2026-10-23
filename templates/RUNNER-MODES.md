# Runner modes of the shipped code cells

A `code` cell a turn passes runs `warm`, a rarely called one (`rare`) runs
`cold`, and every row that is not `warm` off `rare` says why in writing:
`cold` always (on `rare` why the cell is called rarely, on a turn for example
a subprocess with a signal handler), `resident` only for a cell that caches
its own durable state in memory, `warm` on `rare` only with a
reason (a warm child stands 30 minutes after its last job, and the apply
broadcast wakes every warm cell that answers it -- GH #1294). `mode` is
the cell's `params.runner_mode` (absent reads `cold`); `passes` says where the
cell sits: `turn` (every turn before the answer), `tool` (the tool round of a
turn), `session` (admission, session events), `after` (every turn after the
answer), `rare` (build, operation, night, timer). A new code cell gets its mode
per `docs/cell-types.md` § `code` (which mode) and its row here in the same
change; the lock test holds the table against the configs. A row is the mode a template
ships; a consumer that calls a turn cell of a template only rarely may set it `cold`
through its ref marker's `override_params` and writes the reason in its own README
(`simple-agent`, § Runner modes).

| cell | mode | passes | reason |
|---|---|---|---|
| `access/invoke` | warm | tool | one slot (`max_concurrency: 1`): a local broker read per credential ask, no network; the default pool of 4 held 9 children for 5 instances of one member after boot (GH #1294) |
| `access/policy` | cold | rare | rarely called: a grant decision; a warm child would stand 30 min after each call (GH #1294) |
| `access/sweep` | cold | rare | rarely called: the TTL timer; a warm child would stand 30 min after each call (GH #1294) |
| `affinity/brief` | warm | turn | |
| `affinity/gate` | warm | after | |
| `affinity/porter` | warm | session | |
| `affinity/push` | warm | session | |
| `archive-bridge` | warm | after | |
| `argus/door` | cold | rare | rarely called: an operation round (probe, meter, mutation); a warm child would stand 30 min after each call (GH #1294) |
| `argus/meter` | cold | rare | rarely called: an operation round (probe, meter, mutation); a warm child would stand 30 min after each call (GH #1294) |
| `argus/mutator` | cold | rare | rarely called: an operation round (probe, meter, mutation); a warm child would stand 30 min after each call (GH #1294) |
| `argus/probe` | cold | rare | rarely called: an operation round (probe, meter, mutation); a warm child would stand 30 min after each call (GH #1294) |
| `builder-librarian/catalogue` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `builder-librarian/retrieve` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `builder/brief` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `builder/classify` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `builder/eyes` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `builder/lib` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `builder/normalise` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `builder/recipes` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `builder/unknown` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `builder/weave` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `canvy/layout` | cold | rare | rarely called: a topology snapshot for a display, on a timer or a mutation; a warm child would stand 30 min after each call (GH #1294) |
| `canvy/probe` | cold | rare | rarely called: a topology snapshot for a display, on a timer or a mutation; a warm child would stand 30 min after each call (GH #1294) |
| `chat-channel` | warm | turn | |
| `coder-pipeline/coderprep` | warm | turn | |
| `coder-pipeline/collector` | warm | turn | |
| `coder-pipeline/dispatch` | warm | turn | |
| `coder-pipeline/drain` | cold | rare | rarely called: a night or timer pass; a warm child would stand 30 min after each call (GH #1294) |
| `coder-pipeline/plannerprep` | warm | turn | |
| `coder-pipeline/revout` | warm | turn | |
| `coder-pipeline/revprep` | warm | turn | |
| `coder-pipeline/taskarchive` | warm | after | |
| `cogny/ask` | warm | tool | |
| `cogny/schemas` | cold | rare | rarely called: it answers the collector's menu ask, at boot and after each apply (the `mutation_committed` broadcast); a warm child would stand 30 min after each call (GH #1294) |
| `cogny/splitter` | warm | tool | |
| `collector/assemble` | warm | turn | one slot (`max_concurrency: 1`), as strictly serial as before; it holds no cache of its own; keep 1 -- an instance that raises it gives up that serial order |
| `colony-view/layout` | cold | rare | rarely called: a topology snapshot for a display, on a timer or a mutation; a warm child would stand 30 min after each call (GH #1294) |
| `colony-view/probe` | resident | rare | caches the counts of the last topology snapshot for the `stats` read |
| `curator/handover` | resident | session | caches its own handover writes; after a restart the ledger read decides |
| `curator/history` | resident | tool | caches the last `marks.seq` it minted; a new child starts from the clock |
| `curator/intake` | resident | turn | caches its wall writes in flight and the episode floor it handed on; after a restart the ledger read decides |
| `curator/policy` | resident | turn | caches the window forms it has put into `blocks`; a new child puts each once more |
| `curator/push` | resident | turn | caches the recall writes it planned while they are in flight; after a restart the ledger read decides |
| `curator/reader` | warm | after | one slot (`max_concurrency: 1`) beside the ledger; it holds no cache of its own; keep 1 -- an instance that raises it gives up that serial order |
| `curator/schemas` | cold | rare | rarely called: it answers the collector's menu ask, at boot and after each apply (the `mutation_committed` broadcast); a warm child would stand 30 min after each call (GH #1294) |
| `curator/stats` | cold | rare | rarely called: an observer's `in_stats`; a warm child would stand 30 min after each call (GH #1294); one slot (`max_concurrency: 1`) beside the ledger; it holds no cache of its own; keep 1 -- an instance that raises it gives up that serial order |
| `curator/writer` | warm | after | one slot (`max_concurrency: 1`) beside the ledger; it holds no cache of its own; keep 1 -- an instance that raises it gives up that serial order |
| `daily-digest/format` | cold | rare | rarely called: a night or timer pass; a warm child would stand 30 min after each call (GH #1294) |
| `daily-digest/shelf` | cold | rare | rarely called: a night or timer pass; a warm child would stand 30 min after each call (GH #1294) |
| `daily-digest/void` | warm | rare | the sink of a push-only bot: rarely called, but every inbound chat ends here and arrives as a burst when it comes; warm since GH #852 so a burst is not a burst of namespace setups (`gh852_a_warm_sink_keeps_up_with_a_flood`) |
| `dispatcher` | warm | turn | |
| `display/compose` | resident | after | caches its store and the object tree it last sent; a killed child rebuilds the same screen |
| `door` | warm | turn | |
| `egon/persona` | warm | turn | |
| `figure/phone` | warm | turn | |
| `figure/world` | resident | turn | caches the turn queue, the open turn, the call book, the phone replies counted per world day, the answer rounds already taken (with the crc32 of their texts) and the capped ones not yet shown; a replaced child degrades as the template README says |
| `file-space/derive` | warm | after | |
| `file-space/embed` | warm | after | |
| `file-space/extract` | warm | turn | its `pdftotext` subprocess ends within the message (`subprocess.run` with a timeout) |
| `file-space/guard` | warm | tool | |
| `file-space/ingest` | warm | turn | |
| `file-space/read` | warm | tool | |
| `file-space/schemas` | cold | rare | rarely called: it answers the collector's menu ask, at boot and after each apply (the `mutation_committed` broadcast); a warm child would stand 30 min after each call (GH #1294) |
| `file-space/tools` | warm | tool | |
| `file-space/write` | warm | tool | |
| `file-space/ws` | warm | tool | |
| `firewall/porter` | warm | session | |
| `firewall/screen` | warm | turn | one slot (`max_concurrency: 1`): a local check per message, no network, pure CPU per phase; messages arriving at once on several channels wait milliseconds for the slot, and each wait is logged (`meclaw::concurrency`, `waited_ms`); the default pool of 4 held 2 children after boot (GH #1294) |
| `firewall/warden` | warm | session | |
| `freeswitch/dial` | warm | tool | |
| `freeswitch/signal` | warm | turn | |
| `graph-space/index` | warm | after | |
| `graph-space/query` | warm | tool | |
| `librarian/index` | warm | after | |
| `librarian/query` | warm | tool | |
| `librarian/schemas` | cold | rare | rarely called: it answers the collector's menu ask, at boot and after each apply (the `mutation_committed` broadcast); a warm child would stand 30 min after each call (GH #1294) |
| `librarian/tools` | warm | tool | |
| `llm-registry/hand` | cold | rare | rarely called: it moves only when the catalogue or a subscriber changes; a warm child would stand 30 min after each call (GH #1294) |
| `llm-registry/select` | cold | rare | rarely called: a lookup when a package is resolved; a warm child would stand 30 min after each call (GH #1294) |
| `memory-drain/drain` | warm | session | |
| `memory-hive/close-glue` | warm | session | |
| `memory-hive/dream-glue` | cold | rare | rarely called: a night or timer pass; a warm child would stand 30 min after each call (GH #1294) |
| `memory-hive/embed` | warm | turn | |
| `memory-hive/entity-glue` | warm | after | |
| `memory-hive/extract-glue` | warm | after | |
| `memory-hive/porter` | warm | session | |
| `memory-hive/recall` | warm | turn | |
| `memory-hive/recur` | cold | rare | rarely called: a night or timer pass; a warm child would stand 30 min after each call (GH #1294) |
| `memory-hive/schemas` | cold | rare | rarely called: it answers the collector's menu ask, at boot and after each apply (the `mutation_committed` broadcast); a warm child would stand 30 min after each call (GH #1294) |
| `memory-hive/tool` | warm | tool | |
| `memory-hive/writer` | warm | after | |
| `objects/brief` | warm | tool | |
| `objects/gate` | warm | tool | |
| `objects/push` | warm | after | |
| `objects/schemas` | cold | rare | rarely called: it answers the collector's menu ask, at boot and after each apply (the `mutation_committed` broadcast); a warm child would stand 30 min after each call (GH #1294) |
| `objects/source` | warm | tool | |
| `objects/tools` | warm | tool | |
| `operator/export` | cold | rare | rarely called: an operator's submit, dump or lifecycle; a warm child would stand 30 min after each call (GH #1294) |
| `operator/intake` | cold | rare | rarely called: an operator's submit, dump or lifecycle; a warm child would stand 30 min after each call (GH #1294) |
| `operator/lifecycle` | cold | rare | rarely called: an operator's submit, dump or lifecycle; a warm child would stand 30 min after each call (GH #1294) |
| `operator/unknown` | cold | rare | rarely called: an operator's submit, dump or lifecycle; a warm child would stand 30 min after each call (GH #1294) |
| `presenter/stage` | resident | after | caches its `shows` and `pending` tables; a new child reads them back before it handles anything |
| `projection/git` | warm | tool | its `git` subprocesses end within the message; their environment is a dict of the run, never `os.environ` |
| `projection/mat` | warm | tool | |
| `projection/run` | warm | tool | its program ends within the message (`communicate` with a timeout, else `killpg`); `signal` is read for `SIGKILL` only, no handler is installed |
| `receptionist/greet` | warm | turn | |
| `research-assistant/archive` | warm | after | |
| `research-assistant/collector` | warm | turn | |
| `research-assistant/dispatch` | warm | turn | |
| `research-assistant/drain` | cold | rare | rarely called: a night or timer pass; a warm child would stand 30 min after each call (GH #1294) |
| `research-assistant/prep` | warm | turn | |
| `research-assistant/shelf` | cold | rare | rarely called: a night or timer pass; a warm child would stand 30 min after each call (GH #1294) |
| `retry` | warm | tool | |
| `scriptlet` | cold | rare | ships blank; its real script comes per instance through `override_params`, often written by the builder model and unreviewed, and the template mode is the default for that code |
| `session-keeper/close` | warm | session | |
| `session-keeper/porter` | warm | session | |
| `session-keeper/stamp` | warm | turn | |
| `simple-agent/door` | resident | turn | caches whether the identity boot ran, the rounds whose watchdog is armed, the answered turns and the turns that wait for the boot; a new child boots once more |
| `simple-agent/splitter` | warm | turn | |
| `slack-agent/persona` | warm | turn | |
| `steward/meter` | cold | rare | rarely called: an operation round (probe, meter, mutation); a warm child would stand 30 min after each call (GH #1294) |
| `steward/mutator` | cold | rare | rarely called: an operation round (probe, meter, mutation); a warm child would stand 30 min after each call (GH #1294) |
| `steward/probe` | cold | rare | rarely called: an operation round (probe, meter, mutation); a warm child would stand 30 min after each call (GH #1294) |
| `submit/gate` | cold | rare | rarely called: the build path (a draft, a check, an apply); a warm child would stand 30 min after each call (GH #1294) |
| `summarizer/prep` | warm | session | |
| `talky/errors` | cold | rare | rarely called: the error drain of the composite; a warm child would stand 30 min after each call (GH #1294) |
| `talky/route` | warm | turn | |
| `talky/schemas` | cold | rare | rarely called: it answers the collector's menu ask, at boot and after each apply (the `mutation_committed` broadcast); a warm child would stand 30 min after each call (GH #1294) |
| `talky/splitter` | warm | turn | |
| `terminal` | warm | after | |
| `tools/build-apply` | warm | tool | |
| `tools/build-draft` | warm | tool | |
| `tools/schemas` | cold | rare | rarely called: it answers the collector's menu ask, at boot and after each apply (the `mutation_committed` broadcast); a warm child would stand 30 min after each call (GH #1294) |
| `tools/unknown` | warm | tool | |
| `tools/weather` | warm | tool | |
