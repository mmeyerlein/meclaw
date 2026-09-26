# peer-north

The sending half of the two-colony proof for GH #617. A real colony, booted in
the test process next to `peer-south`; a message leaves it over one declared
lane and comes back only as a receipt.

## Layout

```
peer-north/
  main/                  # hive marker -- becomes meclaw "/" at bootstrap
    config.json          # root hive: writer -> friend, friend -> receipts, / -> writer
    writer/
      config.json        # code cell, stamps the lane into its header
    friend/
      config.json        # proxy, platform "meclaw", emits lanes "topic" and "gossip"
    receipts/
      config.json        # code cell, terminal sink for the receipts
```

## Flow

The driver posts a turn to `/writer` and names the lane in `hop.route`. The
writer copies the body and stamps that route into its own header, which the
substrate lifts into the `hop` compartment. The edge from `/writer` to `/friend`
adds `hop.peer` and `hop.peer_url` and nothing else; both stay on this side and
never cross. The peer cell judges the message against its own `emits` lanes,
projects the body onto the lane's field list, posts one frame to `peer_url`, and
emits a receipt on `hop.route: "receipt"`. That receipt travels over the peer
cell's own out-edge to `/receipts`: `crossed` with the fields it let through, or
`refused` with the `error_code` and a detail text.

## __PEER_URL__

The literal `__PEER_URL__` in `main/config.json` is filled at run time with the
address of the reverse proxy in front of the south colony. Its port is ephemeral,
so the replacement happens only in the TempDir copy the test boots; the committed
tree keeps the literal.

## __PEER_ORIGIN__

The literal `__PEER_ORIGIN__` in `main/friend/config.json` is the one entry of
the peer cell's `params.egress` (GH #840). It is filled in the same TempDir copy
with the origin of that reverse proxy (`http://127.0.0.1:<port>`): without the
entry the peer cell sends nothing out, and with any other origin every crossing
is `egress_denied`.
