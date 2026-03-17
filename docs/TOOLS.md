# MeClaw — Tool Model
As of: 2026-03-17

---

## Principle: Tools = SQL Functions

Every tool is a PostgreSQL function. Registered in a table. The agent discovers tools via the registry, not hardcoded logic.

---

## Flow with Tools

### Without Tools
```
user_input → context_bee → llm_bee → [LLM: stop] → llm_result → sender_bee
```

### With Tools
```
user_input → context_bee → llm_bee → [LLM: tool_calls] → tool_call message
  → tool_bee: executes SQL function → tool_result message
  → llm_bee: receives tool_result as context → [LLM: stop] → llm_result → sender_bee
```

### Multi-Tool (LLM calls multiple tools sequentially)
```
user_input → context → llm → tool_call → tool_bee → tool_result
  → llm → tool_call → tool_bee → tool_result
  → llm → stop → sender
```

No loop in code — each pass is: llm_bee → router → tool_bee → router → llm_bee.
Loop limit: max N tool calls per task (e.g. 5).

---

## Components

### 1. Tool Registry (Table)

```sql
CREATE TABLE meclaw.tools (
    id          TEXT PRIMARY KEY,          -- 'sql_read', 'memory_store', etc.
    name        TEXT NOT NULL,             -- Display name for LLM
    description TEXT NOT NULL,             -- What the tool does (for LLM)
    parameters  JSONB NOT NULL,            -- JSON Schema of parameters
    handler     TEXT NOT NULL,             -- SQL function: 'meclaw.tool_sql_read'
    enabled     BOOLEAN DEFAULT true,
    created_at  TIMESTAMPTZ DEFAULT clock_timestamp()
);
```

Example:
```sql
INSERT INTO meclaw.tools VALUES (
    'sql_read',
    'sql_read',
    'Execute a read-only SQL query against the database. Returns rows as JSON.',
    '{"type": "object", "properties": {"query": {"type": "string", "description": "SQL SELECT query"}}, "required": ["query"]}',
    'meclaw.tool_sql_read'
);
```

### 2. Tool Definitions for LLM (OpenAI Function Calling Format)

```sql
-- Generates tools[] array for the LLM request
CREATE FUNCTION meclaw.get_tool_definitions()
RETURNS jsonb AS $$
    SELECT jsonb_agg(
        jsonb_build_object(
            'type', 'function',
            'function', jsonb_build_object(
                'name', id,
                'description', description,
                'parameters', parameters
            )
        )
    ) FROM meclaw.tools WHERE enabled = true;
$$;
```

### 3. llm_bee: Include Tools in Request

```json
{
    "model": "anthropic/claude-sonnet-4",
    "messages": [...],
    "tools": [
        {
            "type": "function",
            "function": {
                "name": "sql_read",
                "description": "Execute a read-only SQL query...",
                "parameters": {"type": "object", "properties": {"query": {...}}}
            }
        }
    ],
    "tool_choice": "auto"
}
```

### 4. on_net_response_safe: Detect Tool Calls

LLM responds with `finish_reason: "tool_calls"`:
```json
{
    "choices": [{
        "finish_reason": "tool_calls",
        "message": {
            "tool_calls": [{
                "id": "call_abc123",
                "type": "function",
                "function": {
                    "name": "sql_read",
                    "arguments": "{\"query\": \"SELECT count(*) FROM meclaw.messages\"}"
                }
            }]
        }
    }]
}
```

Instead of an `llm_result` message → create a `tool_call` message:
```sql
INSERT INTO meclaw.messages (type, content) VALUES (
    'tool_call',
    '{"tool_call_id": "call_abc123", "tool_name": "sql_read", "arguments": {"query": "..."},
      "llm_messages": [...]}' -- previous messages for returning to LLM
);
```

### 5. tool_bee: Generic Tool Executor

```sql
CREATE FUNCTION meclaw.tool_bee(p_msg_id UUID)
-- 1. Read tool name from message
-- 2. Look up handler from meclaw.tools registry
-- 3. Call handler: EXECUTE format('SELECT %I(%L)', handler, arguments)
-- 4. Write result into tool_result message
-- 5. tool_result contains llm_messages + tool_response → back to llm_bee
```

### 6. llm_bee: Process Tool Results

When `p_content` contains a `tool_result`, llm_bee builds the prompt like this:
```json
{
    "messages": [
        {"role": "system", "content": "...soul..."},
        ...history...,
        {"role": "user", "content": "How many messages are there?"},
        {"role": "assistant", "content": null, "tool_calls": [{"id": "call_abc123", ...}]},
        {"role": "tool", "tool_call_id": "call_abc123", "content": "{\"count\": 500}"}
    ]
}
```

---

## Graph Changes

### Without Tools
```
test-agent: context_bee → llm_bee (no successor → return)
```

### With Tools
```
test-agent:
  context_bee --on_message→ llm_bee
  llm_bee --on_tool_call→ tool_bee
  tool_bee --on_message→ llm_bee
```

The cycle `llm_bee → tool_bee → llm_bee` is a graph edge, not a code loop.

### router_bee Adjustments
- `tool_call` message → condition `on_tool_call`
- `tool_result` message → condition `on_message` (back to llm_bee)

---

## Loop Protection

```sql
-- In llm_bee: check tool call counter
v_tool_count := (p_content->>'tool_call_count')::int;
IF v_tool_count >= 5 THEN
    -- Too many tool calls → forced stop, respond without tool
    -- tool_choice: "none" in next request
END IF;
```

---

## Tool Implementations (v0.1.0)

### sql_read
```sql
CREATE FUNCTION meclaw.tool_sql_read(p_args JSONB)
RETURNS JSONB AS $$
DECLARE v_result JSONB;
BEGIN
    IF NOT (lower(trim(p_args->>'query')) LIKE 'select%') THEN
        RETURN jsonb_build_object('error', 'Only SELECT queries allowed');
    END IF;
    EXECUTE format('SELECT jsonb_agg(row_to_json(t)) FROM (%s) t', p_args->>'query')
    INTO v_result;
    RETURN COALESCE(v_result, '[]'::jsonb);
END;
$$ LANGUAGE plpgsql;
```

### sql_write
```sql
CREATE FUNCTION meclaw.tool_sql_write(p_args JSONB)
RETURNS JSONB AS $$
DECLARE v_count INT;
BEGIN
    EXECUTE p_args->>'query';
    GET DIAGNOSTICS v_count = ROW_COUNT;
    RETURN jsonb_build_object('rows_affected', v_count);
END;
$$ LANGUAGE plpgsql;
```

### python_exec
```sql
CREATE FUNCTION meclaw.tool_python_exec(p_args JSONB)
RETURNS JSONB AS $fn$
    # Execute Python code
    # Available: requests, urllib, json, re, math, os, subprocess
    # Security: the container IS the sandbox
$fn$ LANGUAGE plpython3u;
```

---

## Trigger Adjustments

### on_net_response_safe
```
finish_reason = 'stop'       → llm_result message (as before)
finish_reason = 'tool_calls' → tool_call message (NEW)
```

### trg_on_message_ready_dispatch
```
next_bee LIKE '%-tool-bee' → meclaw.tool_bee(msg_id)
```

### router_bee
```
Message type 'tool_call'   → condition 'on_tool_call'
Message type 'tool_result' → condition 'on_message'
```

---

## Security

- `sql_read`: SELECT only (LIKE check)
- `sql_write`: Audit log on every mutation
- `python_exec`: Container is the sandbox — full access allowed
- **tool_call_count**: max 5 per task → prevents infinite tool loops
- **Rate limiter**: applies to tool calls that trigger LLM calls

---

## Summary

```
1. Tools = SQL functions, registered in meclaw.tools
2. LLM receives tool definitions in request (OpenAI format)
3. finish_reason: tool_calls → tool_call message
4. Generic tool_bee dispatches to handler from registry
5. tool_result → back to llm_bee with tool output
6. Graph edge: llm → tool → llm (no code loop)
7. Loop protection: max 5 tool calls per task
```
