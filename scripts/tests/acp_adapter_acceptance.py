"""Exercise installed ACP adapters against a loopback model and harmless MCP fixture.
No external model requests or user credentials are required. Build examples first.
Usage: python scripts/tests/acp_adapter_acceptance.py claude|codex ADAPTER_PATH
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path(__file__).resolve().parents[2]
strategy, adapter = sys.argv[1:3]
native_probe = "--native" in sys.argv[3:]
observations = {"requests": 0, "tools": set(), "mcp_result": False, "tool_requested": set(), "rounds": set(), "native_rejected": False}

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        if "count_tokens" in self.path:
            payload = json.dumps({"input_tokens": 10}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(payload)
            return
        observations["requests"] += 1
        tools = body.get("tools", [])
        names = [tool.get("name", tool.get("function", {}).get("name", tool.get("type", ""))) for tool in tools]
        observations["tools"].update(names)
        body_text = json.dumps(body)
        for message in body.get("messages", []):
            for part in message.get("content", []) if isinstance(message.get("content"), list) else []:
                if part.get("type") == "tool_result" and part.get("is_error"):
                    observations["native_rejected"] = True
        observations["mcp_result"] |= "NYATERM_MCP_ACCEPTANCE_OK" in body_text
        for round_id in ["first", "second"]:
            if "NYATERM_MCP_ACCEPTANCE_OK:" + round_id in body_text: observations["rounds"].add(round_id)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        def send(event, data):
            self.wfile.write(("event: " + event + "\ndata: " + json.dumps(data) + "\n\n").encode())
            self.wfile.flush()
        if strategy == "claude":
            name = next((name for name in names if "acceptance_ping" in name), None)
            round_id = "second" if "NYATERM_ACCEPTANCE_SECOND" in body_text else "first"
            call = bool(name and round_id not in observations["tool_requested"])
            if native_probe and call: name = "Bash"
            if call: observations["tool_requested"].add(round_id)
            send("message_start", {"type":"message_start","message":{"id":"msg_fixture_" + str(observations["requests"]),"type":"message","role":"assistant","content":[],"model":body.get("model"),"stop_reason":None,"stop_sequence":None,"usage":{"input_tokens":10,"output_tokens":0}}})
            block = {"type":"tool_use","id":"tool_fixture_" + round_id,"name":name,"input":{}} if call else {"type":"text","text":""}
            send("content_block_start", {"type":"content_block_start","index":0,"content_block":block})
            arguments = {"command": "echo forbidden > " + str(work / "native-escaped.txt")} if native_probe else {}
            delta = {"type":"input_json_delta","partial_json":json.dumps(arguments)} if call else {"type":"text_delta","text":"OK"}
            send("content_block_delta", {"type":"content_block_delta","index":0,"delta":delta})
            send("content_block_stop", {"type":"content_block_stop","index":0})
            send("message_delta", {"type":"message_delta","delta":{"stop_reason":"tool_use" if call else "end_turn","stop_sequence":None},"usage":{"output_tokens":3}})
            send("message_stop", {"type":"message_stop"})
        else:
            response = {"id":"resp_fixture","object":"response","created_at":1,"model":body.get("model"),"status":"in_progress","output":[]}
            send("response.created", {"type":"response.created","response":response})
            item = {"id":"msg_fixture","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"OK","annotations":[]}]}
            send("response.output_item.added", {"type":"response.output_item.added","output_index":0,"item":dict(item, content=[])})
            send("response.output_text.delta", {"type":"response.output_text.delta","item_id":"msg_fixture","output_index":0,"content_index":0,"delta":"OK"})
            send("response.output_item.done", {"type":"response.output_item.done","output_index":0,"item":item})
            response.update(status="completed", output=[item], usage={"input_tokens":10,"output_tokens":3,"total_tokens":13})
            send("response.completed", {"type":"response.completed","response":response})

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
work = pathlib.Path(tempfile.mkdtemp(prefix="acp-acceptance-", dir=ROOT / "target"))
config = work / "config"
config.mkdir()
cwd = work / "workspace"
cwd.mkdir()
url = "http://127.0.0.1:" + str(server.server_port)
env = [{"id":"key", "name":"ANTHROPIC_API_KEY" if strategy == "claude" else "NYA_TEST_API_KEY", "value":"local-fixture-not-a-secret"}]
if strategy == "claude":
    env.append({"id":"url", "name":"ANTHROPIC_BASE_URL", "value":url})
else:
    (config / "config.toml").write_text('model_provider = "fixture"\nmodel = "gpt-5"\n[model_providers.fixture]\nname = "Fixture"\nbase_url = "' + url + '/v1"\nwire_api = "responses"\nenv_key = "NYA_TEST_API_KEY"\n', encoding="utf-8")
profile = {"id":"acceptance", "name":"Acceptance", "strategy":strategy, "executable":str(pathlib.Path(adapter).resolve()), "enabled":True,
    "working_directory":str(cwd), "config_directory":str(config), "environment":env}
# Conflicting persisted configuration must neither add tools nor execute hooks.
forbidden_mcp = work / "forbidden-mcp.txt"
forbidden_hook = work / "forbidden-hook.txt"
extra_server = {"command": str(ROOT / "target/debug/examples/acp_fixture.exe"), "env": {"ACCEPTANCE_FORBIDDEN_START": str(forbidden_mcp)}}
(cwd / ".mcp.json").write_text(json.dumps({"mcpServers":{"forbidden":extra_server}}), encoding="utf-8")
hook_command = "powershell.exe -NoProfile -NonInteractive -Command \"Set-Content -LiteralPath '" + str(forbidden_hook) + "' -Value forbidden\""
(config / "settings.json").write_text(json.dumps({"permissions":{"defaultMode":"auto"}, "hooks":{"SessionStart":[{"hooks":[{"type":"command","command":hook_command}]}]}}), encoding="utf-8")
profile_path = work / "profile.json"
profile_path.write_text(json.dumps(profile), encoding="utf-8")
try:
    fixture_env = {key: value for key, value in os.environ.items() if not key.upper().startswith(("ANTHROPIC_", "OPENAI_", "CLAUDE_", "CODEX_"))}
    result = subprocess.run([str(ROOT / "target/debug/examples/acp_probe.exe"), strategy, adapter, str(profile_path), str(ROOT / "target/debug/examples/acp_fixture.exe")], capture_output=True, text=True, timeout=115, env=fixture_env)
    # Probe output contains only fixed event/status summaries, never peer payloads.
    print(result.stdout.strip())
    print(result.stderr.strip())
    print("probe_exit=", result.returncode)
    print("requests=", observations["requests"])
    print("tools=", sorted(observations["tools"]))
    print("mcp_result=", observations["mcp_result"])
    print("fresh_mcp_rounds=", sorted(observations["rounds"]))
    extra = {name for name in observations["tools"] if not name.startswith("mcp__nyaterm__")}
    isolated = not forbidden_mcp.exists() and not forbidden_hook.exists() and not (work / "native-escaped.txt").exists()
    print("inherited_config_isolated=", isolated)
    expected = observations["native_rejected"] if native_probe else observations["rounds"] == {"first", "second"}
    passed = result.returncode == 0 and observations["requests"] > 0 and not extra and expected and isolated
    print("acceptance=", "PASS" if passed else "FAIL")
    sys.exit(0 if passed else 1)
finally:
    server.shutdown()
