# `fetcher@1.0.0`

An outbound HTTP GET, as one `web_fetch` cell. It exists because of a gap the
library had rather than the substrate: three shipped templates carry a
`web_fetch` inside them -- `daily-digest/fetcher`, `research-assistant/reader`
and `tools/web_fetch` -- and **not one of them was instantiable on its own**. An `add_nodes` entry requires a `name` and a `template`, there is
no form for a bare cell, and so a running colony could not be given the ability
to read a document off the network by any manifest, by any caller, through any
door ([#482](https://github.com/mmeyerlein/meclaw/issues/482)).

That is the whole template: one cell, five knobs, and no target.

## What it delivers

- **A fetch a declaration can ask for.** `{"name": "feed", "template": "fetcher"}`
  and two edges, in the same mutation, and a colony that could only talk to
  itself can read a page.
- **No URL of its own.** The address rides in the `tool_call` args on the wire
  (`{"url": "https://..."}`), because that is where a `web_fetch` cell reads it.
  A fetcher with a target baked into its config would be a fetcher for one
  caller, and the second caller would need a second template.
- **An egress policy that is default-deny.** `web_fetch` runs in the daemon
  process, so `sandbox.network` can never cover it; the cell enforces its own
  private-network deny, re-checks every redirect hop before connecting, and
  keeps link-local shut in **both** steps of the opt-out.
- **A size cap that cuts visibly.** `max_bytes` ends the fetched body with
  `… [truncated, <N> bytes total]`, sets `hop.truncated` and reports the FULL
  size on `hop.bytes` -- a silently shortened payload is worse than a marked one.

## The cell

| cell | type | what it holds |
|---|---|---|
| the template root | `web_fetch` | a reqwest client and five numbers. No `cell.db`, no state, no memory of the last call. |

Single-cell template (one cell of one cell type, the smallest `config.json`
that starts it, and a README that explains its declarations): instantiate it
under a name that says what it fetches, and the instance IS the cell.

## Ports and wiring

Two edges, drawn in the mutation that instantiates it:

```json
[
  { "from": "./ask",   "to": "./feed" },
  { "from": "./feed",  "to": "./parse",
    "condition": "has(hop.http_status) && hop.http_status >= 200 && hop.http_status < 300" }
]
```

| field | meaning |
|---|---|
| `hop.operation` | always `'web_fetch'`, on the error surface too |
| `hop.http_status` | the answer's status. **A non-2xx is a normal result**, not an error -- routing decides |
| `hop.content_type` / `hop.bytes` | what came back and how much of it (the full size, even when cut) |
| `hop.truncated` | present and `true` when `max_bytes` cut the body |
| `hop.redirects` / `hop.final_url` | only after at least one hop: a body that came from somewhere other than the address asked for says so |
| `hop.error_code` | `io_error`, `timeout`, `invalid_input`, `target_blocked`, `too_many_redirects`, `invalid_redirect` |

The condition on the outbound edge is worth drawing: without it a 404 page and a
document take the same lane, and whatever is downstream parses an error page.

## Knobs

There is no env knob here, and that is a decision rather than an omission: four
of the five values are a number or a boolean, a `${VAR}` token is a string, and
a colony-wide default for a per-caller budget is the wrong shape anyway. The
knobs are `override_params` on the node -- a **flat** params object, because a
single-cell template has no inner cell to address, and the path-keyed form
(`{"": …}`) is refused with `schema`:

```json
{"name": "feed", "template": "fetcher@1.0.0",
 "override_params": {"max_bytes": 65536, "external_timeout_ms": 15000}}
```

| param | default | effect |
|---|---|---|
| `external_timeout_ms` | `30000` | operation timeout over the WHOLE call, redirect chain included -- a redirect budget is not a time budget |
| `max_concurrency` | `4` | how many fetches this cell runs at once |
| `max_bytes` | `262144` | size cap on the body handed to the caller. **Inside a tool loop a fetched body is re-sent every round**, so a fetch costs per remaining round, not once; in an agent loop set it far lower |
| `allow_private_networks` | `false` | opens loopback, RFC 1918, ULA, CGNAT and site-local for a mock server or a service on the same host. **Link-local stays shut either way** -- that is where the cloud metadata endpoint lives |
| `max_redirects` | `5` | how many hops the cell follows. Every one of them, the first included, passes the deny again |

## What does NOT live here

- **Every method but GET.** `POST`, `PUT`, `PATCH`, `DELETE` and the
  `method`/`headers`/`body` control that goes with them are a core roadmap defer
  of the cell type ([`docs/cell-types.md`](../../docs/cell-types.md) §
  `web_fetch`). A template cannot ship what the cell does not have.
- **Retries.** A failed fetch is an answer with an `error_code`; whoever wants a
  second attempt draws the lane, and `retry` is a template.
- **Parsing.** What comes back is a string. Turning it into something is the job
  of the cell on the other side of the outbound edge -- `scriptlet`, if nothing
  more specific fits.
- **A target.** See above. It is the one thing this template deliberately does
  not know.

Pinned by
[`crates/meclaw-cells/tests/gh482_the_composer_can_name_the_cells_it_needs.rs`](../../crates/meclaw-cells/tests/gh482_the_composer_can_name_the_cells_it_needs.rs),
which reads the shipped form and then builds a feed out of it -- a clock, this
fetcher, a scriptlet and a shelf, instantiated by one manifest into a colony
that had none of them.
