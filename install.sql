-- meclaw v0.1.0 — Installation Script
-- Run as superuser: psql -U postgres -d meclaw -f install.sql
--
-- Prerequisites:
--   1. PostgreSQL 17+ with shared_preload_libraries = 'pg_cron,pg_net,age'
--   2. pg_background.max_workers = 256 in postgresql.conf
--   3. Database 'meclaw' created: CREATE DATABASE meclaw;
--   4. After install: copy config.example.sql to config.sql and fill credentials

\echo '=== meclaw v0.1.0 Installation ==='
\echo ''

\echo '>>> 01 Extensions'
\i sql/01_extensions.sql

\echo '>>> 02 Schema'
\i sql/02_schema.sql

\echo '>>> 03 Logging'
\i sql/03_logging.sql

\echo '>>> 04 Channel Bee'
\i sql/04_channel_bee.sql

\echo '>>> 05 Router Bee'
\i sql/05_router_bee.sql

\echo '>>> 06 LLM Bee'
\i sql/06_llm_bee.sql

\echo '>>> 07 IO Bees'
\i sql/07_io_bees.sql

\echo '>>> 08 Triggers'
\i sql/08_triggers.sql

\echo '>>> 14 Tools'
\i sql/14_tools.sql

\echo '>>> 15 LLM Providers'
\i sql/15_llm_providers.sql

\echo '>>> 09 AGE Graph'
\i sql/09_age_graph.sql

\echo '>>> 12 Admin Bee'
\i sql/12_admin_bee.sql

\echo '>>> 13 Context Bee'
\i sql/13_context_bee.sql

\echo '>>> 10 Seed Data'
\i sql/10_seed.sql

\echo ''
\echo '=== Installation complete ==='
\echo 'Next steps:'
\echo '  1. cp config.example.sql config.sql'
\echo '  2. Edit config.sql with your credentials'
\echo '  3. psql -d meclaw -f config.sql'
\echo '  4. psql -d meclaw -f sql/11_start.sql'
\echo ''
