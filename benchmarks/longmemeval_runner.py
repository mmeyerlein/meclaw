#!/usr/bin/env python3
"""
MeClaw LongMemEval Benchmark Runner

Feeds LongMemEval sessions into MeClaw's Brain pipeline and evaluates
memory retrieval quality against expected answers.

Usage:
  # Copy to test VM and run:
  python3 longmemeval_runner.py \
    --data /tmp/longmemeval/data/longmemeval_oracle.json \
    --limit 5 --output /tmp/longmemeval/results.json

  # Cumulative mode (brain accumulates across questions):
  python3 longmemeval_runner.py \
    --data /tmp/longmemeval/data/longmemeval_oracle.json \
    --limit 5 --cumulative --output /tmp/longmemeval/results_cumulative.json

  # CTM mode (uses BM25+CTM drift retrieval):
  python3 longmemeval_runner.py \
    --data /tmp/longmemeval/data/longmemeval_oracle.json \
    --limit 5 --ctm --output /tmp/longmemeval/results_ctm.json

  # Skip LLM extraction (cheaper, no entity graph):
  python3 longmemeval_runner.py \
    --data /tmp/longmemeval/data/longmemeval_oracle.json \
    --limit 5 --skip-extraction

Requires: psycopg2-binary
"""

import json
import argparse
import time
import sys
import uuid

try:
    import psycopg2
    import psycopg2.extras
except ImportError:
    print("ERROR: pip install psycopg2-binary")
    sys.exit(1)


DB_DSN = "host=localhost dbname=meclaw user=postgres password=postgres"

# Agent ID that owns the benchmark channel
BENCHMARK_AGENT_ID = "meclaw:agent:walter"


def get_conn(dsn):
    conn = psycopg2.connect(dsn)
    conn.autocommit = True
    return conn


def reset_brain(conn):
    """Clear all brain data for clean benchmark run."""
    with conn.cursor() as cur:
        cur.execute("TRUNCATE meclaw.brain_events CASCADE")
        cur.execute("TRUNCATE meclaw.entity_events CASCADE")
        cur.execute("DELETE FROM meclaw.messages")
        cur.execute("DELETE FROM meclaw.tasks")
        cur.execute("DELETE FROM meclaw.channel_conversation")
    # NOTE: ParadeDB < 0.19.0 had rt_fetch out-of-bounds bug after TRUNCATE
    # requiring DROP+CREATE of BM25 index. Fixed in pg_search >= 0.22.2.
    # See: https://github.com/paradedb/paradedb/issues/2462
    print("  [reset] Brain cleared")


def get_or_create_channel(conn):
    """Ensure benchmark channel exists and is linked to agent, return its ID."""
    with conn.cursor() as cur:
        cur.execute("""
            SELECT id FROM meclaw.channels WHERE name = 'benchmark'
        """)
        row = cur.fetchone()
        if row:
            channel_id = row[0]
        else:
            channel_id = str(uuid.uuid4())
            cur.execute("""
                INSERT INTO meclaw.channels (id, name, type, config)
                VALUES (%s, 'benchmark', 'web', '{}')
            """, (channel_id,))

        # Ensure agent_channel link exists for both agents
        for agent_id in (BENCHMARK_AGENT_ID, "meclaw:agent:system"):
            cur.execute("""
                INSERT INTO meclaw.agent_channels (agent_id, channel_id, role, scope)
                VALUES (%s, %s, 'participant', 'private')
                ON CONFLICT DO NOTHING
            """, (agent_id, channel_id))

        return channel_id


def parse_benchmark_date(date_str):
    """Parse LongMemEval date format: '2023/04/10 (Mon) 17:50' → datetime."""
    if not date_str:
        return None
    try:
        # Remove day-of-week: '2023/04/10 (Mon) 17:50' → '2023/04/10 17:50'
        import re
        clean = re.sub(r'\s*\([A-Za-z]+\)\s*', ' ', date_str).strip()
        from datetime import datetime
        return datetime.strptime(clean, "%Y/%m/%d %H:%M")
    except:
        return None


def feed_message(conn, channel_id, role, content, session_date=""):
    """Insert a single message into MeClaw and trigger extract_bee."""
    msg_id = str(uuid.uuid4())
    task_id = str(uuid.uuid4())

    # Parse session date for temporal indexing
    ts = parse_benchmark_date(session_date)

    with conn.cursor() as cur:
        # Create task first (FK constraint)
        cur.execute("""
            INSERT INTO meclaw.tasks (id, channel_id, status)
            VALUES (%s, %s, 'done')
        """, (task_id, channel_id))

        # extract_bee expects type='user_input' and content->>'input'
        msg_type = "user_input" if role == "user" else "llm_result"
        sender = "benchmark_user" if role == "user" else "meclaw"

        # Content format: extract_bee reads content->>'input' or content->>'output'
        if role == "user":
            content_json = json.dumps({"input": content, "benchmark_date": session_date})
        else:
            content_json = json.dumps({"output": content, "benchmark_date": session_date})

        # Use session timestamp for created_at (temporal indexing!)
        cur.execute("""
            INSERT INTO meclaw.messages (id, task_id, channel_id, type, sender,
                                         status, content, created_at)
            VALUES (%s, %s, %s, %s, %s, 'done', %s, COALESCE(%s, NOW()))
        """, (msg_id, task_id, channel_id, msg_type, sender, content_json, ts))

        # NOTE: extract_bee is triggered automatically by trg_extract_on_done
        # when message status='done'. Do NOT call it manually — causes duplicates.

    return msg_id


def run_signal_pipeline(conn, skip_extraction=False):
    """
    Explicitly run all async/deferred pipeline steps that normally fire via
    pg_background or external triggers, ensuring all 6 ranking signals are
    populated before retrieval.

    Steps:
      1. backfill_extractions  → LLM entity extraction (sets extracted=true,
                                  facts_text via trigger, entity_events rows)
      2. backfill_entity_graph → sync entity/event nodes + INVOLVED_IN edges
                                  into AGE graph
      3. backfill_temporal_edges → NEXT/PREV temporal edges in AGE graph
      4. novelty_bee per event → prototype-based novelty score
      5. facts_text repair     → fill any remaining NULL facts_text
    """
    with conn.cursor() as cur:

        # ── Step 1: LLM entity extraction ────────────────────────────────────
        if skip_extraction:
            print("  [pipeline] skipping LLM extraction (--skip-extraction)")
        else:
            cur.execute("""
                SELECT COUNT(*) FROM meclaw.brain_events
                WHERE extracted = false AND content IS NOT NULL AND length(content) >= 10
            """)
            unextracted = cur.fetchone()[0]

            if unextracted > 0:
                print(f"  [pipeline] LLM extraction: {unextracted} events...")
                t0 = time.time()
                # backfill_extractions processes up to p_limit events with 0.5s rate-limit sleep
                # Call in batches of 50 until done
                total_extracted = 0
                batch = 50
                max_iterations = (unextracted // batch) + 2
                for _ in range(max_iterations):
                    cur.execute("SELECT meclaw.backfill_extractions(%s)", (batch,))
                    n = cur.fetchone()[0]
                    total_extracted += n
                    if n == 0:
                        break
                elapsed = time.time() - t0
                print(f"  [pipeline] extracted {total_extracted} events in {elapsed:.1f}s")
            else:
                print("  [pipeline] LLM extraction: nothing to do")

        # ── Step 2: facts_text repair (should auto-fire via trigger, but ensure) ─
        cur.execute("""
            UPDATE meclaw.brain_events
            SET facts_text = meclaw.build_facts_text(id)
            WHERE extracted = true
              AND facts_text IS NULL
              AND EXISTS (
                  SELECT 1 FROM meclaw.entity_events ee WHERE ee.event_id = id
              )
        """)
        facts_repaired = cur.rowcount
        if facts_repaired > 0:
            print(f"  [pipeline] facts_text repaired for {facts_repaired} events")

        # ── Step 3: AGE entity graph backfill ────────────────────────────────
        cur.execute("""
            SELECT COUNT(*) FROM meclaw.entity_events
        """)
        entity_event_count = cur.fetchone()[0]

        if entity_event_count > 0:
            print(f"  [pipeline] AGE entity graph backfill ({entity_event_count} entity_events)...")
            t0 = time.time()
            cur.execute("SELECT meclaw.backfill_entity_graph(500)")
            result = cur.fetchone()[0]
            elapsed = time.time() - t0
            print(f"  [pipeline] AGE graph: {result} in {elapsed:.1f}s")
        else:
            print("  [pipeline] AGE entity graph: no entity_events yet")

        # ── Step 4: Temporal edge backfill ───────────────────────────────────
        print("  [pipeline] temporal edges backfill...")
        t0 = time.time()
        try:
            cur.execute("SELECT * FROM meclaw.backfill_temporal_edges()")
            row = cur.fetchone()
            elapsed = time.time() - t0
            if row:
                print(f"  [pipeline] temporal: updated={row[0]} created={row[1]} errors={row[2]} in {elapsed:.1f}s")
        except Exception as e:
            print(f"  [pipeline] temporal edges warn: {e}")

        # ── Step 5: novelty_bee per event ────────────────────────────────────
        cur.execute("""
            SELECT id FROM meclaw.brain_events
            WHERE (novelty IS NULL OR novelty = 0)
              AND embedding IS NOT NULL
            ORDER BY seq ASC
        """)
        novelty_ids = [row[0] for row in cur.fetchall()]

        if novelty_ids:
            print(f"  [pipeline] novelty_bee: {len(novelty_ids)} events...")
            t0 = time.time()
            ok = 0
            for eid in novelty_ids:
                try:
                    cur.execute(
                        "SELECT meclaw.novelty_bee(%s, %s)",
                        (BENCHMARK_AGENT_ID, eid)
                    )
                    ok += 1
                except Exception as e:
                    print(f"    [warn] novelty_bee {eid}: {e}")
            elapsed = time.time() - t0
            print(f"  [pipeline] novelty_bee: {ok}/{len(novelty_ids)} in {elapsed:.1f}s")
        else:
            print("  [pipeline] novelty_bee: nothing to do")

        # ── Step 6: BM25 index refresh ───────────────────────────────────────
        # ParadeDB index is updated on INSERT, but a manual refresh can help
        # after bulk ops. Try REINDEX if the function exists.
        try:
            cur.execute("""
                SELECT EXISTS (
                    SELECT 1 FROM pg_indexes
                    WHERE schemaname = 'meclaw'
                      AND tablename = 'brain_events'
                      AND indexname LIKE '%bm25%'
                )
            """)
            has_bm25 = cur.fetchone()[0]
            if has_bm25:
                # Lightweight: just verify the index is healthy (no full REINDEX needed for inserts)
                cur.execute("""
                    SELECT indexname FROM pg_indexes
                    WHERE schemaname = 'meclaw'
                      AND tablename = 'brain_events'
                      AND indexname LIKE '%bm25%'
                """)
                idx = cur.fetchone()
                if idx:
                    print(f"  [pipeline] BM25 index '{idx[0]}' present (no reindex needed for inserts)")
        except Exception as e:
            print(f"  [pipeline] BM25 check warn: {e}")


def retrieve_context_full(conn, question, limit=10, ctm_enabled=False,
                          rerank=False, rerank_pool=20, question_date=None,
                          smart=False):
    """
    Use retrieve_bee to get relevant brain context for a question.
    If smart=True, uses retrieve_smart (temporal expansion + reranking).
    If rerank=True, uses LLM re-ranking (Stage 3) via retrieve_reranked.
    Returns (context_text, source_tags) tuple.
    """
    with conn.cursor() as cur:
        try:
            if smart:
                cur.execute(
                    """
                    SELECT content, score, source
                    FROM meclaw.retrieve_smart(%s, %s, %s, %s, %s, %s)
                    """,
                    (BENCHMARK_AGENT_ID, question, question_date,
                     limit, rerank, ctm_enabled)
                )
            elif rerank:
                cur.execute(
                    """
                    SELECT content, score, source
                    FROM meclaw.retrieve_reranked(%s, %s, %s, %s, %s)
                    """,
                    (BENCHMARK_AGENT_ID, question, limit, ctm_enabled, rerank_pool)
                )
            else:
                cur.execute(
                    """
                    SELECT content, score, source
                    FROM meclaw.retrieve_bee(%s, %s, %s, %s)
                    ORDER BY score DESC
                    """,
                    (BENCHMARK_AGENT_ID, question, limit, ctm_enabled)
                )
            rows = cur.fetchall()
            if rows:
                sources = list({r[2] for r in rows if r[2]})
                if smart:
                    sources.append('smart')
                elif rerank:
                    sources.append('reranked')
                context = "\n".join(r[0] for r in rows if r[0])
                return context, sources
        except Exception as e:
            print(f"    [warn] retrieve: {e}")
            conn.rollback()
            conn.autocommit = True

        # Fallback: direct brain_events search
        try:
            cur.execute("""
                SELECT content, extraction_data
                FROM meclaw.brain_events
                WHERE content IS NOT NULL
                ORDER BY created_at DESC
                LIMIT %s
            """, (limit,))
            rows = cur.fetchall()
            if rows:
                parts = []
                for r in rows:
                    parts.append(r[0])
                    if r[1]:
                        parts.append(json.dumps(r[1]))
                return "\n".join(parts), ["fallback_direct"]
        except Exception as e:
            print(f"    [warn] fallback query: {e}")
            conn.rollback()
            conn.autocommit = True

    return None, []


def check_brain_stats(conn):
    """Print current brain statistics including signal coverage."""
    with conn.cursor() as cur:
        cur.execute("SELECT COUNT(*) FROM meclaw.brain_events")
        brain_count = cur.fetchone()[0]
        cur.execute("SELECT COUNT(*) FROM meclaw.entity_events")
        entity_count = cur.fetchone()[0]
        cur.execute("SELECT COUNT(*) FROM meclaw.messages")
        msg_count = cur.fetchone()[0]
        cur.execute("SELECT COUNT(*) FROM meclaw.brain_events WHERE embedding IS NOT NULL")
        emb_count = cur.fetchone()[0]
        cur.execute("SELECT COUNT(*) FROM meclaw.brain_events WHERE extracted = true")
        ext_count = cur.fetchone()[0]
        cur.execute("SELECT COUNT(*) FROM meclaw.brain_events WHERE facts_text IS NOT NULL")
        facts_count = cur.fetchone()[0]
        cur.execute("SELECT COUNT(*) FROM meclaw.brain_events WHERE novelty > 0")
        novelty_count = cur.fetchone()[0]
        cur.execute("SELECT COUNT(*) FROM meclaw.brain_events WHERE reward != 0")
        reward_count = cur.fetchone()[0]

        print(
            f"  [stats] msgs={msg_count} brain_events={brain_count} | "
            f"signals: similarity={emb_count} recency={brain_count} "
            f"novelty={novelty_count} reward={reward_count} "
            f"personality(facts)={facts_count} graph(entities)={entity_count}"
        )
        print(
            f"           extracted={ext_count}/{brain_count} "
            f"facts_text={facts_count}/{brain_count}"
        )


def run_single_question(conn, channel_id, item, qi, total,
                        cumulative=False, ctm_enabled=False,
                        skip_extraction=False, rerank=False,
                        top_k=10, smart=False, decompose=False):
    """Process one benchmark question: feed sessions, run pipeline, retrieve."""
    qid = item['question_id']
    qtype = item['question_type']
    question = item['question']
    expected = item['answer']
    sessions = item['haystack_sessions']
    dates = item.get('haystack_dates', [])

    print(f"\n{'='*60}")
    print(f"[{qi+1}/{total}] {qid} ({qtype})")
    print(f"  Q: {question[:100]}")
    print(f"  A: {str(expected)[:100]}")
    if cumulative:
        print(f"  [mode] cumulative — brain NOT cleared before this question")

    # In reset mode (default): clear brain before each question
    if not cumulative:
        reset_brain(conn)

    # Feed all sessions for this question
    total_msgs = 0
    for si, session in enumerate(sessions):
        date = dates[si] if si < len(dates) else ""
        for msg in session:
            feed_message(conn, channel_id, msg['role'], msg['content'], date)
            total_msgs += 1
        print(f"  [feed] Session {si+1}/{len(sessions)}: {len(session)} msgs ({date})")

    # ── Wait for pg_background triggers to complete ─────────────────────────
    # extract_bee runs async via pg_background on each message insert.
    # Wait until brain_events appear (up to 10s).
    for _ in range(20):
        with conn.cursor() as cur:
            cur.execute("SELECT count(*) FROM meclaw.brain_events")
            n_events = cur.fetchone()[0]
            if n_events > 0:
                break
        time.sleep(0.5)

    check_brain_stats(conn)

    # ── Batch-embed all events at once (single API call!) ──────────────────
    print(f"  [embed] Batch embedding...")
    t0 = time.time()
    with conn.cursor() as cur:
        cur.execute("SELECT meclaw.compute_embeddings_batch(200)")
        result = cur.fetchone()
        embedded = result[0] if result else 0
    elapsed = time.time() - t0
    print(f"  [embed] {embedded} embeddings in {elapsed:.1f}s")

    # ── Reset spurious rewards from trigger-based feedback_bee ──────────────
    # feedback_bee fires on each user_input and rewards the previous event
    # based on sentiment. For benchmark conversations this is noise.
    with conn.cursor() as cur:
        cur.execute("UPDATE meclaw.brain_events SET reward = 0.0 WHERE reward != 0.0")
        reset_count = cur.rowcount
        if reset_count > 0:
            print(f"  [reward] Reset {reset_count} spurious rewards to 0")

    # ── Run full signal pipeline (LLM extraction, novelty, graph) ──────────
    run_signal_pipeline(conn, skip_extraction=skip_extraction)

    # ── Session Decomposition: split messages into atomic facts ─────────────
    if decompose:
        print("  [decompose] Splitting messages into atomic facts...")
        t0 = time.time()
        with conn.cursor() as cur:
            # Get all brain_events without decomposed facts
            cur.execute("""
                SELECT id, content, to_char(created_at, 'YYYY/MM/DD') as date_str
                FROM meclaw.brain_events
                WHERE content IS NOT NULL AND length(content) > 50
                ORDER BY seq
            """)
            events = cur.fetchall()
            total_facts = 0
            for eid, econtent, edate in events:
                try:
                    cur.execute(
                        "SELECT fact, fact_date, fact_type FROM meclaw.decompose_message(%s, %s)",
                        (econtent, edate)
                    )
                    facts = cur.fetchall()
                    for fact_text, fact_date, fact_type in facts:
                        if fact_text and len(fact_text) > 10 and fact_text != econtent:
                            # Create a new brain_event for each fact
                            parsed_date = None
                            if fact_date and fact_date != "unknown":
                                try:
                                    from datetime import datetime
                                    parsed_date = datetime.strptime(fact_date, "%Y-%m-%d")
                                except:
                                    pass
                            cur.execute("""
                                INSERT INTO meclaw.brain_events 
                                    (channel_id, content, created_at)
                                SELECT channel_id, %s, COALESCE(%s, created_at)
                                FROM meclaw.brain_events WHERE id = %s
                            """, (fact_text, parsed_date, eid))
                            total_facts += 1
                except Exception as e:
                    print(f"    [warn] decompose {eid}: {e}")
            
            elapsed = time.time() - t0
            print(f"  [decompose] {total_facts} facts from {len(events)} events in {elapsed:.1f}s")
            
            # Embed the new fact events
            if total_facts > 0:
                print(f"  [embed] Embedding {total_facts} decomposed facts...")
                cur.execute("SELECT meclaw.compute_embeddings_batch(500)")
                emb_result = cur.fetchone()
                print(f"  [embed] {emb_result[0] if emb_result else 0} new embeddings")

    # ── Final stats after pipeline ──────────────────────────────────────────
    print("  [after pipeline]")
    check_brain_stats(conn)

    # ── Retrieve context for the question ──────────────────────────────────
    ctm_label = " (CTM)" if ctm_enabled else ""
    print(f"  [retrieve{ctm_label}] {question[:80]}")
    question_date = item.get('question_date', None)
    context, sources = retrieve_context_full(conn, question, limit=top_k,
                                              ctm_enabled=ctm_enabled,
                                              rerank=rerank,
                                              question_date=question_date,
                                              smart=smart)

    result = {
        "question_id": qid,
        "question_type": qtype,
        "question": question,
        "expected_answer": expected,
        "retrieved_context": context[:2000] if context else None,
        "sessions_count": len(sessions),
        "messages_fed": total_msgs,
        "retrieve_sources": sources,
    }

    if context:
        print(f"  [retrieved] sources={sources} | {context[:150]}...")
    else:
        print(f"  [retrieved] NOTHING")

    return result


def run_benchmark(data_path, db_dsn, limit=None, output_path=None,
                  cumulative=False, ctm_enabled=False, skip_extraction=False,
                  rerank=False, top_k=10, smart=False, decompose=False):
    """Run the full benchmark."""
    with open(data_path) as f:
        data = json.load(f)

    if limit:
        data = data[:limit]

    conn = get_conn(db_dsn)
    channel_id = get_or_create_channel(conn)

    mode_str = "cumulative" if cumulative else "reset (default)"
    ctm_str = " + CTM" if ctm_enabled else ""
    ext_str = " (skip-extraction)" if skip_extraction else ""
    smart_str = " + smart" if smart else ""
    decompose_str = " + decompose" if decompose else ""
    rerank_str = f" + rerank (top-{top_k})" if rerank else f" (top-{top_k})"

    print(f"MeClaw LongMemEval Benchmark")
    print(f"Dataset: {data_path}")
    print(f"Questions: {len(data)}")
    print(f"Channel: {channel_id}")
    print(f"Agent: {BENCHMARK_AGENT_ID}")
    print(f"Mode: {mode_str}{ctm_str}{ext_str}{rerank_str}{smart_str}{decompose_str}")
    print(f"{'='*60}")

    # In cumulative mode: do one initial reset at start only
    if cumulative:
        print("[cumulative] Initial brain reset (once at start)")
        reset_brain(conn)

    results = []
    for qi, item in enumerate(data):
        result = run_single_question(
            conn, channel_id, item, qi, len(data),
            cumulative=cumulative,
            ctm_enabled=ctm_enabled,
            skip_extraction=skip_extraction,
            rerank=rerank,
            top_k=top_k,
            smart=smart,
            decompose=decompose,
        )
        results.append(result)

    # ── Summary ────────────────────────────────────────────────────────────
    print(f"\n{'='*60}")
    print(f"SUMMARY")
    print(f"{'='*60}")
    retrieved = sum(1 for r in results if r['retrieved_context'])
    print(f"Questions: {len(results)}")
    print(f"Mode: {mode_str}{ctm_str}{ext_str}")
    print(f"Retrieved context: {retrieved}/{len(results)} ({100*retrieved/len(results):.0f}%)")

    # Source distribution
    from collections import Counter
    source_counter = Counter()
    for r in results:
        for s in (r.get('retrieve_sources') or []):
            source_counter[s] += 1
    if source_counter:
        print(f"\nRetrieve sources (per question, may overlap):")
        for src, cnt in source_counter.most_common():
            print(f"  {src}: {cnt}/{len(results)}")

    from collections import Counter as _C
    by_type = _C()
    by_type_ok = _C()
    for r in results:
        by_type[r['question_type']] += 1
        if r['retrieved_context']:
            by_type_ok[r['question_type']] += 1

    print(f"\nBy type:")
    for t in sorted(by_type.keys()):
        ok = by_type_ok.get(t, 0)
        total = by_type[t]
        print(f"  {t}: {ok}/{total}")

    conn.close()

    if output_path:
        with open(output_path, 'w') as f:
            json.dump(results, f, indent=2, ensure_ascii=False)
        print(f"\nResults → {output_path}")

    return results


if __name__ == "__main__":
    p = argparse.ArgumentParser(description="MeClaw LongMemEval Benchmark")
    p.add_argument("--data", required=True)
    p.add_argument("--db", default=DB_DSN)
    p.add_argument("--limit", type=int, default=None)
    p.add_argument("--output", "-o", default=None)
    p.add_argument(
        "--cumulative",
        action="store_true",
        default=False,
        help=(
            "Cumulative mode: brain is NOT reset between questions. "
            "Sessions accumulate so Graph/Temporal/Facts/Prototypes can build "
            "across the full benchmark. Default: reset brain per question."
        ),
    )
    p.add_argument(
        "--ctm",
        action="store_true",
        default=False,
        help=(
            "CTM mode: call retrieve_bee with p_ctm_enabled=TRUE "
            "(BM25 + Contextual Trajectory Matching drift retrieval)."
        ),
    )
    p.add_argument(
        "--skip-extraction",
        action="store_true",
        default=False,
        help=(
            "Skip LLM entity extraction (no backfill_extractions call). "
            "Saves ~15k LLM calls for full benchmark run. "
            "Disables: entity graph, facts_text, graph signal. "
            "Keeps: similarity, recency, novelty (prototype-based)."
        ),
    )
    p.add_argument(
        "--rerank",
        action="store_true",
        default=False,
        help=(
            "Enable LLM re-ranking (Stage 3). Retrieves top-20 candidates, "
            "then uses gpt-4o-mini to re-rank by relevance. ~$0.35 for 500 questions."
        ),
    )
    p.add_argument(
        "--smart",
        action="store_true",
        default=False,
        help=(
            "Smart retrieval: temporal query expansion + time-filtered retrieval "
            "+ LLM re-ranking. The full pipeline. Includes --rerank implicitly."
        ),
    )
    p.add_argument(
        "--decompose",
        action="store_true",
        default=False,
        help=(
            "Session decomposition: LLM splits messages into atomic facts "
            "with individual timestamps. ~$3-5 for 500 questions."
        ),
    )
    p.add_argument(
        "--top-k",
        type=int,
        default=10,
        help="Number of results to return from retrieval (default: 10).",
    )
    args = p.parse_args()

    run_benchmark(args.data, args.db, args.limit, args.output,
                  cumulative=args.cumulative,
                  ctm_enabled=args.ctm,
                  skip_extraction=args.skip_extraction,
                  rerank=args.rerank,
                  top_k=args.top_k,
                  smart=args.smart,
                  decompose=args.decompose)
