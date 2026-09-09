# meclaw-os

meclaw-os is a set of templates that arranges a colony into four nested levels: a shell, an
organisation, a member, and one generation of that member's assistant. All of it is DSL:
directories, `config.json` files and edges, with no Rust of its own. A seed holding one `ref`
declaration grows the whole shell on first boot, and every level below it is one more
declaration into a container that already exists.

## Why it exists

meclaw-os is the reference implementation: a complete agentic OS, grown on that
substrate and nothing else. It answers whether the primitives carry a system larger than one
agent. Identity, memory, screening, keys, channels, a control loop and an authoring path are
compositions of templates here, not features the binary ships.

## The four levels

One rule decides what sits where: **a level owns what its siblings must share**. Memory sits
at the member, so replacing an assistant does not take the history with it. Screening sits
outside the assistant, so a new generation meets the same attacker record and the same rate
window. The capability broker sits at the shell, because two brokers are two answers to one
question. The members of one organisation share a name and a boundary and nothing else, so
that level owns nothing else.

```
/os                          meclaw-os    access  argus  builder  operator (holds submit)
 └── orgs                    (container)
     └── acme                org          a name and a boundary, no cell of its own
         └── members         (container)
             └── alex        member       memory-hive  affinity  firewall  access
                 ├── channels    (container)  telegram-connector  display  freeswitch
                 ├── apps        (container)  colony-view
                 └── assistants  (container)
                     └── scribe  assistant  talky  cogny  tools
```

A container is a real hive with no children and no contract. Its whole job is to be an address
that exists before the mutation that puts something there.

## The parts and what each does

| Part | Level | What it does |
|---|---|---|
| `access` | shell, member | The capability broker. An agent asks in natural language, a handle travels on the wire, and a credential leaves the vault only sealed. |
| `argus` | shell | The control loop. It reads a charter, measures the colony out of its own ledger, decides, and keeps or reverts the change. Every goal ships disabled. |
| `builder` | shell | The intake that turns a structural wish into a manifest, an ordered list of mutation declarations. It applies nothing. |
| `operator` | shell | The one front door a person addresses the OS through, and the hive the submitter lives in. |
| `submit` | inside `operator` | The only reach onto `/colony/mutations` in the whole tree. It asks the broker who may submit before it carries a manifest there. |
| `memory-hive` | member | A member's memory. Every turn becomes an append-only episode row without a model, and recall answers only with rows the current round could have heard. |
| `affinity` | member | The curated record of the people and agents this member knows, with relations, trust, disclosure and an append-only audit. |
| `firewall` | member | Deterministic screening on the way in. Size, sender, forbidden literal and rate, every verdict a comparison and never a model. |
| `talky` | assistant | The conversation surface. One identity, one session store per channel, and the job of keeping a conversation flowing. |
| `cogny` | assistant | The reasoning core. One brain and one class of errand: synthesis, a development over time, multi-step work. |
| `tools` | assistant | The tool surface. `tool_call` in, `tool_result` out, and it hands back the schemas of the tools it serves. |
| `telegram-connector` | channel | One Telegram chat behind one proxy cell, with no persona and no answer of its own. |
| `display` | channel | One screen on a port of its own, which several agents write named views onto without knowing about each other. |
| `freeswitch` | channel | A telephone as a channel. A `voice` cell terminates the media, a small state machine offers `call` and `hangup`. |
| apps | member | A sealed hive in the member's `apps` container. It may observe the conversation, offer a tool and write to a screen, and it never stands in the way. |

The shipped app is `colony-view`, which draws the colony's own topology onto a `display`.

## One declaration

`examples/meclaw-os/` grows a single agent by hand, so each step is readable before a builder
writes them. Its `grow.json` names four templates and draws four edges between them, and the
colony that was empty a moment ago holds seventeen cells:

```json
{
  "scope": "/",
  "diff": {
    "add_nodes": [
      {"name": "door",     "template": "door"},
      {"name": "firewall", "template": "firewall"},
      {"name": "talky",    "template": "talky"},
      {"name": "sink",     "template": "terminal"}
    ],
    "add_edges": [
      {"from": "./door",     "to": "./firewall",
       "condition": "has(hop.route) && hop.route == 'turn' && ..."},
      {"from": "./firewall", "to": "./talky",
       "condition": "has(hop.route) && hop.route == 'pass'"},
      {"from": "./talky",    "to": "./sink",
       "condition": "has(hop.route) && hop.route == 'answer'"},
      {"from": "./talky",    "to": "./sink",
       "condition": "has(hop.route) && hop.route == 'turn_write'"}
    ]
  }
}
```

The file also carries the context modifiers each edge sets, dropped here. The four levels are
grown the same way, one `POST /colony/mutations` each, in
[`examples/organism`](../examples/organism/README.md).

## Where to read on

- [An operating system for agents](why/an-os-for-agents.md), the four levels and what each owns.
- [One assistant, two brains](why/two-brains.md), why `talky` and `cogny` are two templates.
- [Memory that outlives the window](why/memory.md), how the record is written and read.
- [You talk, it shows](why/you-talk-it-shows.md), what voice and screen are aimed at together.
- [`examples/organism`](../examples/organism/README.md), the whole stack in six declarations.
- [`templates/`](../templates/README.md), every template in the library with its version.
- [Security](security.md), the sandbox, the vault and the broker.
