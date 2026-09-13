#!/usr/bin/env python3
"""Fetch plan usage for Kimi / Zhipu / OpenCode Go / Command Code.

Config arrives via environment (keys + enable flags); prints one JSON object:
{"kimi": {"plan": str|None, "windows": [{"label","pct","resetTs"|null}], "error": str|None}, ...}
"""
import json
import os
import sys
import time
import urllib.request
import urllib.parse
from datetime import datetime, timezone

TIMEOUT = 15
# urllib's default "Python-urllib/3.x" UA is blocked by CDNs (403) even with
# a valid key — every request identifies as a normal client instead.
DEFAULT_UA = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) usage-fetch/1.0"

PROVIDERS = ("kimi", "zhipu", "opencode", "commandcode")


def env(name, default=""):
    return os.environ.get(name, default)


def enabled(pid):
    return env(f"USAGE_{pid.upper()}_ENABLED", "On").lower() not in ("0", "false", "off", "no")


def api_key(pid):
    key = env(f"USAGE_{pid.upper()}_KEY", "").strip()
    if key:
        return key
    if pid == "opencode":
        return opencode_go_key()
    if pid == "commandcode":
        return commandcode_key()
    return None


def opencode_go_key():
    base = env("XDG_DATA_HOME") or os.path.join(os.path.expanduser("~"), ".local", "share")
    try:
        with open(os.path.join(base, "opencode", "auth.json"), encoding="utf-8") as f:
            root = json.load(f)
        entry = root.get("opencode-go") or {}
        key = entry.get("key")
        if key:
            return key
        tokens = entry.get("tokens") or []
        return tokens[0] if tokens else None
    except (OSError, ValueError, IndexError):
        return None


def commandcode_key():
    try:
        with open(os.path.join(os.path.expanduser("~"), ".commandcode", "auth.json"), encoding="utf-8") as f:
            return json.load(f).get("apiKey") or None
    except (OSError, ValueError):
        return None


def get(url, key, user_agent=DEFAULT_UA):
    req = urllib.request.Request(url, method="GET")
    req.add_header("Authorization", f"Bearer {key}")
    req.add_header("Accept", "application/json")
    # urllib's default "Python-urllib/3.x" UA is blocked by CDNs (403) even
    # with a valid key — always send a normal client UA.
    req.add_header("User-Agent", user_agent)
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        try:
            body = e.read()
        except Exception:
            body = b""
        return e.code, body
    except Exception as e:
        raise RuntimeError(f"network error: {e}")


def error_text(status, body, provider):
    snippet = body[:200].decode("utf-8", "replace")
    if status in (401, 403):
        return f"{provider} auth failed ({status}): API key invalid or expired"
    if status == 429:
        return f"{provider} rate limited (429)"
    return f"{provider} API error HTTP {status} {snippet}"


def failed(pid, message):
    return {"plan": None, "windows": [], "error": message}


def num(v):
    if isinstance(v, bool):
        return None
    if isinstance(v, (int, float)):
        return float(v)
    if isinstance(v, str):
        try:
            return float(v)
        except ValueError:
            return None
    return None


def clamp_pct(v):
    return min(100.0, max(0.0, v))


def iso_date(s):
    if not s or not isinstance(s, str):
        return None
    try:
        if s.endswith("Z"):
            s = s[:-1] + "+00:00"
        d = datetime.fromisoformat(s)
        if d.tzinfo is None:
            d = d.replace(tzinfo=timezone.utc)
        return d.timestamp()
    except ValueError:
        pass
    try:
        d = datetime.strptime(s, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=timezone.utc)
        return d.timestamp()
    except ValueError:
        return None


def flexible_ts(v):
    n = num(v)
    if n is not None:
        if n <= 0:
            return None
        return n / 1000.0 if n >= 1e12 else n
    if isinstance(v, str) and v:
        return iso_date(v)
    return None


def order_windows(ws):
    def rank(w):
        if "5h" in w["label"]:
            return 0
        if w["label"] == "Weekly":
            return 1
        return 2
    return sorted(ws, key=rank)


# --- Kimi ---

def fetch_kimi(key):
    if not key:
        return failed("kimi", "No API key configured (sk-kimi-…)")
    base = "https://api.kimi.com/coding/v1"
    try:
        status, body = get(base + "/usages", key, user_agent="KimiCLI/1.6")
        if status == 404:
            status, body = get(base + "/usage", key, user_agent="KimiCLI/1.6")
        if status != 200:
            return failed("kimi", error_text(status, body, "Kimi"))
        return parse_kimi(body)
    except RuntimeError as e:
        return failed("kimi", f"Kimi {e}")


def kimi_row(d, wid, label):
    limit = num(d.get("limit", d.get("limit_amount")))
    remaining = num(d.get("remaining"))
    used = num(d.get("used", d.get("used_amount")))
    if used is None and limit is not None and remaining is not None:
        used = limit - remaining
    if not limit or limit <= 0 or used is None:
        return None
    return {"label": label, "pct": clamp_pct(used / limit * 100),
            "resetTs": iso_date(d.get("resetTime")) or iso_date(d.get("reset_at"))}


def parse_kimi(body):
    try:
        root = json.loads(body)
    except ValueError:
        return failed("kimi", "Failed to parse Kimi response")
    if not isinstance(root, dict):
        return failed("kimi", "Failed to parse Kimi response")
    windows = []
    if isinstance(root.get("usage"), dict):
        row = kimi_row(root["usage"], "kimi-week", "Weekly")
        if row:
            windows.append(row)
    if isinstance(root.get("limits"), list):
        for idx, item in enumerate(root["limits"]):
            if not isinstance(item, dict):
                continue
            win = item.get("window") if isinstance(item.get("window"), dict) else {}
            detail = item.get("detail") if isinstance(item.get("detail"), dict) else item
            unit = str(win.get("timeUnit") or "").upper()
            if "MINUTE" not in unit:
                continue
            duration = num(win.get("duration")) or 0
            if duration >= 60 and duration % 60 == 0:
                label = "5h window"
            else:
                label = f"{int(duration)}-minute window"
            row = kimi_row(detail, f"kimi-win-{idx}", label)
            if row:
                windows.append(row)
    level = ((root.get("user") or {}).get("membership") or {}).get("level", "")
    plan = {"LEVEL_FREE": "Adagio", "LEVEL_TRIAL": "Andante", "LEVEL_BASIC": "Moderato",
            "LEVEL_INTERMEDIATE": "Allegretto", "LEVEL_ADVANCED": "Allegro"}.get(level, level or None)
    if not windows:
        return failed("kimi", "No usage windows in Kimi response")
    return {"plan": plan, "windows": order_windows(windows), "error": None}


# --- Zhipu ---

def fetch_zhipu(key):
    if not key:
        return failed("zhipu", "No API key configured")
    base = "https://open.bigmodel.cn"
    try:
        status, body = get(base + "/api/monitor/usage/quota/limit", key)
        if status != 200:
            return failed("zhipu", error_text(status, body, "Zhipu"))
        plan = None
        try:
            s2, b2 = get(base + "/api/biz/subscription/list", key)
            if s2 == 200:
                data = json.loads(b2)
                items = data.get("data") if isinstance(data, dict) else None
                if isinstance(items, list) and items and isinstance(items[0], dict):
                    plan = items[0].get("productName")
        except Exception:
            pass
        return parse_zhipu(body, plan)
    except RuntimeError as e:
        return failed("zhipu", f"Zhipu {e}")


def parse_zhipu(body, plan):
    try:
        root = json.loads(body)
    except ValueError:
        return failed("zhipu", "Failed to parse Zhipu response")
    if not isinstance(root, dict):
        return failed("zhipu", "Failed to parse Zhipu response")
    code = num(root.get("code"))
    if code is not None and code != 200:
        msg = root.get("msg") or ""
        if code == 401:
            return failed("zhipu", f"Zhipu auth failed (401): API key invalid or expired {msg}")
        return failed("zhipu", f"Zhipu API error code={int(code)} {msg}")
    data = root.get("data") if isinstance(root.get("data"), dict) else None
    limits = data.get("limits") if data else None
    if not isinstance(limits, list):
        return failed("zhipu", "Failed to parse Zhipu response")
    windows = []
    for idx, item in enumerate(limits):
        if not isinstance(item, dict):
            continue
        t = item.get("type") or ""
        if t in ("CREDIT_LIMIT", "TOKENS_LIMIT"):
            unit = num(item.get("unit")) or 0
            label = "5h window" if unit == 3 else ("Weekly" if unit == 6 else f"Window #{idx + 1}")
            windows.append({"label": label, "pct": clamp_pct(num(item.get("percentage")) or 0),
                            "resetTs": flexible_ts(item.get("nextResetTime"))})
        elif t == "TIME_LIMIT":
            windows.append({"label": "Tools (Monthly)", "pct": clamp_pct(num(item.get("percentage")) or 0),
                            "resetTs": flexible_ts(item.get("nextResetTime"))})
    if not windows:
        return failed("zhipu", "No usage windows in Zhipu response")
    return {"plan": plan, "windows": order_windows(windows), "error": None}


# --- OpenCode ---

def fetch_opencode(key):
    if not key:
        return failed("opencode", "No API key configured (add it in settings, or log in via opencode CLI)")
    try:
        status, body = get("https://opencode.ai/zen/go/v1/usage", key)
        if status != 200:
            return failed("opencode", error_text(status, body, "OpenCode"))
        try:
            root = json.loads(body)
        except ValueError:
            return failed("opencode", "Failed to parse OpenCode response")
        usage = root.get("usage") if isinstance(root, dict) else None
        if not isinstance(usage, dict):
            return failed("opencode", "Failed to parse OpenCode response")
        windows = []
        for name, label in (("rolling", "5h window"), ("weekly", "Weekly"), ("monthly", "Monthly")):
            w = usage.get(name)
            if not isinstance(w, dict):
                continue
            pct = num(w.get("percent", w.get("usagePercent"))) or 0
            windows.append({"label": label, "pct": clamp_pct(pct),
                            "resetTs": iso_date(w.get("resetsAt"))})
        if not windows:
            return failed("opencode", "No usage windows in OpenCode response")
        return {"plan": "OpenCode Go", "windows": order_windows(windows), "error": None}
    except RuntimeError as e:
        return failed("opencode", f"OpenCode {e}")


# --- Command Code ---

def fetch_commandcode(key):
    if not key:
        return failed("commandcode", "No API key configured (add it in settings, or log in via cmd CLI)")
    base = "https://api.commandcode.ai"
    try:
        status, body = get(base + "/alpha/billing/credits", key)
        if status != 200:
            return failed("commandcode", error_text(status, body, "Command Code"))
        plan = monthly = None
        try:
            s2, b2 = get(base + "/alpha/billing/subscriptions", key)
            if s2 == 200:
                sub = json.loads(b2)
                data = sub.get("data") if isinstance(sub, dict) else None
                if isinstance(data, dict):
                    pid = data.get("planId") or ""
                    if "goat" in pid:
                        plan = "GoAT"
                    elif pid == "go" or pid.endswith("-go"):
                        plan = "Go"
                    else:
                        plan = pid or None
                    monthly = monthly_usage(key, data)
        except Exception:
            pass
        return parse_commandcode(body, plan, monthly)
    except RuntimeError as e:
        return failed("commandcode", f"Command Code {e}")


def monthly_usage(key, data):
    reset = iso_date(data.get("currentPeriodEnd"))
    try:
        url = "https://api.commandcode.ai/alpha/usage/summary"
        start = data.get("currentPeriodStart")
        if start:
            url += "?since=" + urllib.parse.quote(str(start), safe="")
        status, body = get(url, key)
        if status != 200:
            return None
        root = json.loads(body)
        if not isinstance(root, dict):
            return None
        used = num(root.get("totalMonthlyCredits", root.get("totalCost")))
        return (used, reset) if used is not None else None
    except Exception:
        return None


def parse_commandcode(body, plan, monthly):
    try:
        root = json.loads(body)
    except ValueError:
        return failed("commandcode", "Failed to parse Command Code response")
    if not isinstance(root, dict):
        return failed("commandcode", "Failed to parse Command Code response")
    limits = root.get("windowLimits")
    if not isinstance(limits, dict):
        return failed("commandcode", "Failed to parse Command Code response")
    windows = []
    for key_name, wid, label in (("fiveHour", "cc-5h", "5h window"), ("weekly", "cc-weekly", "Weekly")):
        w = limits.get(key_name)
        if not isinstance(w, dict):
            continue
        cap, used = num(w.get("cap")), num(w.get("used"))
        if not cap or cap <= 0 or used is None:
            continue
        windows.append({"label": label, "pct": clamp_pct(used / cap * 100),
                        "resetTs": flexible_ts(w.get("resetAt"))})
    if monthly is not None:
        credits = root.get("credits") if isinstance(root.get("credits"), dict) else None
        remaining = num(credits.get("monthlyCredits")) if credits else None
        used, reset = monthly
        if remaining is not None and used + remaining > 0:
            windows.append({"label": "Monthly", "pct": clamp_pct(used / (used + remaining) * 100),
                            "resetTs": reset})
    if not windows:
        return failed("commandcode", "No usage windows in Command Code response")
    return {"plan": plan, "windows": order_windows(windows), "error": None}


FETCHERS = {"kimi": fetch_kimi, "zhipu": fetch_zhipu,
            "opencode": fetch_opencode, "commandcode": fetch_commandcode}


def main():
    want = sys.argv[1:] or list(PROVIDERS)
    out = {}
    for pid in want:
        if pid not in PROVIDERS:
            continue
        if not enabled(pid):
            out[pid] = {"plan": None, "windows": [], "error": None, "disabled": True, "authed": False}
            continue
        key = api_key(pid)
        result = FETCHERS[pid](key)
        # Whether a credential resolved (settings key or CLI login file):
        # drives visibility in the bar/panel, not just error reporting.
        result["authed"] = key is not None
        out[pid] = result
    print(json.dumps(out))


if __name__ == "__main__":
    main()
