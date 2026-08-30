# `scriptlet@1.0.1`

A script, as one `code` cell, shipped blank. It exists because of a gap the
library had rather than the substrate: thirty-two shipped templates carry a
`code` cell inside them, five of them hold exactly one -- `door`, `terminal`,
`retry`, `dispatcher`, `archive-bridge` -- and **every one of those five carries
a script and a purpose of its own**. There was no plain one to name, an
`add_nodes` entry requires a `name` and a `template`, and so a composer that
needed application logic between two cells spent seven rounds looking for a
template that did not exist
([#482](https://github.com/mmeyerlein/meclaw/issues/482)).

That is the whole template: one cell, one runner, and a pass-through where the
logic goes.

## What it delivers

- **A cell a declaration can ask for.** `{"name": "dedupe", "template": "scriptlet",
  "override_params": {"script_inline": "…"}}` and two edges, in the same
  mutation, and a topology has the piece between its pieces.
- **A blank that runs.** The shipped script reads `body.messages` and writes it
  back unchanged. It is not a placeholder that fails until replaced: an instance
  with no override boots, ticks and passes turns through, which is what makes the
  override a change rather than a repair.
- **Multi-send declared.** `contract.multi_send_capable` is `true`, so a
  replacement script may write a JSON **array** and emit one message per element,
  each evaluated against this cell's edges independently. A blank code cell that
  could only ever emit one message would be half a code cell -- and the
  declaration lives in the `contract`, which an `override_params` cannot reach.
- **A price that is named.** An `override_params` carrying a `script_inline` is
  executable behaviour arriving with a manifest. On the authoring path that is a
  second capability question, `code.author`, and it ships disabled.

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `code` | a runner and a script. **No `cell.db`** -- a `code` cell has none, by cell type. |

Single-cell template (one cell of one cell type, the smallest `config.json` that
starts it, and a README that explains its declarations): instantiate it under a
name that says what it does, and the instance IS the cell.

## The script contract

stdin is one JSON document of **exactly three objects**, all of them always set:

| object | what is in it |
|---|---|
| `envelope` | what the substrate wraps around the payload: `header` (both compartments, `context` and `hop`), `target`, `trace_id`, `ttl`, plus `reply_to`, `parent_message_id` and `correlation_id` when the message carries them |
| `body` | the message's own slots -- `messages`, `system`, whatever a cell puts there |
| `params` | this cell's own configuration, as a read copy. Credentials are stripped recursively, and so is the script itself |

stdout is one content JSON: an optional `header` section the colony reads as
`hop`, plus the body slots. A script never writes `context` -- that is edge
authority. **Read the payload out of `body`**; never win it by subtracting a
hardcoded list of envelope keys, which is the error class the three-object form
was built to end.

Since `scriptlet@1.0.1` this contract also travels: the `SCRIPT` line of
`template.json` § `description.examples` carries the same two sentences plus a
worked pass-through and a worked transform, so the catalogue row a composer
reads publishes it and not only this README. A composer that never opens the
directory wrote `{"content": ...}` on stdout and `data["messages"]` on stdin,
and the cell answered `contract_violation`
([#513](https://github.com/mmeyerlein/meclaw/issues/513)). The line names the two
header compartments too, and for the same reason: the verification run of that
issue read `envelope["context"]`, where nothing is, and looped until the TTL
killed it.

## Ports and wiring

```json
[
  { "from": "./feed",   "to": "./dedupe" },
  { "from": "./dedupe", "to": "./headlines",
    "condition": "has(hop.route) && hop.route == 'store'" }
]
```

| field | meaning |
|---|---|
| `hop.exit_code` / `hop.duration_ms` / `hop.had_stderr` | stamped by the cell after the script ends and **not hijackable** -- process metadata belongs to the cell |
| every other hop key | the script's word. Write the lane you want the edge to match on |
| `hop.error_code` | `invalid_input`, `io_error`, `script_timeout`, `script_failed`, `invalid_json`, `multi_send_not_declared`, `contract_violation` |

`contract.emits` is validated **always-on** for a `code` cell -- unconditionally,
whatever the build profile and whatever `colony.json` says -- because this is the
one user-script-driven trust boundary in the substrate. An emission without
`messages[]` comes back as `contract_violation` instead of travelling.

## Knobs

There is no env knob here, and that is a decision rather than an omission: a
script is a per-instance fact, and a colony-wide one would make two scriptlets in
one colony impossible. The logic is `override_params` on the node -- a **flat**
params object, because a single-cell template has no inner cell to address, and
the path-keyed form (`{"": …}`) is refused with `schema`:

```json
{"name": "dedupe", "template": "scriptlet@1.0.1",
 "override_params": {"script_inline": "import sys, json\ndoc = json.load(sys.stdin)\n…"}}
```

| param | default | effect |
|---|---|---|
| `script_inline` | a pass-through | the script. Above 131 072 bytes it stops travelling in `argv` and is written to a `0600` temp file per spawn instead, which changes nothing a script can observe except `__file__` |
| `runner` | `"python3"` | the only accepted value today; any other is a loud param reject at spawn |
| `runner_mode` | `"cold"` | `cold` is a fresh process per message, `warm` a pool that compiles once and runs each message in a fresh namespace, `resident` exactly one child whose namespace SURVIVES. **`resident` forces `max_concurrency` to 1** -- override both or the spawn rejects |
| `external_timeout_ms` | `10000` | the operation timeout per execution, in every mode. On elapse the child is killed (and replaced, in the two warm modes) and the answer is `script_timeout` |
| `max_concurrency` | `4` | how many executions run at once |

## `code.author` -- the price, and whose it is

A manifest that brings executable behaviour is a different kind of manifest. The
submitter derives that from the diff -- an `override_params` carrying a script
key, or an `add_templates` at all -- and asks its colony a **second**, check-only
question over the same scope: `code.author`. It ships disabled, so a fresh colony
applies manifests and authors no code.

A refusal comes back as `code_author_denied`, and it means *this colony does not
allow imported execution*. It does **not** mean the manifest was malformed, and a
composer that reads it as a form error repairs a draft that was never broken.

The question belongs to the authoring path. The mutation door itself
(`POST /colony/mutations`) asks no capability question of anybody: it checks that
an overridden param **exists**, never what it contains. Whoever puts a door in
front of a colony puts the question in front of the door.

## What does NOT live here

- **State.** A `code` cell has no `cell.db`
  ([`docs/cell-types.md`](../../docs/cell-types.md) § `code`). Anything that must
  outlive a message goes to a store over the topology -- `shelf`, if nothing more
  specific fits. A `resident` namespace is a cache of that store and never its
  truth: the same message stream with a child killed in the middle must produce
  the same outputs.
- **A sandbox key.** A cell instantiated from a template without `params.sandbox`
  gets the default-deny profile, whose runtime set is enough for a
  `script_inline`. This template does **not** declare the key, so an
  `override_params` cannot widen it through here -- an override names a param the
  template already declares.
- **A purpose.** Five templates in this library hold one `code` cell and each of
  them knows what it is for. This one does not, and that is the point.

Pinned by
[`crates/meclaw-cells/tests/gh513_the_scriptlet_publishes_its_script_contract.rs`](../../crates/meclaw-cells/tests/gh513_the_scriptlet_publishes_its_script_contract.rs),
which lifts both published scripts out of `template.json` and runs them through
a real `code` cell grown from this template, and by
[`crates/meclaw-cells/tests/gh482_the_composer_can_name_the_cells_it_needs.rs`](../../crates/meclaw-cells/tests/gh482_the_composer_can_name_the_cells_it_needs.rs),
which reads the shipped form and then builds a feed out of it -- a clock, a
fetcher, this scriptlet and a shelf, instantiated by one manifest into a colony
that had none of them.
