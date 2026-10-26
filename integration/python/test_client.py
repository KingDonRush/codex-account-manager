import json
from pathlib import Path
import sys
import unittest
from codex_account_manager import Client,AsyncClient,ManagerError

BRIDGE=[sys.executable,str(Path(__file__).with_name("fake_bridge.py"))]
class ClientTests(unittest.TestCase):
    def test_authorization_changes_keep_connected_application_identity(self):
        with Client(BRIDGE) as client:
            client.connect("App", "app")
            client.authorize("App", "app", scope={"permissions":["manage_groups"]})
            requests=client.request("fixture/requests")
            self.assertEqual(requests[1]["params"]["applicationId"], "application")
            self.assertEqual(requests[1]["params"]["requestedScope"]["permissions"], ["manage_groups"])
            self.assertEqual(client.application_id, "application")
    def test_direct_execution_and_harness_do_not_send_a_binding_id(self):
        with Client(BRIDGE) as client:
            client.execute("synthetic", cwd=Path.cwd(), account_id="approved")
            with client.open_session(cwd=Path.cwd(), group_id="approved-group"):
                pass
            requests=client.request("fixture/requests")
            submitted=[request for request in requests if request["method"] in {"execution/start","session/open"}]
            self.assertEqual(len(submitted),2)
            self.assertTrue(all("bindingId" not in request["params"] for request in submitted))
            self.assertEqual(submitted[0]["params"]["accountId"],"approved")
            self.assertEqual(submitted[1]["params"]["groupId"],"approved-group")
    def test_connection_metadata_and_jsonl_use_only_pipe(self):
        with Client(BRIDGE) as client:
            result=client.connect("App","app")
            self.assertIsInstance(result,dict)
            self.assertNotIn("signingKey",json.dumps(result))
            renewed=client.authorize("App","app",application_id="application",scope={"permissions":["manage_groups"]})
            self.assertIsInstance(renewed,dict)
            self.assertNotIn("signingKey",json.dumps(renewed))
            job=client.exec("binding","private prompt",cwd=Path.cwd())
            self.assertEqual(job["id"],"job")
            self.assertEqual(list(client.events("job")),[{"type":"turn.completed"}])
        self.assertIsNotNone(client._process.returncode)
    def test_typed_error_does_not_retry_in_another_account(self):
        with Client(BRIDGE) as client:
            with self.assertRaises(ManagerError) as raised:
                client.request("fail")
            self.assertEqual(raised.exception.code,"quota_exhausted")
    def test_close_is_idempotent_and_rejects_future_requests(self):
        client=Client(BRIDGE);client.close();client.close()
        with self.assertRaises(ManagerError):client.request("catalog/read")

    def test_harness_routes_host_requests_without_implicit_approval(self):
        with Client(BRIDGE) as client:
            with client.session("binding",cwd=Path.cwd()) as session:
                result=session.rpc("thread/start",{"sandboxPolicy":{"type":"readOnly"}})
                self.assertEqual(result["thread"]["id"],"owned")
                requests=[]
                def decide(request):
                    requests.append(request)
                    return {"decision":"decline"}
                result=session.rpc("turn/start",{"threadId":"owned"},host_handler=decide)
                self.assertEqual(result["hostResult"],{"decision":"decline"})
                self.assertEqual(len(requests),1)
                self.assertEqual(session.metadata["accountId"],"pinned-account")
            self.assertTrue(list(session.events()))

    def test_missing_host_handler_closes_session_instead_of_approving(self):
        with Client(BRIDGE) as client:
            session=client.session("binding",cwd=Path.cwd())
            with self.assertRaises(ManagerError) as raised:session.rpc("turn/start",{})
            self.assertEqual(raised.exception.code,"host_request_unhandled")

class AsyncClientTests(unittest.IsolatedAsyncioTestCase):
    async def test_async_direct_routes_keep_exact_account_and_group_options(self):
        async with AsyncClient(BRIDGE) as client:
            await client.execute("synthetic",cwd=Path.cwd(),group_id="approved-group")
            async with await client.open_session(cwd=Path.cwd(),account_id="approved"):
                pass
            requests=await client.request("fixture/requests")
            submitted=[request for request in requests if request["method"] in {"execution/start","session/open"}]
            self.assertTrue(all("bindingId" not in request["params"] for request in submitted))
            self.assertEqual(submitted[0]["params"]["groupId"],"approved-group")
            self.assertEqual(submitted[1]["params"]["accountId"],"approved")
    async def test_async_harness_and_exec_are_available(self):
        async with AsyncClient(BRIDGE) as client:
            await client.connect("fixture","fixture")
            job=await client.exec("binding","synthetic",cwd=Path.cwd())
            self.assertEqual([event async for event in client.events(job["id"])],[{"type":"turn.completed"}])
            async with await client.session("binding",cwd=Path.cwd()) as session:
                result=await session.rpc("thread/start",{})
                self.assertEqual(result["thread"]["id"],"owned")

if __name__=="__main__":unittest.main()
