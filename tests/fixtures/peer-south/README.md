# peer-south

The receiving half of the two-colony proof for GH #617. A real colony, booted in
the test process next to `peer-north`; its peer cell is mounted on the colony's
one listener and accepts exactly one declared lane.

## Layout

```
peer-south/
  main/                  # hive marker -- becomes meclaw "/" at bootstrap
    config.json          # root hive: friend -> sink, friend -> receipts, / -> friend
    friend/
      config.json        # proxy, platform "meclaw", mount "peer", accepts lane "topic"
    sink/
      config.json        # code cell, terminal sink for the arrivals
    receipts/
      config.json        # code cell, terminal sink for the receipts
```

## Flow

A frame is posted to `/peer/` on the colony's listener. Who sent it comes only
from the `X-Meclaw-Peer` header, which the reverse proxy in front writes after it
has checked the sender; a frame that names a sender itself is `invalid_frame`, and
a request without the header is refused the same way. The mount judges the frame
against this side's own `accepts` lanes and answers the receipt on the wire. An
accepted frame becomes an arrival on the lane's route with `hop.peer` set to the
header value, carrying the frame's trace, and travels over the peer cell's
out-edge to `/sink`. Next to it the peer cell books its own `crossed` receipt; a
refused frame leaves a `refused` receipt instead.

## Why the receipt edge exists

A receipt is an ordinary emission of the peer cell and is routed over its
out-edges. Without the edge on `hop.route == 'receipt'` every receipt this side
books would find no route and end as a `no_route` dead letter.
