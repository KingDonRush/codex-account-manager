"""The manager owns credentials/processes; the application owns its harness."""
from __future__ import annotations

import asyncio
import json
import queue
import subprocess
import sys
import threading
import time
from collections.abc import AsyncIterator, Iterator
from pathlib import Path


class ManagerError(RuntimeError):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code


class Client:
    """One local connection; close it deterministically with a context manager."""

    def __init__(self, executable: str | list[str] = "codex-account-manager-cli"):
        self._process = subprocess.Popen(
            [executable] if isinstance(executable,str) else executable, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=None, bufsize=0,
            creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0,
        )
        self._responses: queue.Queue[object] = queue.Queue(maxsize=32)
        self._lock = threading.Lock()
        self._id = 0
        self._closed = False
        self.application_id: str | None = None
        self._reader = threading.Thread(target=self._read, name="manager-client", daemon=True)
        self._reader.start()

    def _read(self) -> None:
        output = self._process.stdout
        if output is None:
            return
        try:
            while line := output.readline(262145):
                if len(line) > 262144 or not line.endswith(b'\n'):
                    raise ManagerError("frame_too_large", "Manager response exceeds limit")
                self._responses.put(json.loads(line), timeout=5)
        except (ValueError, queue.Full, ManagerError) as error:
            try:
                self._responses.put_nowait(error)
            except queue.Full:
                pass
        finally:
            try:
                self._responses.put_nowait(ManagerError("disconnected", "Local manager connection closed"))
            except queue.Full:
                pass

    def request(self, method: str, params: object = None, *, timeout: float = 180) -> object:
        with self._lock:
            if self._closed or self._process.stdin is None:
                raise ManagerError("closed", "Client is closed")
            self._id += 1
            encoded = json.dumps({"id": self._id, "method": method, "params": params or {}}, ensure_ascii=False)
            if len(encoded.encode("utf-8")) > 262144:
                raise ManagerError("frame_too_large", "Request exceeds limit")
            self._process.stdin.write((encoded + "\n").encode("utf-8"))
            self._process.stdin.flush()
            try:
                response = self._responses.get(timeout=timeout)
            except queue.Empty as error:
                self.close()
                raise ManagerError("timeout", "Request timed out; do not resubmit a potentially started execution") from error
            if isinstance(response, Exception):
                raise response
            if not isinstance(response, dict) or response.get("id") != self._id:
                self.close()
                raise ManagerError("protocol_error", "Response correlation does not match")
            failure = response.get("error")
            if isinstance(failure, dict):
                raise ManagerError(str(failure.get("code", "unknown")), str(failure.get("message", "Request failed")))
            return response.get("result")

    def connect(self, name: str, slug: str, *, application_id: str | None = None, scope: dict[str, object] | None = None) -> object:
        requested = {
            "accountIds": [], "groupIds": [], "permissions": ["read_metadata", "execute"],
            "globalIncludingFuture": False, "backgroundGroupIds": [], "expiresAt": None, "permanent": False,
        }
        if scope:
            requested.update(scope)
        return self._remember_application(self.request("client/connect", {"name": name, "slug": slug, "applicationId": application_id or self.application_id, "requestedScope": requested}))

    def _remember_application(self, receipt: object) -> object:
        if isinstance(receipt, dict) and receipt.get("authenticated") is True and isinstance(receipt.get("applicationId"), str):
            self.application_id = receipt["applicationId"]
        return receipt

    def exec(self, binding_id: str, prompt: str, *, cwd: str | Path, arguments: list[str] | None = None, account_id: str | None = None) -> dict[str, object]:
        """Compatibility for saved bindings; new applications use execute."""
        result = self.request("execution/start", {"bindingId": binding_id, "prompt": prompt,
            "cwd": str(Path(cwd).resolve()), "arguments": arguments or [], "accountId": account_id})
        if not isinstance(result, dict):
            raise ManagerError("protocol_error", "Execution metadata missing")
        return result

    def execute(self, prompt: str, *, cwd: str | Path, arguments: list[str] | None = None,
                account_id: str | None = None, group_id: str | None = None) -> dict[str, object]:
        """Use approved resources directly; selection is pinned before inference."""
        result = self.request("execution/start", {"prompt": prompt, "cwd": str(Path(cwd).resolve()),
            "arguments": arguments or [], "accountId": account_id, "groupId": group_id})
        if not isinstance(result, dict):
            raise ManagerError("protocol_error", "Execution metadata missing")
        return result

    def events(self, execution_id: str) -> Iterator[dict[str, object]]:
        offset = 0
        tail = ""
        while True:
            page = self.request("execution/events", {"executionId": execution_id, "offset": offset})
            if not isinstance(page, dict):
                raise ManagerError("protocol_error", "Event page missing")
            chunk = str(page.get("chunk", ""))
            offset = int(page.get("nextOffset", offset))
            tail += chunk
            while "\n" in tail:
                line, tail = tail.split("\n", 1)
                if line.strip():
                    event = json.loads(line)
                    if isinstance(event, dict):
                        yield event
            if not chunk and page.get("state") in {"completed", "failed", "cancelled", "interrupted"}:
                self.request("execution/ack", {"executionId": execution_id})
                return
            time.sleep(0.1)

    def cancel(self, execution_id: str) -> object:
        return self.request("execution/cancel", {"executionId": execution_id})

    def authorize(self, name: str, slug: str, *, application_id: str | None = None, scope: dict[str, object] | None = None) -> object:
        """Explicit human decision for renewal/expansion, never an automatic retry."""
        requested = {"accountIds": [], "groupIds": [], "permissions": ["read_metadata", "execute"], "globalIncludingFuture": False,
                     "backgroundGroupIds": [], "expiresAt": None, "permanent": False, **(scope or {})}
        return self._remember_application(self.request("client/authorize", {"name": name, "slug": slug, "applicationId": application_id or self.application_id, "requestedScope": requested}))

    def session(self, binding_id: str, *, cwd: str | Path, account_id: str | None = None, persist_history: bool = False):
        """Compatibility for saved bindings; new applications use open_session."""
        from codex_account_manager_harness import ServerSession
        metadata = self.request("session/open", {"bindingId": binding_id, "cwd": str(Path(cwd).resolve()), "accountId": account_id, "persistHistory": persist_history})
        if not isinstance(metadata, dict):
            raise ManagerError("protocol_error", "Session metadata missing")
        return ServerSession(self, metadata)

    def open_session(self, *, cwd: str | Path, account_id: str | None = None,
                     group_id: str | None = None, persist_history: bool = False):
        """Open an App Server with approved resources and no binding registration."""
        from codex_account_manager_harness import ServerSession
        metadata = self.request("session/open", {"cwd": str(Path(cwd).resolve()),
            "accountId": account_id, "groupId": group_id, "persistHistory": persist_history})
        if not isinstance(metadata, dict):
            raise ManagerError("protocol_error", "Session metadata missing")
        return ServerSession(self, metadata)

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        if self._process.stdin:
            self._process.stdin.close()
        try:
            self._process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self._process.terminate()
            try:
                self._process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self._process.kill()
                self._process.wait()
        self._reader.join(timeout=2)
        if self._process.stdout:
            self._process.stdout.close()

    def __enter__(self) -> Client:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()


class AsyncClient:
    """Async wrapper; cancellation closes the owned connection instead of abandoning a reader."""

    def __init__(self, executable: str | list[str] = "codex-account-manager-cli"):
        self.client = Client(executable)

    async def request(self, method: str, params: object = None, *, timeout: float = 180) -> object:
        try:
            return await asyncio.to_thread(self.client.request, method, params, timeout=timeout)
        except asyncio.CancelledError:
            await asyncio.to_thread(self.client.close)
            raise

    async def __aenter__(self) -> AsyncClient:
        return self

    async def authorize(self, name: str, slug: str, *, application_id: str | None = None, scope: dict[str, object] | None = None) -> object:
        try:
            return await asyncio.to_thread(self.client.authorize, name, slug, application_id=application_id, scope=scope)
        except asyncio.CancelledError:
            await asyncio.to_thread(self.client.close)
            raise

    async def __aexit__(self, *args: object) -> None:
        await asyncio.to_thread(self.client.close)

    async def connect(self, name: str, slug: str, *, application_id: str | None = None, scope: dict[str, object] | None = None) -> object:
        try:
            return await asyncio.to_thread(self.client.connect, name, slug, application_id=application_id, scope=scope)
        except asyncio.CancelledError:
            await asyncio.to_thread(self.client.close)
            raise

    async def exec(self, binding_id: str, prompt: str, *, cwd: str | Path, arguments: list[str] | None = None, account_id: str | None = None) -> dict[str, object]:
        try:
            return await asyncio.to_thread(self.client.exec, binding_id, prompt, cwd=cwd, arguments=arguments, account_id=account_id)
        except asyncio.CancelledError:
            await asyncio.to_thread(self.client.close)
            raise

    async def execute(self, prompt: str, *, cwd: str | Path, arguments: list[str] | None = None,
                      account_id: str | None = None, group_id: str | None = None) -> dict[str, object]:
        try:
            return await asyncio.to_thread(self.client.execute, prompt, cwd=cwd, arguments=arguments,
                                           account_id=account_id, group_id=group_id)
        except asyncio.CancelledError:
            await asyncio.to_thread(self.client.close)
            raise

    async def events(self, execution_id: str) -> AsyncIterator[dict[str, object]]:
        iterator = self.client.events(execution_id)
        try:
            while True:
                step = asyncio.create_task(asyncio.to_thread(next, iterator, None))
                try:
                    event = await asyncio.shield(step)
                except asyncio.CancelledError:
                    await asyncio.to_thread(self.client.close)
                    await asyncio.gather(step, return_exceptions=True)
                    raise
                if event is None:
                    return
                yield event
        finally:
            iterator.close()

    async def session(self, binding_id: str, *, cwd: str | Path, account_id: str | None = None, persist_history: bool = False):
        from codex_account_manager_harness import AsyncServerSession, ServerSession
        metadata = await self.request("session/open", {"bindingId": binding_id, "cwd": str(Path(cwd).resolve()), "accountId": account_id, "persistHistory": persist_history})
        if not isinstance(metadata, dict):
            raise ManagerError("protocol_error", "Session metadata missing")
        return AsyncServerSession(ServerSession(self.client, metadata))

    async def open_session(self, *, cwd: str | Path, account_id: str | None = None,
                           group_id: str | None = None, persist_history: bool = False):
        from codex_account_manager_harness import AsyncServerSession, ServerSession
        metadata = await self.request("session/open", {"cwd": str(Path(cwd).resolve()),
            "accountId": account_id, "groupId": group_id, "persistHistory": persist_history})
        if not isinstance(metadata, dict):
            raise ManagerError("protocol_error", "Session metadata missing")
        return AsyncServerSession(ServerSession(self.client, metadata))
