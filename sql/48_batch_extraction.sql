-- =============================================================================
-- Batch Extraction: 1 LLM Call pro Session statt pro Event
-- =============================================================================
-- Problem: llm_extract_entities macht 1 API Call pro brain_event.
--   Bei 30 Events/Frage = 30 Calls × 0.5s = 15s + Rate Limits.
-- Lösung: Alle Events einer Session als Batch an 1 LLM Call.
--   30 Events → 1 Call → Ergebnisse auf Events verteilen.
-- =============================================================================

-- =============================================================================
-- 1. Batch-Extract: Alle unextrahierten Events eines Channels auf einmal
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.batch_extract_entities(
    p_channel_id UUID,
    p_limit INT DEFAULT 200
)
RETURNS INT AS $fn$
import json
import requests

# Collect all unextracted events for this channel
plan = plpy.prepare("""
    SELECT id, content, seq, created_at::text as created_at
    FROM meclaw.brain_events
    WHERE channel_id = $1
      AND extracted = false
      AND content IS NOT NULL
      AND length(content) >= 10
    ORDER BY seq ASC
    LIMIT $2
""", ["uuid", "int4"])
rows = plan.execute([str(p_channel_id), p_limit])

if not rows:
    return 0

# Build a single combined text with event markers
event_texts = []
event_map = {}  # seq → event_id
for row in rows:
    eid = str(row["id"])
    seq = row["seq"]
    content = row["content"][:2000]  # Cap per event
    created = row["created_at"]
    event_map[seq] = eid
    event_texts.append(f"[MSG {seq} | {created}]\n{content}")

combined = "\n\n".join(event_texts)
# Cap total at ~6000 chars for cost/context control
if len(combined) > 6000:
    combined = combined[:6000]

# Get LLM provider
plan_prov = plpy.prepare(
    "SELECT base_url, api_key FROM meclaw.llm_providers WHERE id = $1", ["text"])
prov = plan_prov.execute(["openrouter"])
if not prov:
    plpy.warning("batch_extract_entities: no openrouter provider")
    return 0

api_key = prov[0]["api_key"]

prompt = f"""Extract ALL entities, facts, and relations from this conversation.

Conversation:
---
{combined}
---

Return ONLY valid JSON:
{{
  "entities": [
    {{"name": "exact name", "type": "person|project|tool|concept|organization|location|event", "aliases": [], "msg_seqs": [1,2]}}
  ],
  "facts": [
    {{"fact": "atomic fact statement", "msg_seq": 1, "date": "YYYY-MM-DD or unknown", "type": "event|preference|fact|opinion"}}
  ],
  "relations": [
    {{"subject": "entity name", "predicate": "works_on|uses|discusses|mentions|lives_in|related_to", "object": "entity name"}}
  ]
}}

Rules:
- Extract ALL named entities (persons, places, organizations, tools, events)
- Extract atomic facts: each fact is one self-contained statement
- Include date references in facts (resolve relative dates using msg timestamps)
- msg_seqs: which message numbers reference this entity/fact
- If the same entity appears with different names, use the most complete name and list aliases
- Be thorough — extract everything, don't summarize"""

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
            "model": "openai/gpt-4o-mini",
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.0,
            "max_tokens": 2000,
            "response_format": {"type": "json_object"}
        },
        timeout=60
    )
    resp.raise_for_status()
    result = resp.json()
    response_text = result["choices"][0]["message"]["content"]
    parsed = json.loads(response_text)

except Exception as e:
    plpy.warning(f"batch_extract_entities API error: {e}")
    # Mark all as extracted (failed) to prevent retry loops
    for eid in event_map.values():
        plpy.execute(plpy.prepare(
            "UPDATE meclaw.brain_events SET extracted = TRUE, extracted_at = clock_timestamp(), "
            "extraction_data = $1::jsonb WHERE id = $2",
            ["text", "uuid"]
        ), [json.dumps({"error": str(e)}), eid])
    return 0

# Process entities
entities = parsed.get("entities", [])
entity_id_map = {}  # name → entity_id
for ent in entities:
    if not isinstance(ent, dict) or not ent.get("name"):
        continue
    name = ent["name"]
    etype = ent.get("type", "concept")
    aliases = ent.get("aliases", [])
    try:
        plan_resolve = plpy.prepare(
            "SELECT meclaw.create_or_resolve_entity($1, $2, $3::text[])",
            ["text", "text", "text[]"])
        r = plan_resolve.execute([name, etype, aliases])
        entity_id = r[0]["create_or_resolve_entity"]
        entity_id_map[name.lower()] = entity_id

        # Link to relevant events
        msg_seqs = ent.get("msg_seqs", [])
        for seq in msg_seqs:
            if seq in event_map:
                eid = event_map[seq]
                plpy.execute(plpy.prepare(
                    "INSERT INTO meclaw.entity_events (entity_id, event_id, relation_type) "
                    "VALUES ($1, $2, 'MENTIONED_IN') ON CONFLICT DO NOTHING",
                    ["text", "uuid"]
                ), [entity_id, eid])
    except Exception as e:
        plpy.warning(f"batch_extract entity '{name}': {e}")

# Process facts → update facts_text on relevant events
facts = parsed.get("facts", [])
fact_by_seq = {}  # seq → [fact strings]
for fact in facts:
    if not isinstance(fact, dict) or not fact.get("fact"):
        continue
    seq = fact.get("msg_seq")
    if seq and seq in event_map:
        fact_by_seq.setdefault(seq, []).append(fact["fact"])

# Process relations
relations = parsed.get("relations", [])
for rel in relations:
    if not isinstance(rel, dict):
        continue
    subj = entity_id_map.get(rel.get("subject", "").lower())
    obj = entity_id_map.get(rel.get("object", "").lower())
    if subj and obj and subj != obj:
        try:
            plpy.execute(plpy.prepare(
                "SELECT meclaw.age_link_entities($1, $2, $3)",
                ["text", "text", "text"]
            ), [subj, obj, rel.get("predicate", "related_to")])
        except Exception as e:
            plpy.warning(f"batch_extract relation: {e}")

# Mark all events as extracted, store facts_text
extracted_count = 0
for seq, eid in event_map.items():
    facts_for_event = fact_by_seq.get(seq, [])
    facts_text = " | ".join(facts_for_event) if facts_for_event else None
    try:
        plpy.execute(plpy.prepare(
            "UPDATE meclaw.brain_events SET "
            "extracted = TRUE, extracted_at = clock_timestamp(), "
            "extraction_data = $1::jsonb, "
            "facts_text = COALESCE($2, facts_text) "
            "WHERE id = $3",
            ["text", "text", "uuid"]
        ), [json.dumps({"batch": True, "entities": len(entities), "facts": len(facts)}),
            facts_text, eid])
        extracted_count += 1
    except Exception as e:
        plpy.warning(f"batch_extract update {eid}: {e}")

return extracted_count
$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.batch_extract_entities IS
'Batch entity+fact extraction: 1 LLM call per channel/session instead of per event.
Extracts entities, atomic facts, and relations from all unextracted events at once.
~6x fewer API calls than per-event extraction.';

-- =============================================================================
-- 2. Backfill wrapper: batch extraction for all channels with unextracted events
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.backfill_extractions_batch(p_limit INT DEFAULT 200)
RETURNS INT AS $$
DECLARE
    v_channel RECORD;
    v_total INT := 0;
    v_extracted INT;
BEGIN
    FOR v_channel IN
        SELECT DISTINCT channel_id
        FROM meclaw.brain_events
        WHERE extracted = false
          AND content IS NOT NULL
          AND length(content) >= 10
        LIMIT 10
    LOOP
        v_extracted := meclaw.batch_extract_entities(v_channel.channel_id, p_limit);
        v_total := v_total + v_extracted;
    END LOOP;
    RETURN v_total;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.backfill_extractions_batch IS
'Batch extraction wrapper: iterates over channels with unextracted events,
runs batch_extract_entities per channel. 1 LLM call per channel.';
