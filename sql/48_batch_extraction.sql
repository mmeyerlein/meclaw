-- =============================================================================
-- Segment-Based Extraction v3: Facts as separate brain_events
-- =============================================================================
-- Each extracted fact becomes its OWN brain_event with:
--   - content = the atomic fact
--   - embedding = computed via batch embedding
--   - BM25 searchable via content field (no facts_text needed)
--   - Linked to source events via extraction_data
--
-- This is the Honcho approach: atomic conclusions as first-class entries.
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.extract_segment(
    p_channel_id UUID,
    p_event_ids UUID[],  -- events in this segment
    p_api_key TEXT
)
RETURNS INT AS $fn$
import json
import requests

if not p_event_ids:
    return 0

# Fetch events
id_list = ",".join(f"'{str(eid)}'" for eid in p_event_ids)
rows = plpy.execute(f"""
    SELECT id, content, created_at::text as created_at
    FROM meclaw.brain_events
    WHERE id IN ({id_list})
    ORDER BY seq ASC
""")

if not rows:
    return 0

# Build combined text
event_texts = []
event_ids = []
for i, row in enumerate(rows):
    idx = i + 1
    event_ids.append(str(row["id"]))
    event_texts.append(f"[MSG {idx} | {row['created_at']}]\n{row['content'][:2000]}")

combined = "\n\n".join(event_texts)

prompt = f"""Extract ALL facts from this conversation. Each fact must be a complete, self-contained statement.

Conversation:
---
{combined}
---

Return JSON: {{"facts": ["fact1", "fact2", ...]}}

Rules:
- Each fact is ONE atomic statement with WHO, WHAT, WHEN
- Include: events attended, items bought, people met, preferences stated, activities done
- Include specific details: names, dates, durations, quantities, locations
- Resolve relative dates using message timestamps (e.g., "two months ago" → actual date)
- Keep each fact under 50 words
- Extract 5-15 facts per segment
- Skip generic advice/tips from the assistant — focus on USER-specific information"""

try:
    resp = requests.post(
        "https://openrouter.ai/api/v1/chat/completions",
        headers={
            "Authorization": f"Bearer {p_api_key}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://meclaw.ai",
            "X-Title": "MeClaw"
        },
        json={
            "model": "openai/gpt-4o-mini",
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.0,
            "max_tokens": 1000,
            "response_format": {"type": "json_object"}
        },
        timeout=60
    )
    resp.raise_for_status()
    result = resp.json()
    parsed = json.loads(result["choices"][0]["message"]["content"])

except Exception as e:
    plpy.warning(f"extract_segment error: {e}")
    # Mark events as extracted to prevent retries
    for eid in event_ids:
        plpy.execute(plpy.prepare(
            "UPDATE meclaw.brain_events SET extracted = TRUE, extracted_at = clock_timestamp() WHERE id = $1",
            ["uuid"]), [eid])
    return 0

# Get facts list (handle both {"facts": [...]} and [...])
facts = parsed.get("facts", parsed) if isinstance(parsed, dict) else parsed
if not isinstance(facts, list):
    facts = []

# Get timestamps for facts (use first event's timestamp)
first_ts = rows[0]["created_at"] if rows else None

# Insert each fact as a NEW brain_event
created = 0
for fact in facts:
    fact_text = fact if isinstance(fact, str) else fact.get("fact", str(fact)) if isinstance(fact, dict) else str(fact)
    if not fact_text or len(fact_text.strip()) < 10:
        continue
    try:
        plpy.execute(plpy.prepare(
            "INSERT INTO meclaw.brain_events "
            "(channel_id, content, extracted, extracted_at, extraction_data, created_at) "
            "VALUES ($1, $2, TRUE, clock_timestamp(), $3::jsonb, $4::timestamptz)",
            ["uuid", "text", "text", "text"]
        ), [str(p_channel_id), fact_text.strip(),
            json.dumps({"type": "extracted_fact", "source_events": event_ids[:3]}),
            first_ts])
        created += 1
    except Exception as e:
        plpy.warning(f"extract_segment insert fact: {e}")

# Mark source events as extracted
for eid in event_ids:
    plpy.execute(plpy.prepare(
        "UPDATE meclaw.brain_events SET extracted = TRUE, extracted_at = clock_timestamp(), "
        "extraction_data = COALESCE(extraction_data, '{}'::jsonb) || $1::jsonb WHERE id = $2",
        ["text", "uuid"]
    ), [json.dumps({"segment_processed": True, "facts_created": created}), eid])

return created
$fn$ LANGUAGE plpython3u;

-- =============================================================================
-- 2. Batch wrapper: segments all unextracted events in a channel
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.batch_extract_entities(
    p_channel_id UUID,
    p_limit INT DEFAULT 200,
    p_batch_size INT DEFAULT 8
)
RETURNS INT AS $fn$
import json

# Get API key once
prov = plpy.execute(plpy.prepare(
    "SELECT api_key FROM meclaw.llm_providers WHERE id = $1", ["text"]),
    ["openrouter"])
if not prov:
    plpy.warning("batch_extract: no openrouter provider")
    return 0
api_key = prov[0]["api_key"]

# Get unextracted events
rows = list(plpy.execute(plpy.prepare("""
    SELECT id FROM meclaw.brain_events
    WHERE channel_id = $1
      AND extracted = false
      AND content IS NOT NULL
      AND length(content) >= 10
      AND (extraction_data IS NULL OR extraction_data->>'type' IS DISTINCT FROM 'extracted_fact')
    ORDER BY seq ASC
    LIMIT $2
""", ["uuid", "int4"]), [str(p_channel_id), p_limit]))

if not rows:
    return 0

# Split into segments
all_ids = [str(row["id"]) for row in rows]
segments = [all_ids[i:i+p_batch_size] for i in range(0, len(all_ids), p_batch_size)]

total = 0
for seg in segments:
    n = plpy.execute(plpy.prepare(
        "SELECT meclaw.extract_segment($1, $2::uuid[], $3)",
        ["uuid", "text[]", "text"]),
        [str(p_channel_id), seg, api_key])[0]["extract_segment"]
    total += n

return total
$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.batch_extract_entities IS
'Segment-based extraction v3: extracts atomic facts as separate brain_events.
Each fact gets its own embedding and is BM25-searchable.
8 messages per segment, 1 LLM call per segment.';

-- =============================================================================
-- 3. Backfill wrapper
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.backfill_extractions_batch(p_limit INT DEFAULT 200)
RETURNS INT AS $$
DECLARE
    v_channel RECORD;
    v_total INT := 0;
BEGIN
    FOR v_channel IN
        SELECT DISTINCT channel_id
        FROM meclaw.brain_events
        WHERE extracted = false
          AND content IS NOT NULL
          AND length(content) >= 10
        LIMIT 10
    LOOP
        v_total := v_total + meclaw.batch_extract_entities(v_channel.channel_id, p_limit);
    END LOOP;
    RETURN v_total;
END;
$$ LANGUAGE plpgsql;
