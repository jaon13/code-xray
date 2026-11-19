#!/usr/bin/env python3
"""Cross-platform PreToolUse command safety hook."""

from __future__ import annotations

import json
import re
import sys

DESTRUCTIVE_RE = re.compile(
    r"git\s+(?:reset\s+--hard|clean\s+-[a-zA-Z]*f|checkout\s+--\s+\.)|"
    r"(?:rm\s+-rf|Remove-Item\s+.*-Recurse).*\b(?:src|tests)\b",
    re.I,
)


def main() -> int:
    raw = sys.stdin.read()
    if not raw.strip():
        print('{"decision":"allow"}')
        return 0
    try:
        payload = json.loads(raw)
        args = (payload.get("toolCall") or {}).get("args") or {}
        cmd = str(args.get("CommandLine") or args.get("command") or "")
        if DESTRUCTIVE_RE.search(cmd):
            print(
                json.dumps(
                    {
                        "decision": "ask",
                        "reason": "Destructive workspace command detected by PreToolUse safety hook. Confirm before proceeding.",
                    }
                )
            )
            return 0
    except Exception:
        pass
    print('{"decision":"allow"}')
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
