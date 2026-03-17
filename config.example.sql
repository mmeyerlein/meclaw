-- meclaw Configuration
-- Copy this file to config.sql and fill in your values.
-- config.sql is in .gitignore and will NOT be committed.

-- Telegram Bot
INSERT INTO meclaw.channels (id, name, type, config) VALUES (
    '00000000-0000-0000-0000-000000000001',
    'telegram-main',
    'telegram',
    '{"bot_token": "YOUR_TELEGRAM_BOT_TOKEN", "chat_id": "YOUR_CHAT_ID", "poll_timeout": 25}'
) ON CONFLICT (id) DO UPDATE SET config = EXCLUDED.config;

-- LLM Providers
UPDATE meclaw.llm_providers SET api_key = 'YOUR_OPENROUTER_API_KEY' WHERE id = 'openrouter';

-- Local vLLM (adjust URL if different)
UPDATE meclaw.llm_providers SET base_url = 'http://localhost:8000/v1' WHERE id = 'vllm-local';
