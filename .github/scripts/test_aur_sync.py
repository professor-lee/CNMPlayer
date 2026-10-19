#!/usr/bin/env python3
"""Offline integration checks using real Git repositories and makepkg metadata."""

import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / ".github/scripts/aur_sync.sh"


class AurSyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cnmplayer-aur-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.remotes = self.root / "remotes"
        self.remotes.mkdir()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.assets = self.root / "assets"
        self.assets.mkdir()
        for name in ("source", "amd64", "aarch64"):
            (self.assets / name).write_bytes(f"distinct {name} release content\n".encode())
        curl = self.bin / "curl"
        curl.write_text(
            "#!/usr/bin/env python3\n"
            "import os, pathlib, sys\n"
            "args = sys.argv[1:]\n"
            "url = next(arg for arg in args if arg.startswith('https://'))\n"
            "name = 'source' if '/archive/' in url else ('amd64' if '_amd64.' in url else 'aarch64')\n"
            "if os.environ.get('FAIL_ASSET') == name: sys.exit(22)\n"
            "pathlib.Path(args[args.index('-o') + 1]).write_bytes((pathlib.Path(os.environ['TEST_ASSETS']) / name).read_bytes())\n"
        )
        curl.chmod(0o755)
        config = self.root / "gitconfig"
        config.write_text(
            '[user]\n name = AUR test\n email = aur-test@example.invalid\n'
            f'[url "{self.remotes}/"]\n'
            ' insteadOf = https://aur.archlinux.org/\n'
            ' insteadOf = ssh://aur@aur.archlinux.org/\n'
            '[protocol "file"]\n allow = always\n'
        )
        self.env = os.environ.copy()
        # Do not inherit credentials, injected Git config, or author identity.
        for key in tuple(self.env):
            if key.startswith("GIT_"):
                del self.env[key]
        self.env.update(
            GIT_CONFIG_GLOBAL=str(config),
            GIT_CONFIG_NOSYSTEM="1",
            GIT_TERMINAL_PROMPT="0",
            GIT_ALLOW_PROTOCOL="file",
            PATH=f"{self.bin}:{os.environ['PATH']}",
            TEST_ASSETS=str(self.assets),
        )

    def run_command(self, *args, cwd=None, check=True):
        result = subprocess.run(args, cwd=cwd, env=self.env, text=True, capture_output=True)
        if check and result.returncode:
            self.fail(f"{args}: {result.stdout}\n{result.stderr}")
        return result

    def sync(self, *args, check=True):
        return self.run_command("bash", str(SCRIPT), *args, check=check)

    def seed(self, package, recipe=None):
        remote = self.remotes / f"{package}.git"
        seed = self.root / f"seed-{package}"
        seed.mkdir()
        self.run_command("git", "init", "--bare", "--initial-branch=master", str(remote))
        self.run_command("git", "init", "--initial-branch=master", str(seed))
        if recipe is None:
            recipe = f"pkgname={package}\npkgver=0.1.0\npkgrel=1\narch=('x86_64')\npkgdesc='Old recipe'\n"
        (seed / "PKGBUILD").write_text(recipe)
        metadata = self.run_command("makepkg", "--printsrcinfo", cwd=seed).stdout
        (seed / ".SRCINFO").write_text(metadata)
        self.run_command("git", "add", "PKGBUILD", ".SRCINFO", cwd=seed)
        self.run_command("git", "commit", "-m", "Initial packaging", cwd=seed)
        self.run_command("git", "push", str(remote), "master", cwd=seed)
        return remote

    def head(self, remote):
        return self.run_command("git", "--git-dir", str(remote), "rev-parse", "master").stdout.strip()

    def content(self, remote, name):
        return self.run_command("git", "--git-dir", str(remote), "show", f"master:{name}").stdout

    def test_release_publishes_verified_metadata_and_second_run_is_noop(self):
        remotes = [self.seed(name) for name in ("cnmplayer", "cnmplayer-bin")]
        self.sync("release", "v1.2.3", "false")
        source = self.content(remotes[0], ".SRCINFO")
        binary = self.content(remotes[1], ".SRCINFO")
        self.assertIn("\tpkgver = 1.2.3\n", source)
        for asset, metadata in (("source", source), ("amd64", binary), ("aarch64", binary)):
            checksum = hashlib.sha256((self.assets / asset).read_bytes()).hexdigest()
            self.assertIn(checksum, metadata)
        self.assertIn("/v1.2.3.tar.gz", source)
        self.assertIn("CNMPlayer_v1.2.3_linux_amd64.tar.xz", binary)
        self.assertIn("CNMPlayer_v1.2.3_linux_aarch64.tar.xz", binary)
        heads = [self.head(remote) for remote in remotes]
        self.sync("release", "v1.2.3", "false")
        self.assertEqual(heads, [self.head(remote) for remote in remotes])

    def test_dry_run_preserves_both_remote_histories(self):
        remotes = [self.seed(name) for name in ("cnmplayer", "cnmplayer-bin")]
        heads = [self.head(remote) for remote in remotes]
        self.sync("release", "v1.2.3", "true")
        self.assertEqual(heads, [self.head(remote) for remote in remotes])

    def test_missing_architecture_asset_does_not_publish_either_package(self):
        remotes = [self.seed(name) for name in ("cnmplayer", "cnmplayer-bin")]
        heads = [self.head(remote) for remote in remotes]
        self.env["FAIL_ASSET"] = "aarch64"
        result = self.sync("release", "v1.2.3", "false", check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(heads, [self.head(remote) for remote in remotes])

    def test_git_recipe_updates_once_and_identical_recipe_is_noop(self):
        remote = self.seed("cnmplayer-git")
        before = self.head(remote)
        self.sync("git", "false")
        updated = self.head(remote)
        self.assertNotEqual(before, updated)
        self.assertEqual(
            self.content(remote, "PKGBUILD"),
            (ROOT / "packaging/aur/cnmplayer-git/PKGBUILD").read_text(),
        )
        self.sync("git", "false")
        self.assertEqual(updated, self.head(remote))

    def test_git_dry_run_does_not_publish_recipe_changes(self):
        remote = self.seed("cnmplayer-git")
        before = self.head(remote)
        self.sync("git", "true")
        self.assertEqual(before, self.head(remote))

    def test_stale_metadata_is_repaired_without_recipe_changes(self):
        recipe = (ROOT / "packaging/aur/cnmplayer-git/PKGBUILD").read_text()
        remote = self.seed("cnmplayer-git", recipe)
        seed = self.root / "seed-cnmplayer-git"
        (seed / ".SRCINFO").write_text("pkgbase = cnmplayer-git\npkgver = stale\n")
        self.run_command("git", "commit", "-am", "Stale metadata", cwd=seed)
        self.run_command("git", "push", str(remote), "master", cwd=seed)
        before = self.head(remote)
        self.sync("git", "false")
        self.assertNotEqual(before, self.head(remote))
        self.assertEqual(recipe, self.content(remote, "PKGBUILD"))
        expected = self.run_command("makepkg", "--printsrcinfo", cwd=seed).stdout
        self.assertEqual(expected, self.content(remote, ".SRCINFO"))

    def test_invalid_inputs_fail_before_publication(self):
        for args in (("release", "../../bad", "false"), ("git", "yes"), ("release", "v1.2.3")):
            with self.subTest(args=args):
                self.assertNotEqual(self.sync(*args, check=False).returncode, 0)


if __name__ == "__main__":
    unittest.main()
