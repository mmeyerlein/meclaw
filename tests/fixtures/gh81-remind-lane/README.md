# `remind` — a timer driven as a tool lane (GitHub #81)

The smallest colony in which an agent schedules something, gets a `tool_result`
back, and sees the schedule fire. No bridge cell anywhere: the `timer` is reached
the same way `bash` and `file` are reached.

```text
(agent turn)  ->  /dispatch  --has(hop.tool_name) && hop.tool_name == 'remind'-->  /remind
                                --has(hop.msg_type) && hop.msg_type == 'timer_op_ack'-->    /ack
                                --has(hop.msg_type) && hop.msg_type == 'timer_op_error'-->  /drain
                                --has(hop.schedule_name) && hop.schedule_name == '...'-->   /notify
```

`dispatch` is the ordinary dispatcher a tool loop already has. It unwraps the
assistant's `{name, arguments}` into a `tool_call` turn and sets `hop.tool_name`;
it knows nothing about timers. In a full loop, `/ack` is the collector and the
`tool_result` fans back in on its `id`.

The directory name `main/` becomes meclaw `/` at bootstrap, which is why the
paths above have no `/main` prefix.

## Run it

```bash
cp -r tests/fixtures/gh81-remind-lane /tmp/remind-lane
cargo run --bin meclaw -- --root /tmp/remind-lane --api 127.0.0.1:7811
```

Schedule something. The `at` is parked in 2099 so nothing fires by accident:

```bash
curl -s -X POST http://127.0.0.1:7811/messages -H 'Content-Type: application/json' -d '{
  "target": "/dispatch",
  "body": {"messages": [{
    "origin": "assistant", "type": "tool_call", "id": "call-1",
    "text": "{\"name\":\"remind\",\"arguments\":\"{\\\"op\\\":\\\"add\\\",\\\"schedule_id\\\":\\\"0190a3f2-0000-7000-8000-0000000000e1\\\",\\\"schedule_name\\\":\\\"assistant-reminder\\\",\\\"at\\\":\\\"2099-01-01T09:00:00Z\\\",\\\"emit_to\\\":\\\"/notify\\\",\\\"emit_body\\\":{\\\"messages\\\":[{\\\"origin\\\":\\\"user\\\",\\\"type\\\":\\\"text\\\",\\\"text\\\":\\\"stretch your legs\\\"}]}}\"}"
  }]}}'

# the ack, carrying call-1
curl -s 'http://127.0.0.1:7811/colony/messages?to_path_prefix=/ack&limit=5'
```

Fire it now (`op: trigger`, GitHub #17), rather than waiting until 2099:

```bash
curl -s -X POST http://127.0.0.1:7811/messages -H 'Content-Type: application/json' -d '{
  "target": "/dispatch",
  "body": {"messages": [{
    "origin": "assistant", "type": "tool_call", "id": "call-2",
    "text": "{\"name\":\"remind\",\"arguments\":\"{\\\"op\\\":\\\"trigger\\\",\\\"schedule_id\\\":\\\"0190a3f2-0000-7000-8000-0000000000e1\\\"}\"}"
  }]}}'

# the firing, with the full auto-header set
curl -s 'http://127.0.0.1:7811/colony/messages?to_path_prefix=/notify&limit=5'
```

An op on an unknown `schedule_id` answers on the same `tool_call` id and lands in
`/drain`. Every port that can carry an error needs a destination; an unrouted
error is a caller waiting forever.

## Pinned by

`crates/meclaw-cli/tests/gh81_remind_lane_e2e.rs` boots this exact tree and reads
all four lanes.
