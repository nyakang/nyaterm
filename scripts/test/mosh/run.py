#!/usr/bin/env python3
"""Run the opt-in Mosh transport test against a disposable local Docker server."""
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[3]

def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def main():
    run("docker", "build", "-t", "nyaterm-mosh-test", str(Path(__file__).parent))
    name = "nyaterm-mosh-test-" + uuid.uuid4().hex[:12]
    with tempfile.TemporaryDirectory(prefix="nyaterm-mosh-test-") as directory:
        key = Path(directory) / "id_ed25519"
        run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key))
        try:
            run("docker", "run", "-d", "--name", name, "--cap-add", "NET_ADMIN",
                "-v", f"{key}.pub:/root/.ssh/authorized_keys:ro", "nyaterm-mosh-test",
                stdout=subprocess.DEVNULL)
            host = subprocess.check_output(["docker", "inspect", "--format",
                "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}", name], text=True).strip()
            for _ in range(100):
                try:
                    with socket.create_connection((host, 22), timeout=0.2):
                        break
                except OSError:
                    time.sleep(0.1)
            else:
                raise RuntimeError("SSH fixture did not become ready")
            env = dict(os.environ, NYATERM_MOSH_TEST_HOST=host,
                       NYATERM_MOSH_TEST_KEY=str(key), NYATERM_MOSH_TEST_CONTAINER=name)
            run("cargo", "test", "-p", "nyaterm-transport", "--test", "mosh_live", "--",
                "--ignored", cwd=ROOT, env=env)
        finally:
            subprocess.run(["docker", "rm", "-f", name], check=False, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
