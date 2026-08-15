# subcolony-echo

A minimal, self-contained child colony for the `subcolony` cell type. It is a
REAL colony, not a stub: the parent drives it as an ordinary `meclaw` process
over the JSON stdio wire.

## Layout

```
subcolony-echo/
  child/                 # hive marker — becomes meclaw "/" at bootstrap
    config.json          # root hive, ingress edge + return edge
    echo/
      config.json        # code cell, inline Python
```

`assert_single_root_dir` strips the filesystem root name: `child/` becomes the
meclaw root `/`, and the cell below it is `/echo`.

## Flow

An ingress frame becomes a message at `/`. The ingress edge forwards it to
`/echo` (condition `!has(hop.finish_reason)`, so only inbound messages match).
The code cell answers and sets `header.finish_reason`, which lands in the `hop`
compartment. The return edge sends the reply back to `/`, where the ingress edge
no longer matches — no out-edge, so the message egresses to stdout as a JSON
reply frame.

## Run it by hand

```bash
printf '%s\n' '{"v":1,"type":"message","body":{"messages":[{"origin":"user","type":"text","text":"hello"}]}}' \
  | meclaw --root tests/fixtures/subcolony-echo --stdio-format json
```

Expect a `ready` frame followed by a reply frame.
