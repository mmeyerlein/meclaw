# `ask` — the smallest colony that answers a turn (GitHub #623)

Two cells and one edge. A door echoes the user turn back as an assistant turn
and stamps the route the answer travels on; a sink ends the lane.

```text
(a turn over HTTP)  ->  /door  --(every emission)-->  /sink
```

The door stamps `hop.route = "answer"`, or `"error"` when the text starts in
`fail`. That is the whole difference between the two, and it is what lets one
fixture cover both exit codes of `meclaw ask` without a provider, a wall clock
or a tool loop. A turn addressed straight at `/sink` is never answered, which is
the third case: the call runs into its `--timeout`.

The directory name `main/` becomes meclaw `/` at bootstrap, which is why the
paths above have no `/main` prefix.

## Run it

```bash
cp -r tests/fixtures/gh623-ask /tmp/ask-fixture
cargo run -p meclaw-cli -- --root /tmp/ask-fixture --daemon --api 127.0.0.1:7777

# in a second shell
cargo run -p meclaw-cli -- ask --api 127.0.0.1:7777 --target /door "hello"
# echo: hello
```

Used by `crates/meclaw-cli/tests/gh623_ask_e2e.rs`.
