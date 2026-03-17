-- MeClaw v0.1.0 — Schema
-- Stand: 2026-03-16 22:32

CREATE SCHEMA IF NOT EXISTS meclaw;

-- Channels (Telegram, etc.)
CREATE TABLE IF NOT EXISTS meclaw.channels (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    type       TEXT NOT NULL,
    config     JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- Tasks (eine User-Anfrage = ein Task)
CREATE TABLE IF NOT EXISTS meclaw.tasks (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id UUID REFERENCES meclaw.channels(id),
    status     TEXT NOT NULL DEFAULT 'ready',
    created_at TIMESTAMPTZ DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- Messages (append-only, nur status/assigned_to/waiting_for updatebar)
CREATE TABLE IF NOT EXISTS meclaw.messages (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    task_id     UUID REFERENCES meclaw.tasks(id),
    channel_id  UUID REFERENCES meclaw.channels(id),
    previous_id UUID REFERENCES meclaw.messages(id),
    type        TEXT NOT NULL,
    sender      TEXT,
    status      TEXT NOT NULL DEFAULT 'ready',
    assigned_to TEXT,
    waiting_for TEXT,
    next_bee    TEXT,
    content     JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- Events (vollständiges Audit-Log)
CREATE TABLE IF NOT EXISTS meclaw.events (
    id         BIGSERIAL PRIMARY KEY,
    msg_id     UUID,
    task_id    UUID,
    bee_type   TEXT,
    event      TEXT NOT NULL,
    payload    JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- Net Requests Tracking (welcher pg_net Request gehört zu was)
CREATE TABLE IF NOT EXISTS meclaw.net_requests (
    net_req_id BIGINT PRIMARY KEY,
    type       TEXT NOT NULL,
    ref_id     UUID,
    created_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- LLM Jobs Staging (für pg_background → eigene Transaktion)
CREATE TABLE IF NOT EXISTS meclaw.llm_jobs (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    msg_id     UUID NOT NULL,
    url        TEXT NOT NULL,
    body       JSONB NOT NULL,
    timeout_ms INT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- Indexe
CREATE INDEX IF NOT EXISTS idx_messages_channel_id ON meclaw.messages (channel_id);
CREATE INDEX IF NOT EXISTS idx_messages_task_id    ON meclaw.messages (task_id);
CREATE INDEX IF NOT EXISTS idx_messages_status     ON meclaw.messages (status) WHERE status NOT IN ('done', 'failed');
CREATE INDEX IF NOT EXISTS idx_messages_next_bee   ON meclaw.messages (next_bee) WHERE next_bee IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_messages_waiting    ON meclaw.messages (waiting_for) WHERE status = 'waiting';
CREATE INDEX IF NOT EXISTS idx_net_requests_type   ON meclaw.net_requests (type);
CREATE INDEX IF NOT EXISTS idx_tasks_status        ON meclaw.tasks (status) WHERE status NOT IN ('done', 'failed');
CREATE INDEX IF NOT EXISTS idx_events_msg_id       ON meclaw.events (msg_id);
CREATE INDEX IF NOT EXISTS idx_events_task_id      ON meclaw.events (task_id);
CREATE INDEX IF NOT EXISTS idx_events_event        ON meclaw.events (event);
CREATE INDEX IF NOT EXISTS idx_events_created      ON meclaw.events (created_at);

-- Channel Chunks (Konversations-Chunking für Optimierung)
CREATE TABLE IF NOT EXISTS meclaw.channel_chunks (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id  UUID NOT NULL REFERENCES meclaw.channels(id),
    chunk_index INT NOT NULL,
    started_at  TIMESTAMPTZ NOT NULL,
    ended_at    TIMESTAMPTZ,
    msg_count   INT NOT NULL DEFAULT 0,
    status      TEXT NOT NULL DEFAULT 'open',
    created_at  TIMESTAMPTZ DEFAULT clock_timestamp(),
    UNIQUE (channel_id, chunk_index)
);

CREATE INDEX IF NOT EXISTS idx_channel_chunks_open 
    ON meclaw.channel_chunks (channel_id, status) WHERE status = 'open';

-- Views
CREATE OR REPLACE VIEW meclaw.recent_events AS
SELECT id, created_at, bee_type, event, msg_id, task_id, payload
FROM meclaw.events ORDER BY id DESC;

CREATE OR REPLACE VIEW meclaw.channel_conversation AS
SELECT 
    m.id, m.task_id, m.channel_id, m.type, m.created_at,
    CASE WHEN m.type = 'user_input' THEN 'user'
         WHEN m.type = 'llm_result' THEN 'assistant'
         ELSE 'system' END AS role,
    CASE WHEN m.type = 'user_input' THEN m.content->>'input'
         WHEN m.type = 'llm_result' THEN m.content->>'output'
         ELSE NULL END AS text
FROM meclaw.messages m
WHERE m.type IN ('user_input', 'llm_result') AND m.status = 'done'
ORDER BY m.created_at;

-- Rate limits
CREATE TABLE IF NOT EXISTS meclaw.rate_limits (
    id text PRIMARY KEY,
    max_count integer NOT NULL,
    window_sec integer NOT NULL,
    created_at timestamptz DEFAULT clock_timestamp()
);
