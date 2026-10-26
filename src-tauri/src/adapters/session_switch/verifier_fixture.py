#!/usr/bin/python3
"""Synthetic JSONL boundary: never contacts a server or reads the user's home."""
import json
import os
from pathlib import Path
import sys
import time

root = Path(__file__).parent
config = json.loads((root / "scenario.json").read_text())
home = Path(os.environ["CODEX_HOME"])
auth_path = home / "auth.json"
external = False
access = None


def record(method, refresh=False):
    with (root / "requests.jsonl").open("a") as output:
        output.write(json.dumps({
            "method": method, "external": external, "refresh": refresh,
            "stopped": (root / "desktop-stopped").exists(),
            "pid": os.getpid(),
        }) + "\n")


def answer(request, result=None, error=None):
    message = {"id": request["id"]}
    message["error" if error else "result"] = error if error else result
    print(json.dumps(message), flush=True)


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if "id" not in request:
        continue
    params = request.get("params") or {}
    if method == "initialize":
        record(method)
        answer(request, {})
    elif method == "account/login/start":
        assert params["type"] == "chatgptAuthTokens"
        assert set(params) == {"type", "accessToken", "chatgptAccountId"}
        assert params["chatgptAccountId"] == "synthetic-account"
        assert params["accessToken"].startswith("synthetic-")
        external = True
        access = params["accessToken"]
        record(method)
        answer(request, {})
    elif method == "account/read":
        refresh = params.get("refreshToken", False)
        record(method, refresh)
        if config.get("blocked_read"):
            time.sleep(30)
        if config.get("network_failure"):
            answer(request, error={"code": -32000, "message": "synthetic network unavailable"})
            continue
        if config.get("access_refresh_rejected"):
            answer(request, error={"code": 401, "message": "invalid_grant"})
            continue
        if external and config.get("access_unauthorized"):
            answer(request, error={"code": 401, "message": "401 Unauthorized access token expired"})
            continue
        if refresh:
            assert not external, "access-only clients must never refresh"
            if config.get("requires_stop", True):
                assert (root / "desktop-stopped").exists(), "refresh before releasing Desktop"
            if config.get("refresh_rejected"):
                answer(request, error={"code": 401, "message": "refresh_token_reused"})
                continue
            document = json.loads(auth_path.read_text())
            assert document["tokens"]["account_id"] == "synthetic-account"
            if config.get("expected_generation"):
                assert document["tokens"]["generation"] == config["expected_generation"], "old generation replayed"
            document["tokens"]["access_token"] = "synthetic-access-renewed"
            document["tokens"]["refresh_token"] = "synthetic-refresh-renewed"
            document["tokens"]["generation"] = "renewed"
            auth_path.write_text(json.dumps(document))
            (root / "rotated").touch()
        if external:
            healthy = config.get("healthy", False) or access == "synthetic-access-renewed"
        else:
            healthy = config.get("healthy", False) or refresh
        account = {"type": "chatgpt", "email": "synthetic@example.test", "planType": "plus"} if healthy and not config.get("still_null") else None
        answer(request, {"account": account, "requiresOpenaiAuth": True})
    elif method == "account/rateLimits/read":
        record(method)
        if config.get("quota_failure"):
            answer(request, error={"code": -32000, "message": "synthetic quota unavailable"})
        else:
            answer(request, {"rateLimits": {"primary": {"usedPercent": 30, "windowDurationMins": 300}}})
    else:
        raise AssertionError("unexpected RPC method")
