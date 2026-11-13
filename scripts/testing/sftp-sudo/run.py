#!/usr/bin/env python3
"""Disposable Linux integration matrix. No host accounts or sudo policy are changed."""
import json
import os
from pathlib import Path
import secrets
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
IMAGE = "nyaterm-sftp-sudo-fixture:local"


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def main():
    run("docker", "build", "-t", IMAGE, str(Path(__file__).parent))
    build = run("cargo", "test", "-p", "nyaterm-files", "--test", "sftp_sudo_live", "--no-run", "--message-format=json", cwd=ROOT, stdout=subprocess.PIPE)
    artifacts = [json.loads(line) for line in build.stdout.splitlines()]
    executable = next(item["executable"] for item in artifacts if item.get("executable") and item.get("target", {}).get("name") == "sftp_sudo_live")
    for case in ("password", "nopass", "denied", "requiretty", "internal", "missing", "limited"):
        container = run("docker", "run", "--rm", "-d", "-p", "127.0.0.1::22", "-e", f"NYATERM_SUDO_CASE={case}", IMAGE, capture_output=True).stdout.strip()
        try:
            password = secrets.token_urlsafe(24)
            run("docker", "exec", "-i", container, "chpasswd", input=f"fixture:{password}\n", capture_output=True)
            port = run("docker", "port", container, "22", capture_output=True).stdout.strip().rsplit(":", 1)[1]
            # sshd only starts after fresh host keys have been generated.
            for _ in range(50):
                probe = subprocess.run(["docker", "exec", container, "test", "-f", "/run/sshd.pid"], capture_output=True)
                if probe.returncode == 0:
                    break
                time.sleep(0.1)
            environment = dict(os.environ, NYATERM_SUDO_PORT=port, NYATERM_SUDO_PASSWORD=password, NYATERM_SUDO_CASE=case)
            run(executable, "--ignored", cwd=ROOT, env=environment)
            # Closing grants must not leave privileged sftp-server workers alive.
            run("docker", "exec", container, "sh", "-c", "! ps -eo uid,comm | awk '$1 == 0 && $2 == \"sftp-server\" {found=1} END {exit !found}'", capture_output=True)
            print(f"{case}: PASS", flush=True)
        finally:
            run("docker", "stop", "-t", "2", container, capture_output=True)


if __name__ == "__main__":
    main()
