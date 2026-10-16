"""Exercise published Codex ACP's native goal and Plan RPCs with a loopback model.

Each process has its own CODEX_HOME and workspace. No user's CLI/CodeG process,
credentials, config, or data are modified. Accept both the release-pinned CLI
and a newer CLI so capability drift is observable in CI.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import time

spec = importlib.util.spec_from_file_location("resume_fixture", Path(__file__).with_name("test-codex-resume.py"))
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--acp", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--enable-goals", action="store_true")
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    workspace = root / "workspace"
    workspace.mkdir()
    provider = h.Provider()
    home, _, _, _ = h.fixture(root / "fixture", workspace, provider, 1)
    # Goal support is experimental in the release-pinned CLI. This affects only
    # this disposable fixture home, and the test must fail if support is absent.
    if args.enable_goals:
        with (home / "config.toml").open("a") as config:
            config.write("\n[features]\ngoals = true\n")
    command = [str(args.acp.resolve())]
    if args.acp.suffix == ".js":
        command.insert(0, "node")
    rpc = h.RPC(command, home, workspace, root / "acp.stderr.log", {"CODEX_PATH": str(args.cli.resolve())})
    evidence = {}
    try:
        init, _ = rpc.rpc("initialize", {"protocolVersion": 1, "clientInfo": {"name": "codeg-goal-regression", "version": "1"}, "clientCapabilities": {"_meta": {"jetbrains": {"air": {"version": 1}}}}})
        advertised = init.get("_meta", {}).get("jetbrains", {}).get("air", {}).get("goal", {})
        assert set(advertised["actions"]) == {"set", "pause", "resume", "clear"}, advertised
        session, _ = rpc.rpc("session/new", {"cwd": str(workspace), "mcpServers": []})
        sid = session["sessionId"]
        plan, _ = rpc.rpc("session/set_config_option", {"sessionId": sid, "configId": "collaboration_mode", "value": "plan"})
        assert any(option.get("id") == "collaboration_mode" and option["currentValue"] == "plan" for option in plan["configOptions"]), plan
        rpc.rpc("session/set_config_option", {"sessionId": sid, "configId": "collaboration_mode", "value": "default"})
        method = advertised["controlMethod"]
        for action in ("set", "pause", "resume", "clear"):
            params = {"sessionId": sid, "action": action}
            if action == "set":
                params["objective"] = "Exercise native goal controls on synthetic data"
            reply, _ = rpc.rpc(method, params, timeout=45)
            assert reply == {}, reply
            # Pause/clear changes metadata; CodeG must also cancel the native
            # turn. This order is part of the application contract under test.
            if action in ("pause", "clear"):
                rpc.send({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": sid}})
                time.sleep(.15)
        snapshots = []
        for entry in rpc.notifications:
            update = entry.get("params", {}).get("update", {})
            meta = update.get("_meta", {}).get("jetbrains", {}).get("air", {})
            if update.get("sessionUpdate") == "session_info_update" and "goal" in meta:
                snapshots.append(meta["goal"])
        assert any(goal and goal["status"] == "active" for goal in snapshots), snapshots
        assert any(goal and goal["status"] == "paused" for goal in snapshots), snapshots
        assert any(goal is None for goal in snapshots), snapshots
        assert not any(entry["method"] == "session/prompt" for entry in rpc.timeline)
        evidence = {"passed": True, "advertised": advertised, "goal_statuses": [goal["status"] if goal else None for goal in snapshots], "native_plan": True, "prompt_simulation": False, "timeline": rpc.timeline}
    finally:
        rpc.close()
        provider.server.shutdown()
        (root / "verification.json").write_text(json.dumps(evidence, indent=2))
    print(json.dumps(evidence))


if __name__ == "__main__":
    main()
