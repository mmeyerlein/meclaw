#!/bin/bash
# MeClaw v0.1.0 — Docker Init Script
# Runs as docker-entrypoint-initdb.d entrypoint (first boot only)

echo "========================================="
echo "  MeClaw v0.1.0 — Initializing"
echo "========================================="

PGUSER="${POSTGRES_USER:-postgres}"
MECLAW_DIR="/docker-entrypoint-initdb.d/meclaw"

# 1. Create database
echo ">>> Creating database 'meclaw'..."
psql -U "$PGUSER" -c "CREATE DATABASE meclaw;" 2>/dev/null || echo "    (database already exists)"

# 2. Run all SQL files in dependency order
FAILED=0
for sqlfile in \
    sql/01_extensions.sql \
    sql/02_schema.sql \
    sql/03_logging.sql \
    sql/04_channel_bee.sql \
    sql/05_router_bee.sql \
    sql/06_llm_bee.sql \
    sql/07_io_bees.sql \
    sql/08_triggers.sql \
    sql/09_age_graph.sql \
    sql/12_admin_bee.sql \
    sql/13_context_bee.sql \
    sql/14_tools.sql \
    sql/15_llm_providers.sql \
    sql/10_seed.sql \
    sql/16_brain_schema.sql \
    sql/17_age_agents.sql \
    sql/18_seed_agents.sql \
    sql/19_extract_bee.sql \
    sql/20_retrieve_bee.sql \
    sql/21_context_bee_v2.sql \
    sql/22_embedding_bee.sql \
    sql/23_novelty_bee.sql \
    sql/24_feedback_bee.sql \
    sql/25_phase3_graph_intelligence.sql \
    sql/26_consolidation_bee.sql \
    sql/27_ctm_retrieval.sql \
    sql/28_extract_bee_v2.sql \
    sql/29_phase7_robustness.sql \
    sql/30_smoke_tests.sql \
    sql/31_phase8_swarm.sql \
    sql/32_phase9_context_pipeline.sql \
    sql/33_phase10_tests.sql \
    sql/34_temporal_edges.sql \
    sql/35_fact_keys.sql \
    sql/36_trigger_chain.sql \
    sql/37_ctm_retrieval_v2.sql \
    sql/38_prototypes_activation.sql \
    sql/39_user_modeling.sql
do
    echo ">>> $(basename $sqlfile)"
    OUTPUT=$(psql -U "$PGUSER" -d meclaw -f "$MECLAW_DIR/$sqlfile" 2>&1)
    if echo "$OUTPUT" | grep -qi "ERROR:"; then
        echo "    ⚠️  Errors in $sqlfile (continuing...)"
        echo "$OUTPUT" | grep -i "ERROR:" | head -5
        FAILED=$((FAILED + 1))
    fi
done

# 3. Apply user config (credentials)
if [ -f /config/config.sql ]; then
    echo ">>> Applying config.sql..."
    psql -U "$PGUSER" -d meclaw -f /config/config.sql 2>&1 | grep -v "^$"
else
    echo ">>> No config.sql found — using defaults (no credentials)"
    echo "    Mount config.sql to /config/config.sql"
fi

# 4. Web channel (always ensure it exists)
psql -U "$PGUSER" -d meclaw -c "
    INSERT INTO meclaw.channels (id, name, type, config) VALUES (
        '00000000-0000-0000-0000-000000000002', 'web-admin', 'web', '{}'
    ) ON CONFLICT (id) DO NOTHING;
" 2>/dev/null

# 5. Start Telegram poll
echo ">>> Starting channel bee..."
psql -U "$PGUSER" -d meclaw -f "$MECLAW_DIR/sql/11_start.sql" 2>&1 | grep -v "^$"

# 6. Run smoke tests
echo ">>> Running smoke tests..."
RESULT=$(psql -U "$PGUSER" -d meclaw -t -c "SELECT meclaw.run_smoke_tests();" 2>&1)
echo "    $RESULT"

echo ""
echo "========================================="
if [ $FAILED -eq 0 ]; then
    echo "  MeClaw v0.1.0 — Ready ✅"
else
    echo "  MeClaw v0.1.0 — Ready (${FAILED} file(s) had errors)"
fi
echo "  Run smoke tests:  SELECT meclaw.run_smoke_tests();"
echo "  Run full tests:   SELECT meclaw.run_all_tests();"
echo "========================================="
