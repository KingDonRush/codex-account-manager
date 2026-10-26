"""Deliberate fixture: no Codex/coffer/network/model calls."""
import json
import sys
events = b""
pending = None
state = "ready"
requests = []
def emit(message):
    global events
    events += (json.dumps(message) + "\n").encode("utf-8")

for line in sys.stdin:
    request=json.loads(line)
    method=request["method"]
    error=None
    requests.append(request)
    if method in {"client/connect","client/authorize"}:result={"authenticated":True,"grantId":"grant","applicationId":"application"}
    elif method=="execution/start":result={"id":"job","state":"queued"}
    elif method=="execution/events":
        offset=request["params"]["offset"]
        result={"chunk":'{"type":"turn.completed"}\n' if offset==0 else "","nextOffset":26,"state":"completed"}
    elif method=="session/open":
        result={"id":"session","accountId":"pinned-account","state":"ready","eventsOffset":len(events)}
    elif method=="session/send":
        message=request["params"]["message"]
        if message.get("method")=="thread/start":
            emit({"method":"thread/started","params":{"thread":{"id":"owned"}}})
            emit({"id":message["id"],"result":{"thread":{"id":"owned"}}})
        elif message.get("method")=="turn/start":
            pending=message
            emit({"id":message["id"],"method":"item/tool/requestUserInput","params":{"fixture":True}})
        elif "result" in message and pending:
            emit({"id":pending["id"],"result":{"accepted":True,"hostResult":message["result"]}})
            emit({"method":"turn/completed","params":{"turn":{"status":"completed"}}})
            pending=None
        result={"accepted":True}
    elif method=="session/events":
        offset=request["params"]["offset"]
        result={"chunk":events[offset:].decode("utf-8"),"nextOffset":len(events),"state":state}
    elif method=="session/close":state="closed";result={"closing":True}
    elif method=="fail":result=None;error={"code":"quota_exhausted","message":"Sem saldo"}
    elif method=="fixture/requests":result=requests[:-1]
    else:result={}
    print(json.dumps({"id":request["id"],"result":result,"error":error}),flush=True)
