#!/usr/bin/env python3
"""Verify the checked-in ASY-04 result and measured source identity."""

from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RESULT_PATH = Path(__file__).with_name("result.json")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    result = json.loads(RESULT_PATH.read_text(encoding="utf-8"))
    assert result["schema_version"] == 1
    assert result["iterations"] == 32
    assert result["timer_delay_us"] == 1_000
    assert result["blocking_duration_us"] == 10_000

    direct = result["direct_sync"]
    isolated = result["blocking_pool"]
    assert isolated["p95_lateness_us"] < direct["p95_lateness_us"]
    assert isolated["p95_lateness_us"] * 4 <= direct["p95_lateness_us"]
    assert isolated["max_lateness_us"] < direct["max_lateness_us"]

    expected_reduction = (
        (direct["p95_lateness_us"] - isolated["p95_lateness_us"])
        / direct["p95_lateness_us"]
        * 100
    )
    expected_improvement = (
        direct["p95_lateness_us"] / isolated["p95_lateness_us"]
    )
    assert math.isclose(
        result["p95_lateness_reduction_percent"],
        expected_reduction,
        rel_tol=1e-12,
    )
    assert math.isclose(
        result["p95_lateness_improvement"],
        expected_improvement,
        rel_tol=1e-12,
    )

    for relative, expected in result["source"]["files"].items():
        actual = sha256(ROOT / relative)
        assert actual == expected, f"{relative}: expected {expected}, found {actual}"

    print("ASY-04 runtime-starvation result verified")


if __name__ == "__main__":
    main()
