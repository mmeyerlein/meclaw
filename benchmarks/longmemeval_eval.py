#!/usr/bin/env python3
"""
MeClaw LongMemEval Evaluator

Scores retrieved context against expected answers using:
1. Exact/substring match
2. LLM-as-judge (does the context contain enough info to answer?)

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

# Eval prompt for LLM judge
READER_PROMPT = """Based on the following context from a conversation history, answer the question concisely. If the context doesn't contain enough information to answer, say "INSUFFICIENT CONTEXT".

Context: __CONTEXT__

Question: __QUESTION__

Answer (be concise, just the key facts):"""

JUDGE_PROMPT = """You are evaluating a memory retrieval system. Given a question, the expected answer, and the retrieved context, determine:

1. **contains_answer**: Does the retrieved context contain the information needed to answer the question correctly? (true/false)
2. **answer_quality**: Rate the retrieval quality (0-3):
   - 0: Context is completely irrelevant
   - 1: Context is tangentially related but doesn't contain the answer
   - 2: Context contains relevant info but not enough to fully answer
   - 3: Context clearly contains the answer or enough info to derive it
3. **brief_reason**: One sentence explaining your rating.

Respond ONLY in JSON format with keys: contains_answer (true/false), answer_quality (0-3), brief_reason (string).

Question: __QUESTION__
Expected Answer: __EXPECTED__
Retrieved Context: __CONTEXT__
System's Answer: __ANSWER__"""


def llm_judge(question, expected, context, api_key, model="openai/gpt-4o-mini", answer=None):
    """Use LLM to judge if context contains the answer."""
    prompt = JUDGE_PROMPT.replace("__QUESTION__", question).replace(
        "__EXPECTED__", str(expected)[:500]).replace(
        "__CONTEXT__", (context or "EMPTY")[:3000]).replace(
        "__ANSWER__", (answer or "NO ANSWER")[:500])
    
    try:
        resp = requests.post(
            "https://openrouter.ai/api/v1/chat/completions",
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
                "HTTP-Referer": "https://meclaw.ai",
                "X-Title": "MeClaw"
            },
            json={
                "model": model,
                "messages": [{"role": "user", "content": prompt}],
                "temperature": 0,
                "max_tokens": 200,
                "response_format": {"type": "json_object"}
            },
            timeout=30
        )
        
        if resp.status_code == 429:
            time.sleep(2)
            return llm_judge(question, expected, context, api_key, model)
        
        resp.raise_for_status()
        content = resp.json()["choices"][0]["message"]["content"]
        return json.loads(content)
    except Exception as e:
        return {"contains_answer": False, "answer_quality": -1, "brief_reason": f"eval error: {e}"}


def llm_reader(question, context, api_key, model="openai/gpt-4o-mini"):
    """LLM reads the context and generates an answer."""
    if not context or context.strip() == "":
        return "INSUFFICIENT CONTEXT"
    
    prompt = READER_PROMPT.replace("__CONTEXT__", context[:4000]).replace("__QUESTION__", question)
    
    try:
        resp = requests.post(
            "https://openrouter.ai/api/v1/chat/completions",
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
                "HTTP-Referer": "https://meclaw.ai",
                "X-Title": "MeClaw"
            },
            json={
                "model": model,
                "messages": [{"role": "user", "content": prompt}],
                "temperature": 0,
                "max_tokens": 200
            },
            timeout=30
        )
        if resp.status_code == 429:
            time.sleep(2)
            return llm_reader(question, context, api_key, model)
        resp.raise_for_status()
        return resp.json()["choices"][0]["message"]["content"].strip()
    except Exception as e:
        return f"READER ERROR: {e}"


def substring_match(expected, context):
    """Simple substring check."""
    if not context:
        return False
    exp = str(expected).lower().strip()
    ctx = context.lower()
    
    # Direct match
    if exp in ctx:
        return True
    
    # Try individual words for short answers
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
    print(f"LLM Judge: {'YES' if llm_eval and api_key else 'NO (substring only)'}")
    print("=" * 60)
    
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
        
        # LLM Reader: generate answer from context
        llm_answer = None
        if llm_eval and api_key:
            llm_answer = llm_reader(question, context, api_key)
            time.sleep(0.05)
        
        # LLM judge (now also evaluates the generated answer)
        if llm_eval and api_key:
            judge = llm_judge(question, expected, context, api_key, answer=llm_answer)
            time.sleep(0.05)
        else:
            judge = {"contains_answer": sub_match, "answer_quality": 3 if sub_match else 0, "brief_reason": "substring only"}
        
        # Also check if LLM answer matches expected
        answer_match = substring_match(expected, llm_answer) if llm_answer else False
        
        score = {
            "question_id": qid,
            "question_type": qtype,
            "substring_match": sub_match,
            "llm_answer": llm_answer,
            "answer_match": answer_match,
            "llm_contains_answer": judge.get("contains_answer", False),
            "llm_quality": judge.get("answer_quality", -1),
            "llm_reason": judge.get("brief_reason", ""),
        }
        scores.append(score)
        
        # Track by type
        if qtype not in type_scores:
            type_scores[qtype] = {"total": 0, "sub_hit": 0, "llm_hit": 0, "answer_hit": 0, "quality_sum": 0}
        type_scores[qtype]["total"] += 1
        if sub_match:
            type_scores[qtype]["sub_hit"] += 1
        if answer_match:
            type_scores[qtype]["answer_hit"] += 1
        if judge.get("contains_answer"):
            type_scores[qtype]["llm_hit"] += 1
        q = judge.get("answer_quality", 0)
        if q >= 0:
            type_scores[qtype]["quality_sum"] += q
        
        # Progress
        status = "✅" if judge.get("contains_answer") else ("⚠️" if judge.get("answer_quality", 0) >= 2 else "❌")
        if (i + 1) % 25 == 0 or i < 5:
            print(f"  [{i+1}/{len(results)}] {status} {qtype}: q={judge.get('answer_quality', '?')} — {judge.get('brief_reason', '')[:60]}")
    
    # Summary
    print(f"\n{'='*60}")
    print(f"EVALUATION RESULTS")
    print(f"{'='*60}")
    
    total = len(scores)
    sub_hits = sum(1 for s in scores if s["substring_match"])
    llm_hits = sum(1 for s in scores if s["llm_contains_answer"])
    answer_hits = sum(1 for s in scores if s.get("answer_match"))
    avg_quality = sum(s["llm_quality"] for s in scores if s["llm_quality"] >= 0) / max(1, sum(1 for s in scores if s["llm_quality"] >= 0))
    
    print(f"\nOverall ({total} questions):")
    print(f"  Substring match:     {sub_hits}/{total} ({100*sub_hits/total:.1f}%)")
    print(f"  LLM Reader correct:  {answer_hits}/{total} ({100*answer_hits/total:.1f}%)")
    print(f"  LLM Judge correct:   {llm_hits}/{total} ({100*llm_hits/total:.1f}%)")
    print(f"  Avg quality score:   {avg_quality:.2f}/3.00")
    
    print(f"\nBy type:")
    print(f"  {'Type':<30} {'Substr':>8} {'Reader':>8} {'Judge':>8} {'AvgQ':>8}")
    print(f"  {'-'*30} {'-'*8} {'-'*8} {'-'*8} {'-'*8}")
    for qtype in sorted(type_scores.keys()):
        ts = type_scores[qtype]
        avg_q = ts["quality_sum"] / ts["total"] if ts["total"] > 0 else 0
        print(f"  {qtype:<30} {ts['sub_hit']:>3}/{ts['total']:<3}  {ts['answer_hit']:>3}/{ts['total']:<3}  {ts['llm_hit']:>3}/{ts['total']:<3}  {avg_q:>6.2f}")
    
    # Quality distribution
    qual_dist = Counter(s["llm_quality"] for s in scores)
    print(f"\nQuality distribution:")
    for q in sorted(qual_dist.keys()):
        bar = "█" * (qual_dist[q] // 5)
        print(f"  {q}: {qual_dist[q]:>4} {bar}")
    
    # Save
    output = {
        "summary": {
            "total": total,
            "substring_match": sub_hits,
            "llm_contains_answer": llm_hits,
            "avg_quality": round(avg_quality, 3),
            "by_type": type_scores,
        },
        "scores": scores,
    }
    
    if output_path:
        with open(output_path, 'w') as f:
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
    
    evaluate(args.results, args.api_key, args.output, 
             llm_eval=not args.no_llm, limit=args.limit)
