#!/usr/bin/python3
"""Synthetic exec boundary. No network, model or user coffer access."""
import json
import os
from pathlib import Path
import sys
import time

root = Path(__file__).parent
scenario = json.loads((root / "scenario.json").read_text())
home = Path(os.environ["CODEX_HOME"])
assert home.name.startswith("codex-account-manager-")
assert "exec" == sys.argv[1]
assert 'cli_auth_credentials_store="file"' in sys.argv
assert 'forced_login_method="chatgpt"' in sys.argv
assert sys.stdin.read() == "synthetic prompt"
document = json.loads((home / "auth.json").read_text())
assert document["tokens"]["account_id"] == "synthetic-remote"
if scenario.get("external"):
    assert document["auth_mode"] == "chatgptAuthTokens"
    assert document["tokens"]["refresh_token"] == ""
    assert "generation" not in document["tokens"]
    assert "synthetic-refresh" not in json.dumps(document)
    with (root / "processes.jsonl").open("a") as report:
        report.write(json.dumps({"pid": os.getpid(), "home": str(home), "generation": "external"}) + "\n")
    print(json.dumps({"type": "turn.completed"}), flush=True)
    sys.exit(0)
with (root / "processes.jsonl").open("a") as report:
    report.write(json.dumps({"pid": os.getpid(), "home": str(home), "generation": document["tokens"]["generation"]}) + "\n")
document["tokens"]["access_token"] = "synthetic-access-renewed"
document["tokens"]["refresh_token"] = "synthetic-refresh-renewed"
document["tokens"]["generation"] = "renewed"
(home / "auth.json").write_text(json.dumps(document))
if scenario.get("coffer_failure"):
    (root / "coffer-unavailable").touch()
print(json.dumps({"type": "item.completed", "item": {"text": "synthetic result"}}), flush=True)
(root / "rotated").touch()
if scenario.get("cancel"):
    time.sleep(30)
print(json.dumps({"type": "turn.completed"}), flush=True)
sys.exit(scenario.get("exit_code", 0))
