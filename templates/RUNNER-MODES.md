# Runner modes of the shipped code cells

Every `code` cell runs `warm` unless this table says in writing why not:
`resident` only for a cell that caches its own durable state in memory, `cold`
only with a reason (for example a subprocess with a signal handler). `mode` is
the cell's `params.runner_mode` (absent reads `cold`); `passes` says where the
cell sits: `turn` (every turn before the answer), `tool` (the tool round of a
turn), `session` (admission, session events), `after` (every turn after the
answer), `rare` (build, operation, night, timer). A new code cell gets its mode
per `docs/cell-types.md` § `code` (which mode) and its row here in the same
change; the lock test holds the table against the configs. A row is the mode a template
ships; a consumer may set a rarely called cell `cold` through its ref marker's
`override_params` and writes the reason in its own README (`simple-agent`, § Runner modes).

| cell | mode | passes | reason |
|---|---|---|---|
| `access/invoke` | warm | tool | |
| `access/policy` | warm | rare | |
| `access/sweep` | warm | rare | |
| `affinity/brief` | warm | turn | |
| `affinity/gate` | warm | after | |
| `affinity/porter` | warm | session | |
| `affinity/push` | warm | session | |
| `archive-bridge` | warm | after | |
| `argus/door` | warm | rare | |
| `argus/meter` | warm | rare | |
| `argus/mutator` | warm | rare | |
| `argus/probe` | warm | rare | |
| `builder-librarian/catalogue` | warm | rare | |
| `builder-librarian/retrieve` | warm | rare | |
| `builder/brief` | warm | rare | |
| `builder/classify` | warm | rare | |
| `builder/eyes` | warm | rare | |
| `builder/lib` | warm | rare | |
| `builder/normalise` | warm | rare | |
| `builder/recipes` | warm | rare | |
| `builder/unknown` | warm | rare | |
| `builder/weave` | warm | rare | |
| `canvy/layout` | warm | rare | |
| `canvy/probe` | warm | rare | |
| `chat-channel` | warm | turn | |
| `coder-pipeline/coderprep` | warm | turn | |
| `coder-pipeline/collector` | warm | turn | |
| `coder-pipeline/dispatch` | warm | turn | |
| `coder-pipeline/drain` | warm | rare | |
| `coder-pipeline/plannerprep` | warm | turn | |
| `coder-pipeline/revout` | warm | turn | |
| `coder-pipeline/revprep` | warm | turn | |
| `coder-pipeline/taskarchive` | warm | after | |
| `cogny/ask` | warm | tool | |
| `cogny/schemas` | warm | rare | |
| `cogny/splitter` | warm | tool | |
| `collector/assemble` | warm | turn | one slot (`max_concurrency: 1`), as strictly serial as before; it holds no cache of its own; keep 1 -- an instance that raises it gives up that serial order |
| `colony-view/layout` | warm | rare | |
| `colony-view/probe` | resident | rare | caches the counts of the last topology snapshot for the `stats` read |
| `curator/handover` | resident | session | caches its own handover writes; after a restart the ledger read decides |
| `curator/history` | resident | tool | caches the last `marks.seq` it minted; a new child starts from the clock |
| `curator/intake` | resident | turn | caches its wall writes in flight and the episode floor it handed on; after a restart the ledger read decides |
| `curator/policy` | resident | turn | caches the window forms it has put into `blocks`; a new child puts each once more |
| `curator/push` | resident | turn | caches the recall writes it planned while they are in flight; after a restart the ledger read decides |
| `curator/reader` | warm | after | one slot (`max_concurrency: 1`) beside the ledger; it holds no cache of its own; keep 1 -- an instance that raises it gives up that serial order |
| `curator/schemas` | warm | rare | |
| `curator/stats` | warm | rare | one slot (`max_concurrency: 1`) beside the ledger; it holds no cache of its own; keep 1 -- an instance that raises it gives up that serial order |
| `curator/writer` | warm | after | one slot (`max_concurrency: 1`) beside the ledger; it holds no cache of its own; keep 1 -- an instance that raises it gives up that serial order |
| `daily-digest/format` | warm | rare | |
| `daily-digest/shelf` | warm | rare | |
| `daily-digest/void` | warm | rare | |
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
| `file-space/schemas` | warm | rare | |
| `file-space/tools` | warm | tool | |
| `file-space/write` | warm | tool | |
| `file-space/ws` | warm | tool | |
| `firewall/porter` | warm | session | |
| `firewall/screen` | warm | turn | |
| `firewall/warden` | warm | session | |
| `freeswitch/dial` | warm | tool | |
| `freeswitch/signal` | warm | turn | |
| `graph-space/index` | warm | after | |
| `graph-space/query` | warm | tool | |
| `librarian/index` | warm | after | |
| `librarian/query` | warm | tool | |
| `librarian/schemas` | warm | rare | |
| `librarian/tools` | warm | tool | |
| `llm-registry/hand` | warm | rare | |
| `llm-registry/select` | warm | rare | |
| `memory-drain/drain` | warm | session | |
| `memory-hive/close-glue` | warm | session | |
| `memory-hive/dream-glue` | warm | rare | |
| `memory-hive/embed` | warm | turn | |
| `memory-hive/entity-glue` | warm | after | |
| `memory-hive/extract-glue` | warm | after | |
| `memory-hive/porter` | warm | session | |
| `memory-hive/recall` | warm | turn | |
| `memory-hive/recur` | warm | rare | |
| `memory-hive/schemas` | warm | rare | |
| `memory-hive/tool` | warm | tool | |
| `memory-hive/writer` | warm | after | |
| `objects/brief` | warm | tool | |
| `objects/gate` | warm | tool | |
| `objects/push` | warm | after | |
| `objects/schemas` | warm | rare | |
| `objects/source` | warm | tool | |
| `objects/tools` | warm | tool | |
| `operator/export` | warm | rare | |
| `operator/intake` | warm | rare | |
| `operator/lifecycle` | warm | rare | |
| `operator/unknown` | warm | rare | |
| `presenter/stage` | resident | after | caches its `shows` and `pending` tables; a new child reads them back before it handles anything |
| `projection/git` | warm | tool | its `git` subprocesses end within the message; their environment is a dict of the run, never `os.environ` |
| `projection/mat` | warm | tool | |
| `projection/run` | warm | tool | its program ends within the message (`communicate` with a timeout, else `killpg`); `signal` is read for `SIGKILL` only, no handler is installed |
| `receptionist/greet` | warm | turn | |
| `research-assistant/archive` | warm | after | |
| `research-assistant/collector` | warm | turn | |
| `research-assistant/dispatch` | warm | turn | |
| `research-assistant/drain` | warm | rare | |
| `research-assistant/prep` | warm | turn | |
| `research-assistant/shelf` | warm | rare | |
| `retry` | warm | tool | |
| `scriptlet` | cold | rare | ships blank; its real script comes per instance through `override_params`, often written by the builder model and unreviewed, and the template mode is the default for that code |
| `session-keeper/close` | warm | session | |
| `session-keeper/porter` | warm | session | |
| `session-keeper/stamp` | warm | turn | |
| `simple-agent/door` | resident | turn | caches whether the identity boot ran, the rounds whose watchdog is armed, the answered turns and the turns that wait for the boot; a new child boots once more |
| `simple-agent/splitter` | warm | turn | |
| `slack-agent/persona` | warm | turn | |
| `steward/meter` | warm | rare | |
| `steward/mutator` | warm | rare | |
| `steward/probe` | warm | rare | |
| `submit/gate` | warm | rare | |
| `summarizer/prep` | warm | session | |
| `talky/errors` | warm | rare | |
| `talky/route` | warm | turn | |
| `talky/schemas` | warm | rare | |
| `talky/splitter` | warm | turn | |
| `terminal` | warm | after | |
| `tools/build-apply` | warm | tool | |
| `tools/build-draft` | warm | tool | |
| `tools/schemas` | warm | rare | |
| `tools/unknown` | warm | tool | |
| `tools/weather` | warm | tool | |
