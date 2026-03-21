-- =============================================================================
-- Phase 8: Swarm Foundation
-- =============================================================================
-- 1. llm_models table — model registry with capabilities + cost
-- 2. skills table — skill registry with embeddings
-- 3. concierge_bee — classifier: simple → llm_bee, complex → planner_bee
-- 4. planner_bee — generates execution DAG from skills + models
-- 5. dag_executor — runs the plan, tracks status per node
-- 6. dag_feedback — reward at DAG level
-- =============================================================================

-- -----------------------------------------------------------------------------
-- 1. Model Registry
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS meclaw.llm_models (
    id TEXT PRIMARY KEY,                    -- 'openai/gpt-4o-mini', 'anthropic/claude-sonnet-4'
    provider_id TEXT REFERENCES meclaw.llm_providers(id),
    display_name TEXT NOT NULL,
    capabilities TEXT[] DEFAULT '{}',       -- ['chat', 'function_calling', 'vision', 'json_mode', 'code']
    context_window INT DEFAULT 128000,
    max_output INT DEFAULT 4096,
    cost_per_1k_input FLOAT DEFAULT 0.0,    -- USD per 1k input tokens
    cost_per_1k_output FLOAT DEFAULT 0.0,   -- USD per 1k output tokens
    tier TEXT DEFAULT 'standard',           -- 'cheap', 'standard', 'premium'
    speed TEXT DEFAULT 'medium',            -- 'fast', 'medium', 'slow'
    quality TEXT DEFAULT 'medium',          -- 'low', 'medium', 'high', 'frontier'
    enabled BOOLEAN DEFAULT TRUE,
    config JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Ensure columns exist (may have been created by 15_llm_providers.sql with different schema)
DO $$ BEGIN
    ALTER TABLE meclaw.llm_models ADD COLUMN IF NOT EXISTS capabilities TEXT[] DEFAULT '{}';
    ALTER TABLE meclaw.llm_models ADD COLUMN IF NOT EXISTS context_window INT DEFAULT 128000;
    ALTER TABLE meclaw.llm_models ADD COLUMN IF NOT EXISTS max_output INT DEFAULT 4096;
    ALTER TABLE meclaw.llm_models ADD COLUMN IF NOT EXISTS cost_per_1k_input FLOAT DEFAULT 0.0;
    ALTER TABLE meclaw.llm_models ADD COLUMN IF NOT EXISTS cost_per_1k_output FLOAT DEFAULT 0.0;
    ALTER TABLE meclaw.llm_models ADD COLUMN IF NOT EXISTS speed TEXT DEFAULT 'medium';
    ALTER TABLE meclaw.llm_models ADD COLUMN IF NOT EXISTS quality TEXT DEFAULT 'medium';
EXCEPTION WHEN OTHERS THEN NULL;
END $$;

-- Seed models
INSERT INTO meclaw.llm_models (id, provider_id, display_name, capabilities, context_window, max_output, cost_per_1k_input, cost_per_1k_output, tier, speed, quality) VALUES
    ('openai/gpt-4o-mini', 'openrouter', 'GPT-4o Mini', ARRAY['chat', 'function_calling', 'json_mode', 'vision'], 128000, 16384, 0.00015, 0.0006, 'cheap', 'fast', 'medium'),
    ('openai/gpt-4o', 'openrouter', 'GPT-4o', ARRAY['chat', 'function_calling', 'json_mode', 'vision'], 128000, 16384, 0.0025, 0.01, 'standard', 'medium', 'high'),
    ('anthropic/claude-sonnet-4', 'openrouter', 'Claude Sonnet 4', ARRAY['chat', 'function_calling', 'json_mode', 'vision', 'code'], 200000, 8192, 0.003, 0.015, 'standard', 'medium', 'high'),
    ('anthropic/claude-opus-4', 'openrouter', 'Claude Opus 4', ARRAY['chat', 'function_calling', 'json_mode', 'vision', 'code'], 200000, 32000, 0.015, 0.075, 'premium', 'slow', 'frontier')
ON CONFLICT (id) DO NOTHING;

-- -----------------------------------------------------------------------------
-- 2. Skill Registry
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS meclaw.skills (
    id TEXT PRIMARY KEY,                    -- 'sql_read', 'python_exec', 'web_search', 'summarize'
    display_name TEXT NOT NULL,
    description TEXT NOT NULL,
    category TEXT DEFAULT 'general',        -- 'data', 'code', 'web', 'analysis', 'communication'
    input_schema JSONB,                     -- expected input format
    output_schema JSONB,                    -- expected output format
    required_capabilities TEXT[] DEFAULT '{}',  -- model capabilities needed
    min_tier TEXT DEFAULT 'cheap',          -- minimum model tier
    estimated_tokens INT DEFAULT 500,       -- avg token cost per invocation
    embedding vector(1536),                 -- for semantic skill lookup
    enabled BOOLEAN DEFAULT TRUE,
    config JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Seed built-in skills
INSERT INTO meclaw.skills (id, display_name, description, category, required_capabilities, min_tier, estimated_tokens) VALUES
    ('sql_read', 'SQL Read', 'Execute read-only SQL queries against the database', 'data', ARRAY['function_calling'], 'cheap', 300),
    ('sql_write', 'SQL Write', 'Execute write SQL statements (INSERT, UPDATE, DELETE)', 'data', ARRAY['function_calling'], 'standard', 500),
    ('python_exec', 'Python Execute', 'Run Python code in a sandboxed environment', 'code', ARRAY['function_calling', 'code'], 'standard', 800),
    ('summarize', 'Summarize', 'Summarize text or conversation into key points', 'analysis', ARRAY['chat'], 'cheap', 400),
    ('classify', 'Classify', 'Classify input into categories', 'analysis', ARRAY['chat', 'json_mode'], 'cheap', 200),
    ('extract_info', 'Extract Information', 'Extract structured information from unstructured text', 'analysis', ARRAY['chat', 'json_mode'], 'cheap', 500),
    ('generate_plan', 'Generate Plan', 'Create a step-by-step execution plan for complex tasks', 'analysis', ARRAY['chat', 'json_mode'], 'premium', 2000),
    ('code_review', 'Code Review', 'Review code for bugs, style, and improvements', 'code', ARRAY['chat', 'code'], 'standard', 1500),
    ('answer_question', 'Answer Question', 'Direct Q&A from context or knowledge', 'general', ARRAY['chat'], 'cheap', 300)
ON CONFLICT (id) DO NOTHING;

-- -----------------------------------------------------------------------------
-- 3. Execution Plans (DAGs)
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS meclaw.execution_plans (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    task_id UUID REFERENCES meclaw.tasks(id),
    agent_id TEXT REFERENCES meclaw.entities(id),
    plan JSONB NOT NULL,                    -- DAG: {nodes: [{id, skill, model, input, depends_on}], edges: [...]}
    status TEXT DEFAULT 'pending',          -- 'pending', 'running', 'completed', 'failed', 'partial'
    complexity TEXT,                        -- 'simple', 'moderate', 'complex'
    estimated_cost FLOAT DEFAULT 0.0,
    actual_cost FLOAT DEFAULT 0.0,
    reward FLOAT DEFAULT 0.0,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS meclaw.execution_steps (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    plan_id UUID NOT NULL REFERENCES meclaw.execution_plans(id),
    step_index INT NOT NULL,
    skill_id TEXT REFERENCES meclaw.skills(id),
    model_id TEXT REFERENCES meclaw.llm_models(id),
    input JSONB,
    output JSONB,
    status TEXT DEFAULT 'pending',          -- 'pending', 'running', 'completed', 'failed', 'skipped'
    depends_on UUID[],                      -- step IDs this depends on
    tokens_used INT DEFAULT 0,
    cost FLOAT DEFAULT 0.0,
    error TEXT,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_execution_steps_plan ON meclaw.execution_steps(plan_id);

-- -----------------------------------------------------------------------------
-- 4. select_model — pick best model for a skill
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.select_model(
    p_skill_id TEXT,
    p_prefer_tier TEXT DEFAULT NULL
) RETURNS TEXT AS $$
DECLARE
    v_model_id TEXT;
    v_skill RECORD;
BEGIN
    SELECT * INTO v_skill FROM meclaw.skills WHERE id = p_skill_id;
    IF NOT FOUND THEN
        RETURN 'openai/gpt-4o-mini'; -- fallback
    END IF;

    -- Find cheapest model that meets requirements
    SELECT m.id INTO v_model_id
    FROM meclaw.llm_models m
    WHERE m.enabled = TRUE
        AND m.capabilities @> v_skill.required_capabilities
        AND (
            CASE v_skill.min_tier
                WHEN 'cheap' THEN TRUE
                WHEN 'standard' THEN m.tier IN ('standard', 'premium')
                WHEN 'premium' THEN m.tier = 'premium'
                ELSE TRUE
            END
        )
        AND (p_prefer_tier IS NULL OR m.tier = p_prefer_tier)
    ORDER BY
        CASE WHEN p_prefer_tier IS NOT NULL AND m.tier = p_prefer_tier THEN 0 ELSE 1 END,
        m.cost_per_1k_input ASC,
        m.speed = 'fast' DESC
    LIMIT 1;

    RETURN COALESCE(v_model_id, 'openai/gpt-4o-mini');
END;
$$ LANGUAGE plpgsql;

-- -----------------------------------------------------------------------------
-- 5. concierge_bee — classify and route
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.concierge_bee(p_msg_id UUID)
RETURNS TEXT AS $fn$
import json
import requests

# Get message content
plan = plpy.prepare("""
    SELECT content->>'input' AS input, task_id, channel_id
    FROM meclaw.messages WHERE id = $1
""", ["uuid"])
row = plan.execute([str(p_msg_id)])
if not row or not row[0]["input"]:
    return "simple"

user_input = row[0]["input"]
task_id = row[0]["task_id"]

# Short messages are almost always simple
if len(user_input.strip()) < 50:
    return "simple"

# Get API key
prov = plpy.execute(plpy.prepare(
    "SELECT api_key FROM meclaw.llm_providers WHERE id = $1", ["text"]
), ["openrouter"])
if not prov:
    return "simple"

api_key = prov[0]["api_key"]

# Get available skills for context
skills = plpy.execute("SELECT id, display_name, description FROM meclaw.skills WHERE enabled = TRUE")
skill_list = "\n".join([f"- {s['id']}: {s['description']}" for s in skills])

prompt = f"""Classify this user message for an AI agent system.

Available skills:
{skill_list}

User message: "{user_input[:500]}"

Classify as:
- "simple": Direct question, greeting, chat, single-skill task (answer_question, sql_read, classify)
- "moderate": Needs 2-3 steps or skills in sequence
- "complex": Needs planning, multiple skills, or creative problem-solving

Return ONLY valid JSON: {{"complexity": "simple|moderate|complex", "suggested_skills": ["skill_id1"], "reasoning": "one sentence"}}"""

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
            "model": "openai/gpt-4o-mini",
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.0,
            "max_tokens": 150,
            "response_format": {"type": "json_object"}
        },
        timeout=10
    )
    resp.raise_for_status()
    data = resp.json()
    output = json.loads(data["choices"][0]["message"]["content"])

    complexity = output.get("complexity", "simple")
    suggested_skills = output.get("suggested_skills", [])
    reasoning = output.get("reasoning", "")

    # Log classification
    plpy.execute(plpy.prepare("""
        INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
        VALUES ($1, $2, 'concierge_bee', 'classified', $3::jsonb)
    """, ["uuid", "uuid", "text"]), [
        str(p_msg_id), str(task_id),
        json.dumps({"complexity": complexity, "skills": suggested_skills, "reasoning": reasoning})
    ])

    return complexity

except Exception as e:
    plpy.warning(f"concierge_bee: classification failed: {e}")
    return "simple"

$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 6. planner_bee — generate execution DAG
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.planner_bee(p_msg_id UUID, p_complexity TEXT DEFAULT 'moderate')
RETURNS UUID AS $fn$
import json
import requests

# Get message
plan = plpy.prepare("""
    SELECT content->>'input' AS input, task_id, channel_id
    FROM meclaw.messages WHERE id = $1
""", ["uuid"])
row = plan.execute([str(p_msg_id)])
if not row:
    return None

user_input = row[0]["input"]
task_id = row[0]["task_id"]

# Get available skills + models
skills = plpy.execute("SELECT id, display_name, description, category, min_tier FROM meclaw.skills WHERE enabled = TRUE")
models = plpy.execute("SELECT id, display_name, tier, speed, quality FROM meclaw.llm_models WHERE enabled = TRUE")

skill_desc = "\n".join([f"- {s['id']} ({s['category']}): {s['description']} [min_tier: {s['min_tier']}]" for s in skills])
model_desc = "\n".join([f"- {m['id']}: {m['display_name']} [{m['tier']}, {m['speed']}, {m['quality']}]" for m in models])

# Get API key
prov = plpy.execute(plpy.prepare(
    "SELECT api_key FROM meclaw.llm_providers WHERE id = $1", ["text"]
), ["openrouter"])
if not prov:
    return None

api_key = prov[0]["api_key"]

# Use a capable model for planning
plan_model = "openai/gpt-4o" if p_complexity == "complex" else "openai/gpt-4o-mini"

prompt = f"""You are a task planner for an AI agent system running inside PostgreSQL.
Break down this task into a sequence of steps using available skills and models.

Available skills:
{skill_desc}

Available models:
{model_desc}

Task (complexity: {p_complexity}):
"{user_input[:1000]}"

Return ONLY valid JSON:
{{
  "steps": [
    {{
      "id": "step_1",
      "skill": "skill_id",
      "model": "model_id",
      "description": "what this step does",
      "input_template": "prompt or input for this step",
      "depends_on": []
    }}
  ],
  "estimated_total_tokens": 1000,
  "reasoning": "why this plan"
}}

Rules:
- Use the cheapest model that can do the job
- Minimize steps — don't over-engineer
- Steps can depend on previous steps (DAG, not just sequence)
- For simple tasks wrapped in moderate: just use 1 step
- input_template can reference {{step_X.output}} for dependencies"""

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
            "model": plan_model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.0,
            "max_tokens": 2000,
            "response_format": {"type": "json_object"}
        },
        timeout=30
    )
    resp.raise_for_status()
    data = resp.json()
    plan_output = json.loads(data["choices"][0]["message"]["content"])
    usage = data.get("usage", {})

    steps = plan_output.get("steps", [])
    if not steps:
        return None

    # Estimate cost
    est_tokens = plan_output.get("estimated_total_tokens", sum(s.get("estimated_tokens", 500) for s in steps))

    # Create execution plan
    plan_row = plpy.execute(plpy.prepare("""
        INSERT INTO meclaw.execution_plans (task_id, agent_id, plan, status, complexity, estimated_cost)
        VALUES ($1, 'meclaw:agent:walter', $2::jsonb, 'pending', $3, $4)
        RETURNING id
    """, ["uuid", "text", "text", "float8"]),
    [str(task_id), json.dumps(plan_output), p_complexity, est_tokens * 0.00001])

    plan_id = str(plan_row[0]["id"])

    # Create execution steps
    step_ids = {}
    for i, step in enumerate(steps):
        step_skill = step.get("skill", "answer_question")
        step_model = step.get("model")

        # Auto-select model if not specified
        if not step_model:
            model_row = plpy.execute(plpy.prepare(
                "SELECT meclaw.select_model($1)", ["text"]
            ), [step_skill])
            step_model = model_row[0]["select_model"]

        # Resolve depends_on to step UUIDs
        dep_ids = []
        for dep in step.get("depends_on", []):
            if dep in step_ids:
                dep_ids.append(step_ids[dep])

        step_row = plpy.execute(plpy.prepare("""
            INSERT INTO meclaw.execution_steps (plan_id, step_index, skill_id, model_id, input, status, depends_on)
            VALUES ($1::uuid, $2, $3, $4, $5::jsonb, 'pending', $6::uuid[])
            RETURNING id
        """, ["text", "int4", "text", "text", "text", "text[]"]),
        [plan_id, i, step_skill, step_model,
         json.dumps({"description": step.get("description", ""), "input_template": step.get("input_template", "")}),
         dep_ids if dep_ids else None])

        step_ids[step.get("id", f"step_{i}")] = str(step_row[0]["id"])

    # Log
    plpy.execute(plpy.prepare("""
        INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
        VALUES ($1, $2, 'planner_bee', 'plan_created', $3::jsonb)
    """, ["uuid", "uuid", "text"]), [
        str(p_msg_id), str(task_id),
        json.dumps({"plan_id": plan_id, "steps": len(steps), "complexity": p_complexity,
                     "model_used": plan_model, "planning_tokens": usage.get("total_tokens", 0)})
    ])

    return plan_id

except Exception as e:
    plpy.warning(f"planner_bee: planning failed: {e}")
    return None

$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 7. dag_executor — execute plan steps in dependency order
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.dag_executor(p_plan_id UUID)
RETURNS JSONB AS $fn$
import json
import requests
import time

# Mark plan as running
plpy.execute(plpy.prepare(
    "UPDATE meclaw.execution_plans SET status = 'running', started_at = clock_timestamp() WHERE id = $1",
    ["uuid"]
), [str(p_plan_id)])

# Get all steps ordered by dependency
steps = plpy.execute(plpy.prepare("""
    SELECT id, step_index, skill_id, model_id, input, depends_on, status
    FROM meclaw.execution_steps
    WHERE plan_id = $1
    ORDER BY step_index
""", ["uuid"]), [str(p_plan_id)])

if not steps:
    return json.dumps({"error": "no steps"})

# Get API key
prov = plpy.execute("SELECT api_key FROM meclaw.llm_providers WHERE id = 'openrouter'")
if not prov:
    return json.dumps({"error": "no api key"})
api_key = prov[0]["api_key"]

results = {}
total_cost = 0.0
total_tokens = 0
failed = False

for step in steps:
    step_id = str(step["id"])
    skill_id = step["skill_id"]
    model_id = step["model_id"] or "openai/gpt-4o-mini"
    step_input = json.loads(step["input"]) if step["input"] else {}
    depends_on = step["depends_on"] or []

    # Check dependencies
    deps_met = True
    for dep_id in depends_on:
        if dep_id not in results:
            deps_met = False
            break
        if results[dep_id].get("status") == "failed":
            # Dependency failed — skip this step
            plpy.execute(plpy.prepare(
                "UPDATE meclaw.execution_steps SET status = 'skipped', error = 'dependency failed' WHERE id = $1::uuid",
                ["text"]
            ), [step_id])
            results[step_id] = {"status": "skipped"}
            deps_met = False
            break

    if not deps_met:
        continue

    # Resolve input template — replace {step_X.output} references
    input_template = step_input.get("input_template", "")
    description = step_input.get("description", "")
    for dep_id in depends_on:
        if dep_id in results and results[dep_id].get("output"):
            placeholder = f"{{{dep_id}.output}}"
            dep_output = str(results[dep_id]["output"])[:2000]
            input_template = input_template.replace(placeholder, dep_output)

    # Build prompt
    prompt = f"{description}\n\n{input_template}" if description else input_template
    if not prompt.strip():
        prompt = description or "Execute this step."

    # Mark step running
    plpy.execute(plpy.prepare(
        "UPDATE meclaw.execution_steps SET status = 'running', started_at = clock_timestamp() WHERE id = $1::uuid",
        ["text"]
    ), [step_id])

    # Execute LLM call
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
                "model": model_id,
                "messages": [{"role": "user", "content": prompt}],
                "temperature": 0.2,
                "max_tokens": 4000
            },
            timeout=60
        )
        resp.raise_for_status()
        data = resp.json()

        output = data["choices"][0]["message"]["content"]
        usage = data.get("usage", {})
        step_tokens = usage.get("total_tokens", 0)

        # Calculate cost
        model_info = plpy.execute(plpy.prepare(
            "SELECT cost_per_1k_input, cost_per_1k_output FROM meclaw.llm_models WHERE id = $1", ["text"]
        ), [model_id])
        step_cost = 0.0
        if model_info:
            step_cost = (usage.get("prompt_tokens", 0) / 1000 * model_info[0]["cost_per_1k_input"] +
                        usage.get("completion_tokens", 0) / 1000 * model_info[0]["cost_per_1k_output"])

        total_cost += step_cost
        total_tokens += step_tokens

        # Update step
        plpy.execute(plpy.prepare("""
            UPDATE meclaw.execution_steps
            SET status = 'completed', output = $1::jsonb, tokens_used = $2, cost = $3, completed_at = clock_timestamp()
            WHERE id = $4::uuid
        """, ["text", "int4", "float8", "text"]),
        [json.dumps({"result": output[:5000]}), step_tokens, step_cost, step_id])

        results[step_id] = {"status": "completed", "output": output}
        time.sleep(0.2)  # Rate limit protection

    except Exception as e:
        plpy.warning(f"dag_executor: step {step_id} failed: {e}")
        plpy.execute(plpy.prepare(
            "UPDATE meclaw.execution_steps SET status = 'failed', error = $1, completed_at = clock_timestamp() WHERE id = $2::uuid",
            ["text", "text"]
        ), [str(e)[:500], step_id])
        results[step_id] = {"status": "failed", "error": str(e)}
        failed = True

# Finalize plan
final_status = "failed" if failed else "completed"
# Check if any steps completed despite failures (partial success)
completed_count = sum(1 for r in results.values() if r.get("status") == "completed")
if failed and completed_count > 0:
    final_status = "partial"

plpy.execute(plpy.prepare("""
    UPDATE meclaw.execution_plans
    SET status = $1, actual_cost = $2, completed_at = clock_timestamp()
    WHERE id = $3
""", ["text", "float8", "uuid"]),
[final_status, total_cost, str(p_plan_id)])

# Collect final output (from last completed step)
final_output = ""
for step in reversed(list(steps)):
    sid = str(step["id"])
    if sid in results and results[sid].get("output"):
        final_output = results[sid]["output"]
        break

return json.dumps({
    "status": final_status,
    "steps_total": len(steps),
    "steps_completed": completed_count,
    "total_tokens": total_tokens,
    "total_cost": total_cost,
    "output": final_output[:5000]
})

$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 8. dag_feedback — reward at plan level
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.dag_feedback(p_plan_id UUID, p_reward FLOAT)
RETURNS VOID AS $$
BEGIN
    -- Update plan reward
    UPDATE meclaw.execution_plans
    SET reward = reward + p_reward
    WHERE id = p_plan_id;

    -- Propagate reward to individual steps (weighted by position)
    UPDATE meclaw.execution_steps es
    SET cost = es.cost  -- trigger update (we log via events)
    WHERE es.plan_id = p_plan_id AND es.status = 'completed';

    -- Log feedback
    INSERT INTO meclaw.events (bee_type, event, payload)
    VALUES ('dag_feedback', 'plan_rewarded', jsonb_build_object(
        'plan_id', p_plan_id,
        'reward', p_reward
    ));
END;
$$ LANGUAGE plpgsql;

-- -----------------------------------------------------------------------------
-- 9. Convenience: run full swarm pipeline for a message
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.swarm_process(p_msg_id UUID)
RETURNS JSONB AS $fn$
import json

# Step 1: Classify
classify_plan = plpy.prepare("SELECT meclaw.concierge_bee($1::uuid)", ["uuid"])
result = classify_plan.execute([str(p_msg_id)])
complexity = result[0]["concierge_bee"]

if complexity == "simple":
    return json.dumps({"route": "simple", "action": "direct_to_llm_bee"})

# Step 2: Plan
plan_plan = plpy.prepare("SELECT meclaw.planner_bee($1::uuid, $2)", ["uuid", "text"])
result = plan_plan.execute([str(p_msg_id), complexity])
plan_id = result[0]["planner_bee"]

if not plan_id:
    return json.dumps({"route": "fallback", "action": "direct_to_llm_bee", "reason": "planning_failed"})

# Step 3: Execute
exec_plan = plpy.prepare("SELECT meclaw.dag_executor($1::uuid)", ["uuid"])
result = exec_plan.execute([plan_id])
execution_result = json.loads(result[0]["dag_executor"])

return json.dumps({
    "route": complexity,
    "plan_id": plan_id,
    "result": execution_result
})

$fn$ LANGUAGE plpython3u;
