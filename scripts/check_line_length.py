#!/usr/bin/env python3

import subprocess
import sys
from pathlib import Path
from typing import Iterable, Sequence

MAX_WIDTH = 100

# Root of the repository
ROOT = Path(__file__).resolve().parents[1]

# Enforce line length on Rust sources only.
CODE_EXTENSIONS = {".rs"}

# Directories that should never be scanned.
EXCLUDED_DIRS = {".git", ".cargo", "target"}


def tracked_files() -> Sequence[Path]:
    """Return all tracked repository files."""
    result = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return [ROOT / line.strip() for line in result.stdout.splitlines() if line.strip()]


def should_check(path: Path) -> bool:
    """True if the file should be scanned for line-length violations."""
    if any(part in EXCLUDED_DIRS for part in path.parts):
        return False
    return path.suffix in CODE_EXTENSIONS


def iter_violations(paths: Iterable[Path]) -> list[str]:
    """Collect all line-length violations across the provided paths."""
    violations: list[str] = []
    for path in paths:
        if not should_check(path):
            continue
        try:
            with path.open("r", encoding="utf-8") as fh:
                for idx, line in enumerate(fh, 1):
                    if len(line.rstrip("\n")) > MAX_WIDTH:
                        violations.append(f"{path}:{idx}")
        except UnicodeDecodeError:
            # Skip files that are not valid UTF-8 text.
            continue
    return violations


def main() -> int:
    violations = iter_violations(tracked_files())
    if violations:
        print(f"Files exceed {MAX_WIDTH} characters:")
        print("\n".join(violations))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
