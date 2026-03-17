-- MeClaw v0.1.0 — AGE Graph Setup
-- Hives, Bees, Edges

LOAD 'age';
SET search_path = ag_catalog, "$user", public;

-- Create graph if not exists
DO $$ BEGIN
    PERFORM create_graph('meclaw_graph');
EXCEPTION WHEN others THEN
    -- Graph already exists, continue
    NULL;
END $$;

-- Hives (Programme)
SELECT * FROM cypher('meclaw_graph', $$
    MERGE (h:Hive {id: 'main-graph', name: 'Main Graph', type: 'main'})
    RETURN h.id
$$) AS (id agtype);

SELECT * FROM cypher('meclaw_graph', $$
    MERGE (h:Hive {id: 'test-agent', name: 'Test Agent', type: 'agent'})
    RETURN h.id
$$) AS (id agtype);

-- Bees (Nodes)
SELECT * FROM cypher('meclaw_graph', $$
    MERGE (b:Bee {id: 'main-receiver-bee', type: 'receiver_bee', hive: 'main-graph', config: '{}'})
    RETURN b.id
$$) AS (id agtype);

SELECT * FROM cypher('meclaw_graph', $$
    MERGE (b:Bee {id: 'main-call-bee', type: 'call_bee', hive: 'main-graph', config: '{"target_hive": "test-agent"}'})
    RETURN b.id
$$) AS (id agtype);

SELECT * FROM cypher('meclaw_graph', $$
    MERGE (b:Bee {id: 'main-sender-bee', type: 'sender_bee', hive: 'main-graph', config: '{}'})
    RETURN b.id
$$) AS (id agtype);

SELECT * FROM cypher('meclaw_graph', $$
    MERGE (b:Bee {id: 'test-llm-bee', type: 'llm_bee', hive: 'test-agent', config: '{"soul": "Du bist Walter, ein KI-Assistent von Marcus Meyer. Locker, direkt, trocken-humorvoll wenn es passt. Kein Corporate-Deutsch, kein Geschleime. Antworte immer auf Deutsch. Kurz und klar. Du bist ein Fuchs 🦊", "llm_url": "http://10.235.74.1:8000/v1", "llm_model": "Qwen/Qwen3.5-9B", "max_tokens": 1024, "timeout_ms": 120000}'})
    RETURN b.id
$$) AS (id agtype);

-- Edges (Routing)
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Bee {id: 'main-receiver-bee'}), (b:Bee {id: 'main-call-bee'})
    MERGE (a)-[:NEXT {condition: 'on_message'}]->(b)
    RETURN a.id, b.id
$$) AS (a agtype, b agtype);

SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Bee {id: 'main-call-bee'}), (b:Bee {id: 'main-sender-bee'})
    MERGE (a)-[:NEXT {condition: 'on_return'}]->(b)
    RETURN a.id, b.id
$$) AS (a agtype, b agtype);

-- Entry Edges (Hive Entry Points)
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (h:Hive {id: 'main-graph'}), (b:Bee {id: 'main-receiver-bee'})
    MERGE (h)-[:ENTRY]->(b)
    RETURN h.id, b.id
$$) AS (h agtype, b agtype);

-- Context Bee
SELECT * FROM cypher('meclaw_graph', $$
    MERGE (b:Bee {id: 'test-context-bee', type: 'context_bee', hive: 'test-agent', config: '{}'})
    RETURN b.id
$$) AS (id agtype);

-- test-agent Entry → context_bee → llm_bee
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (h:Hive {id: 'test-agent'}), (b:Bee {id: 'test-context-bee'})
    MERGE (h)-[:ENTRY]->(b)
    RETURN h.id, b.id
$$) AS (h agtype, b agtype);

SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Bee {id: 'test-context-bee'}), (b:Bee {id: 'test-llm-bee'})
    MERGE (a)-[:NEXT {condition: 'on_message'}]->(b)
    RETURN a.id, b.id
$$) AS (a agtype, b agtype);

-- Tool Bee
SELECT * FROM cypher('meclaw_graph', $$
    MERGE (b:Bee {id: 'test-tool-bee', name: 'Tool Bee', type: 'tool_bee'})
    RETURN b.id
$$) AS (id agtype);

SELECT * FROM cypher('meclaw_graph', $$
    MATCH (h:Hive {id: 'test-agent'}), (b:Bee {id: 'test-tool-bee'})
    MERGE (h)-[:ENTRY]->(b)
    RETURN h.id, b.id
$$) AS (h agtype, b agtype);

-- LLM → Tool Bee (on_tool_call)
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Bee {id: 'test-llm-bee'}), (b:Bee {id: 'test-tool-bee'})
    MERGE (a)-[:NEXT {condition: 'on_tool_call'}]->(b)
    RETURN a.id, b.id
$$) AS (a agtype, b agtype);

-- Tool Bee → LLM (on_tool_result)
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Bee {id: 'test-tool-bee'}), (b:Bee {id: 'test-llm-bee'})
    MERGE (a)-[:NEXT {condition: 'on_tool_result'}]->(b)
    RETURN a.id, b.id
$$) AS (a agtype, b agtype);
