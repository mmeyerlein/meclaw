"""Upload the dashboard `dev-tempo` and its alert rules to Grafana (W0).

Everything about the target comes from the caller's environment, nothing is
read from a file of secrets: `MECLAW_GRAFANA_URL` (base URL), the user
`GRAFANA_ADMIN_USER` (default `admin`) and the password `GRAFANA_ADMIN_PASS`
(the caller sets it with the one-key grep form of the machine rules). The
data source uid is looked up by name (`VictoriaMetrics`) and put in for the
placeholder `${DS_VM}` -- never written into the files. The upload is
idempotent: the dashboard goes up with `overwrite`, every rule is a PUT when
its uid exists, else a POST.

Result line: `GRAFANA dashboard=dev-tempo status=<http> panels=<n> rules=<n>`.
"""

import base64
import json
import os
import pathlib
import sys
import urllib.error
import urllib.request

REPO = pathlib.Path(__file__).resolve().parents[2]
DEFAULT_DIR = REPO / "workshop" / "grafana"
PLACEHOLDER = "${DS_VM}"
DATASOURCE = "VictoriaMetrics"


class Grafana:
    def __init__(self, base, user, password):
        self.base = base.rstrip("/")
        self.auth = base64.b64encode(f"{user}:{password}".encode()).decode()

    def call(self, method, path, body=None):
        req = urllib.request.Request(
            self.base + path, method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Authorization": f"Basic {self.auth}", "Content-Type": "application/json",
                     "X-Disable-Provenance": "true"})
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                raw = resp.read()
                return resp.status, (json.loads(raw) if raw else {})
        except urllib.error.HTTPError as exc:
            return exc.code, {"message": exc.read().decode(errors="replace")[:300]}


def _load(path, ds_uid):
    return json.loads(pathlib.Path(path).read_text().replace(PLACEHOLDER, ds_uid))


def push(g, dash_path, alerts_path):
    status, ds = g.call("GET", f"/api/datasources/name/{DATASOURCE}")
    if status != 200:
        raise RuntimeError(f"data source {DATASOURCE}: HTTP {status}")
    ds_uid = ds["uid"]
    alerts = _load(alerts_path, ds_uid)
    folder = alerts["folder"]
    if g.call("GET", f"/api/folders/{folder['uid']}")[0] != 200:
        status, _ = g.call("POST", "/api/folders", folder)
        if status not in (200, 201):
            raise RuntimeError(f"folder {folder['uid']}: HTTP {status}")
    status, points = g.call("GET", "/api/v1/provisioning/contact-points")
    names = {c.get("name") for c in points} if status == 200 and isinstance(points, list) else set()
    if alerts["contact_point"] not in names:
        raise RuntimeError(f"contact point {alerts['contact_point']!r} does not exist")
    dash = _load(dash_path, ds_uid)
    dash.pop("id", None)
    status, res = g.call("POST", "/api/dashboards/db",
                         {"dashboard": dash, "overwrite": True, "folderUid": folder["uid"],
                          "message": "pipeline grafana push"})
    if status != 200:
        raise RuntimeError(f"dashboard: HTTP {status} {res.get('message', '')}")
    status, existing = g.call("GET", "/api/v1/provisioning/alert-rules")
    have = {r.get("uid") for r in existing} if status == 200 and isinstance(existing, list) else set()
    for rule in alerts["rules"]:
        rule = dict(rule, folderUID=folder["uid"], ruleGroup=alerts["group"])
        if rule["uid"] in have:
            status, res = g.call("PUT", f"/api/v1/provisioning/alert-rules/{rule['uid']}", rule)
        else:
            status, res = g.call("POST", "/api/v1/provisioning/alert-rules", rule)
        if status not in (200, 201):
            raise RuntimeError(f"rule {rule['uid']}: HTTP {status} {res.get('message', '')}")
    group_path = f"/api/v1/provisioning/folder/{folder['uid']}/rule-groups/{alerts['group']}"
    status, group = g.call("GET", group_path)
    if status == 200:
        group["interval"] = alerts["interval"]
        g.call("PUT", group_path, group)
    status, check = g.call("GET", f"/api/dashboards/uid/{dash['uid']}")
    panels = [p for p in check.get("dashboard", {}).get("panels", []) if p.get("type") != "row"]
    return {"status": status, "panels": len(panels), "rules": [r["title"] for r in alerts["rules"]]}


def _cmd_grafana(args):
    base = os.environ.get("MECLAW_GRAFANA_URL", "").strip()
    password = os.environ.get("GRAFANA_ADMIN_PASS", "")
    if not base or not password:
        print("grafana: set MECLAW_GRAFANA_URL and GRAFANA_ADMIN_PASS in the environment",
              file=sys.stderr)
        return 2
    g = Grafana(base, os.environ.get("GRAFANA_ADMIN_USER", "admin"), password)
    try:
        res = push(g, args.dashboard, args.alerts)
    except (RuntimeError, OSError) as exc:
        print(f"grafana: {exc}", file=sys.stderr)
        return 1
    print(f"GRAFANA dashboard=dev-tempo status={res['status']} panels={res['panels']} "
          f"rules={len(res['rules'])}")
    for title in res["rules"]:
        print(f"GRAFANA rule {title}")
    return 0 if res["status"] == 200 else 1


def register(sub):
    """`pipeline grafana push` -- dashboard + alert rules, idempotent."""
    p = sub.add_parser("grafana", help="upload the dev-tempo dashboard and alert rules")
    p.add_argument("action", choices=("push",))
    p.add_argument("--dashboard", default=str(DEFAULT_DIR / "dev-tempo.json"))
    p.add_argument("--alerts", default=str(DEFAULT_DIR / "dev-tempo-alerts.json"))
    p.set_defaults(func=_cmd_grafana)
