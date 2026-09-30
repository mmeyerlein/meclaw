# gh921

The tree of the webhook proof for GH #921. A real colony, booted in the test
process; two `proxy` cells on `platform: "webhook"` hold mounts on the colony's
one listener.

## Layout

```
gh921/
  main/                  # hive marker -- becomes meclaw "/" at bootstrap
    config.json          # hook, token -> sink on their lanes; both -> receipts
    hook/
      config.json        # mount "probe-hook", hmac_sha256 over the raw body,
                         # header X-Probe-Signature, prefix "sha256=", 1 KiB limit
    token/
      config.json        # mount "probe-token", token in X-Probe-Token
    sink/
      config.json        # code cell, terminal sink for the arrivals
    receipts/
      config.json        # code cell, terminal sink for the receipts
```

## Flow

A POST to `/probe-hook/` or `/probe-token/` is verified against the secret the
mount holds (`${PROBE_HOOK_SECRET}`, `${PROBE_TOKEN_SECRET}`; the test writes
both into `.env`). A verified request is answered `202` and emitted once, with
the lane as `hop.route`, over the cell's out-edge to `/sink`; next to it the
cell books a `crossed` receipt. A request that does not prove itself is answered
`401`, emits nothing, and leaves a `refused` receipt with `webhook_unverified`.

## Why the receipt edge exists

A receipt is an ordinary emission of the cell and is routed over its out-edges.
Without the edge on `hop.route == 'receipt'` every receipt would find no route
and end as a `no_route` dead letter.
