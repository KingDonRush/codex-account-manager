#!/usr/bin/python3
"""Synthetic OAuth/App Server boundary: no provider calls, model or user home."""
import json
import os
from pathlib import Path
import sys

root = Path(__file__).parent
scenario = json.loads((root / "scenario.json").read_text())
home = Path(os.environ["CODEX_HOME"])
with (root / "login-processes.jsonl").open("a") as file:
    file.write(json.dumps({"pid": os.getpid(), "home": str(home)}) + "\n")

def send(value):
    print(json.dumps(value), flush=True)

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        send({"id": request["id"], "result": {}})
    elif method == "account/login/start":
        assert request["params"]["type"] == "chatgpt"
        send({"id": request["id"], "result": {"loginId": "synthetic-login", "authUrl": "https://example.test/synthetic-login"}})
        if scenario.get("success"):
            doc = {"auth_mode": "chatgpt", "tokens": {"account_id": "synthetic-remote", "access_token": "synthetic-access", "refresh_token": "synthetic-refresh"}}
            (home / "auth.json").write_text(json.dumps(doc))
            send({"method": "account/login/completed", "params": {"loginId": "synthetic-login", "success": True}})
            send({"method": "account/updated", "params": {"authMode": "chatgpt"}})
    elif method == "account/read":
        send({"id": request["id"], "result": {"account": {"type": "chatgpt", "email": "synthetic@example.test", "planType": "plus"}}})
    elif method == "account/rateLimits/read":
        send({"id": request["id"], "result": {"rateLimits": {"primary": {"usedPercent": 20}}}})
    elif method == "account/login/cancel":
        send({"id": request["id"], "result": {}})
    else:
        raise AssertionError("unexpected RPC operation")
