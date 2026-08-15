#!/usr/bin/env bash
# meclaw 60-second demo: start the swarm daemon, watch one task run end to end.
#
# Record it as an asciinema cast with:
#   asciinema rec docs/demo.cast -c "bash docs/demo.sh"
#
# Prereqs: a release build (cargo build --release) and an OpenAI-compatible key.
# Put OPENROUTER_API_KEY=... in examples/swarm/.env, or export it before running.
set -euo pipefail

API=127.0.0.1:7777
ROOT=./examples/swarm

echo '# meclaw: build an agentic harness swarm as a directory tree.'
echo '# starting the swarm colony as a daemon...'
echo
./target/release/meclaw --root "$ROOT" --daemon --api "$API" --env "$ROOT/.env" >/tmp/meclaw-demo.log 2>&1 &
DAEMON=$!
trap 'kill $DAEMON 2>/dev/null || true' EXIT
sleep 3

echo "\$ curl -s http://$API/health"
curl -s "http://$API/health"; echo
echo "# UI is live at http://$API/ui/"
echo

echo '# send the swarm a task. it enters at /prep:'
echo "\$ curl -X POST http://$API/messages -d '{...What is 6 * 7? Use a tool...}'"
curl -s -X POST "http://$API/messages" -H 'Content-Type: application/json' -d '{
  "target": "/prep",
  "body": {"messages": [{"origin": "user", "type": "text", "text": "What is 6 * 7? Use a tool."}]}
}'; echo
sleep 6

echo
echo '# the trace: llm -> dispatch -> calc -> collector -> llm (the loopback) -> done'
TID=$(curl -s "http://$API/colony/trace?limit=1" | python3 -c 'import sys,json;print(json.load(sys.stdin)["trace"][0]["trace_id"])')
curl -s "http://$API/colony/trace?trace_id=$TID&limit=40" | python3 -c '
import sys, json
for h in sorted(json.load(sys.stdin)["trace"], key=lambda x: x["created_at"]):
    print("  %-10s -> %-10s" % (h["from_path"], h["to_path"]))
'
echo
echo '# the answer, straight from the model, with the tool result fed back over an edge:'
curl -s "http://$API/colony/trace?trace_id=$TID&limit=40" | python3 -c '
import sys, json
rows = sorted(json.load(sys.stdin)["trace"], key=lambda x: x["created_at"])
for h in rows:
    if h["to_path"] == "/done":
        b = json.loads(h.get("body_payload") or "{}")
        for m in b.get("messages", []):
            if m.get("type") == "text":
                print("  ->", m.get("text"))
'
echo
echo '# dead letters:'
curl -s "http://$API/colony/dead_letters"; echo
echo
echo '# no loops were used in the making of this run. the loop is an edge.'
