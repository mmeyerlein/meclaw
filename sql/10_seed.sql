-- MeClaw v0.1.0 — Seed Data (no credentials)
-- Credentials go in config.sql (see config.example.sql)

-- Default Telegram Channel (placeholder — configure in config.sql)
INSERT INTO meclaw.channels (id, name, type, config) VALUES (
    '00000000-0000-0000-0000-000000000001',
    'telegram-main',
    'telegram',
    '{"bot_token": "", "chat_id": "", "poll_timeout": 25}'
) ON CONFLICT (id) DO NOTHING;

-- LLM Providers (no keys — configure in config.sql)
INSERT INTO meclaw.llm_providers (id, name, base_url, api_key, type, priority) VALUES
    ('vllm-local', 'vLLM Local', 'http://localhost:8000/v1', NULL, 'openai', 10),
    ('openrouter', 'OpenRouter', 'https://openrouter.ai/api/v1', NULL, 'openai', 1)
ON CONFLICT (id) DO NOTHING;

-- Default Models
INSERT INTO meclaw.llm_models (id, provider_id, model_name, display_name, tier, max_tokens, cost_per_1k_in, cost_per_1k_out, supports_tools) VALUES
    ('qwen-9b', 'vllm-local', 'Qwen/Qwen3.5-9B', 'Qwen 3.5 9B', 'small', 1024, 0, 0, true),
    ('sonnet-4', 'openrouter', 'anthropic/claude-sonnet-4', 'Claude Sonnet 4', 'large', 4096, 0.003, 0.015, true),
    ('haiku-3.5', 'openrouter', 'anthropic/claude-3.5-haiku', 'Claude 3.5 Haiku', 'medium', 4096, 0.0008, 0.004, true),
    ('gpt-4o-mini', 'openrouter', 'openai/gpt-4o-mini', 'GPT-4o Mini', 'small', 2048, 0.00015, 0.0006, true)
ON CONFLICT (id) DO NOTHING;

-- Default Tools
INSERT INTO meclaw.tools (id, name, description, parameters, handler) VALUES
    ('sql_read', 'sql_read', 'Execute a read-only SQL SELECT query against the database.', '{"type":"object","properties":{"query":{"type":"string","description":"SQL SELECT query"}},"required":["query"]}', 'meclaw.tool_sql_read'),
    ('sql_write', 'sql_write', 'Execute a SQL write query (INSERT, UPDATE, DELETE). DROP/TRUNCATE blocked.', '{"type":"object","properties":{"query":{"type":"string","description":"SQL write query"}},"required":["query"]}', 'meclaw.tool_sql_write'),
    ('python_exec', 'python_exec', 'Execute Python code. Store result in variable "result" or use print().', '{"type":"object","properties":{"code":{"type":"string","description":"Python code"}},"required":["code"]}', 'meclaw.tool_python_exec')
ON CONFLICT (id) DO NOTHING;

-- Rate Limits
INSERT INTO meclaw.rate_limits (id, max_count, window_sec) VALUES
    ('llm_per_minute', 10, 60),
    ('llm_per_hour', 100, 3600),
    ('llm_per_day', 500, 86400)
ON CONFLICT (id) DO NOTHING;

-- Watchdog Cron Jobs
SELECT cron.schedule('meclaw-watchdog', '* * * * *', 'SELECT meclaw.watchdog()');
SELECT cron.schedule('admin-bee-watchdog', '* * * * *', 'SELECT meclaw.admin_bee_watchdog()');
