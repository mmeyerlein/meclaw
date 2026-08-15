#!/usr/bin/env python3
"""Phase-9 demo: read input message JSON on stdin, emit a JSON-Array
   of N store-insert tool_calls on stdout (Multi-Send)."""
import sys, json, uuid

inp = json.load(sys.stdin)["body"]
items = inp.get("items", [])
out = []
for it in items:
    out.append({
        "messages": [{
            "origin": "assistant",
            "type": "tool_call",
            "text": json.dumps({"operation": "insert", "table": "items", "row": it}),
            "id": str(uuid.uuid4()),
        }]
    })
sys.stdout.write(json.dumps(out))
