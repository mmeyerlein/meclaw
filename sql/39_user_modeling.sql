-- MeClaw v0.1.0 — Phase C3: User Modeling aktivieren
-- Date: 2026-03-21
-- Ref: docs/BRAIN.md (Entity Observations, User Preferences, observed_profile)
--
-- Aufgaben:
-- 1. llm_extract_entities: Prompt um User-Präferenzen erweitert
--    → preferences[] im LLM-Output → observe_entity() Calls
-- 2. consolidate_observations(): standalone Funktion obs → observed_profile
-- 3. Rückwärtskompatibel: extraction_data Format unverändert
-- =============================================================================

-- =============================================================================
-- 1. consolidate_observations() — standalone Sync: entity_observations → observed_profile
-- =============================================================================
-- Kann on-demand aufgerufen werden (ergänzt den nightly consolidation_bee).
-- Aggregiert alle aktiven Observations eines Entities zu observed_profile JSONB.
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.consolidate_observations(
    p_entity_id TEXT DEFAULT NULL,   -- NULL = alle Entities
    p_agent_id  TEXT DEFAULT NULL    -- NULL = alle Agents
) RETURNS TABLE(entity_id TEXT, observations_synced INT) AS $$
DECLARE
    v_row RECORD;
    v_count INT;
BEGIN
    FOR v_row IN
        SELECT
            eo.entity_id,
            jsonb_object_agg(
                eo.key,
                jsonb_build_object(
                    'value',      eo.value->'value',
                    'confidence', eo.confidence,
                    'type',       eo.observation_type,
                    'count',      eo.observation_count
                )
            ) AS profile,
            COUNT(*) AS obs_count
        FROM meclaw.entity_observations eo
        WHERE eo.superseded_by IS NULL
          AND (p_entity_id IS NULL OR eo.entity_id = p_entity_id)
          AND (p_agent_id  IS NULL OR eo.agent_id  = p_agent_id)
        GROUP BY eo.entity_id
    LOOP
        UPDATE meclaw.entities e
        SET observed_profile = v_row.profile,
            updated_at = clock_timestamp()
        WHERE e.id = v_row.entity_id;

        entity_id        := v_row.entity_id;
        observations_synced := v_row.obs_count;
        RETURN NEXT;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.consolidate_observations IS
'Syncs entity_observations → entities.observed_profile on-demand.
Can filter by entity_id and/or agent_id. NULL = all.';

-- =============================================================================
-- 2. llm_extract_entities — erweitert um User-Präferenzen
-- =============================================================================
-- Breaking-Change-free: extraction_data enthält zusätzlich preferences_found INT
-- Das preferences[] Array im LLM-Output ist optional/additive.
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.llm_extract_entities(p_event_id UUID)
RETURNS VOID AS $fn$
import json
import requests

# ── Get event content + channel + message sender ──────────────────────────────
plan = plpy.prepare("""
    SELECT be.content, be.channel_id::text AS channel_id,
           m.sender, m.channel_id::text AS msg_channel_id
    FROM meclaw.brain_events be
    LEFT JOIN meclaw.messages m ON m.id = be.message_id
    WHERE be.id = $1
""", ["uuid"])
result = plan.execute([str(p_event_id)])
if not result or not result[0]["content"]:
    return

content     = result[0]["content"][:4000]
channel_id  = result[0]["channel_id"]
sender      = result[0]["sender"] or ""

if len(content.strip()) < 10:
    plpy.execute(plpy.prepare(
        "UPDATE meclaw.brain_events SET extracted = TRUE, extracted_at = clock_timestamp(), extraction_data = $1::jsonb WHERE id = $2",
        ["text", "uuid"]
    ), [json.dumps({"skipped": "too_short"}), str(p_event_id)])
    return

# ── Resolve sender → entity_id ───────────────────────────────────────────────
# Try to find entity by canonical_name or alias matching the sender field
sender_entity_id = None
if sender.strip():
    # Direct lookup by canonical_name
    ep = plpy.prepare("""
        SELECT id FROM meclaw.entities
        WHERE lower(canonical_name) = lower($1)
           OR $1 = ANY(aliases)
        LIMIT 1
    """, ["text"])
    er = ep.execute([sender.strip()])
    if er:
        sender_entity_id = er[0]["id"]

# ── Get LLM provider config ───────────────────────────────────────────────────
plan_prov = plpy.prepare("SELECT base_url, api_key, config FROM meclaw.llm_providers WHERE id = $1", ["text"])
prov = plan_prov.execute(["openrouter"])
if not prov:
    plpy.warning("llm_extract_entities: no openrouter provider found")
    return

api_key = prov[0]["api_key"]
model = "openai/gpt-4o-mini"

# ── Build prompt (entities + relations + preferences) ────────────────────────
preference_hint = ""
if sender.strip():
    preference_hint = f'\nThe message sender is "{sender}". Preferences/interests/opinions expressed in first person belong to this sender.'

prompt = f"""Extract entities, relations, and user preferences from this message. Return ONLY valid JSON.

Message:
---
{content}
---
{preference_hint}
Return this exact JSON structure:
{{
  "entities": [
    {{"name": "exact name", "type": "person|project|tool|concept|organization|location|event", "aliases": []}}
  ],
  "relations": [
    {{"subject": "entity name", "predicate": "works_on|uses|discusses|mentions|created|related_to", "object": "entity name"}}
  ],
  "preferences": [
    {{"key": "preference_key", "value": "preference_value", "type": "preference|interest|behavior|dislike", "confidence": 0.8, "evidence": "short quote from message"}}
  ]
}}

Rules for entities/relations (unchanged):
- Only extract clearly stated entities, don't infer
- Use the most specific type possible
- Relations must reference entities from the entities list
- If no entities found, return empty arrays
- Names should be canonical (e.g. "Marcus Meyer" not "Marcus")

Rules for preferences (NEW):
- Extract only first-person statements about likes, dislikes, interests, communication style, habits
- Examples: "I prefer Thai food" → key="food_preference", value="Thai", type="preference"
- Examples: "I hate meetings" → key="meetings", value="avoids", type="dislike"
- Examples: "I always use Python" → key="programming_language", value="Python", type="behavior"
- Examples: "I'm a morning person" → key="time_preference", value="morning", type="behavior"
- Use snake_case keys, short English values
- confidence: how certain is this preference (0.5–1.0)
- If no preferences detected, return empty array for "preferences"
- Max 5 preferences per message"""

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
            "max_tokens": 1200,
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

    entities    = extracted.get("entities", [])
    relations   = extracted.get("relations", [])
    preferences = extracted.get("preferences", [])   # NEW: user preferences

    # ── Process entities (unchanged) ─────────────────────────────────────────
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
                pass
        except Exception as ex:
            plpy.warning(f"llm_extract_entities: entity processing failed for '{name}': {ex}")

    # ── Process relations (unchanged) ─────────────────────────────────────────
    for rel in relations:
        subj = rel.get("subject", "").strip().lower()
        pred = rel.get("predicate", "related_to").strip().upper()
        obj  = rel.get("object", "").strip().lower()

        subj_id = entity_ids.get(subj)
        obj_id  = entity_ids.get(obj)

        if not subj_id or not obj_id or subj_id == obj_id:
            continue

        try:
            plpy.execute(plpy.prepare("""
                INSERT INTO meclaw.entity_events (entity_id, event_id, relation_type, confidence)
                VALUES ($1, $2, $3, 0.7)
                ON CONFLICT (entity_id, event_id, relation_type) DO NOTHING
            """, ["text", "uuid", "text"]), [subj_id, str(p_event_id), pred])
        except Exception:
            pass

        try:
            plpy.execute(plpy.prepare(
                "SELECT meclaw.age_link_entities($1, $2, $3)",
                ["text", "text", "text"]
            ), [subj_id, obj_id, pred])
        except Exception:
            pass

    # ── Process preferences (NEW) ─────────────────────────────────────────────
    # agent making observations = Walter (the AI)
    agent_id     = "meclaw:agent:walter"
    prefs_stored = 0

    for pref in preferences:
        pref_key   = pref.get("key", "").strip()
        pref_value = pref.get("value", "").strip()
        pref_type  = pref.get("type", "preference").strip()
        pref_conf  = float(pref.get("confidence", 0.65))
        evidence   = pref.get("evidence", "")[:200]

        if not pref_key or not pref_value:
            continue

        # Determine which entity this preference belongs to
        target_entity_id = sender_entity_id
        if not target_entity_id:
            # Try to find sender entity among extracted entities
            if sender.strip():
                resolved = entity_ids.get(sender.strip().lower())
                if resolved:
                    target_entity_id = resolved

        if not target_entity_id:
            plpy.warning(f"llm_extract_entities: no entity for sender '{sender}', skipping preference '{pref_key}'")
            continue

        # Validate entity exists before observing
        ev = plpy.execute(plpy.prepare(
            "SELECT 1 FROM meclaw.entities WHERE id = $1", ["text"]
        ), [target_entity_id])
        if not ev:
            plpy.warning(f"llm_extract_entities: target entity {target_entity_id} not found, skipping")
            continue

        # Build value JSONB
        obs_value = json.dumps({
            "value": pref_value,
            "evidence": evidence,
            "source": "llm_extraction"
        })

        try:
            plpy.execute(plpy.prepare("""
                SELECT meclaw.observe_entity($1, $2, $3::uuid, $4, $5, $6::jsonb, $7)
            """, ["text", "text", "text", "text", "text", "text", "float8"]),
            [agent_id, target_entity_id, channel_id, pref_type, pref_key, obs_value, pref_conf])
            prefs_stored += 1
        except Exception as ex:
            plpy.warning(f"llm_extract_entities: observe_entity failed for pref '{pref_key}': {ex}")

    # Trigger on-demand profile sync if any preferences were stored
    if prefs_stored > 0:
        try:
            plpy.execute(plpy.prepare(
                "SELECT * FROM meclaw.consolidate_observations($1, $2)",
                ["text", "text"]
            ), [sender_entity_id or target_entity_id if preferences else None, agent_id])
        except Exception as ex:
            plpy.warning(f"llm_extract_entities: consolidate_observations failed: {ex}")

    # ── Store extraction result (backward-compatible, +preferences_found) ─────
    extraction_meta = {
        "entities_found":    len(entities),
        "relations_found":   len(relations),
        "preferences_found": prefs_stored,     # NEW field (additive, no breaking change)
        "entity_ids":        entity_ids,
        "model":             model,
        "usage":             usage
    }

    plpy.execute(plpy.prepare(
        "UPDATE meclaw.brain_events SET extracted = TRUE, extracted_at = clock_timestamp(), extraction_data = $1::jsonb WHERE id = $2",
        ["text", "uuid"]
    ), [json.dumps(extraction_meta), str(p_event_id)])

    # Log cost
    total_tokens      = usage.get("total_tokens", 0)
    prompt_tokens     = usage.get("prompt_tokens", 0)
    completion_tokens = usage.get("completion_tokens", 0)

    plpy.execute(plpy.prepare("""
        INSERT INTO meclaw.events (bee_type, event, payload)
        VALUES ('extract_bee', 'llm_extraction_complete', $1::jsonb)
    """, ["text"]), [json.dumps({
        "event_id":    str(p_event_id),
        "entities":    len(entities),
        "relations":   len(relations),
        "preferences": prefs_stored,
        "sender":      sender or None,
        "model":       model,
        "tokens": {
            "prompt":     prompt_tokens,
            "completion": completion_tokens,
            "total":      total_tokens
        }
    })])

except Exception as e:
    plpy.warning(f"llm_extract_entities failed for {p_event_id}: {e}")
    plpy.execute(plpy.prepare(
        "UPDATE meclaw.brain_events SET extraction_data = $1::jsonb WHERE id = $2",
        ["text", "uuid"]
    ), [json.dumps({"error": str(e)[:500]}), str(p_event_id)])

$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.llm_extract_entities IS
'Two-stage LLM extraction: entities+relations (unchanged) + user preferences (C3 addition).
Preferences are stored via observe_entity() and trigger consolidate_observations().
extraction_data gains a new optional field "preferences_found" (backward-compatible).';

-- =============================================================================
-- 3. Test helper: inject_preference_test_message
-- =============================================================================
-- Erzeugt einen brain_event mit Präferenz-Inhalt für einen bekannten Sender
-- und ruft direkt llm_extract_entities auf (synchron, für Tests).
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.inject_preference_test(
    p_content TEXT,
    p_sender  TEXT DEFAULT 'benchmark_user',
    p_channel_id UUID DEFAULT '00000000-0000-0000-0000-000000000002'::uuid
) RETURNS UUID AS $$
DECLARE
    v_msg_id   UUID;
    v_event_id UUID;
BEGIN
    -- Create a minimal test message
    INSERT INTO meclaw.messages (channel_id, type, sender, status, content)
    VALUES (p_channel_id, 'user_input', p_sender, 'done',
            jsonb_build_object('input', p_content))
    RETURNING id INTO v_msg_id;

    -- Create brain_event
    INSERT INTO meclaw.brain_events (message_id, channel_id, content, extracted)
    VALUES (v_msg_id, p_channel_id, p_content, FALSE)
    RETURNING id INTO v_event_id;

    -- Run extraction synchronously
    PERFORM meclaw.llm_extract_entities(v_event_id);

    RAISE NOTICE 'inject_preference_test: event_id=% msg_id=%', v_event_id, v_msg_id;
    RETURN v_event_id;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.inject_preference_test IS
'Test helper: Creates a brain_event with given content+sender and runs llm_extract_entities synchronously.
Use for C3 User Modeling validation.';
