-- MeClaw v0.1.0 — Admin Bee (persistent HTTP Server)
-- Startet via: SELECT pg_background_launch('SELECT meclaw.admin_bee(8080)');

-- Helper: Graph-Daten als JSON aus AGE exportieren
CREATE OR REPLACE FUNCTION meclaw.export_graph_data()
RETURNS jsonb AS $fn$
DECLARE
    v_nodes jsonb := '[]'::jsonb;
    v_edges jsonb := '[]'::jsonb;
    v_rec record;
    v_cypher text;
BEGIN
    LOAD 'age';
    SET LOCAL search_path = meclaw, ag_catalog, "$user", public;

    -- Hives
    v_cypher := 'SELECT * FROM cypher(''meclaw_graph'', $cyp$ MATCH (h:Hive) RETURN h.id, h.name, h.type $cyp$) AS (id agtype, name agtype, type agtype)';
    FOR v_rec IN EXECUTE v_cypher
    LOOP
        v_nodes := v_nodes || jsonb_build_object(
            'id', trim(both '"' from v_rec.id::text),
            'label', trim(both '"' from v_rec.name::text),
            'group', 'hive',
            'shape', 'diamond'
        );
    END LOOP;

    -- Bees
    v_cypher := 'SELECT * FROM cypher(''meclaw_graph'', $cyp$ MATCH (b:Bee) RETURN b.id, b.type, b.hive $cyp$) AS (id agtype, type agtype, hive agtype)';
    FOR v_rec IN EXECUTE v_cypher
    LOOP
        v_nodes := v_nodes || jsonb_build_object(
            'id', trim(both '"' from v_rec.id::text),
            'label', trim(both '"' from v_rec.id::text),
            'group', trim(both '"' from v_rec.type::text),
            'title', trim(both '"' from v_rec.type::text) || ' (' || trim(both '"' from v_rec.hive::text) || ')'
        );
    END LOOP;

    -- NEXT Edges
    v_cypher := 'SELECT * FROM cypher(''meclaw_graph'', $cyp$ MATCH (a)-[r:NEXT]->(b) RETURN a.id, b.id, r.condition $cyp$) AS (from_id agtype, to_id agtype, cond agtype)';
    FOR v_rec IN EXECUTE v_cypher
    LOOP
        v_edges := v_edges || jsonb_build_object(
            'from', trim(both '"' from v_rec.from_id::text),
            'to', trim(both '"' from v_rec.to_id::text),
            'label', trim(both '"' from v_rec.cond::text),
            'arrows', 'to'
        );
    END LOOP;

    -- ENTRY Edges
    v_cypher := 'SELECT * FROM cypher(''meclaw_graph'', $cyp$ MATCH (h:Hive)-[r:ENTRY]->(b:Bee) RETURN h.id, b.id $cyp$) AS (hive_id agtype, bee_id agtype)';
    FOR v_rec IN EXECUTE v_cypher
    LOOP
        v_edges := v_edges || jsonb_build_object(
            'from', trim(both '"' from v_rec.hive_id::text),
            'to', trim(both '"' from v_rec.bee_id::text),
            'label', 'entry',
            'arrows', 'to',
            'dashes', true
        );
    END LOOP;

    RETURN jsonb_build_object('nodes', v_nodes, 'edges', v_edges);
END;
$fn$ LANGUAGE plpgsql;


-- Helper: Web-Chat Message einfügen (aufgerufen via pg_background)
CREATE OR REPLACE FUNCTION meclaw._web_chat_insert(p_task_id uuid, p_content jsonb)
RETURNS void AS $wcf$
BEGIN
    INSERT INTO meclaw.channels (id, name, type, config) VALUES 
        ('00000000-0000-0000-0000-000000000002', 'web-admin', 'web', '{}'::jsonb)
        ON CONFLICT (id) DO NOTHING;
    INSERT INTO meclaw.tasks (id, channel_id, status) VALUES 
        (p_task_id, '00000000-0000-0000-0000-000000000002'::uuid, 'running');
    INSERT INTO meclaw.messages (task_id, channel_id, type, sender, status, next_bee, content) VALUES 
        (p_task_id, '00000000-0000-0000-0000-000000000002'::uuid, 
         'user_input', 'web-admin', 'done', NULL, p_content);
END;
$wcf$ LANGUAGE plpgsql;


-- Admin Bee: persistent HTTP Server
CREATE OR REPLACE FUNCTION meclaw.admin_bee(p_port int DEFAULT 8080)
RETURNS void AS $$
from http.server import HTTPServer, BaseHTTPRequestHandler
import json
import traceback

STYLE = """
<style>
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, monospace; background: #0d1117; color: #c9d1d9; }
    .header { background: #161b22; border-bottom: 1px solid #30363d; padding: 16px 24px; display: flex; align-items: center; gap: 12px; }
    .header h1 { font-size: 20px; color: #f0f6fc; }
    .header .bee { font-size: 24px; }
    .nav { background: #161b22; border-bottom: 1px solid #30363d; padding: 8px 24px; display: flex; gap: 16px; }
    .nav a { color: #58a6ff; text-decoration: none; padding: 8px 12px; border-radius: 6px; }
    .nav a:hover { background: #1f2937; }
    .nav a.active { background: #1f6feb; color: #fff; }
    .container { max-width: 1200px; margin: 24px auto; padding: 0 24px; }
    .card { background: #161b22; border: 1px solid #30363d; border-radius: 8px; padding: 20px; margin-bottom: 16px; }
    .card h2 { color: #f0f6fc; margin-bottom: 12px; font-size: 16px; }
    table { width: 100%; border-collapse: collapse; }
    th, td { text-align: left; padding: 8px 12px; border-bottom: 1px solid #21262d; font-size: 13px; white-space: nowrap; }
    td:last-child { white-space: normal; }
    th { color: #8b949e; font-weight: 600; }
    .badge { display: inline-block; padding: 2px 8px; border-radius: 12px; font-size: 12px; font-weight: 500; }
    .badge-green { background: #1b4332; color: #2dd4bf; }
    .badge-yellow { background: #3d2e00; color: #fbbf24; }
    .badge-red { background: #3b1219; color: #f87171; }
    .badge-blue { background: #0c2d48; color: #58a6ff; }
    .stat { text-align: center; }
    .stat .number { font-size: 32px; font-weight: 700; color: #f0f6fc; }
    .stat .label { font-size: 12px; color: #8b949e; margin-top: 4px; }
    .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 16px; }
    pre { background: #0d1117; padding: 12px; border-radius: 6px; overflow-x: auto; font-size: 12px; }
    #graph { width: 100%; height: 600px; border: 1px solid #30363d; border-radius: 8px; }
    details { cursor: pointer; }
    details summary { color: #58a6ff; font-size: 12px; }
    details summary:hover { color: #79c0ff; }
    details pre { margin-top: 8px; max-height: 400px; overflow-y: auto; white-space: pre-wrap; word-break: break-all; }
    .json-key { color: #ff7b72; }
    .json-str { color: #a5d6ff; }
    .json-num { color: #79c0ff; }
    .json-bool { color: #ffa657; }
    .json-null { color: #8b949e; }
</style>
"""

NAV = """
<div class="nav">
    <a href="/" {active_home}>Status</a>
    <a href="/graph" {active_graph}>Graph</a>
    <a href="/events" {active_events}>Events</a>
    <a href="/messages" {active_messages}>Messages</a>
    <a href="/bees" {active_bees}>Bees</a>
    <a href="/models" {active_models}>Models</a>
    <a href="/chat" {active_chat}>Chat</a>
</div>
"""

def page(title, body, active=""):
    nav = NAV.format(
        active_home='class="active"' if active == "home" else "",
        active_graph='class="active"' if active == "graph" else "",
        active_events='class="active"' if active == "events" else "",
        active_messages='class="active"' if active == "messages" else "",
        active_bees='class="active"' if active == "bees" else "",
        active_models='class="active"' if active == "models" else "",
        active_chat='class="active"' if active == "chat" else ""
    )
    return f"""<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>MeClaw — {title}</title>{STYLE}</head>
<body>
<div class="header"><span class="bee">🐝</span><h1>MeClaw Admin</h1></div>
{nav}
<div class="container">{body}</div>
</body></html>"""

def badge(status):
    colors = {"done": "green", "ready": "blue", "running": "yellow", "waiting": "yellow", "failed": "red"}
    return f'<span class="badge badge-{colors.get(status, "blue")}">{status}</span>'

def fmt_time(ts):
    """HH:MM:SS-mmm Format"""
    if not ts:
        return ""
    s = str(ts)
    # Extract HH:MM:SS and milliseconds
    if '.' in s:
        time_part = s[11:19]
        ms_part = s[20:23].ljust(3, '0')  # 3-digit ms
        return f'{time_part}-{ms_part}'
    return s[11:19] + '-000'

def expandable_json(data, preview_len=80):
    """Klickbares JSON: Kurzversion + aufklappbares Pretty-Print"""
    if not data or data == '{}' or data == 'None':
        return '<code>{}</code>'
    s = str(data)
    short = s[:preview_len]
    if len(s) <= preview_len:
        return f'<code>{short}</code>'
    try:
        pretty = json.dumps(json.loads(s), indent=2, ensure_ascii=False)
    except Exception:
        pretty = s
    import html as html_mod
    pretty_escaped = html_mod.escape(pretty)
    return f'<details><summary><code>{html_mod.escape(short)}…</code></summary><pre>{pretty_escaped}</pre></details>'

def query(sql, args=None):
    try:
        if args:
            return plpy.execute(plpy.prepare(sql, args[0]), args[1])
        return plpy.execute(sql)
    except Exception as e:
        plpy.notice(f"Query error: {e}")
        return []

class Handler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass  # suppress access logs

    def respond(self, code, content_type, body):
        self.send_response(code)
        self.send_header("Content-Type", content_type)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(body.encode("utf-8") if isinstance(body, str) else body)

    def do_GET(self):
        try:
            path = self.path.split("?")[0]
            if path == "/":
                self.handle_status()
            elif path == "/graph":
                self.handle_graph()
            elif path == "/events":
                self.handle_events()
            elif path == "/messages":
                self.handle_messages()
            elif path == "/bees":
                self.handle_bees()
            elif path == "/models":
                self.handle_models()
            elif path == "/chat":
                self.handle_chat_page()
            elif path == "/api/graph":
                self.handle_api_graph()
            elif path == "/api/status":
                self.handle_api_status()
            elif path == "/api/chat/history":
                self.handle_chat_history()
            elif path.startswith("/api/chat/poll"):
                from urllib.parse import urlparse, parse_qs
                qs = parse_qs(urlparse(self.path).query)
                tid = qs.get("task_id", [None])[0]
                self.handle_chat_poll(tid)
            else:
                self.respond(404, "text/html", page("404", '<div class="card"><h2>404 — Nicht gefunden</h2></div>'))
        except Exception as e:
            tb = traceback.format_exc()
            self.respond(500, "text/html", page("Error", f'<div class="card"><h2>Error</h2><pre>{tb}</pre></div>'))

    def do_POST(self):
        try:
            path = self.path.split("?")[0]
            content_length = int(self.headers.get('Content-Length', 0))
            post_data = self.rfile.read(content_length).decode('utf-8') if content_length > 0 else '{}'
            if path == "/api/chat/send":
                self.handle_chat_send(json.loads(post_data))
            else:
                self.respond(404, "application/json", '{"error": "not found"}')
        except Exception as e:
            self.respond(500, "application/json", json.dumps({"error": str(e)}))

    def handle_status(self):
        # Stats
        stats = query("SELECT status, count(*) as cnt FROM meclaw.messages GROUP BY status ORDER BY status")
        total = sum(r["cnt"] for r in stats)
        tasks = query("SELECT status, count(*) as cnt FROM meclaw.tasks GROUP BY status ORDER BY status")
        poll = query("SELECT count(*) as cnt FROM meclaw.net_requests WHERE type = 'telegram_poll'")
        poll_active = poll[0]["cnt"] if poll else 0

        stats_html = ""
        for r in stats:
            stats_html += f'<div class="stat card"><div class="number">{r["cnt"]}</div><div class="label">{r["status"]}</div></div>'

        # Recent events
        events = query("SELECT event, bee_type, payload, created_at FROM meclaw.events ORDER BY id DESC LIMIT 15")
        events_html = "<tr><th>Zeit</th><th>Event</th><th>Bee</th><th>Payload</th></tr>"
        for e in events:
            ts = fmt_time(e["created_at"])
            p = expandable_json(e["payload"])
            events_html += f'<tr><td>{ts}</td><td>{e["event"]}</td><td>{e["bee_type"] or ""}</td><td>{p}</td></tr>'

        body = f"""
        <div class="grid" style="margin-bottom:16px">
            <div class="stat card"><div class="number">{total}</div><div class="label">Messages Total</div></div>
            {stats_html}
            <div class="stat card"><div class="number">{"✅" if poll_active > 0 else "❌"}</div><div class="label">Poll Active</div></div>
        </div>
        <div class="card"><h2>Recent Events</h2><table>{events_html}</table></div>
        """
        self.respond(200, "text/html", page("Status", body, "home"))

    def handle_graph(self):
        body = """
        <div class="card">
            <h2>Hive Graph</h2>
            <div id="graph"></div>
            <div style="margin-top:12px; display:flex; gap:16px; flex-wrap:wrap; font-size:12px; color:#8b949e;">
                <span>🏠 Hive</span>
                <span style="color:#22c55e">📡 Receiver</span>
                <span style="color:#a855f7">📞 Call</span>
                <span style="color:#f97316">🧠 LLM</span>
                <span style="color:#2dd4bf">📤 Sender</span>
                <span style="color:#6b7280">⚙️ Other</span>
                <span style="color:#30363d">- - - Entry Edge</span>
            </div>
        </div>
        <script src="https://unpkg.com/vis-network/standalone/umd/vis-network.min.js"></script>
        <script>
        var ICONS = { hive: '🏠', receiver_bee: '📡', call_bee: '📞', llm_bee: '🧠', sender_bee: '📤', channel_bee: '📡' };
        var COLORS = { hive: '#1f6feb', receiver_bee: '#22c55e', call_bee: '#a855f7', llm_bee: '#f97316', sender_bee: '#2dd4bf', channel_bee: '#22c55e' };
        function shortLabel(id, group) {
            // "main-receiver-bee" -> "receiver", "test-llm-bee" -> "llm", "main-graph" -> "main-graph"
            if (group === 'hive') return id;
            var parts = id.split('-');
            // Remove hive prefix and -bee suffix
            parts = parts.filter(p => p !== 'bee' && p !== 'main' && p !== 'test');
            return parts.join('-') || id;
        }
        fetch('/api/graph').then(r => r.json()).then(data => {
            var nodes = new vis.DataSet(data.nodes.map(n => ({
                id: n.id,
                label: (ICONS[n.group] || '⚙️') + ' ' + shortLabel(n.id, n.group),
                title: n.id + ' (' + n.group + ')',
                color: { background: (COLORS[n.group] || '#6b7280') + '22', border: COLORS[n.group] || '#6b7280', highlight: { background: (COLORS[n.group] || '#6b7280') + '44', border: COLORS[n.group] || '#6b7280' } },
                font: { color: '#e6edf3', size: 14, face: '-apple-system, BlinkMacSystemFont, sans-serif' },
                borderWidth: 2,
                shape: n.group === 'hive' ? 'diamond' : 'box',
                size: n.group === 'hive' ? 25 : 20,
                margin: { top: 10, bottom: 10, left: 12, right: 12 }
            })));
            var edges = new vis.DataSet(data.edges.map(e => ({
                from: e.from,
                to: e.to,
                label: e.label || '',
                arrows: { to: { enabled: true, scaleFactor: 0.8 } },
                dashes: e.dashes || false,
                color: { color: e.dashes ? '#1f6feb55' : '#58a6ff88', highlight: '#58a6ff' },
                font: { color: '#c9d1d9', size: 11, strokeWidth: 0, face: 'monospace', background: '#161b22' },
                width: e.dashes ? 1 : 2,
                smooth: { type: 'cubicBezier', roundness: 0.3 }
            })));
            new vis.Network(document.getElementById('graph'),
                { nodes, edges },
                { layout: { hierarchical: { direction: 'LR', sortMethod: 'directed', levelSeparation: 220, nodeSpacing: 120 } },
                  physics: false,
                  interaction: { hover: true, tooltipDelay: 100 },
                  nodes: { shadow: { enabled: true, color: 'rgba(0,0,0,0.3)', size: 5 } },
                  edges: { shadow: false } }
            );
        });
        </script>
        """
        self.respond(200, "text/html", page("Graph", body, "graph"))

    def handle_events(self):
        # Parse query params
        from urllib.parse import urlparse
        parsed = urlparse(self.path)
        show_poll = 'poll' in parsed.query

        if show_poll:
            events = query("SELECT id, event, bee_type, msg_id, task_id, payload, created_at FROM meclaw.events ORDER BY id DESC LIMIT 100")
        else:
            events = query("SELECT id, event, bee_type, msg_id, task_id, payload, created_at FROM meclaw.events WHERE event != 'poll_started' ORDER BY id DESC LIMIT 100")

        rows = "<tr><th>#</th><th>Zeit</th><th>Event</th><th>Bee</th><th>Msg</th><th>Payload</th></tr>"
        for e in events:
            ts = fmt_time(e["created_at"])
            mid = str(e["msg_id"])[:8] if e["msg_id"] else ""
            p = expandable_json(e["payload"])
            rows += f'<tr><td>{e["id"]}</td><td>{ts}</td><td>{e["event"]}</td><td>{e["bee_type"] or ""}</td><td><code>{mid}</code></td><td>{p}</td></tr>'

        checked = 'checked' if show_poll else ''
        filter_html = f"""
        <div style="margin-bottom:12px; display:flex; align-items:center; gap:8px;">
            <label style="display:flex; align-items:center; gap:6px; cursor:pointer; font-size:13px; color:#8b949e;">
                <input type="checkbox" {checked} onchange="window.location.href = this.checked ? '/events?poll' : '/events'" 
                    style="accent-color:#58a6ff; cursor:pointer;">
                poll_started anzeigen
            </label>
        </div>
        """
        body = f'<div class="card"><h2>Events (letzte 100)</h2>{filter_html}<table>{rows}</table></div>'
        self.respond(200, "text/html", page("Events", body, "events"))

    def handle_messages(self):
        msgs = query("SELECT id, type, sender, status, next_bee, assigned_to, content, created_at FROM meclaw.messages ORDER BY created_at DESC LIMIT 50")
        rows = "<tr><th>Zeit</th><th>Type</th><th>Status</th><th>Sender</th><th>Next Bee</th><th>Content</th></tr>"
        for m in msgs:
            ts = fmt_time(m["created_at"])
            c = expandable_json(m["content"])
            rows += f'<tr><td>{ts}</td><td>{m["type"]}</td><td>{badge(m["status"])}</td><td>{m["sender"] or ""}</td><td>{m["next_bee"] or ""}</td><td>{c}</td></tr>'

        body = f'<div class="card"><h2>Messages (letzte 50)</h2><table>{rows}</table></div>'
        self.respond(200, "text/html", page("Messages", body, "messages"))

    def handle_bees(self):
        graph_data = query("SELECT meclaw.export_graph_data() as data")
        data = json.loads(graph_data[0]["data"]) if graph_data else {"nodes": [], "edges": []}

        rows = "<tr><th>ID</th><th>Type</th><th>Hive</th></tr>"
        for n in data["nodes"]:
            rows += f'<tr><td>{n["id"]}</td><td>{n["group"]}</td><td>{n.get("title", "")}</td></tr>'

        body = f'<div class="card"><h2>Bees & Hives</h2><table>{rows}</table></div>'
        self.respond(200, "text/html", page("Bees", body, "bees"))

    def handle_models(self):
        # Providers
        providers = query("SELECT id, name, base_url, CASE WHEN api_key IS NOT NULL THEN '***' || right(api_key, 6) ELSE NULL END as api_key_masked, type, enabled, priority FROM meclaw.llm_providers ORDER BY priority")
        prov_rows = "<tr><th>ID</th><th>Name</th><th>URL</th><th>API Key</th><th>Type</th><th>Priority</th><th>Status</th></tr>"
        for p in providers:
            status = badge("done") if p["enabled"] else badge("failed")
            key = f'<code>{p["api_key_masked"]}</code>' if p["api_key_masked"] else '<span style="color:#8b949e">none</span>'
            prov_rows += f'<tr><td><code>{p["id"]}</code></td><td>{p["name"]}</td><td><code>{p["base_url"][:40]}</code></td><td>{key}</td><td>{p["type"]}</td><td>{p["priority"]}</td><td>{status}</td></tr>'

        # Models
        models = query("""
            SELECT m.id, m.provider_id, m.model_name, m.display_name, m.tier, 
                   m.max_tokens, m.supports_tools, m.enabled,
                   m.cost_per_1k_in, m.cost_per_1k_out
            FROM meclaw.llm_models m ORDER BY m.tier, m.provider_id, m.id
        """)
        tier_colors = {"small": "blue", "medium": "yellow", "large": "green", "reasoning": "red"}
        model_rows = "<tr><th>ID</th><th>Provider</th><th>Model</th><th>Tier</th><th>Max Tokens</th><th>Tools</th><th>Cost (in/out per 1k)</th><th>Status</th></tr>"
        for m in models:
            status = badge("done") if m["enabled"] else badge("failed")
            tier = f'<span class="badge badge-{tier_colors.get(m["tier"], "blue")}">{m["tier"]}</span>'
            tools = "✅" if m["supports_tools"] else "❌"
            cost_in = f'${float(m["cost_per_1k_in"]):.4f}' if float(m["cost_per_1k_in"]) > 0 else 'free'
            cost_out = f'${float(m["cost_per_1k_out"]):.4f}' if float(m["cost_per_1k_out"]) > 0 else 'free'
            model_rows += f'<tr><td><code>{m["id"]}</code></td><td>{m["provider_id"]}</td><td><code>{m["model_name"]}</code></td><td>{tier}</td><td>{m["max_tokens"]}</td><td>{tools}</td><td>{cost_in} / {cost_out}</td><td>{status}</td></tr>'

        # Usage stats (letzte 24h)
        usage = query("""
            SELECT payload->>'model_id' as model_id, 
                   payload->>'provider' as provider,
                   count(*) as calls
            FROM meclaw.events 
            WHERE event = 'llm_request' 
            AND created_at >= clock_timestamp() - interval '24 hours'
            AND payload->>'model_id' IS NOT NULL
            GROUP BY payload->>'model_id', payload->>'provider'
            ORDER BY count(*) DESC
        """)
        usage_rows = "<tr><th>Model</th><th>Provider</th><th>Calls (24h)</th></tr>"
        for u in usage:
            usage_rows += f'<tr><td><code>{u["model_id"]}</code></td><td>{u["provider"]}</td><td>{u["calls"]}</td></tr>'

        # Rate Limits
        rates = query("SELECT * FROM meclaw.rate_limits ORDER BY window_sec")
        rate_rows = "<tr><th>Limit</th><th>Max</th><th>Window</th></tr>"
        for r in rates:
            window = f'{r["window_sec"]}s'
            if r["window_sec"] == 60: window = "1 min"
            elif r["window_sec"] == 3600: window = "1 hour"
            elif r["window_sec"] == 86400: window = "1 day"
            rate_rows += f'<tr><td><code>{r["id"]}</code></td><td>{r["max_count"]}</td><td>{window}</td></tr>'

        body = f"""
        <div class="card"><h2>Providers</h2><table>{prov_rows}</table></div>
        <div class="card"><h2>Models</h2><table>{model_rows}</table></div>
        <div class="grid" style="grid-template-columns: 1fr 1fr;">
            <div class="card"><h2>Usage (24h)</h2><table>{usage_rows}</table></div>
            <div class="card"><h2>Rate Limits</h2><table>{rate_rows}</table></div>
        </div>
        """
        self.respond(200, "text/html", page("Models", body, "models"))

    def handle_chat_page(self):
        body = """
        <div class="card" style="height: calc(100vh - 200px); display: flex; flex-direction: column;">
            <h2>🦊 Chat with Walter</h2>
            <div id="chat-messages" style="flex: 1; overflow-y: auto; padding: 12px; background: #0d1117; border-radius: 6px; margin: 12px 0;">
            </div>
            <div style="display: flex; gap: 8px;">
                <input type="text" id="chat-input" placeholder="Nachricht eingeben..."
                    style="flex:1; padding:10px 14px; background:#161b22; border:1px solid #30363d; border-radius:6px; color:#c9d1d9; font-size:14px; outline:none;"
                    onkeypress="if(event.key==='Enter')sendMessage()">
                <button onclick="sendMessage()"
                    style="padding:10px 20px; background:#1f6feb; color:#fff; border:none; border-radius:6px; cursor:pointer; font-size:14px;">
                    Senden
                </button>
            </div>
        </div>
        <script>
        const chatBox = document.getElementById('chat-messages');
        const chatInput = document.getElementById('chat-input');

        function addMessage(role, text, time) {
            const div = document.createElement('div');
            div.style.cssText = 'margin-bottom:12px; display:flex; flex-direction:column;' +
                (role === 'user' ? 'align-items:flex-end;' : 'align-items:flex-start;');
            const bubble = document.createElement('div');
            bubble.style.cssText = 'max-width:70%; padding:10px 14px; border-radius:12px; font-size:14px; line-height:1.5; white-space:pre-wrap;' +
                (role === 'user' ? 'background:#1f6feb; color:#fff;' : 'background:#21262d; color:#c9d1d9;');
            bubble.textContent = text;
            div.appendChild(bubble);
            if (time) {
                const ts = document.createElement('span');
                ts.style.cssText = 'font-size:11px; color:#8b949e; margin-top:2px;';
                ts.textContent = time;
                div.appendChild(ts);
            }
            chatBox.appendChild(div);
            chatBox.scrollTop = chatBox.scrollHeight;
        }

        function addTyping() {
            const div = document.createElement('div');
            div.id = 'typing';
            div.style.cssText = 'margin-bottom:12px;';
            div.innerHTML = '<span style="color:#8b949e; font-size:13px;">🦊 Walter tippt...</span>';
            chatBox.appendChild(div);
            chatBox.scrollTop = chatBox.scrollHeight;
        }

        function removeTyping() {
            const el = document.getElementById('typing');
            if (el) el.remove();
        }

        async function pollForResponse(taskId) {
            for (let i = 0; i < 150; i++) {
                await new Promise(r => setTimeout(r, 400));
                try {
                    const resp = await fetch('/api/chat/poll?task_id=' + taskId);
                    const data = await resp.json();
                    if (data.status === 'done') {
                        removeTyping();
                        const now = new Date().toLocaleTimeString('de-DE', {hour:'2-digit',minute:'2-digit'});
                        addMessage('assistant', data.response, now);
                        return;
                    }
                } catch(e) {}
            }
            removeTyping();
            addMessage('assistant', '❌ Timeout (60s)');
        }

        async function sendMessage() {
            const text = chatInput.value.trim();
            if (!text) return;
            chatInput.value = '';
            const now = new Date().toLocaleTimeString('de-DE', {hour:'2-digit',minute:'2-digit'});
            addMessage('user', text, now);
            addTyping();

            try {
                const resp = await fetch('/api/chat/send', {
                    method: 'POST',
                    headers: {'Content-Type': 'application/json'},
                    body: JSON.stringify({message: text})
                });
                const data = await resp.json();
                if (data.error) {
                    removeTyping();
                    addMessage('assistant', '❌ ' + data.error);
                } else {
                    pollForResponse(data.task_id);
                }
            } catch(e) {
                removeTyping();
                addMessage('assistant', '❌ Verbindungsfehler: ' + e.message);
            }
        }

        // Load history on page load
        fetch('/api/chat/history').then(r => r.json()).then(data => {
            (data.messages || []).forEach(m => {
                addMessage(m.role, m.content, m.time || '');
            });
        });

        chatInput.focus();
        </script>
        """
        self.respond(200, "text/html", page("Chat", body, "chat"))

    def handle_chat_send(self, data):
        """Chat: async — insert message, return task_id immediately"""
        import uuid
        import psycopg2
        text = data.get("message", "").strip()
        if not text:
            self.respond(400, "application/json", '{"error": "empty message"}')
            return

        task_id = str(uuid.uuid4())
        content_json = json.dumps({
            "input": text, "current_bee": "main-receiver-bee",
            "stack": [], "telegram_chat_id": ""
        }, ensure_ascii=False)

        conn = None
        try:
            conn = psycopg2.connect("dbname=meclaw user=postgres")
            conn.autocommit = True
            cur = conn.cursor()
            cur.execute("SELECT meclaw._web_chat_insert(%s::uuid, %s::jsonb)", (task_id, content_json))
            plpy.notice(f"Web chat: task={task_id}, text={text[:50]}")
            self.respond(200, "application/json", json.dumps({
                "task_id": task_id, "status": "accepted"
            }))
        except Exception as e:
            plpy.notice(f"Chat error: {e}")
            self.respond(500, "application/json", json.dumps({"error": str(e)}))
        finally:
            if conn:
                conn.close()

    def handle_chat_poll(self, task_id):
        """Chat: poll for response by task_id"""
        import psycopg2
        if not task_id:
            self.respond(400, "application/json", '{"error": "missing task_id"}')
            return
        conn = None
        try:
            conn = psycopg2.connect("dbname=meclaw user=postgres")
            conn.autocommit = True
            cur = conn.cursor()
            cur.execute(
                "SELECT content->>'output' FROM meclaw.messages WHERE task_id = %s::uuid AND type = 'llm_result' AND status = 'done' LIMIT 1",
                (task_id,))
            row = cur.fetchone()
            if row and row[0]:
                self.respond(200, "application/json", json.dumps({
                    "status": "done", "response": row[0]
                }, ensure_ascii=False))
            else:
                self.respond(200, "application/json", '{"status": "pending"}')
        except Exception as e:
            self.respond(500, "application/json", json.dumps({"error": str(e)}))
        finally:
            if conn:
                conn.close()

    def handle_chat_history(self):
        """Letzte Chat-Messages aus dem Web-Channel"""
        result = query("""
            SELECT role, text as content, to_char(created_at, 'HH24:MI') as time
            FROM meclaw.channel_conversation
            WHERE channel_id = '00000000-0000-0000-0000-000000000002'
            ORDER BY created_at DESC LIMIT 20
        """)
        messages = [{"role": r["role"], "content": r["content"], "time": r["time"]} for r in reversed(result)] if result else []
        self.respond(200, "application/json", json.dumps({"messages": messages}, ensure_ascii=False))

    def handle_api_graph(self):
        result = query("SELECT meclaw.export_graph_data() as data")
        data = result[0]["data"] if result else "{}"
        self.respond(200, "application/json", data if isinstance(data, str) else json.dumps(data))

    def handle_api_status(self):
        stats = query("SELECT status, count(*) as cnt FROM meclaw.messages GROUP BY status")
        poll = query("SELECT count(*) as cnt FROM meclaw.net_requests WHERE type = 'telegram_poll'")
        self.respond(200, "application/json", json.dumps({
            "messages": {r["status"]: r["cnt"] for r in stats},
            "poll_active": poll[0]["cnt"] > 0 if poll else False
        }))

import socket

plpy.notice(f"admin_bee starting on port {p_port}")

class ReusableHTTPServer(HTTPServer):
    allow_reuse_address = True
    allow_reuse_port = True
    def server_bind(self):
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        except (AttributeError, OSError):
            pass
        super().server_bind()

server = ReusableHTTPServer(("0.0.0.0", p_port), Handler)
try:
    server.serve_forever()
except Exception as e:
    plpy.notice(f"admin_bee stopped: {e}")
finally:
    server.server_close()
$$ LANGUAGE plpython3u;


-- Watchdog: prüft ob admin_bee läuft, startet neu wenn nicht
CREATE OR REPLACE FUNCTION meclaw.admin_bee_watchdog()
RETURNS void AS $$
DECLARE
    v_running boolean;
BEGIN
    SELECT EXISTS(
        SELECT 1 FROM pg_stat_activity
        WHERE query LIKE '%admin_bee(%' AND state = 'active'
        AND pid != pg_backend_pid()
    ) INTO v_running;

    IF NOT v_running THEN
        PERFORM meclaw.log_event(NULL, NULL, 'admin_bee', 'admin_bee_restart', '{"reason": "watchdog"}'::jsonb);
        PERFORM pg_background_launch('SELECT meclaw.admin_bee(8080)');
    END IF;
END;
$$ LANGUAGE plpgsql;
