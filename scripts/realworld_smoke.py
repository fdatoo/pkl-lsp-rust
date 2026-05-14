#!/usr/bin/env python3
"""Open a real-world Pkl corpus through pkl-lsp over JSON-RPC."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import queue
import subprocess
import sys
import threading
import time
import urllib.parse


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run an end-to-end pkl-lsp diagnostic smoke test."
    )
    parser.add_argument(
        "roots",
        nargs="+",
        type=pathlib.Path,
        help="Repository or directory roots to scan for .pkl files.",
    )
    parser.add_argument(
        "--binary",
        type=pathlib.Path,
        default=pathlib.Path("target/release/pkl-lsp"),
        help="Path to the pkl-lsp binary.",
    )
    parser.add_argument(
        "--namespace",
        action="append",
        default=[],
        metavar="NAME=PATH",
        help="Namespace mapping to pass through PKL_LSP_NAMESPACES. Repeatable.",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=60.0,
        help="Seconds to wait for diagnostic publications after opening files.",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=25,
        help="Number of diagnostic messages to show.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON instead of text.",
    )
    parser.add_argument(
        "--allow-missing-publications",
        action="store_true",
        help="Exit 0 even if not every opened file publishes diagnostics.",
    )
    return parser.parse_args()


def lsp_send(stdin, payload: dict) -> None:
    data = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    stdin.write(b"Content-Length: " + str(len(data)).encode("ascii") + b"\r\n\r\n")
    stdin.write(data)
    stdin.flush()


def lsp_reader(stdout, out: queue.Queue) -> None:
    while True:
        header = b""
        while True:
            line = stdout.readline()
            if not line:
                return
            if line == b"\r\n":
                break
            header += line

        length = None
        for item in header.decode("ascii", "replace").split("\r\n"):
            if item.lower().startswith("content-length:"):
                length = int(item.split(":", 1)[1].strip())
                break
        if length is None:
            continue

        body = stdout.read(length)
        try:
            out.put(json.loads(body))
        except json.JSONDecodeError as err:
            out.put({"json_parse_error": str(err), "body": body.decode("utf-8", "replace")})


def stderr_reader(stderr, out: queue.Queue) -> None:
    for line in stderr:
        out.put(line.decode("utf-8", "replace").rstrip())


def collect_files(roots: list[pathlib.Path]) -> list[pathlib.Path]:
    files: list[pathlib.Path] = []
    for root in roots:
        root = root.expanduser().resolve()
        if root.is_file() and root.suffix == ".pkl":
            files.append(root)
        elif root.is_dir():
            files.extend(path for path in root.rglob("*.pkl") if path.is_file())
        else:
            raise SystemExit(f"root does not exist: {root}")
    return sorted(set(files))


def common_root(paths: list[pathlib.Path]) -> pathlib.Path:
    common = os.path.commonpath([str(path.parent) for path in paths])
    return pathlib.Path(common)


def run_smoke(args: argparse.Namespace) -> tuple[dict, int]:
    files = collect_files(args.roots)
    if not files:
        raise SystemExit("no .pkl files found")

    binary = args.binary.expanduser()
    if not binary.exists():
        raise SystemExit(f"binary does not exist: {binary}")

    env = os.environ.copy()
    if args.namespace:
        env["PKL_LSP_NAMESPACES"] = ",".join(args.namespace)

    proc = subprocess.Popen(
        [str(binary)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    assert proc.stdin is not None
    assert proc.stdout is not None
    assert proc.stderr is not None

    messages: queue.Queue = queue.Queue()
    stderr_lines: queue.Queue = queue.Queue()
    threading.Thread(target=lsp_reader, args=(proc.stdout, messages), daemon=True).start()
    threading.Thread(target=stderr_reader, args=(proc.stderr, stderr_lines), daemon=True).start()

    next_id = 1

    def request(method: str, params):
        nonlocal next_id
        request_id = next_id
        next_id += 1
        lsp_send(
            proc.stdin,
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params},
        )
        return request_id

    def notify(method: str, params) -> None:
        lsp_send(proc.stdin, {"jsonrpc": "2.0", "method": method, "params": params})

    root = common_root(files)
    init_id = request(
        "initialize",
        {
            "processId": os.getpid(),
            "rootUri": root.as_uri(),
            "capabilities": {},
            "workspaceFolders": [{"uri": root.as_uri(), "name": root.name or "pkl-corpus"}],
        },
    )

    while True:
        message = messages.get(timeout=10)
        if "json_parse_error" in message:
            raise SystemExit(f"invalid JSON-RPC message from server: {message}")
        if message.get("id") == init_id:
            if "error" in message:
                raise SystemExit(f"initialize failed: {message['error']}")
            break

    notify("initialized", {})

    uris: list[str] = []
    for path in files:
        uri = path.as_uri()
        uris.append(uri)
        notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": uri,
                    "languageId": "pkl",
                    "version": 1,
                    "text": path.read_text(encoding="utf-8", errors="replace"),
                }
            },
        )

    diagnostics: dict[str, list[dict]] = {}
    last_message = time.time()
    deadline = time.time() + args.timeout
    while time.time() < deadline:
        try:
            message = messages.get(timeout=0.5)
        except queue.Empty:
            if len(diagnostics) >= len(files) and time.time() - last_message > 2:
                break
            continue
        last_message = time.time()
        if message.get("method") == "textDocument/publishDiagnostics":
            params = message.get("params", {})
            diagnostics[params.get("uri", "")] = params.get("diagnostics", [])

    shutdown_id = request("shutdown", None)
    try:
        while True:
            message = messages.get(timeout=2)
            if message.get("id") == shutdown_id:
                break
    except queue.Empty:
        pass
    notify("exit", {})

    try:
        proc.stdin.close()
    except BrokenPipeError:
        pass

    try:
        returncode = proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        returncode = proc.wait(timeout=5)

    clean = 0
    warn_only = 0
    error_files = 0
    total = 0
    by_message: dict[str, int] = {}
    examples: dict[str, str] = {}

    for uri in uris:
        items = diagnostics.get(uri, [])
        total += len(items)
        has_error = any(item.get("severity", 1) == 1 for item in items)
        has_warning = any(item.get("severity") == 2 for item in items)
        if has_error:
            error_files += 1
        elif has_warning or items:
            warn_only += 1
        else:
            clean += 1
        for item in items:
            message = item.get("message", "")
            by_message[message] = by_message.get(message, 0) + 1
            examples.setdefault(message, uri)

    stderr_sample: list[str] = []
    while not stderr_lines.empty() and len(stderr_sample) < 20:
        stderr_sample.append(stderr_lines.get())

    summary = {
        "files": len(files),
        "diagnostic_publications": len(diagnostics),
        "clean": clean,
        "warn_only": warn_only,
        "error_files": error_files,
        "diagnostics": total,
        "top_diagnostics": [
            {
                "count": count,
                "message": message,
                "example": urllib.parse.unquote(examples[message].removeprefix("file://")),
            }
            for message, count in sorted(
                by_message.items(), key=lambda item: item[1], reverse=True
            )[: args.top]
        ],
        "stderr_sample": stderr_sample,
        "returncode": returncode,
    }

    exit_code = 0
    if returncode != 0:
        exit_code = 1
    if (
        not args.allow_missing_publications
        and summary["diagnostic_publications"] != summary["files"]
    ):
        exit_code = 1
    return summary, exit_code


def print_text(summary: dict) -> None:
    print(json.dumps({k: v for k, v in summary.items() if k != "top_diagnostics"}, indent=2))
    print("top diagnostics:")
    for item in summary["top_diagnostics"]:
        print(f"{item['count']:4} {item['message']} :: {item['example']}")


def main() -> int:
    args = parse_args()
    summary, exit_code = run_smoke(args)
    if args.json:
        print(json.dumps(summary, indent=2))
    else:
        print_text(summary)
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
