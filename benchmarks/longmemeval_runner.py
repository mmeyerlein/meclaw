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
    print("  [reset] Brain cleared")


def get_or_create_channel(conn):
    """Ensure benchmark channel exists, return its ID."""
    with conn.cursor() as cur:
        cur.execute("""
            SELECT id FROM meclaw.channels WHERE name = 'benchmark'
        """)
        row = cur.fetchone()
        if row:
            return row[0]
        
        ch_id = str(uuid.uuid4())
        cur.execute("""
            INSERT INTO meclaw.channels (id, name, type, config)
            VALUES (%s, 'benchmark', 'web', '{}')
        """, (ch_id,))
        return ch_id


def feed_message(conn, channel_id, role, content, session_date=""):
    """Insert a single message into MeClaw and trigger extract_bee."""
    msg_id = str(uuid.uuid4())
    task_id = str(uuid.uuid4())
    
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
        
        cur.execute("""
            INSERT INTO meclaw.messages (id, task_id, channel_id, type, sender, 
                                         status, content, created_at)
            VALUES (%s, %s, %s, %s, %s, 'done', %s, NOW())
        """, (msg_id, task_id, channel_id, msg_type, sender, content_json))
        
        # Trigger extract_bee on user messages (brain processing)
        if role == "user":
            try:
                cur.execute("SELECT meclaw.extract_bee(%s)", (msg_id,))
            except Exception as e:
                print(f"    [warn] extract_bee: {e}")
    
    return msg_id


def retrieve_context(conn, question, limit=10):
    """Use retrieve_bee to get relevant brain context for a question."""
    with conn.cursor() as cur:
        try:
            cur.execute("SELECT meclaw.retrieve_bee('meclaw:agent:system', %s, %s)", 
                       (question, limit))
            result = cur.fetchone()
            if result and result[0]:
                return result[0]
        except Exception as e:
            print(f"    [warn] retrieve_bee: {e}")
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
                return "\n".join(parts)
        except Exception as e:
            print(f"    [warn] fallback query: {e}")
            conn.rollback()
            conn.autocommit = True
    
    return None


def check_brain_stats(conn):
    """Print current brain statistics."""
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
        
        print(f"  [stats] msgs={msg_count} brain_events={brain_count} "
              f"(embedded={emb_count}, extracted={ext_count}) entities={entity_count}")


def run_single_question(conn, channel_id, item, qi, total):
    """Process one benchmark question: feed sessions, ask, return result."""
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
    
    # Reset brain for this question
    reset_brain(conn)
    
    # Feed all sessions
    total_msgs = 0
    for si, session in enumerate(sessions):
        date = dates[si] if si < len(dates) else ""
        for msg in session:
            feed_message(conn, channel_id, msg['role'], msg['content'], date)
            total_msgs += 1
        print(f"  [feed] Session {si+1}/{len(sessions)}: {len(session)} msgs ({date})")
    
    check_brain_stats(conn)
    
    # Batch-embed all events at once (single API call!)
    print(f"  [embed] Batch embedding...")
    t0 = time.time()
    with conn.cursor() as cur:
        cur.execute("SELECT meclaw.compute_embeddings_batch(200)")
        result = cur.fetchone()
        embedded = result[0] if result else 0
    elapsed = time.time() - t0
    print(f"  [embed] {embedded} embeddings in {elapsed:.1f}s")
    
    # Retrieve context for the question
    context = retrieve_context(conn, question)
    
    result = {
        "question_id": qid,
        "question_type": qtype,
        "question": question,
        "expected_answer": expected,
        "retrieved_context": context[:2000] if context else None,
        "sessions_count": len(sessions),
        "messages_fed": total_msgs,
    }
    
    if context:
        print(f"  [retrieved] {context[:150]}...")
    else:
        print(f"  [retrieved] NOTHING")
    
    return result


def run_benchmark(data_path, db_dsn, limit=None, output_path=None):
    """Run the full benchmark."""
    with open(data_path) as f:
        data = json.load(f)
    
    if limit:
        data = data[:limit]
    
    conn = get_conn(db_dsn)
    channel_id = get_or_create_channel(conn)
    
    print(f"MeClaw LongMemEval Benchmark")
    print(f"Dataset: {data_path}")
    print(f"Questions: {len(data)}")
    print(f"Channel: {channel_id}")
    print(f"{'='*60}")
    
    results = []
    for qi, item in enumerate(data):
        result = run_single_question(conn, channel_id, item, qi, len(data))
        results.append(result)
    
    # Summary
    print(f"\n{'='*60}")
    print(f"SUMMARY")
    print(f"{'='*60}")
    retrieved = sum(1 for r in results if r['retrieved_context'])
    print(f"Questions: {len(results)}")
    print(f"Retrieved context: {retrieved}/{len(results)} ({100*retrieved/len(results):.0f}%)")
    
    from collections import Counter
    by_type = Counter()
    by_type_ok = Counter()
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
    args = p.parse_args()
    
    run_benchmark(args.data, args.db, args.limit, args.output)
