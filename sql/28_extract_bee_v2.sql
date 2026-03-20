-- =============================================================================
-- Phase 6: Real LLM Extraction — extract_bee_v2
-- =============================================================================
-- Replaces the raw-content-only extract_bee with a two-stage pipeline:
--   Stage 1: Store raw content + embedding (same as before)
--   Stage 2: LLM extracts entities + relations → AGE graph + entity resolution
-- =============================================================================

-- -----------------------------------------------------------------------------
-- 1. New columns on brain_events for extraction metadata
-- -----------------------------------------------------------------------------
ALTER TABLE meclaw.brain_events
    ADD COLUMN IF NOT EXISTS extracted BOOLEAN DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS extracted_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS extraction_data JSONB;

-- Index for unextracted events
CREATE INDEX IF NOT EXISTS idx_brain_events_unextracted
    ON meclaw.brain_events (seq) WHERE extracted = FALSE;

-- -----------------------------------------------------------------------------
-- 2. Entity-Event junction table (which entities appear in which events)
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS meclaw.entity_events (
    entity_id TEXT NOT NULL REFERENCES meclaw.entities(id),
    event_id UUID NOT NULL REFERENCES meclaw.brain_events(id),
    relation_type TEXT DEFAULT 'MENTIONED_IN',  -- MENTIONED_IN, CREATED_BY, DISCUSSED, WORKS_ON, etc.
    confidence FLOAT DEFAULT 0.8,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (entity_id, event_id, relation_type)
);

CREATE INDEX IF NOT EXISTS idx_entity_events_event ON meclaw.entity_events(event_id);
CREATE INDEX IF NOT EXISTS idx_entity_events_entity ON meclaw.entity_events(entity_id);

-- -----------------------------------------------------------------------------
-- 2b. AGE graph helpers (avoid SQL MERGE keyword conflict)
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.age_upsert_entity(p_entity_id TEXT, p_name TEXT, p_type TEXT)
RETURNS VOID AS $fn$
    try:
        plpy.execute("SET search_path = ag_catalog, meclaw, public")
        plpy.execute(plpy.prepare("""
            SELECT * FROM cypher('meclaw_graph', $$
                MERGE (e:Entity {entity_id: '%s'})
                SET e.name = '%s', e.type = '%s'
            $$) AS (v agtype)
        """ % (
            p_entity_id.replace("'", "''"),
            p_name.replace("'", "''"),
            p_type.replace("'", "''")
        )))
    except Exception as e:
        plpy.warning(f"age_upsert_entity: {e}")
$fn$ LANGUAGE plpython3u;

CREATE OR REPLACE FUNCTION meclaw.age_link_entity_event(p_entity_id TEXT, p_event_id TEXT)
RETURNS VOID AS $fn$
    try:
        plpy.execute("SET search_path = ag_catalog, meclaw, public")
        plpy.execute("""
            SELECT * FROM cypher('meclaw_graph', $$
                MERGE (e:Entity {entity_id: '%s'})
                MERGE (ev:Event {event_id: '%s'})
                MERGE (e)-[:INVOLVED_IN]->(ev)
            $$) AS (v agtype)
        """ % (
            p_entity_id.replace("'", "''"),
            p_event_id.replace("'", "''")
        ))
    except Exception as e:
        plpy.warning(f"age_link_entity_event: {e}")
$fn$ LANGUAGE plpython3u;

CREATE OR REPLACE FUNCTION meclaw.age_link_entities(p_from TEXT, p_to TEXT, p_type TEXT)
RETURNS VOID AS $fn$
    try:
        plpy.execute("SET search_path = ag_catalog, meclaw, public")
        plpy.execute("""
            SELECT * FROM cypher('meclaw_graph', $$
                MERGE (a:Entity {entity_id: '%s'})
                MERGE (b:Entity {entity_id: '%s'})
                MERGE (a)-[:RELATES_TO {type: '%s'}]->(b)
            $$) AS (v agtype)
        """ % (
            p_from.replace("'", "''"),
            p_to.replace("'", "''"),
            p_type.replace("'", "''")
        ))
    except Exception as e:
        plpy.warning(f"age_link_entities: {e}")
$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 3. create_or_resolve_entity — find existing or create new entity
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.create_or_resolve_entity(
    p_name TEXT,
    p_type TEXT DEFAULT 'concept',
    p_aliases TEXT[] DEFAULT '{}'
) RETURNS TEXT AS $$
DECLARE
    v_entity_id TEXT;
    v_slug TEXT;
BEGIN
    -- Try to resolve existing entity
    v_entity_id := meclaw.resolve_entity(p_name);
    IF v_entity_id IS NOT NULL THEN
        -- Merge aliases if new ones provided
        IF array_length(p_aliases, 1) > 0 THEN
            UPDATE meclaw.entities
            SET aliases = (
                SELECT array_agg(DISTINCT a)
                FROM unnest(aliases || p_aliases) AS a
            ),
            updated_at = clock_timestamp()
            WHERE id = v_entity_id;
        END IF;
        RETURN v_entity_id;
    END IF;

    -- Create slug-based ID
    v_slug := lower(regexp_replace(trim(p_name), '[^a-zA-Z0-9äöüß]+', '-', 'g'));
    v_slug := trim(v_slug, '-');
    v_entity_id := 'meclaw:' || p_type || ':' || v_slug;

    -- Check if slug-ID already exists (collision)
    IF EXISTS (SELECT 1 FROM meclaw.entities WHERE id = v_entity_id) THEN
        RETURN v_entity_id;
    END IF;

    -- Create new entity
    INSERT INTO meclaw.entities (id, canonical_name, entity_type, aliases)
    VALUES (v_entity_id, trim(p_name), p_type, p_aliases)
    ON CONFLICT (id) DO NOTHING;

    -- Create AGE graph node via plpython3u helper to avoid SQL MERGE conflict
    BEGIN
        PERFORM meclaw.age_upsert_entity(v_entity_id, trim(p_name), p_type);
    EXCEPTION WHEN OTHERS THEN
        -- AGE errors shouldn't block extraction
        NULL;
    END;

    RETURN v_entity_id;
END;
$$ LANGUAGE plpgsql;

-- -----------------------------------------------------------------------------
-- 4. llm_extract_entities — LLM-based entity + relation extraction
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.llm_extract_entities(p_event_id UUID)
RETURNS VOID AS $fn$
import json
import requests

# Get event content
plan = plpy.prepare("SELECT content, channel_id FROM meclaw.brain_events WHERE id = $1", ["uuid"])
result = plan.execute([str(p_event_id)])
if not result or not result[0]["content"]:
    return

content = result[0]["content"][:4000]  # Cap for cost control
channel_id = result[0]["channel_id"]

if len(content.strip()) < 10:
    # Too short to extract meaningful entities
    plpy.execute(plpy.prepare(
        "UPDATE meclaw.brain_events SET extracted = TRUE, extracted_at = clock_timestamp(), extraction_data = $1::jsonb WHERE id = $2",
        ["text", "uuid"]
    ), [json.dumps({"skipped": "too_short"}), str(p_event_id)])
    return

# Get LLM provider config
plan_prov = plpy.prepare("SELECT base_url, api_key, config FROM meclaw.llm_providers WHERE id = $1", ["text"])
prov = plan_prov.execute(["openrouter"])
if not prov:
    plpy.warning("llm_extract_entities: no openrouter provider found")
    return

api_key = prov[0]["api_key"]
config = json.loads(prov[0]["config"]) if prov[0]["config"] else {}

# Use cheap model for extraction
model = "openai/gpt-4o-mini"

prompt = f"""Extract entities and relations from this message. Return ONLY valid JSON.

Message:
---
{content}
---

Return this exact JSON structure:
{{
  "entities": [
    {{"name": "exact name", "type": "person|project|tool|concept|organization|location|event", "aliases": []}}
  ],
  "relations": [
    {{"subject": "entity name", "predicate": "works_on|uses|discusses|mentions|created|related_to", "object": "entity name"}}
  ]
}}

Rules:
- Only extract clearly stated entities, don't infer
- Use the most specific type possible
- Relations must reference entities from the entities list
- If no entities found, return {{"entities": [], "relations": []}}
- Names should be canonical (e.g. "Marcus Meyer" not "Marcus")"""

try:
    resp = requests.post(
        "https://openrouter.ai/api/v1/chat/completions",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://meclaw.ai",
            "X-Title": "MeClaw"
        },
        json={
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.0,
            "max_tokens": 1000,
            "response_format": {"type": "json_object"}
        },
        timeout=30
    )
    resp.raise_for_status()
    data = resp.json()

    llm_output = data.get("choices", [{}])[0].get("message", {}).get("content", "")
    usage = data.get("usage", {})

    # Parse LLM response
    try:
        extracted = json.loads(llm_output)
    except json.JSONDecodeError:
        # Try to find JSON in the response
        import re
        match = re.search(r'\{.*\}', llm_output, re.DOTALL)
        if match:
            extracted = json.loads(match.group())
        else:
            plpy.warning(f"llm_extract_entities: could not parse LLM response for {p_event_id}")
            plpy.execute(plpy.prepare(
                "UPDATE meclaw.brain_events SET extracted = TRUE, extracted_at = clock_timestamp(), extraction_data = $1::jsonb WHERE id = $2",
                ["text", "uuid"]
            ), [json.dumps({"error": "parse_failed", "raw": llm_output[:500]}), str(p_event_id)])
            return

    entities = extracted.get("entities", [])
    relations = extracted.get("relations", [])

    # Process entities
    entity_ids = {}
    for ent in entities:
        name = ent.get("name", "").strip()
        etype = ent.get("type", "concept").strip()
        aliases = ent.get("aliases", [])
        if not name or len(name) < 2:
            continue

        try:
            row = plpy.execute(plpy.prepare(
                "SELECT meclaw.create_or_resolve_entity($1, $2, $3::text[])",
                ["text", "text", "text[]"]
            ), [name, etype, aliases])
            entity_id = row[0]["create_or_resolve_entity"]
            entity_ids[name.lower()] = entity_id

            # Link entity to event
            plpy.execute(plpy.prepare("""
                INSERT INTO meclaw.entity_events (entity_id, event_id, relation_type, confidence)
                VALUES ($1, $2, 'MENTIONED_IN', 0.8)
                ON CONFLICT (entity_id, event_id, relation_type) DO NOTHING
            """, ["text", "uuid"]), [entity_id, str(p_event_id)])

            # AGE: Entity INVOLVED_IN Event
            try:
                plpy.execute(plpy.prepare(
                    "SELECT meclaw.age_link_entity_event($1, $2)",
                    ["text", "text"]
                ), [entity_id, str(p_event_id)])
            except Exception:
                pass  # AGE errors non-fatal
        except Exception as ex:
            plpy.warning(f"llm_extract_entities: entity processing failed for '{name}': {ex}")

    # Process relations
    for rel in relations:
        subj = rel.get("subject", "").strip().lower()
        pred = rel.get("predicate", "related_to").strip().upper()
        obj = rel.get("object", "").strip().lower()

        subj_id = entity_ids.get(subj)
        obj_id = entity_ids.get(obj)

        if not subj_id or not obj_id or subj_id == obj_id:
            continue

        # Store typed relation in entity_events
        try:
            plpy.execute(plpy.prepare("""
                INSERT INTO meclaw.entity_events (entity_id, event_id, relation_type, confidence)
                VALUES ($1, $2, $3, 0.7)
                ON CONFLICT (entity_id, event_id, relation_type) DO NOTHING
            """, ["text", "uuid", "text"]), [subj_id, str(p_event_id), pred])
        except Exception:
            pass

        # AGE: Typed edge between entities
        try:
            plpy.execute(plpy.prepare(
                "SELECT meclaw.age_link_entities($1, $2, $3)",
                ["text", "text", "text"]
            ), [subj_id, obj_id, pred])
        except Exception:
            pass

    # Store extraction result
    extraction_meta = {
        "entities_found": len(entities),
        "relations_found": len(relations),
        "entity_ids": entity_ids,
        "model": model,
        "usage": usage
    }

    plpy.execute(plpy.prepare(
        "UPDATE meclaw.brain_events SET extracted = TRUE, extracted_at = clock_timestamp(), extraction_data = $1::jsonb WHERE id = $2",
        ["text", "uuid"]
    ), [json.dumps(extraction_meta), str(p_event_id)])

    # Log cost
    total_tokens = usage.get("total_tokens", 0)
    prompt_tokens = usage.get("prompt_tokens", 0)
    completion_tokens = usage.get("completion_tokens", 0)

    plpy.execute(plpy.prepare("""
        INSERT INTO meclaw.events (bee_type, event, payload)
        VALUES ('extract_bee', 'llm_extraction_complete', $1::jsonb)
    """, ["text"]), [json.dumps({
        "event_id": str(p_event_id),
        "entities": len(entities),
        "relations": len(relations),
        "model": model,
        "tokens": {"prompt": prompt_tokens, "completion": completion_tokens, "total": total_tokens}
    })])

except Exception as e:
    plpy.warning(f"llm_extract_entities failed for {p_event_id}: {e}")
    plpy.execute(plpy.prepare(
        "UPDATE meclaw.brain_events SET extraction_data = $1::jsonb WHERE id = $2",
        ["text", "uuid"]
    ), [json.dumps({"error": str(e)[:500]}), str(p_event_id)])

$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 5. Updated extract_bee — two-stage pipeline
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.extract_bee(p_msg_id UUID)
RETURNS VOID AS $$
DECLARE
    v_channel_id UUID;
    v_content TEXT;
    v_message_type TEXT;
    v_task_id UUID;
    v_event_id UUID;
BEGIN
    -- Get message details
    SELECT channel_id, content->>'input', type, task_id
    INTO v_channel_id, v_content, v_message_type, v_task_id
    FROM meclaw.messages
    WHERE id = p_msg_id;

    -- Only extract from user_input and llm_result messages
    IF v_message_type NOT IN ('user_input', 'llm_result') THEN
        RETURN;
    END IF;

    -- Skip if no content
    IF v_content IS NULL OR v_content = '' THEN
        SELECT content->>'output' INTO v_content
        FROM meclaw.messages WHERE id = p_msg_id;

        IF v_content IS NULL OR v_content = '' THEN
            RETURN;
        END IF;
    END IF;

    -- Stage 1: Create brain_event with raw content (same as before)
    INSERT INTO meclaw.brain_events (
        message_id, channel_id, agent_id, content, extracted
    ) VALUES (
        p_msg_id, v_channel_id, NULL, v_content, FALSE
    ) RETURNING id INTO v_event_id;

    -- Stage 1b: Compute embedding asynchronously
    BEGIN
        PERFORM pg_background_launch(
            format('SELECT meclaw.compute_embedding(%L::uuid)', v_event_id)
        );
    EXCEPTION WHEN OTHERS THEN
        INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
        VALUES (p_msg_id, v_task_id, 'extract_bee', 'embedding_bg_failed',
            jsonb_build_object('error', SQLERRM, 'event_id', v_event_id));
    END;

    -- Stage 2: LLM entity extraction asynchronously
    BEGIN
        PERFORM pg_background_launch(
            format('SELECT meclaw.llm_extract_entities(%L::uuid)', v_event_id)
        );
    EXCEPTION WHEN OTHERS THEN
        INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
        VALUES (p_msg_id, v_task_id, 'extract_bee', 'llm_extraction_bg_failed',
            jsonb_build_object('error', SQLERRM, 'event_id', v_event_id));
    END;

    -- Update channel extraction tracking
    UPDATE meclaw.channels
    SET last_extracted_seq = COALESCE(
        (SELECT MAX(seq) FROM meclaw.brain_events WHERE channel_id = v_channel_id), 0
    ),
    extraction_status = 'idle',
    updated_at = clock_timestamp()
    WHERE id = v_channel_id;

    -- Log the extraction event
    INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
    VALUES (p_msg_id, v_task_id, 'extract_bee', 'extraction_queued',
        jsonb_build_object('channel_id', v_channel_id, 'content_length', length(v_content), 'event_id', v_event_id, 'stages', ARRAY['embedding', 'llm_extraction']));
END;
$$ LANGUAGE plpgsql;

-- -----------------------------------------------------------------------------
-- 6. Backfill function — extract entities from existing unextracted events
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.backfill_extractions(p_limit INT DEFAULT 10)
RETURNS INT AS $fn$
import time

plan = plpy.prepare("""
    SELECT id FROM meclaw.brain_events
    WHERE extracted = FALSE AND content IS NOT NULL AND length(content) >= 10
    ORDER BY seq ASC LIMIT $1
""", ["int4"])
events = plan.execute([p_limit])

count = 0
for row in events:
    event_id = str(row["id"])
    try:
        plpy.execute(plpy.prepare(
            "SELECT meclaw.llm_extract_entities($1::uuid)", ["uuid"]
        ), [event_id])
        count += 1
        time.sleep(0.5)  # Rate limit protection
    except Exception as e:
        plpy.warning(f"backfill_extractions: failed for {event_id}: {e}")

return count
$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 7. Mark existing events as unextracted for backfill
-- -----------------------------------------------------------------------------
UPDATE meclaw.brain_events SET extracted = FALSE WHERE extracted IS NULL;
