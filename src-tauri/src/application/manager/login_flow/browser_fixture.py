#!/usr/bin/python3
"""Private CDP process fixture, never a real browser/profile or network client."""
import json
import os
from pathlib import Path
import time

root = Path(__file__).parent
scenario = json.loads((root / "scenario.json").read_text())
targets = {}
counter = 0
with (root / "browser-processes.jsonl").open("a") as file:
    file.write(json.dumps({"pid": os.getpid()}) + "\n")
reader = os.fdopen(os.dup(3), "rb", buffering=0)
writer = os.fdopen(os.dup(4), "wb", buffering=0)
while True:
    frame = bytearray()
    while True:
        value = reader.read(1)
        if not value:
            raise SystemExit(0)
        if value == b"\0":
            break
        frame.extend(value)
    request = json.loads(frame)
    scenario = json.loads((root / "scenario.json").read_text())
    method = request["method"]
    with (root / "browser-requests.jsonl").open("a") as file:
        file.write(json.dumps({"method": method, "targetId": request.get("params", {}).get("targetId"), "pid": os.getpid()}) + "\n")
    if method == "Target.createTarget":
        counter += 1
        if scenario.get("fail_create_number") == counter:
            writer.write(json.dumps({"id": request["id"], "error": {"code": -32000, "message": "synthetic target rejected"}}).encode() + b"\0")
            continue
        target_id = f"synthetic-target-{counter}"
        targets[target_id] = {"targetId": target_id, "type": "page"}
        result = {"targetId": target_id}
    elif method == "Target.getTargets":
        result = {"targetInfos": list(targets.values())}
    elif method == "Target.closeTarget":
        if scenario.get("refuse_close") or scenario.get("refuse_target") == request["params"]["targetId"]:
            result = {"success": False}
        else:
            targets.pop(request["params"]["targetId"], None)
            result = {"success": True}
    else:
        raise AssertionError("unexpected CDP operation")
    writer.write(json.dumps({"id": request["id"], "result": result}).encode() + b"\0")
