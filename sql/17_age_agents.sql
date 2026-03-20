-- MeClaw v0.1.0 — AGE Graph: Agent & Channel Nodes
-- Date: 2026-03-20
-- Ref: docs/ARCHITECTURE.md (Agent = Multi-Hive Root, System Agent)
--
-- Extends the AGE graph with:
--   - Agent nodes (system, user-facing agents)
--   - Channel nodes
--   - OWNS edges (Agent → Hive)
--   - SUBSCRIBES edges (Agent → Channel)
--   - SERVES edges (Agent → Person entity)

LOAD 'age';
SET search_path = ag_catalog, "$user", public;

-- =============================================================================
-- 1. System Agent — the first agent, owns all infrastructure hives
-- =============================================================================

SELECT * FROM cypher('meclaw_graph', $$
    MERGE (a:Agent {
        id: 'meclaw:agent:system',
        name: 'System Agent',
        type: 'system',
        description: 'Infrastructure agent. Owns router, channel IO, admin, and logging hives.'
    })
    RETURN a.id
$$) AS (id agtype);

-- System Agent OWNS existing infrastructure hives
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Agent {id: 'meclaw:agent:system'}), (h:Hive {id: 'main-graph'})
    MERGE (a)-[:OWNS]->(h)
    RETURN a.id, h.id
$$) AS (agent agtype, hive agtype);

-- =============================================================================
-- 2. Channel Nodes — mirror meclaw.channels into the graph
-- =============================================================================

-- telegram-main channel
SELECT * FROM cypher('meclaw_graph', $$
    MERGE (c:Channel {
        id: 'channel:telegram-main',
        channel_type: 'telegram',
        external_id: '300850023',
        name: 'telegram-main'
    })
    RETURN c.id
$$) AS (id agtype);

-- web-admin channel
SELECT * FROM cypher('meclaw_graph', $$
    MERGE (c:Channel {
        id: 'channel:web-admin',
        channel_type: 'web',
        name: 'web-admin'
    })
    RETURN c.id
$$) AS (id agtype);

-- System Agent subscribes to all infrastructure channels
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Agent {id: 'meclaw:agent:system'}), (c:Channel {id: 'channel:telegram-main'})
    MERGE (a)-[:SUBSCRIBES {scope: 'shared', role: 'owner'}]->(c)
    RETURN a.id, c.id
$$) AS (agent agtype, channel agtype);

SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Agent {id: 'meclaw:agent:system'}), (c:Channel {id: 'channel:web-admin'})
    MERGE (a)-[:SUBSCRIBES {scope: 'shared', role: 'owner'}]->(c)
    RETURN a.id, c.id
$$) AS (agent agtype, channel agtype);

-- =============================================================================
-- 3. Test Agent → proper Agent node with OWNS
-- =============================================================================

-- The existing 'test-agent' hive belongs to a "walter" agent
-- (currently it's the only user-facing hive in the system)
SELECT * FROM cypher('meclaw_graph', $$
    MERGE (a:Agent {
        id: 'meclaw:agent:walter',
        name: 'Walter',
        type: 'agent',
        description: 'Personal AI assistant for Marcus Meyer. Locker, direkt, trocken-humorvoll.'
    })
    RETURN a.id
$$) AS (id agtype);

-- Walter OWNS the test-agent hive
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Agent {id: 'meclaw:agent:walter'}), (h:Hive {id: 'test-agent'})
    MERGE (a)-[:OWNS]->(h)
    RETURN a.id, h.id
$$) AS (agent agtype, hive agtype);

-- Walter subscribes to telegram channel
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Agent {id: 'meclaw:agent:walter'}), (c:Channel {id: 'channel:telegram-main'})
    MERGE (a)-[:SUBSCRIBES {scope: 'private', role: 'participant'}]->(c)
    RETURN a.id, c.id
$$) AS (agent agtype, channel agtype);

-- Walter subscribes to web-admin channel
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Agent {id: 'meclaw:agent:walter'}), (c:Channel {id: 'channel:web-admin'})
    MERGE (a)-[:SUBSCRIBES {scope: 'private', role: 'participant'}]->(c)
    RETURN a.id, c.id
$$) AS (agent agtype, channel agtype);

-- =============================================================================
-- 4. Person Entity: Marcus Meyer (in the graph)
-- =============================================================================

SELECT * FROM cypher('meclaw_graph', $$
    MERGE (p:Entity {
        id: 'meclaw:person:marcus-meyer',
        name: 'Marcus Meyer',
        type: 'person'
    })
    RETURN p.id
$$) AS (id agtype);

-- Marcus communicates via telegram channel
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (p:Entity {id: 'meclaw:person:marcus-meyer'}), (c:Channel {id: 'channel:telegram-main'})
    MERGE (p)-[:COMMUNICATES_VIA]->(c)
    RETURN p.id, c.id
$$) AS (person agtype, channel agtype);

-- Walter SERVES Marcus
SELECT * FROM cypher('meclaw_graph', $$
    MATCH (a:Agent {id: 'meclaw:agent:walter'}), (p:Entity {id: 'meclaw:person:marcus-meyer'})
    MERGE (a)-[:SERVES {trust_level: 'full'}]->(p)
    RETURN a.id, p.id
$$) AS (agent agtype, person agtype);
