#!/usr/bin/env python3
"""Phase-9 demo: trigger a store-select unconditionally on every
   inbound message."""
import sys, json, uuid

sys.stdin.read()  # Discard input -- we trigger select unconditionally.
sys.stdout.write(json.dumps({
    "messages": [{
        "origin": "assistant",
        "type": "tool_call",
        "text": json.dumps({
            "operation": "select",
            "table": "items",
            "columns": ["id", "name"],
        }),
        "id": str(uuid.uuid4()),
    }]
}))
