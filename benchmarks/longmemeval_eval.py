#!/usr/bin/env python3
"""
MeClaw LongMemEval Evaluator

Scores retrieved context against expected answers using:
1. Exact/substring match
2. LLM-as-judge (two separate metrics):
   - context_quality: does the context CONTAIN the answer? (independent of generated answer)
   - answer_correct: is the generated answer correct?

Usage:
  python3 longmemeval_eval.py --results results_full_500.json \
    --api-key sk-or-... --output eval_results.json
"""

import json
import argparse
import sys
import time
import requests
from collections import Counter

# Reader: always give best guess — never refuse with "INSUFFICIENT CONTEXT"
READER_PROMPT = """Based on the following context from a conversation history, answer the question as accurately as possible. Always provide your best answer based on the available information — even if the context is incomplete or ambiguous.

Context: __CONTEXT__

Question: __QUESTION__

Answer (be concise, just the key facts; if uncertain, prefix with "Best guess:"):"""

# Context Judge: evaluates ONLY whether the retrieved context contains the answer.
# Does NOT see the generated answer — prevents contamination from reader output.
CONTEXT_JUDGE_PROMPT = """You are evaluating a memory retrieval system. Given a question, the expected answer, and the retrieved context, determine:

1. **contains_answer**: Does the retrieved context contain the information needed to answer the question correctly? (true/false)
2. **context_quality**: Rate the retrieval quality (0-3):
   - 0: Context is completely irrelevant or empty
   - 1: Context is tangentially related but doesn't contain the answer
   - 2: Context contains relevant info but not enough to fully answer
   - 3: Context clearly contains the answer or enough info to derive it
3. **brief_reason**: One sentence explaining your rating.

Respond ONLY in JSON format with keys: contains_answer (true/false), context_quality (0-3), brief_reason (string).

Question: __QUESTION__
Expected Answer: __EXPECTED__
Retrieved Context: __CONTEXT__"""

# Answer Judge: evaluates ONLY whether the generated answer is correct.
# Does NOT see the raw context — evaluates answer quality independently.
ANSWER_JUDGE_PROMPT = """You are evaluating whether a system's answer to a question is correct.

1. **answer_correct**: Is the system's answer correct or sufficiently close to the expected answer? (true/false)
2. **brief_reason**: One sentence explaining.

Respond ONLY in JSON format with keys: answer_correct (true/false), brief_reason (string).

Question: __QUESTION__
Expected Answer: __EXPECTED__
System's Answer: __ANSWER__"""


def _call_llm(prompt, api_key, model, max_tokens=200, json_format=True):
    """Shared LLM call helper."""
    payload = {
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0,
        "max_tokens": max_tokens,
    }
    if json_format:
        payload["response_format"] = {"type": "json_object"}

    resp = requests.post(
        "https://openrouter.ai/api/v1/chat/completions",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://meclaw.ai",
            "X-Title": "MeClaw",
        },
        json=payload,
        timeout=30,
    )
    if resp.status_code == 429:
        time.sleep(2)
        return _call_llm(prompt, api_key, model, max_tokens, json_format)
    resp.raise_for_status()
    return resp.json()["choices"][0]["message"]["content"].strip()


def llm_context_judge(question, expected, context, api_key, model="openai/gpt-4o-mini"):
    """Judge v3a: evaluates ONLY context quality — no generated answer contamination."""
    prompt = (
        CONTEXT_JUDGE_PROMPT
        .replace("__QUESTION__", question)
        .replace("__EXPECTED__", str(expected)[:500])
        .replace("__CONTEXT__", (context or "EMPTY")[:3000])
    )
    try:
        raw = _call_llm(prompt, api_key, model)
        return json.loads(raw)
    except Exception as e:
        return {"contains_answer": False, "context_quality": -1, "brief_reason": f"eval error: {e}"}


def llm_answer_judge(question, expected, answer, api_key, model="openai/gpt-4o-mini"):
    """Judge v3b: evaluates ONLY whether the generated answer is correct."""
    if not answer:
        return {"answer_correct": False, "brief_reason": "no answer generated"}
    prompt = (
        ANSWER_JUDGE_PROMPT
        .replace("__QUESTION__", question)
        .replace("__EXPECTED__", str(expected)[:500])
        .replace("__ANSWER__", str(answer)[:500])
    )
    try:
        raw = _call_llm(prompt, api_key, model)
        return json.loads(raw)
    except Exception as e:
        return {"answer_correct": False, "brief_reason": f"eval error: {e}"}


def llm_reader(question, context, api_key, model="openai/gpt-4o-mini"):
    """LLM reads the context and generates a best-effort answer."""
    if not context or context.strip() == "":
        return "No context retrieved."

    prompt = (
        READER_PROMPT
        .replace("__CONTEXT__", context[:4000])
        .replace("__QUESTION__", question)
    )
    try:
        return _call_llm(prompt, api_key, model, json_format=False)
    except Exception as e:
        return f"READER ERROR: {e}"


def substring_match(expected, context):
    """Simple substring check."""
    if not context:
        return False
    exp = str(expected).lower().strip()
    ctx = context.lower()

    if exp in ctx:
        return True

    words = exp.split()
    if len(words) <= 3:
        return all(w in ctx for w in words if len(w) > 2)

    return False


def evaluate(results_path, api_key=None, output_path=None, llm_eval=True, limit=None):
    """Evaluate benchmark results."""
    with open(results_path) as f:
        results = json.load(f)

    if limit:
        results = results[:limit]

    print(f"LongMemEval Evaluation — {len(results)} questions")
    print(f"LLM Judge: {'YES (context_quality + answer_correct separate)' if llm_eval and api_key else 'NO (substring only)'}")
    print("=" * 70)

    scores = []
    type_scores = {}

    for i, r in enumerate(results):
        qid = r["question_id"]
        qtype = r["question_type"]
        question = r["question"]
        expected = r["expected_answer"]
        context = r.get("retrieved_context", "")

        # Substring match
        sub_match = substring_match(expected, context)

        # LLM Reader: generate best-effort answer from context
        llm_answer = None
        if llm_eval and api_key:
            llm_answer = llm_reader(question, context, api_key)
            time.sleep(0.05)

        # Judge A: context quality (like v1, but no answer contamination)
        if llm_eval and api_key:
            ctx_judge = llm_context_judge(question, expected, context, api_key)
            time.sleep(0.05)
        else:
            ctx_judge = {
                "contains_answer": sub_match,
                "context_quality": 3 if sub_match else 0,
                "brief_reason": "substring only",
            }

        # Judge B: answer correctness (separate metric)
        if llm_eval and api_key and llm_answer:
            ans_judge = llm_answer_judge(question, expected, llm_answer, api_key)
            time.sleep(0.05)
        else:
            ans_judge = {
                "answer_correct": substring_match(expected, llm_answer) if llm_answer else False,
                "brief_reason": "substring only",
            }

        score = {
            "question_id": qid,
            "question_type": qtype,
            # Substring baseline
            "substring_match": sub_match,
            # Reader output
            "llm_answer": llm_answer,
            # Context quality (Judge A — clean, like v1)
            "context_contains_answer": ctx_judge.get("contains_answer", False),
            "context_quality": ctx_judge.get("context_quality", -1),
            "context_reason": ctx_judge.get("brief_reason", ""),
            # Answer correctness (Judge B — separate metric)
            "answer_correct": ans_judge.get("answer_correct", False),
            "answer_reason": ans_judge.get("brief_reason", ""),
        }
        scores.append(score)

        # Track by type
        if qtype not in type_scores:
            type_scores[qtype] = {
                "total": 0,
                "sub_hit": 0,
                "ctx_hit": 0,
                "ans_hit": 0,
                "quality_sum": 0,
            }
        ts = type_scores[qtype]
        ts["total"] += 1
        if sub_match:
            ts["sub_hit"] += 1
        if ctx_judge.get("contains_answer"):
            ts["ctx_hit"] += 1
        if ans_judge.get("answer_correct"):
            ts["ans_hit"] += 1
        q = ctx_judge.get("context_quality", 0)
        if q >= 0:
            ts["quality_sum"] += q

        # Progress
        ctx_ok = ctx_judge.get("contains_answer", False)
        ans_ok = ans_judge.get("answer_correct", False)
        status = "✅" if ctx_ok else ("⚠️" if ctx_judge.get("context_quality", 0) >= 2 else "❌")
        ans_icon = "✅" if ans_ok else "❌"
        if (i + 1) % 25 == 0 or i < 5:
            print(
                f"  [{i+1}/{len(results)}] ctx={status} ans={ans_icon} {qtype}: "
                f"q={ctx_judge.get('context_quality','?')} — {ctx_judge.get('brief_reason','')[:50]}"
            )

    # ── Summary ───────────────────────────────────────────────────────────────
    print(f"\n{'='*70}")
    print(f"EVALUATION RESULTS")
    print(f"{'='*70}")

    total = len(scores)
    sub_hits = sum(1 for s in scores if s["substring_match"])
    ctx_hits = sum(1 for s in scores if s["context_contains_answer"])
    ans_hits = sum(1 for s in scores if s["answer_correct"])
    quality_vals = [s["context_quality"] for s in scores if s["context_quality"] >= 0]
    avg_quality = sum(quality_vals) / max(1, len(quality_vals))

    print(f"\nOverall ({total} questions):")
    print(f"  Substring match:           {sub_hits:>4}/{total} ({100*sub_hits/total:.1f}%)")
    print(f"  Context quality (Judge A): {ctx_hits:>4}/{total} ({100*ctx_hits/total:.1f}%)  ← clean context score, like v1")
    print(f"  Answer correct  (Judge B): {ans_hits:>4}/{total} ({100*ans_hits/total:.1f}%)  ← generated answer score")
    print(f"  Avg context quality score: {avg_quality:.2f}/3.00")

    print(f"\nBy type:")
    print(f"  {'Type':<32} {'Substr':>8} {'CtxHit':>8} {'AnsHit':>8} {'AvgQ':>6}")
    print(f"  {'-'*32} {'-'*8} {'-'*8} {'-'*8} {'-'*6}")
    for qtype in sorted(type_scores.keys()):
        ts = type_scores[qtype]
        avg_q = ts["quality_sum"] / ts["total"] if ts["total"] > 0 else 0
        sub_pct = f"{ts['sub_hit']}/{ts['total']}"
        ctx_pct = f"{ts['ctx_hit']}/{ts['total']}"
        ans_pct = f"{ts['ans_hit']}/{ts['total']}"
        print(f"  {qtype:<32} {sub_pct:>8} {ctx_pct:>8} {ans_pct:>8} {avg_q:>6.2f}")

    # Quality distribution
    qual_dist = Counter(s["context_quality"] for s in scores)
    print(f"\nContext quality distribution (0=irrelevant … 3=perfect):")
    for q in sorted(qual_dist.keys()):
        bar = "█" * (qual_dist[q] // max(1, total // 50))
        label = {-1: "error", 0: "irrelevant", 1: "tangential", 2: "partial", 3: "perfect"}.get(q, str(q))
        print(f"  {q} ({label:<10}): {qual_dist[q]:>4} {bar}")

    # Save
    output = {
        "summary": {
            "total": total,
            "substring_match": sub_hits,
            "context_contains_answer": ctx_hits,
            "answer_correct": ans_hits,
            "avg_context_quality": round(avg_quality, 3),
            "by_type": type_scores,
        },
        "scores": scores,
    }

    if output_path:
        with open(output_path, "w") as f:
            json.dump(output, f, indent=2, ensure_ascii=False)
        print(f"\nResults → {output_path}")

    return output


if __name__ == "__main__":
    p = argparse.ArgumentParser(description="MeClaw LongMemEval Evaluator")
    p.add_argument("--results", required=True)
    p.add_argument("--api-key", default=None)
    p.add_argument("--output", "-o", default=None)
    p.add_argument("--no-llm", action="store_true", help="Skip LLM evaluation")
    p.add_argument("--limit", type=int, default=None)
    args = p.parse_args()

    evaluate(
        args.results,
        args.api_key,
        args.output,
        llm_eval=not args.no_llm,
        limit=args.limit,
    )
