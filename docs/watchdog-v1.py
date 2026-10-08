# >>> watchdog v1 (R-AG-1, GH #1096)
import datetime as _wd_dt
import uuid as _wd_uuid

WATCHDOG_NS = _wd_uuid.UUID("6f1c2a8e-3b7d-5e90-9a41-0c2d7e5b8f13")


def watchdog_id(name):
    """Stable schedule id for a watchdog name: re-arming the name replaces it."""
    return str(_wd_uuid.uuid5(WATCHDOG_NS, str(name)))


def _iso(ms):
    return _wd_dt.datetime.fromtimestamp(
        int(ms) / 1000.0, tz=_wd_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def once(name, at_ms, emit_to, body, catch_up=True):
    """One-shot watchdog: fires once at `at_ms` (epoch ms). `at_ms` must lie
    ahead: the timer refuses a past moment (`at_in_past`), so a caller whose
    moment has come does the work now instead of arming. `catch_up` (default
    on) fires a moment missed while the colony was down once on its next
    start, with `late: true`; without it a missed watchdog never fires."""
    return {"op": "add", "schedule_id": watchdog_id(name), "schedule_name": str(name),
            "at": _iso(at_ms), "emit_to": emit_to, "emit_body": body,
            "rearm": True, "catch_up": bool(catch_up)}


def retrigger(name, now_ms, quiet_ms, emit_to, body):
    """Debounce: fires `quiet_ms` (> 0) after the last call; every call
    pushes it out."""
    return once(name, int(now_ms) + max(1, int(quiet_ms)), emit_to, body)


def calendar(name, cron, emit_to, body):
    """Calendar watchdog (Jenkins-style): real calendar points only, never a
    tick. `cron` is the timer's 6-field Quartz form, seconds first, in UTC
    (`0 0 3 * * *`)."""
    return {"op": "add", "schedule_id": watchdog_id(name), "schedule_name": str(name),
            "cron": cron, "emit_to": emit_to, "emit_body": body, "rearm": True}


def cancel(name):
    """Disarm a watchdog by name. A watchdog that is not armed (never armed,
    already fired) answers `timer_op_error` / `schedule_not_found`."""
    return {"op": "remove", "schedule_id": watchdog_id(name)}
# <<< watchdog v1
