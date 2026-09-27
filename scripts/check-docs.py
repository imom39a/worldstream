#!/usr/bin/env python3
"""Check local Markdown link targets in the tracked source tree."""

from pathlib import Path
import re
import subprocess
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]


def main():
    names = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=ROOT
    ).decode().split("\0")
    failures = []
    count = 0
    for name in sorted(set(names)):
        path = ROOT / name
        if path.suffix != ".md" or not path.is_file():
            continue
        source = re.sub(r"(?ms)^\s*(```|~~~).*?^\s*\1\s*$", "", path.read_text())
        for match in re.finditer(r"\]\(([^\s)]+)(?:\s+\"[^\"]*\")?\)", source):
            target = urlsplit(match.group(1).strip("<>"))
            if target.scheme or target.netloc or not target.path:
                continue
            count += 1
            if not (path.parent / unquote(target.path)).exists():
                failures.append(f"{name}: missing {target.path}")
    for failure in failures:
        print(failure)
    print(f"Checked {count} local documentation links; {len(failures)} missing targets.")
    return bool(failures)


if __name__ == "__main__":
    raise SystemExit(main())
