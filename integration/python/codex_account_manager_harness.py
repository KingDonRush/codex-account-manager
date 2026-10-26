"""Pinned App Server transport. Tool/approval decisions belong to the calling app."""
from __future__ import annotations

import asyncio
import json
import threading
import time
from collections import deque
from collections.abc import AsyncIterator, Callable, Iterator
from typing import TYPE_CHECKING
from codex_account_manager import ManagerError

if TYPE_CHECKING:
    from codex_account_manager import Client

HostHandler = Callable[[dict[str, object]], object]
TERMINAL = {"closed", "failed", "interrupted"}

class ServerSession:
    """One event reader per logical session; closing never touches Desktop chats."""
    def __init__(self, client: Client, metadata: dict[str, object]):
        self.client = client
        self.metadata = metadata
        self.id = str(metadata["id"])
        self._offset = int(str(metadata.get("eventsOffset", 0)))
        self._tail = ""
        self._rpc = 0
        self._reading = threading.Lock()
        self._buffer: deque[dict[str, object]] = deque()

    def send(self, message: dict[str, object]) -> object:
        return self.client.request("session/send", {"sessionId": self.id, "message": message})

    def respond(self, request_id: object, result: object) -> object:
        return self.send({"id": request_id, "result": result})

    def rpc(self, method: str, params: object = None, *, host_handler: HostHandler | None = None, timeout: float = 120) -> object:
        if not self._reading.acquire(blocking=False):
            raise ManagerError("reader_busy", "Use one event reader per session")
        try:
            self._rpc += 1
            request_id = self._rpc
            self.send({"id": request_id, "method": method, "params": params or {}})
            deadline = time.monotonic() + timeout
            for message in self._messages(deadline):
                if "method" not in message and message.get("id") == request_id:
                    failure = message.get("error")
                    if isinstance(failure, dict):
                        raise ManagerError("rpc_error", str(failure.get("message", "App Server request failed")))
                    return message.get("result")
                if "method" in message and "id" in message:
                    if host_handler is None:
                        self.close()
                        raise ManagerError("host_request_unhandled", "Provide your application's tool/approval handler")
                    self.respond(message["id"], host_handler(message))
                else:
                    if len(self._buffer) >= 256:
                        self.close()
                        raise ManagerError("event_buffer_full", "Consume notifications before continuing")
                    self._buffer.append(message)
            raise ManagerError("session_closed", "Session ended before its response")
        finally:
            self._reading.release()

    def events(self, *, timeout: float = 120) -> Iterator[dict[str, object]]:
        if not self._reading.acquire(blocking=False):
            raise ManagerError("reader_busy", "Use one event reader per session")
        try:
            while self._buffer:
                yield self._buffer.popleft()
            yield from self._messages(time.monotonic() + timeout)
        finally:
            self._reading.release()

    def _messages(self, deadline: float) -> Iterator[dict[str, object]]:
        while time.monotonic() < deadline:
            page = self.client.request("session/events", {"sessionId": self.id, "offset": self._offset})
            if not isinstance(page, dict):
                raise ManagerError("protocol_error", "Session event page missing")
            self._offset = int(str(page["nextOffset"]))
            self._tail += str(page["chunk"])
            while "\n" in self._tail:
                line, self._tail = self._tail.split("\n", 1)
                message = json.loads(line)
                if not isinstance(message, dict):
                    raise ManagerError("protocol_error", "App Server frame is not an object")
                yield message
            if len(self._tail.encode("utf-8")) > 262144:
                raise ManagerError("frame_too_large", "App Server frame exceeds limit")
            if not page["chunk"] and page.get("state") in TERMINAL:
                return
            time.sleep(0.05)
        self.close()
        raise ManagerError("timeout", "No automatic retry: the operation may have started")

    def close(self) -> object:
        return self.client.request("session/close", {"sessionId": self.id})

    def __enter__(self) -> ServerSession:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

class AsyncServerSession:
    """A sync host handler runs in a worker, never in the asyncio event loop."""
    def __init__(self, session: ServerSession):
        self.session = session
        self.id = session.id
        self.metadata = session.metadata

    async def rpc(self, method: str, params: object = None, *, host_handler: HostHandler | None = None, timeout: float = 120) -> object:
        try:
            return await asyncio.to_thread(self.session.rpc, method, params, host_handler=host_handler, timeout=timeout)
        except asyncio.CancelledError:
            await asyncio.to_thread(self.session.client.close)
            raise

    async def respond(self, request_id: object, result: object) -> object:
        return await asyncio.to_thread(self.session.respond, request_id, result)

    async def events(self, *, timeout: float = 120) -> AsyncIterator[dict[str, object]]:
        iterator = self.session.events(timeout=timeout)
        try:
            while True:
                step = asyncio.create_task(asyncio.to_thread(next, iterator, None))
                try:
                    message = await asyncio.shield(step)
                except asyncio.CancelledError:
                    await asyncio.to_thread(self.session.client.close)
                    await asyncio.gather(step, return_exceptions=True)
                    raise
                if message is None:
                    return
                yield message
        finally:
            iterator.close()

    async def close(self) -> object:
        return await asyncio.to_thread(self.session.close)

    async def __aenter__(self) -> AsyncServerSession:
        return self

    async def __aexit__(self, *args: object) -> None:
        await self.close()
