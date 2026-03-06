#!/usr/bin/env python3
import argparse
import glob
import json
import os
import re
import signal
import shlex
import subprocess
import sys
import time
import urllib.error
import urllib.request


def parse_ports(value: str) -> list[int]:
    # Parse comma-separated ports into integers.
    ports: list[int] = []
    for part in value.split(","):
        part = part.strip()
        if not part:
            continue
        ports.append(int(part))
    if not ports:
        raise ValueError("no ports provided")
    return ports


def http_get_json(url: str) -> dict:
    # Simple GET returning JSON payload.
    req = urllib.request.Request(url, method="GET")
    with urllib.request.urlopen(req, timeout=2) as resp:
        data = resp.read().decode("utf-8")
    return json.loads(data)


def json_rpc(url: str, method: str) -> dict:
    # Minimal JSON-RPC POST helper.
    payload = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": method, "params": []}
    ).encode("utf-8")
    req = urllib.request.Request(
        url, data=payload, headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=2) as resp:
        data = resp.read().decode("utf-8")
    return json.loads(data)


def block_height(rpc_url: str) -> int:
    # Fetch the current block height from an RPC endpoint.
    resp = json_rpc(rpc_url, "eth_blockNumber")
    if "result" not in resp:
        raise RuntimeError(f"missing result in response from {rpc_url}: {resp}")
    return int(resp["result"], 16)


def is_proposer(metrics_url: str) -> bool:
    # Metrics endpoint returns {"is_proposer": true|false}.
    try:
        data = http_get_json(metrics_url)
    except urllib.error.URLError:
        return False
    value = data.get("is_proposer")
    return bool(value)


def pid_command(pid: int) -> str | None:
    # Read command line for a PID. Returns None if ps is unavailable.
    try:
        output = subprocess.check_output(
            ["ps", "-p", str(pid), "-o", "command="],
            text=True,
        ).strip()
    except subprocess.CalledProcessError:
        return None
    return output or None


def pid_matches(pid: int, http_port: int) -> bool:
    # Ensure PID is a rbft-node instance listening on the expected HTTP port.
    cmd = pid_command(pid)
    if cmd is None:
        return False
    expected = f"--http.port {http_port}"
    return "rbft-node" in cmd and expected in cmd


def find_pid(http_port: int, require_match: bool = True) -> int | None:
    # Find PID by matching the node command line, with lsof fallback.
    candidates: list[int] = []
    try:
        output = subprocess.check_output(
            ["pgrep", "-f", f"rbft-node node --http.port {http_port}"],
            text=True,
        ).strip()
    except subprocess.CalledProcessError:
        output = ""
    if not output:
        try:
            output = subprocess.check_output(
                ["lsof", "-i", f":{http_port}", "-t"],
                text=True,
            ).strip()
        except subprocess.CalledProcessError:
            return None
    for line in output.splitlines():
        line = line.strip()
        if not line:
            continue
        if line.isdigit():
            candidates.append(int(line))
    if not candidates:
        return None
    for pid in candidates:
        if pid_matches(pid, http_port):
            return pid
    if require_match:
        return None
    return candidates[0]


def default_restart_cmd() -> list[str] | None:
    # Default to scripts/restart_node.sh <port> if it exists.
    script_path = os.path.join(
        os.path.dirname(os.path.abspath(__file__)),
        "restart_node.sh",
    )
    if os.path.isfile(script_path):
        return ["/bin/bash", script_path, "{port}"]
    return None


def default_log_dir() -> str | None:
    # Prefer target/testnet/logs if it exists, otherwise ~/.rbft/testnet/logs.
    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    repo_logs = os.path.join(repo_root, "target", "testnet", "logs")
    if os.path.isdir(repo_logs):
        return repo_logs
    home_logs = os.path.join(os.path.expanduser("~"), ".rbft", "testnet", "logs")
    if os.path.isdir(home_logs):
        return home_logs
    return None


def collect_log_files(log_dir: str, pattern: str) -> list[str]:
    # Return log file paths matching a glob pattern.
    return sorted(glob.glob(os.path.join(log_dir, pattern)))


def log_offsets(paths: list[str]) -> dict[str, int]:
    # Track current file sizes so we only read new lines.
    offsets: dict[str, int] = {}
    for path in paths:
        try:
            offsets[path] = os.path.getsize(path)
        except OSError:
            continue
    return offsets


def read_new_lines(path: str, offset: int) -> tuple[list[str], int]:
    # Read lines from a file starting at offset; return lines and new offset.
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            handle.seek(offset)
            data = handle.read()
            new_offset = handle.tell()
    except OSError:
        return [], offset
    lines = data.splitlines()
    return lines, new_offset


def wait_for_log_pattern(
    paths: list[str],
    offsets: dict[str, int],
    pattern: re.Pattern[str],
    timeout: float,
    poll_interval: float,
) -> tuple[str, str] | tuple[None, None]:
    # Wait for a matching line to appear in the log files.
    deadline = time.time() + timeout
    while time.time() < deadline:
        for path in paths:
            offset = offsets.get(path, 0)
            lines, new_offset = read_new_lines(path, offset)
            offsets[path] = new_offset
            for line in lines:
                if pattern.search(line):
                    return path, line
        time.sleep(poll_interval)
    return None, None


def run_restart(cmd_template: list[str], port: int, delay: float) -> bool:
    # Restart the node using a command template, replacing {port}.
    if delay > 0:
        time.sleep(delay)
    cmd = [part.replace("{port}", str(port)) for part in cmd_template]
    try:
        subprocess.check_call(cmd)
    except subprocess.CalledProcessError as exc:
        print(f"error: restart failed: {exc}", file=sys.stderr)
        return False
    return True


def wait_for_proposer(
    ports: list[int],
    rpc_host: str,
    confirm: int,
    poll_interval: float,
    timeout: float,
) -> tuple[int, int] | tuple[None, None]:
    # Wait until we observe a stable proposer for N consecutive polls.
    stable_counts = {port: 0 for port in ports}
    deadline = time.time() + timeout
    while time.time() < deadline:
        found = None
        for port in ports:
            metrics_port = 9000 + (port % 10)
            metrics_url = f"http://{rpc_host}:{metrics_port}/health"
            if is_proposer(metrics_url):
                found = port
                break
        for port in ports:
            stable_counts[port] = stable_counts[port] + 1 if port == found else 0
        if found is not None and stable_counts[found] >= confirm:
            pid = find_pid(found)
            if pid is not None:
                return found, pid
        time.sleep(poll_interval)
    return None, None


def choose_poll_port(
    ports: list[int],
    rpc_host: str,
    exclude: int | None = None,
) -> int | None:
    # Pick the first port that answers eth_blockNumber.
    for port in ports:
        if exclude is not None and port == exclude:
            continue
        rpc_url = f"http://{rpc_host}:{port}"
        try:
            _ = block_height(rpc_url)
            return port
        except Exception:
            continue
    return None


def wait_for_height_increase(
    rpc_url: str,
    height_before: int,
    poll_interval: float,
    timeout: float,
) -> int | None:
    # Wait for a new block and return the new height.
    deadline = time.time() + timeout
    height_now = height_before
    while time.time() < deadline:
        time.sleep(poll_interval)
        try:
            height_now = block_height(rpc_url)
        except Exception:
            continue
        if height_now > height_before:
            return height_now
    return None


def wait_quiet_block_window(
    rpc_url: str,
    height_before: int,
    quiet_ms: int,
    poll_interval: float,
) -> int:
    # Ensure no new blocks are produced for quiet_ms before taking action.
    deadline = time.time() + (quiet_ms / 1000.0)
    current = height_before
    while time.time() < deadline:
        time.sleep(poll_interval)
        try:
            current = block_height(rpc_url)
        except Exception:
            continue
        if current != height_before:
            height_before = current
            deadline = time.time() + (quiet_ms / 1000.0)
    return height_before


def wait_next_block(
    rpc_url: str,
    height_before: int,
    poll_interval: float,
    timeout: float,
) -> tuple[int, int] | tuple[None, None]:
    # Poll until block height increases, returning (height_after, elapsed_ms).
    start_ms = int(time.time() * 1000)
    deadline = time.time() + timeout
    height_now = height_before
    while time.time() < deadline:
        time.sleep(poll_interval)
        try:
            height_now = block_height(rpc_url)
        except Exception:
            continue
        if height_now > height_before:
            end_ms = int(time.time() * 1000)
            return height_now, end_ms - start_ms
    return None, None


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Measure time from killing proposer to next block."
    )
    parser.add_argument(
        "--ports",
        default="8545,8544,8543,8542",
        help=(
            "Comma-separated HTTP RPC ports to check "
            "(default: 8545,8544,8543,8542)."
        ),
    )
    parser.add_argument(
        "--rpc-host",
        default="localhost",
        help="RPC/metrics host (default: localhost).",
    )
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=0.2,
        help="Seconds between block height polls (default: 0.2).",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=60.0,
        help="Seconds to wait for next block (default: 60).",
    )
    parser.add_argument(
        "--proposer-timeout",
        type=float,
        default=30.0,
        help="Seconds to wait for proposer election (default: 30).",
    )
    parser.add_argument(
        "--confirm",
        type=int,
        default=3,
        help="Consecutive proposer polls required (default: 3).",
    )
    parser.add_argument(
        "--block-interval-ms",
        type=int,
        default=500,
        help="Expected block interval in ms (default: 500).",
    )
    parser.add_argument(
        "--align-after-block-ms",
        type=int,
        default=50,
        help="Wait for a new block, then sleep this many ms (default: 50).",
    )
    parser.add_argument(
        "--align-timeout-ms",
        type=int,
        default=None,
        help="Timeout waiting for a new block (default: 2x block interval).",
    )
    parser.add_argument(
        "--quiet-ms",
        type=int,
        default=None,
        help="Quiet window in ms before action (default: 70%% of interval).",
    )
    parser.add_argument(
        "--guard-ms",
        type=int,
        default=None,
        help="Verify no new block for this ms after STOP (default: interval).",
    )
    parser.add_argument(
        "--signal",
        default="TERM",
        choices=["TERM", "KILL", "STOP"],
        help="Signal to send to proposer (default: TERM).",
    )
    parser.add_argument(
        "--mode",
        default="kill",
        choices=["kill", "stop", "stop-then-kill"],
        help="How to interrupt proposer (default: kill).",
    )
    parser.add_argument(
        "--max-retries",
        type=int,
        default=3,
        help="Retries if proposer already proposed (default: 3).",
    )
    parser.add_argument(
        "--poll-port",
        type=int,
        default=None,
        help="RPC port to poll for block height (default: first non-proposer).",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Only print proposer info; do not kill.",
    )
    parser.add_argument(
        "--unsafe-kill",
        action="store_true",
        help="Allow killing PID even if it does not look like rbft-node.",
    )
    parser.add_argument(
        "--restart",
        action="store_true",
        default=True,
        help="Restart the proposer after measurement (default: true).",
    )
    parser.add_argument(
        "--restart-cmd",
        default=None,
        help=(
            "Restart command template (use {port}). "
            "Default: scripts/restart_node.sh {port}."
        ),
    )
    parser.add_argument(
        "--restart-delay",
        type=float,
        default=1.0,
        help="Seconds to wait before restart (default: 1).",
    )
    parser.add_argument(
        "--wait-log",
        action="store_true",
        help="Wait for a warning log indicating a missed proposal.",
    )
    parser.add_argument(
        "--log-dir",
        default=None,
        help="Log directory to watch (default: auto-detect).",
    )
    parser.add_argument(
        "--log-glob",
        default="val*.log",
        help="Glob pattern for log files (default: val*.log).",
    )
    parser.add_argument(
        "--log-pattern",
        default=r"round timeout.*did not propose",
        help="Regex pattern to confirm proposer miss.",
    )
    parser.add_argument(
        "--log-timeout",
        type=float,
        default=5.0,
        help="Seconds to wait for the log pattern (default: 5).",
    )
    parser.add_argument(
        "--log-poll-interval",
        type=float,
        default=0.2,
        help="Seconds between log polls (default: 0.2).",
    )
    args = parser.parse_args()

    # Determine which validator is the current proposer.
    try:
        ports = parse_ports(args.ports)
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    quiet_ms = args.quiet_ms
    if quiet_ms is None:
        quiet_ms = max(50, int(args.block_interval_ms * 0.7))
    guard_ms = args.guard_ms
    if guard_ms is None:
        guard_ms = args.block_interval_ms
    align_timeout_ms = args.align_timeout_ms
    if align_timeout_ms is None:
        align_timeout_ms = max(args.block_interval_ms * 2, 1000)

    for attempt in range(1, args.max_retries + 1):
        poll_port = args.poll_port
        if poll_port is None:
            poll_port = choose_poll_port(ports, args.rpc_host)
        if poll_port is None:
            print("error: no responsive RPC ports found", file=sys.stderr)
            return 1

        # Capture current height before shutting down the proposer.
        poll_url = f"http://{args.rpc_host}:{poll_port}"
        try:
            height_before = block_height(poll_url)
        except Exception as exc:
            print(
                f"error: cannot read block height from {poll_url}: {exc}",
                file=sys.stderr,
            )
            return 1

        if args.align_after_block_ms >= 0:
            next_height = wait_for_height_increase(
                rpc_url=poll_url,
                height_before=height_before,
                poll_interval=args.poll_interval,
                timeout=align_timeout_ms / 1000.0,
            )
            if next_height is None:
                print("timeout waiting for alignment block; retrying", file=sys.stderr)
                continue
            height_before = next_height
            time.sleep(args.align_after_block_ms / 1000.0)

        proposer_port, pid = wait_for_proposer(
            ports=ports,
            rpc_host=args.rpc_host,
            confirm=args.confirm,
            poll_interval=args.poll_interval,
            timeout=args.proposer_timeout,
        )
        if proposer_port is None or pid is None:
            print(
                "error: no proposer found (metrics /health not reporting is_proposer)",
                file=sys.stderr,
            )
            return 1

        if poll_port == proposer_port:
            alt_poll_port = choose_poll_port(ports, args.rpc_host, exclude=proposer_port)
            if alt_poll_port is not None:
                poll_port = alt_poll_port
                poll_url = f"http://{args.rpc_host}:{poll_port}"
                try:
                    height_before = block_height(poll_url)
                except Exception as exc:
                    print(
                        f"error: cannot read block height from {poll_url}: {exc}",
                        file=sys.stderr,
                    )
                    return 1

        pid = find_pid(proposer_port, require_match=not args.unsafe_kill)
        if pid is None:
            print(
                "error: could not confirm proposer PID as rbft-node; "
                "use --unsafe-kill to bypass",
                file=sys.stderr,
            )
            return 1

        print(f"proposer_port={proposer_port} pid={pid} attempt={attempt}")
        print(f"poll_port={poll_port} height_before={height_before}")

        if args.dry_run:
            return 0

        log_paths: list[str] = []
        log_offsets_map: dict[str, int] = {}
        log_pattern = None
        if args.wait_log:
            log_dir = args.log_dir or default_log_dir()
            if log_dir is None:
                print("error: no log directory found", file=sys.stderr)
                return 1
            log_paths = collect_log_files(log_dir, args.log_glob)
            if not log_paths:
                print("error: no log files matched pattern", file=sys.stderr)
                return 1
            log_offsets_map = log_offsets(log_paths)
            log_pattern = re.compile(args.log_pattern)

        height_before = wait_quiet_block_window(
            rpc_url=poll_url,
            height_before=height_before,
            quiet_ms=quiet_ms,
            poll_interval=args.poll_interval,
        )

        if args.mode in {"stop", "stop-then-kill"}:
            try:
                subprocess.check_call(["kill", "-STOP", str(pid)])
            except subprocess.CalledProcessError as exc:
                print(f"error: failed to stop proposer: {exc}", file=sys.stderr)
                return 1

            stall_start_ms = int(time.time() * 1000)
            height_after, _ = wait_next_block(
                rpc_url=poll_url,
                height_before=height_before,
                poll_interval=args.poll_interval,
                timeout=guard_ms / 1000.0,
            )
            if height_after is not None:
                subprocess.check_call(["kill", "-CONT", str(pid)])
                print(
                    "proposer already produced a block; retrying",
                    file=sys.stderr,
                )
                continue
            stall_end_ms = int(time.time() * 1000)
            print(f"stall_ms={stall_end_ms - stall_start_ms}")

        if args.mode == "stop":
            if args.restart:
                print(
                    "warning: --restart ignored in stop-only mode",
                    file=sys.stderr,
                )
            return 0

        kill_signal = args.signal
        if args.mode == "stop-then-kill":
            kill_signal = "KILL"

        try:
            subprocess.check_call(["kill", f"-{kill_signal}", str(pid)])
        except subprocess.CalledProcessError as exc:
            print(f"error: failed to send signal: {exc}", file=sys.stderr)
            return 1

        height_after, elapsed_ms = wait_next_block(
            rpc_url=poll_url,
            height_before=height_before,
            poll_interval=args.poll_interval,
            timeout=args.timeout,
        )
        if height_after is not None and elapsed_ms is not None:
            print(f"height_after={height_after} elapsed_ms={elapsed_ms}")
            if args.wait_log and log_pattern is not None:
                match_path, match_line = wait_for_log_pattern(
                    log_paths,
                    log_offsets_map,
                    log_pattern,
                    args.log_timeout,
                    args.log_poll_interval,
                )
                if match_path is None:
                    print("warning: log pattern not found", file=sys.stderr)
                else:
                    print(f"log_match_file={match_path}")
                    print(f"log_match_line={match_line}")
            if args.restart:
                restart_cmd = None
                if args.restart_cmd is None:
                    restart_cmd = default_restart_cmd()
                else:
                    restart_cmd = shlex.split(args.restart_cmd)
                if restart_cmd is None:
                    print(
                        "warning: restart requested but no restart command found",
                        file=sys.stderr,
                    )
                else:
                    run_restart(restart_cmd, proposer_port, args.restart_delay)
            return 0

        print("timeout waiting for next block; retrying", file=sys.stderr)

    print("error: exceeded retries", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
