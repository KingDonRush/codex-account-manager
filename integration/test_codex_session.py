import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from codex_session import SessionBusy, session_lease


class SessionLeaseTests(unittest.TestCase):
    def test_competing_owner_is_rejected_and_release_keeps_inode(self):
        with tempfile.TemporaryDirectory(prefix="codex-lease-test-") as directory:
            home = Path(directory)
            with session_lease(home, timeout=0):
                inode = (home / "auth-switch.lock").stat().st_ino
                with self.assertRaises(SessionBusy):
                    with session_lease(home, timeout=0):
                        self.fail("two exclusive owners")
            with session_lease(home, timeout=0):
                self.assertEqual((home / "auth-switch.lock").stat().st_ino, inode)

    def test_exception_does_not_leave_a_lock(self):
        with tempfile.TemporaryDirectory(prefix="codex-lease-test-") as directory:
            home = Path(directory)
            with self.assertRaises(ValueError):
                with session_lease(home):
                    raise ValueError("fixture")
            with session_lease(home, timeout=0):
                self.assertTrue((home / "auth-switch.lock").is_file())

    def test_lease_blocks_an_independent_process(self):
        with tempfile.TemporaryDirectory(prefix="codex-lease-test-") as directory:
            with session_lease(Path(directory)):
                child = subprocess.run([
                    sys.executable, "-B", "-c",
                    "from pathlib import Path; from codex_session import session_lease\n"
                    "with session_lease(Path(__import__('sys').argv[1]), timeout=0): pass",
                    directory,
                ], cwd=Path(__file__).parent, capture_output=True, timeout=3)
                self.assertNotEqual(child.returncode, 0)
                self.assertIn(b"SessionBusy", child.stderr)


if __name__ == "__main__":
    unittest.main()
