-- MeClaw v0.1.0 — Brain Schema (Memory Hive)
-- Date: 2026-03-20
-- Ref: docs/BRAIN.md, docs/ARCHITECTURE.md, docs/DESIGN.md

-- =============================================================================
-- 1. Extend existing channels table
-- =============================================================================

-- Add extraction tracking columns to channels
ALTER TABLE meclaw.channels
    ADD COLUMN IF NOT EXISTS channel_type TEXT,
    ADD COLUMN IF NOT EXISTS external_id TEXT,
    ADD COLUMN IF NOT EXISTS extraction_status TEXT DEFAULT 'idle',
    ADD COLUMN IF NOT EXISTS last_extracted_seq BIGINT DEFAULT 0,
    ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ DEFAULT clock_timestamp();

-- Backfill channel_type from existing type column
UPDATE meclaw.channels SET channel_type = type WHERE channel_type IS NULL;

-- =============================================================================
-- 2. Add seq to messages (global ordering for knowledge graph versioning)
-- =============================================================================

-- NOTE: Adding GENERATED ALWAYS AS IDENTITY to an existing table with active
-- triggers can cause lock contention. For Phase 1, brain_events has its own seq.
-- messages.seq will be added during a maintenance window (Phase 2).
-- For now, messages.created_at provides temporal ordering.

-- =============================================================================
-- 3. Entities (AIEOS-compatible, all types: person, agent, workspace, etc.)
-- =============================================================================

CREATE TABLE IF NOT EXISTS meclaw.entities (
    -- Identity
    id TEXT PRIMARY KEY,                -- 'meclaw:person:marcus-meyer', 'meclaw:agent:walter', 'meclaw:workspace:gisela'
    canonical_name TEXT NOT NULL,
    aliases TEXT[] DEFAULT '{}',
    entity_type TEXT NOT NULL,          -- 'person', 'agent', 'workspace', 'system', 'project', 'tool', 'concept'

    -- AIEOS Psychology (Soul Layer)
    neural_matrix JSONB,                -- {creativity: 0.7, empathy: 0.8, logic: 0.9, adaptability: 0.6, charisma: 0.5, reliability: 0.8}
    traits JSONB,                       -- {ocean: {openness: 0.8, ...}, mbti: 'INTJ', enneagram: '', temperament: ''}
    moral_compass JSONB,                -- {alignment: 'neutral-good', core_values: [...], conflict_resolution: ''}

    -- AIEOS Linguistics
    linguistics JSONB,                  -- {formality: 0.3, forbidden_words: [...], catchphrases: [...], vocabulary_level: 'technical'}

    -- AIEOS Capabilities
    capabilities JSONB,                 -- [{name: 'sql_read', priority: 1, description: '', uri: ''}, ...]

    -- AIEOS Metadata (optional, for agent-to-agent discovery)
    aieos_entity_id UUID,
    aieos_public_key TEXT,              -- Ed25519

    -- Dual Profile (primarily for persons, but available for all)
    explicit_profile JSONB DEFAULT '{}',  -- self-reported data
    observed_profile JSONB DEFAULT '{}',  -- agent-learned, consolidated from entity_observations

    -- Embedding (for entity resolution / similarity)
    embedding vector(1536),

    -- Timestamps
    created_seq BIGINT,
    created_at TIMESTAMPTZ DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- Indexes for entity lookups
CREATE INDEX IF NOT EXISTS idx_entities_type ON meclaw.entities (entity_type);
CREATE INDEX IF NOT EXISTS idx_entities_aliases ON meclaw.entities USING GIN (aliases);
CREATE INDEX IF NOT EXISTS idx_entities_embedding ON meclaw.entities USING hnsw (embedding vector_cosine_ops);

-- =============================================================================
-- 4. Agent-Channel Subscriptions
-- =============================================================================

CREATE TABLE IF NOT EXISTS meclaw.agent_channels (
    agent_id TEXT NOT NULL REFERENCES meclaw.entities(id),
    channel_id UUID NOT NULL REFERENCES meclaw.channels(id),
    role TEXT NOT NULL DEFAULT 'participant',    -- 'owner', 'participant', 'observer'
    scope TEXT NOT NULL DEFAULT 'private',       -- 'private', 'shared', 'workspace'
    subscribed_at TIMESTAMPTZ DEFAULT clock_timestamp(),
    PRIMARY KEY (agent_id, channel_id)
);

CREATE INDEX IF NOT EXISTS idx_agent_channels_channel ON meclaw.agent_channels (channel_id);

-- =============================================================================
-- 5. Brain Events (append-only, channel-level extraction + agent-level rewards)
-- =============================================================================

CREATE TABLE IF NOT EXISTS meclaw.brain_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seq BIGINT GENERATED ALWAYS AS IDENTITY,

    -- Source references
    message_id UUID REFERENCES meclaw.messages(id),
    channel_id UUID REFERENCES meclaw.channels(id),

    -- Agent scope: NULL = shared extraction (channel-level), non-NULL = agent-level overlay
    agent_id TEXT REFERENCES meclaw.entities(id),

    -- Content
    content TEXT NOT NULL,
    embedding vector(1536),

    -- Value signals (agent-level, mutable)
    reward FLOAT DEFAULT 0.0,
    novelty FLOAT DEFAULT 0.0,
    reward_updated_seq BIGINT DEFAULT 0,

    -- Timestamps
    created_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- Indexes for brain_events
CREATE INDEX IF NOT EXISTS idx_brain_events_seq ON meclaw.brain_events (seq);
CREATE INDEX IF NOT EXISTS idx_brain_events_channel ON meclaw.brain_events (channel_id);
CREATE INDEX IF NOT EXISTS idx_brain_events_agent ON meclaw.brain_events (agent_id);
CREATE INDEX IF NOT EXISTS idx_brain_events_message ON meclaw.brain_events (message_id);
CREATE INDEX IF NOT EXISTS idx_brain_events_embedding ON meclaw.brain_events USING hnsw (embedding vector_cosine_ops);
CREATE INDEX IF NOT EXISTS idx_brain_events_reward ON meclaw.brain_events (reward DESC) WHERE agent_id IS NOT NULL;

-- =============================================================================
-- 6. Prototypes (emergent concepts, agent-scoped)
-- =============================================================================

CREATE TABLE IF NOT EXISTS meclaw.prototypes (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES meclaw.entities(id),

    -- Centroid embedding
    centroid vector(1536),

    -- Statistics
    weight FLOAT DEFAULT 1.0,
    activation_count INT DEFAULT 0,
    value_mean FLOAT DEFAULT 0.0,
    value_variance FLOAT DEFAULT 0.0,
    last_activated_seq BIGINT DEFAULT 0,

    -- Timestamps
    created_seq BIGINT,
    created_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

CREATE INDEX IF NOT EXISTS idx_prototypes_agent ON meclaw.prototypes (agent_id);
CREATE INDEX IF NOT EXISTS idx_prototypes_centroid ON meclaw.prototypes USING hnsw (centroid vector_cosine_ops);

-- =============================================================================
-- 7. Prototype Associations (Hebbian co-activation, agent-scoped)
-- =============================================================================

CREATE TABLE IF NOT EXISTS meclaw.prototype_associations (
    prototype_a TEXT NOT NULL REFERENCES meclaw.prototypes(id),
    prototype_b TEXT NOT NULL REFERENCES meclaw.prototypes(id),
    weight FLOAT DEFAULT 0.0,
    last_updated_seq BIGINT,
    PRIMARY KEY (prototype_a, prototype_b)
);

-- =============================================================================
-- 8. Entity Observations (agent's learned observations about entities)
-- =============================================================================

CREATE TABLE IF NOT EXISTS meclaw.entity_observations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Who is being observed, by whom, in which channel
    entity_id TEXT NOT NULL REFERENCES meclaw.entities(id),
    agent_id TEXT NOT NULL REFERENCES meclaw.entities(id),
    channel_id UUID REFERENCES meclaw.channels(id),

    -- Observation data
    observation_type TEXT NOT NULL,      -- 'preference', 'behavior', 'fact', 'relationship'
    key TEXT NOT NULL,                   -- 'communication_style', 'timezone', 'interests', etc.
    value JSONB NOT NULL,                -- {value: 'direct', evidence: 'never uses filler words'}
    confidence FLOAT DEFAULT 0.5,        -- 0.0 - 1.0, increases with repeated observations

    -- Tracking
    observation_count INT DEFAULT 1,
    first_observed_seq BIGINT,
    last_observed_seq BIGINT,
    superseded_by UUID REFERENCES meclaw.entity_observations(id),

    -- Timestamps
    created_at TIMESTAMPTZ DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

CREATE INDEX IF NOT EXISTS idx_entity_obs_entity ON meclaw.entity_observations (entity_id);
CREATE INDEX IF NOT EXISTS idx_entity_obs_agent ON meclaw.entity_observations (agent_id);
CREATE INDEX IF NOT EXISTS idx_entity_obs_key ON meclaw.entity_observations (entity_id, key);
CREATE INDEX IF NOT EXISTS idx_entity_obs_confidence ON meclaw.entity_observations (confidence DESC) WHERE superseded_by IS NULL;

-- =============================================================================
-- 9. Decision Traces (immutable audit trail, agent-scoped)
-- =============================================================================

CREATE TABLE IF NOT EXISTS meclaw.decision_traces (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seq BIGINT GENERATED ALWAYS AS IDENTITY,

    -- Agent scope
    agent_id TEXT NOT NULL REFERENCES meclaw.entities(id),

    -- Decision context
    query TEXT NOT NULL,
    evidence_ids UUID[],                 -- brain_event IDs used as evidence
    prototypes_activated TEXT[],         -- prototype IDs that were activated
    q_value_estimates JSONB,             -- estimated value of different actions

    -- Outcome
    action_taken TEXT,
    reward FLOAT DEFAULT 0.0,

    -- Timestamps
    created_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

CREATE INDEX IF NOT EXISTS idx_decision_traces_agent ON meclaw.decision_traces (agent_id);
CREATE INDEX IF NOT EXISTS idx_decision_traces_seq ON meclaw.decision_traces (seq);

-- =============================================================================
-- 10. Helper view: agent memory statistics
-- =============================================================================

CREATE OR REPLACE VIEW meclaw.agent_memory_stats AS
SELECT
    e.id AS agent_id,
    e.canonical_name AS agent_name,
    e.entity_type,
    (SELECT COUNT(*) FROM meclaw.brain_events be WHERE be.agent_id = e.id) AS personal_events,
    (SELECT COUNT(*) FROM meclaw.prototypes p WHERE p.agent_id = e.id) AS prototypes,
    (SELECT COUNT(*) FROM meclaw.entity_observations eo WHERE eo.agent_id = e.id) AS observations,
    (SELECT COUNT(*) FROM meclaw.decision_traces dt WHERE dt.agent_id = e.id) AS decisions,
    (SELECT AVG(be.reward) FROM meclaw.brain_events be WHERE be.agent_id = e.id) AS avg_reward,
    (SELECT COUNT(*) FROM meclaw.agent_channels ac WHERE ac.agent_id = e.id) AS subscribed_channels
FROM meclaw.entities e
WHERE e.entity_type IN ('agent', 'system', 'workspace');
