"""An OCI round-trip must retain Cargo fingerprint timestamps and file modes.

GitHub Actions artifact ZIPs normalize raw file metadata; our archived GHCR
payload is explicitly PAX tar + zstd before reaching upload-artifact.
"""

import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


class CargoPackageArchiveContract(unittest.TestCase):
    def test_pax_zstd_round_trip_keeps_nsec_mtime_mode_and_symlinks(self):
        with tempfile.TemporaryDirectory(prefix="muninn-ghcr-archive-") as root:
            work = Path(root)
            seed = work / "seed"
            restored = work / "restored"
            (seed / "target" / "debug").mkdir(parents=True)
            restored.mkdir()
            executable = seed / "target" / "debug" / "crate-fixture"
            executable.write_bytes(b"valid rust compilation")
            executable.chmod(0o751)
            mtime_ns = 1_700_000_000_123_456_789
            os.utime(executable, ns=(mtime_ns, mtime_ns))
            (seed / "target" / "debug" / "linked-fixture").symlink_to("crate-fixture")

            archive = work / "snapshot.tar.zst"
            subprocess.run([
                "tar", "--format=posix", "--use-compress-program=zstd -1 -T2",
                "-cf", str(archive), "-C", str(seed), ".",
            ], check=True, capture_output=True)
            subprocess.run([
                "tar", "--use-compress-program=zstd -d -T2",
                "-xf", str(archive), "-C", str(restored),
            ], check=True, capture_output=True)

            copied = restored / "target" / "debug" / "crate-fixture"
            self.assertEqual(copied.read_bytes(), executable.read_bytes())
            self.assertEqual(copied.stat().st_mtime_ns, mtime_ns)
            self.assertEqual(stat.S_IMODE(copied.stat().st_mode), 0o751)
            self.assertEqual((copied.parent / "linked-fixture").readlink(), Path("crate-fixture"))


if __name__ == "__main__":
    unittest.main()
