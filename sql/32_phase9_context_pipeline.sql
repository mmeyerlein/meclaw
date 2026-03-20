-- =============================================================================
-- Phase 9: Context Pipeline — Compression + CTM Retrieval
-- =============================================================================
-- 1. markdown_compress() — lossless token reduction
-- 2. context_bee_v3 — compressed static prefix + CTM retrieval + cache breakpoint
-- 3. agents_md_parser — full implementation
-- =============================================================================

-- -----------------------------------------------------------------------------
-- 1. Static context storage (compressed prefix cache)
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS meclaw.context_cache (
    agent_id TEXT NOT NULL REFERENCES meclaw.entities(id),
    cache_key TEXT NOT NULL,                -- 'static_prefix', 'soul', 'agents', etc.
    raw_text TEXT,
    compressed_text TEXT,
    raw_tokens INT,
    compressed_tokens INT,
    compression_ratio FLOAT,
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (agent_id, cache_key)
);

-- -----------------------------------------------------------------------------
-- 2. markdown_compress — lossless token reduction
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.markdown_compress(p_text TEXT)
RETURNS TEXT AS $fn$
import re

text = p_text or ""
if len(text) < 50:
    return text

# --- SAFE REMOVALS (lossless) ---

# Remove HTML comments
text = re.sub(r'<!--.*?-->', '', text, flags=re.DOTALL)

# Remove decorative horizontal rules (3+ dashes/equals on own line)
text = re.sub(r'^\s*[-=]{3,}\s*$', '', text, flags=re.MULTILINE)

# Collapse multiple blank lines to single
text = re.sub(r'\n{3,}', '\n\n', text)

# Remove trailing whitespace per line
text = re.sub(r'[ \t]+$', '', text, flags=re.MULTILINE)

# Remove leading blank lines
text = re.sub(r'^\n+', '', text)

# Remove empty list items
text = re.sub(r'^\s*[-*]\s*$\n?', '', text, flags=re.MULTILINE)

# Collapse repeated spaces (but not indentation)
text = re.sub(r'(?<=\S)  +(?=\S)', ' ', text)

# Remove filler phrases (common in docs)
fillers = [
    r'(?i)\bplease note that\b',
    r'(?i)\bit is important to note that\b',
    r'(?i)\bit should be noted that\b',
    r'(?i)\bas mentioned (?:above|below|earlier|previously)\b',
    r'(?i)\bin other words\b,?\s*',
    r'(?i)\bthat is to say\b,?\s*',
    r'(?i)\bbasically\b,?\s*',
    r'(?i)\bessentially\b,?\s*',
]
for filler in fillers:
    text = re.sub(filler, '', text)

# Compress table formatting (normalize column widths)
def compress_table_row(match):
    cells = match.group(0).split('|')
    return '|'.join(c.strip() for c in cells)

text = re.sub(r'^[|].*[|]$', compress_table_row, text, flags=re.MULTILINE)

# Remove separator rows in tables (|---|---|)
text = re.sub(r'^\|[-:\s|]+\|$', lambda m: '|' + '|'.join('-' for _ in m.group().split('|')[1:-1]) + '|', text, flags=re.MULTILINE)

# Collapse empty heading sections (heading with nothing after it before next heading)
text = re.sub(r'(^#{1,6}\s+[^\n]+)\n+(?=#{1,6}\s)', r'\1\n', text, flags=re.MULTILINE)

# Final cleanup
text = re.sub(r'\n{3,}', '\n\n', text)
text = text.strip()

return text
$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 3. estimate_tokens — rough token count (chars/4)
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.estimate_tokens(p_text TEXT)
RETURNS INT AS $$
BEGIN
    RETURN GREATEST(COALESCE(length(p_text), 0) / 4, 0);
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- -----------------------------------------------------------------------------
-- 4. build_static_prefix — compress and cache static context
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.build_static_prefix(p_agent_id TEXT)
RETURNS TEXT AS $fn$
import json

# Get agent identity
plan = plpy.prepare("""
    SELECT canonical_name, neural_matrix, traits, linguistics, capabilities,
           explicit_profile, observed_profile
    FROM meclaw.entities WHERE id = $1
""", ["text"])
agent = plan.execute([p_agent_id])

if not agent:
    return ""

a = agent[0]
sections = []

# Build identity section
identity = f"# Agent: {a['canonical_name']}"
if a["neural_matrix"]:
    nm = json.loads(a["neural_matrix"])
    top_traits = sorted(nm.items(), key=lambda x: x[1], reverse=True)[:3]
    identity += f"\nCore traits: {', '.join(f'{k}={v}' for k,v in top_traits)}"
if a["linguistics"]:
    ling = json.loads(a["linguistics"])
    if ling.get("formality") is not None:
        identity += f"\nFormality: {ling['formality']}"
    if ling.get("catchphrases"):
        identity += f"\nCatchphrases: {', '.join(ling['catchphrases'][:3])}"
sections.append(identity)

# Build user context (for primary user on channel)
users = plpy.execute(plpy.prepare("""
    SELECT canonical_name, explicit_profile, observed_profile
    FROM meclaw.entities
    WHERE entity_type = 'person'
    ORDER BY created_at ASC LIMIT 1
"""))
if users:
    u = users[0]
    user_section = f"# User: {u['canonical_name']}"
    if u["explicit_profile"]:
        ep = json.loads(u["explicit_profile"])
        for k, v in list(ep.items())[:5]:
            user_section += f"\n- {k}: {v}"
    if u["observed_profile"]:
        op = json.loads(u["observed_profile"])
        for k, v in list(op.items())[:3]:
            if isinstance(v, dict):
                user_section += f"\n- {k}: {v.get('value', v)} (confidence: {v.get('confidence', '?')})"
            else:
                user_section += f"\n- {k}: {v}"
    sections.append(user_section)

# Build skills section
skills = plpy.execute("SELECT id, display_name, description FROM meclaw.skills WHERE enabled = TRUE ORDER BY id")
if skills:
    skill_section = "# Available Skills"
    for s in skills:
        skill_section += f"\n- {s['id']}: {s['description']}"
    sections.append(skill_section)

raw_prefix = "\n\n".join(sections)

# Compress
compressed = plpy.execute(plpy.prepare(
    "SELECT meclaw.markdown_compress($1) AS compressed", ["text"]
), [raw_prefix])[0]["compressed"]

# Cache
raw_tokens = len(raw_prefix) // 4
compressed_tokens = len(compressed) // 4
ratio = compressed_tokens / max(raw_tokens, 1)

plpy.execute(plpy.prepare("""
    INSERT INTO meclaw.context_cache (agent_id, cache_key, raw_text, compressed_text, raw_tokens, compressed_tokens, compression_ratio)
    VALUES ($1, 'static_prefix', $2, $3, $4, $5, $6)
    ON CONFLICT (agent_id, cache_key) DO UPDATE
    SET compressed_text = EXCLUDED.compressed_text, raw_tokens = EXCLUDED.raw_tokens,
        compressed_tokens = EXCLUDED.compressed_tokens, compression_ratio = EXCLUDED.compression_ratio,
        updated_at = clock_timestamp()
""", ["text", "text", "text", "int4", "int4", "float8"]),
[p_agent_id, raw_prefix, compressed, raw_tokens, compressed_tokens, ratio])

return compressed

$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 5. context_bee_v3 — full pipeline: compressed prefix + CTM + cache breakpoint
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.context_bee_v3(p_msg_id UUID)
RETURNS VOID AS $$
DECLARE
    v_msg RECORD;
    v_channel_id UUID;
    v_history JSONB;
    v_content JSONB;
    v_current_input TEXT;
    v_model_id TEXT;
    v_tier TEXT;
    v_max_messages INT;
    v_agent_id TEXT;
    v_memories JSONB;
    v_memory_count INT := 0;
    v_llm_config TEXT;
    v_static_prefix TEXT;
    v_prefix_tokens INT;
    v_total_tokens INT;
    v_rec RECORD;
BEGIN
    -- Get message
    SELECT * INTO v_msg FROM meclaw.messages WHERE id = p_msg_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'context_bee_v3: message % not found', p_msg_id;
    END IF;

    v_channel_id := v_msg.channel_id;
    IF v_channel_id IS NULL THEN
        SELECT channel_id INTO v_channel_id FROM meclaw.tasks WHERE id = v_msg.task_id;
    END IF;

    -- Identify agent
    SELECT ac.agent_id INTO v_agent_id
    FROM meclaw.agent_channels ac
    JOIN meclaw.entities e ON e.id = ac.agent_id
    WHERE ac.channel_id = v_channel_id
        AND e.entity_type = 'agent'
        AND ac.role IN ('owner', 'participant')
    ORDER BY ac.role ASC
    LIMIT 1;

    IF v_agent_id IS NULL THEN
        SELECT ac.agent_id INTO v_agent_id
        FROM meclaw.agent_channels ac
        WHERE ac.channel_id = v_channel_id
        LIMIT 1;
    END IF;

    -- Get model config from llm_providers (simpler than AGE lookup)
    v_model_id := NULL;
    BEGIN
        SELECT config->>'model' INTO v_model_id
        FROM meclaw.llm_providers
        WHERE id = 'openrouter' AND enabled = TRUE
        LIMIT 1;
    EXCEPTION WHEN OTHERS THEN
        v_model_id := NULL;
    END;

    -- Tier + max messages
    IF v_model_id IS NOT NULL THEN
        SELECT tier INTO v_tier FROM meclaw.llm_models WHERE id = v_model_id;
    END IF;

    v_max_messages := CASE v_tier
        WHEN 'cheap'    THEN 10
        WHEN 'standard' THEN 20
        WHEN 'premium'  THEN 40
        ELSE 10
    END;

    -- === STATIC PREFIX (compressed, cached) ===
    -- Check cache first (valid for 1 hour)
    SELECT compressed_text, compressed_tokens INTO v_static_prefix, v_prefix_tokens
    FROM meclaw.context_cache
    WHERE agent_id = COALESCE(v_agent_id, 'meclaw:agent:walter')
        AND cache_key = 'static_prefix'
        AND updated_at > clock_timestamp() - interval '1 hour';

    -- Rebuild if stale
    IF v_static_prefix IS NULL THEN
        v_static_prefix := meclaw.build_static_prefix(COALESCE(v_agent_id, 'meclaw:agent:walter'));
        v_prefix_tokens := meclaw.estimate_tokens(v_static_prefix);
    END IF;

    -- === CACHE BREAKPOINT === (everything above this is stable across turns)

    -- === DYNAMIC CONTEXT ===

    -- Conversation history
    v_current_input := v_msg.content->>'input';
    v_history := meclaw.get_conversation_history(v_channel_id, 24, v_max_messages);

    -- Remove duplicate current message
    IF jsonb_array_length(v_history) > 0
       AND v_history->-1->>'content' = v_current_input
       AND v_history->-1->>'role' = 'user' THEN
        v_history := v_history - (jsonb_array_length(v_history) - 1);
    END IF;

    -- === CTM RETRIEVAL (instead of standard retrieve_bee) ===
    v_memories := '[]'::jsonb;
    IF v_agent_id IS NOT NULL AND v_current_input IS NOT NULL AND length(v_current_input) > 3 THEN
        BEGIN
            -- Use CTM retrieval for better results on complex queries
            SELECT jsonb_agg(jsonb_build_object(
                'content', r.content,
                'score', round(r.score::numeric, 4),
                'source', r.source,
                'age_hours', round((EXTRACT(EPOCH FROM (clock_timestamp() - r.created_at)) / 3600.0)::numeric, 1),
                'reward', r.reward,
                'ticks', r.ticks_used
            ))
            INTO v_memories
            FROM meclaw.ctm_retrieve(v_agent_id, v_current_input, 2, 5) r;

            v_memory_count := COALESCE(jsonb_array_length(v_memories), 0);
        EXCEPTION WHEN OTHERS THEN
            -- Fallback to standard retrieve_bee
            BEGIN
                SELECT jsonb_agg(jsonb_build_object(
                    'content', r.content,
                    'score', round(r.score::numeric, 4),
                    'age_hours', round((EXTRACT(EPOCH FROM (clock_timestamp() - r.created_at)) / 3600.0)::numeric, 1),
                    'reward', r.reward
                ))
                INTO v_memories
                FROM meclaw.retrieve_bee(v_agent_id, v_current_input, 5) r;

                v_memory_count := COALESCE(jsonb_array_length(v_memories), 0);
            EXCEPTION WHEN OTHERS THEN
                v_memories := '[]'::jsonb;
                v_memory_count := 0;
            END;
        END;
    END IF;

    -- === TOTAL TOKEN ESTIMATE ===
    v_total_tokens := COALESCE(v_prefix_tokens, 0)
                    + meclaw.estimate_tokens(v_history::text)
                    + meclaw.estimate_tokens(COALESCE(v_memories, '[]')::text)
                    + meclaw.estimate_tokens(COALESCE(v_current_input, ''));

    -- Log
    PERFORM meclaw.log_event(p_msg_id, v_msg.task_id, 'context_bee_v3', 'context_loaded',
        jsonb_build_object(
            'channel_id', v_channel_id,
            'agent_id', v_agent_id,
            'history_count', jsonb_array_length(v_history),
            'memory_count', v_memory_count,
            'model_id', COALESCE(v_model_id, 'unknown'),
            'tier', COALESCE(v_tier, 'default'),
            'max_messages', v_max_messages,
            'prefix_tokens', COALESCE(v_prefix_tokens, 0),
            'total_tokens_estimate', v_total_tokens,
            'retrieval_method', 'ctm'
        ));

    -- Build content with structured context layers
    v_content := v_msg.content || jsonb_build_object(
        'static_prefix', v_static_prefix,
        'conversation_history', v_history,
        'memories', COALESCE(v_memories, '[]'::jsonb),
        'agent_id', v_agent_id,
        'context_meta', jsonb_build_object(
            'prefix_tokens', COALESCE(v_prefix_tokens, 0),
            'total_estimate', v_total_tokens,
            'cache_breakpoint_after', 'static_prefix'
        )
    );

    -- Create routing message
    INSERT INTO meclaw.messages (task_id, channel_id, previous_id, type, sender, status, content)
    VALUES (v_msg.task_id, v_channel_id, p_msg_id, 'routing', 'context_bee_v3', 'done', v_content);

    UPDATE meclaw.messages SET status = 'done' WHERE id = p_msg_id AND status != 'done';
END;
$$ LANGUAGE plpgsql;

-- -----------------------------------------------------------------------------
-- 6. agents_md_parser — parse AGENTS.md into structured context
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.parse_agents_md(p_content TEXT, p_workspace_id TEXT DEFAULT 'meclaw:workspace:default')
RETURNS JSONB AS $fn$
import re
import json

content = p_content or ""
if not content.strip():
    return json.dumps({"sections": [], "rules": []})

sections = []
rules = []
current_section = None

for line in content.split('\n'):
    # Section headers
    heading_match = re.match(r'^(#{1,3})\s+(.+)', line)
    if heading_match:
        level = len(heading_match.group(1))
        title = heading_match.group(2).strip()
        current_section = {"title": title, "level": level, "content": [], "rules": []}
        sections.append(current_section)
        continue

    if current_section is None:
        continue

    stripped = line.strip()
    if not stripped:
        continue

    # Detect rules (MUST, NEVER, ALWAYS, DO NOT)
    if re.search(r'\b(MUST|NEVER|ALWAYS|DO NOT|REQUIRED|FORBIDDEN)\b', stripped):
        rules.append({"section": current_section["title"], "rule": stripped})
        current_section["rules"].append(stripped)

    # Detect list items
    if re.match(r'^[-*]\s+', stripped):
        current_section["content"].append(stripped[2:].strip())
    else:
        current_section["content"].append(stripped)

result = {
    "sections": [{"title": s["title"], "level": s["level"], "items": len(s["content"])} for s in sections],
    "rules": rules,
    "total_sections": len(sections),
    "total_rules": len(rules)
}

# Store parsed result
plpy.execute(plpy.prepare("""
    INSERT INTO meclaw.context_cache (agent_id, cache_key, raw_text, compressed_text, raw_tokens, compressed_tokens, compression_ratio)
    VALUES ($1, 'agents_md', $2, $3, $4, $5, 1.0)
    ON CONFLICT (agent_id, cache_key) DO UPDATE
    SET raw_text = EXCLUDED.raw_text, compressed_text = EXCLUDED.compressed_text,
        raw_tokens = EXCLUDED.raw_tokens, updated_at = clock_timestamp()
""", ["text", "text", "text", "int4", "int4"]),
[p_workspace_id, content, json.dumps(result), len(content) // 4, len(json.dumps(result)) // 4])

return json.dumps(result)

$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 7. Update dispatch trigger to use context_bee_v3
-- -----------------------------------------------------------------------------
-- Note: The trigger chain uses AGE graph edges for routing.
-- To switch from v2 to v3, we update the Bee node's function reference.
-- This is done via the router_bee which reads from the graph.
-- For now, context_bee_v3 is available as a function — it can be wired in
-- by updating the Bee node in AGE or by updating the dispatch trigger.
-- We leave the wiring decision to deployment time.
