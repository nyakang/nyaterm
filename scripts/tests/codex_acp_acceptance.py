"""Run the pinned real Codex ACP adapter against a local model and MCP fixture.

Build nyaterm-agent-client examples first. No user credentials or paid model calls.
Usage: python scripts/tests/codex_acp_acceptance.py ADAPTER_PATH [--native]
Set CARGO_TARGET_DIR when the examples were built in an isolated target directory.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[2]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
SUFFIX = ".exe" if os.name == "nt" else ""
PROBE = TARGET / "debug/examples" / ("acp_probe" + SUFFIX)
FIXTURE = TARGET / "debug/examples" / ("acp_fixture" + SUFFIX)
NATIVE = "--native" in sys.argv[2:]
seen = {"tools": set(), "called": set(), "rounds": set(), "rejected": False}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        text = json.dumps(body)
        tool_ids = {}
        for tool in body.get("tools", []):
            if tool.get("type") == "namespace":
                for child in tool.get("tools", []):
                    tool_ids[tool["name"] + "." + child["name"]] = (child["name"], tool["name"])
            else:
                tool_ids[tool.get("name", "")] = (tool.get("name", ""), None)
        names = set(tool_ids)
        seen["tools"].update(names)
        for round_id in ["first", "second"]:
            if "NYATERM_MCP_ACCEPTANCE_OK:" + round_id in text:
                seen["rounds"].add(round_id)
        for item in body.get("input", []):
            if item.get("type") == "function_call_output" and "NYATERM_MCP_ACCEPTANCE_OK" not in str(item.get("output")):
                seen["rejected"] = True
        round_id = "second" if "NYATERM_ACCEPTANCE_SECOND" in text else "first"
        call = round_id not in seen["called"]
        name = next((n for n in names if n.endswith("acceptance_ping")), None)
        if call and name:
            seen["called"].add(round_id)
            arguments = {"cmd": "echo forbidden > " + str(work / "native-escaped.txt")} if NATIVE else {}
            tool_name, namespace = tool_ids[name]
            item = {"type": "function_call", "id": "fc_" + round_id, "call_id": "call_" + round_id,
                    "name": "exec_command" if NATIVE else tool_name, "arguments": json.dumps(arguments), "status": "completed"}
            if namespace and not NATIVE:
                item["namespace"] = namespace
        else:
            item = {"id": "msg_" + round_id, "type": "message", "role": "assistant", "status": "completed",
                    "content": [{"type": "output_text", "text": "OK", "annotations": []}]}
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

        def send(kind, data):
            self.wfile.write(("event: " + kind + "\ndata: " + json.dumps(dict(type=kind, **data)) + "\n\n").encode())
            self.wfile.flush()

        response = {"id": "resp_" + round_id, "object": "response", "created_at": 1,
                    "model": body.get("model"), "status": "in_progress", "output": []}
        send("response.created", {"response": response})
        send("response.output_item.added", {"output_index": 0, "item": item})
        if item["type"] == "message":
            send("response.output_text.delta", {"item_id": item["id"], "output_index": 0, "content_index": 0, "delta": "OK"})
        send("response.output_item.done", {"output_index": 0, "item": item})
        response.update(status="completed", output=[item], usage={"input_tokens": 10, "output_tokens": 3, "total_tokens": 13})
        send("response.completed", {"response": response})


with tempfile.TemporaryDirectory(prefix="nyaterm-codex-acceptance-") as directory:
    work = Path(directory)
    config = work / "config"
    cwd = work / "workspace"
    config.mkdir()
    cwd.mkdir()
    (cwd / ".codex").mkdir()
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    forbidden = work / "forbidden-mcp.txt"
    hook = work / "forbidden-hook.txt"
    q = json.dumps
    base_config = ('model_provider = "fixture"\nmodel = "gpt-5"\n'
                   '[model_providers.fixture]\nname = "Fixture"\n'
                   f'base_url = "http://127.0.0.1:{server.server_port}/v1"\n'
                   'wire_api = "responses"\nenv_key = "NYA_TEST_API_KEY"\n'
                   '[features]\ncodex_hooks = true\nshell_tool = true\n'
                   f'[mcp_servers.nyaterm]\nrequired = true\ncommand = {q(str(FIXTURE))}\n'
                   f'[mcp_servers.nyaterm.env]\nACCEPTANCE_FORBIDDEN_START = {q(str(forbidden))}\n'
                   '[[hooks.SessionStart]]\n[[hooks.SessionStart.hooks]]\ntype = "command"\n'
                   f'command = {q("echo forbidden > " + str(hook))}\n')
    (config / "config.toml").write_text(base_config, encoding="utf-8")
    project_config = (f'[mcp_servers.foreign]\ncommand = {q(str(FIXTURE))}\n'
                      f'[mcp_servers.foreign.env]\nACCEPTANCE_FORBIDDEN_START = {q(str(forbidden))}\n')
    (cwd / ".codex/config.toml").write_text(project_config, encoding="utf-8")
    profile = {"id": "acceptance", "name": "Acceptance", "strategy": "codex", "enabled": True,
               "executable": str(Path(sys.argv[1]).resolve()), "working_directory": str(cwd),
               "config_directory": str(config), "environment": [
                   {"id": "key", "name": "NYA_TEST_API_KEY", "value": "local-fixture-not-a-secret"}]}
    profile_path = work / "profile.json"
    profile_path.write_text(json.dumps(profile), encoding="utf-8")
    environment = {k: v for k, v in os.environ.items() if not k.upper().startswith(("OPENAI_", "CODEX_", "CLAUDE_", "ANTHROPIC_"))}
    try:
        result = subprocess.run([str(PROBE), "codex", sys.argv[1], str(profile_path), str(FIXTURE)],
                                env=environment, capture_output=True, text=True, timeout=110)
        # The probe emits fixed statuses only; it never prints protocol payloads.
        print(result.stdout.strip())
        print(result.stderr.strip())
        expected = seen["rejected"] if NATIVE else seen["rounds"] == {"first", "second"}
        isolated = not any(p.exists() for p in [forbidden, hook, work / "native-escaped.txt"])
        unchanged = (config / "config.toml").read_text(encoding="utf-8") == base_config and (cwd / ".codex/config.toml").read_text(encoding="utf-8") == project_config
        only_nyaterm = bool(seen["tools"]) and all(n.startswith(("mcp__nyaterm_embedded__", "mcp__nyaterm_embedded.")) or n in {"list_mcp_resources", "list_mcp_resource_templates", "read_mcp_resource"} for n in seen["tools"])
        print("tools=", sorted(seen["tools"]))
        print("fresh_mcp_rounds=", sorted(seen["rounds"]))
        print("native_rejected=", seen["rejected"], "isolated=", isolated, "configs_unchanged=", unchanged)
        passed = result.returncode == 0 and expected and isolated and unchanged and only_nyaterm
        print("acceptance=", "PASS" if passed else "FAIL")
        sys.exit(0 if passed else 1)
    finally:
        server.shutdown()
