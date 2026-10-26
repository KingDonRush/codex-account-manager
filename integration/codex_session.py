"""Cooperative auth lease v1. No credentials are read, copied or exported.

Acquire before launching Codex; release only after its whole process tree and
pipe readers have finished. An isolated working directory is not an auth home.
"""
from contextlib import contextmanager
import os
from pathlib import Path
import time


class SessionBusy(TimeoutError):
    """Another integration is using/changing the global Codex session."""


def _try_lock(descriptor: int) -> None:
    if os.name == "posix":
        import fcntl
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return
    if os.name == "nt":
        import msvcrt
        os.lseek(descriptor, 0, os.SEEK_SET)
        msvcrt.locking(descriptor, msvcrt.LK_NBLCK, 1)
        return
    raise NotImplementedError("No session lock backend for this OS")


def _unlock(descriptor: int) -> None:
    if os.name == "posix":
        import fcntl
        fcntl.flock(descriptor, fcntl.LOCK_UN)
    else:
        import msvcrt
        os.lseek(descriptor, 0, os.SEEK_SET)
        msvcrt.locking(descriptor, msvcrt.LK_UNLCK, 1)


@contextmanager
def session_lease(codex_home: Path | None = None, *, timeout: float = 30):
    """Keep the stable inode locked for the full authenticated invocation.

    Cancellation of an asyncio caller must not abandon a background acquisition
    thread: acquire in bounded attempts or use an application-owned executor.
    Never hold this lease while requesting account activation (a deadlock).
    """
    directory = codex_home or Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
    directory = directory.resolve(strict=True)
    lock_path = directory / "auth-switch.lock"
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(lock_path, flags, 0o600)
    acquired = False
    deadline = time.monotonic() + max(0, timeout)
    try:
        while not acquired:
            try:
                _try_lock(descriptor)
                acquired = True
            except (BlockingIOError, PermissionError):
                if time.monotonic() >= deadline:
                    raise SessionBusy("Codex session busy; no CLI was launched") from None
                time.sleep(min(0.025, max(0, deadline - time.monotonic())))
        yield
    finally:
        try:
            if acquired:
                _unlock(descriptor)
        finally:
            os.close(descriptor)
