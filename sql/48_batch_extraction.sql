-- =============================================================================
-- Segment-Based Batch Extraction
-- =============================================================================
-- Splits events into segments of ~8 messages, extracts per segment.
-- Creates a summary brain_event per segment with all extracted facts.
-- ~4x fewer API calls than per-event, but much more precise than 1-call-per-session.
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.batch_extract_entities(
    p_channel_id UUID,
    p_limit INT DEFAULT 200,
    p_batch_size INT DEFAULT 8  -- messages per LLM call (segment size)
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
rows = list(plpy.execute(plpy.prepare("""
    SELECT id, content, seq, created_at::text as created_at
    FROM meclaw.brain_events
    WHERE channel_id = $1
      AND extracted = false
      AND content IS NOT NULL
      AND length(content) >= 10
    ORDER BY seq ASC
    LIMIT $2
""", ["uuid", "int4"]), [str(p_channel_id), p_limit]))

if not rows:
    return 0

# Get LLM provider (once, reuse for all segments)
plan_prov = plpy.prepare(
    "SELECT base_url, api_key FROM meclaw.llm_providers WHERE id = $1", ["text"])
prov = plan_prov.execute(["openrouter"])
if not prov:
    plpy.warning("batch_extract_entities: no openrouter provider")
    return 0
api_key = prov[0]["api_key"]

# Split into segments
segments = []
for i in range(0, len(rows), p_batch_size):
    segments.append(rows[i:i + p_batch_size])

total_extracted = 0

for seg_idx, segment in enumerate(segments):
    # Build combined text for this segment
    event_texts = []
    event_map = {}   # idx → event_id (1-based within segment)
    event_list = []
    for i, row in enumerate(segment):
        idx = i + 1
        eid = str(row["id"])
        content = row["content"][:2000]
        created = row["created_at"]
        event_map[idx] = eid
        event_list.append(eid)
        event_texts.append(f"[MSG {idx} | {created}]\n{content}")

    combined = "\n\n".join(event_texts)

    prompt = f"""Extract ALL entities, facts, and relations from this conversation segment.

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
    {{"fact": "atomic fact statement including WHO, WHAT, WHEN", "msg_seq": 1, "date": "YYYY-MM-DD or unknown", "type": "event|preference|fact|opinion"}}
  ],
  "relations": [
    {{"subject": "entity name", "predicate": "works_on|uses|discusses|mentions|lives_in|attended|related_to", "object": "entity name"}}
  ]
}}

Rules:
- Extract ALL named entities (persons, places, organizations, tools, events, workshops, courses)
- Extract EVERY atomic fact — each fact must be self-contained with WHO did WHAT and WHEN
- Include specific details: names, dates, durations, quantities, preferences
- Resolve relative dates ("two months ago", "last week") using the message timestamps
- msg_seqs: which message numbers contain this entity/fact
- Be THOROUGH — extract everything mentioned, especially specific events, activities, purchases"""

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
        plpy.warning(f"batch_extract segment {seg_idx} error: {e}")
        # Mark as extracted (failed) to prevent retry loops
        for eid in event_map.values():
            plpy.execute(plpy.prepare(
                "UPDATE meclaw.brain_events SET extracted = TRUE, extracted_at = clock_timestamp(), "
                "extraction_data = $1::jsonb WHERE id = $2",
                ["text", "uuid"]
            ), [json.dumps({"error": str(e), "segment": seg_idx}), eid])
        continue

    # Process entities
    entities = parsed.get("entities", [])
    entity_id_map = {}
    for ent in entities:
        if not isinstance(ent, dict) or not ent.get("name"):
            continue
        name = ent["name"]
        etype = ent.get("type", "concept")
        aliases = ent.get("aliases", [])
        try:
            r = plpy.execute(plpy.prepare(
                "SELECT meclaw.create_or_resolve_entity($1, $2, $3::text[])",
                ["text", "text", "text[]"]), [name, etype, aliases])
            entity_id = r[0]["create_or_resolve_entity"]
            entity_id_map[name.lower()] = entity_id
            # Link to relevant events
            for seq in ent.get("msg_seqs", []):
                if seq in event_map:
                    plpy.execute(plpy.prepare(
                        "INSERT INTO meclaw.entity_events (entity_id, event_id, relation_type) "
                        "VALUES ($1, $2, 'MENTIONED_IN') ON CONFLICT DO NOTHING",
                        ["text", "uuid"]), [entity_id, event_map[seq]])
        except Exception as e:
            plpy.warning(f"batch_extract entity '{name}': {e}")

    # Process facts
    facts = parsed.get("facts", [])
    fact_by_idx = {}
    all_facts = []
    for fact in facts:
        if not isinstance(fact, dict) or not fact.get("fact"):
            continue
        all_facts.append(fact["fact"])
        idx = fact.get("msg_seq")
        if idx and idx in event_map:
            fact_by_idx.setdefault(idx, []).append(fact["fact"])

    # Process relations
    for rel in parsed.get("relations", []):
        if not isinstance(rel, dict):
            continue
        subj = entity_id_map.get(rel.get("subject", "").lower())
        obj = entity_id_map.get(rel.get("object", "").lower())
        if subj and obj and subj != obj:
            try:
                plpy.execute(plpy.prepare(
                    "SELECT meclaw.age_link_entities($1, $2, $3)",
                    ["text", "text", "text"]),
                    [subj, obj, rel.get("predicate", "related_to")])
            except Exception as e:
                pass  # Non-critical

    # Update events with facts_text
    all_facts_text = " | ".join(all_facts) if all_facts else None
    for idx, eid in event_map.items():
        facts_for_event = fact_by_idx.get(idx, [])
        facts_text = " | ".join(facts_for_event) if facts_for_event else None
        try:
            plpy.execute(plpy.prepare(
                "UPDATE meclaw.brain_events SET "
                "extracted = TRUE, extracted_at = clock_timestamp(), "
                "extraction_data = $1::jsonb, "
                "facts_text = COALESCE($2, facts_text) "
                "WHERE id = $3",
                ["text", "text", "uuid"]
            ), [json.dumps({"batch": True, "segment": seg_idx,
                           "entities": len(entities), "facts": len(facts_for_event)}),
                facts_text, eid])
            total_extracted += 1
        except Exception as e:
            plpy.warning(f"batch_extract update {eid}: {e}")

    # Create SEGMENT SUMMARY brain_event with all facts from this segment
    if all_facts_text and len(all_facts_text) > 20:
        try:
            first_eid = event_map.get(1)
            if first_eid:
                plan_ts = plpy.prepare(
                    "SELECT channel_id, MIN(created_at) as min_ts FROM meclaw.brain_events "
                    "WHERE id = ANY($1::uuid[]) GROUP BY channel_id", ["text"])
                ts_row = plan_ts.execute(["{" + ",".join(event_list) + "}"])
                if ts_row:
                    plpy.execute(plpy.prepare(
                        "INSERT INTO meclaw.brain_events "
                        "(channel_id, content, facts_text, extracted, extracted_at, "
                        " extraction_data, created_at) "
                        "VALUES ($1, $2, $3, TRUE, clock_timestamp(), "
                        " $4::jsonb, $5)",
                        ["uuid", "text", "text", "text", "timestamptz"]
                    ), [ts_row[0]["channel_id"],
                        "Segment summary: " + all_facts_text[:4000],
                        all_facts_text[:8000],
                        json.dumps({"type": "segment_summary", "segment": seg_idx,
                                   "source_events": len(segment)}),
                        ts_row[0]["min_ts"]])
        except Exception as e:
            plpy.warning(f"batch_extract summary: {e}")

return total_extracted
$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.batch_extract_entities IS
'Segment-based extraction: splits events into batches of p_batch_size (default 8),
1 LLM call per segment. Creates segment_summary brain_events with extracted facts.
More precise than 1-call-per-session, fewer calls than per-event.';

-- =============================================================================
-- 2. Backfill wrapper
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
