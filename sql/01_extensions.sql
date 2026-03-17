-- MeClaw v0.1.0 — Extensions
-- Voraussetzung: shared_preload_libraries = 'pg_cron,pg_net,age'
-- postgresql.conf: pg_background.max_workers = 256

CREATE EXTENSION IF NOT EXISTS pg_cron;
CREATE EXTENSION IF NOT EXISTS pg_net;
CREATE EXTENSION IF NOT EXISTS age;
CREATE EXTENSION IF NOT EXISTS plpython3u;
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS pg_background;
