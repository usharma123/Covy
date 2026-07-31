#!/usr/bin/env python3
"""Verify the checked-in PER-01 result and its measured source identity."""

from __future__ import annotations

import hashlib
import json
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
    assert result["schema_version"] == 3
    assert result["fixture_tasks"] == 300
    assert abs(result["fixture_registry_bytes"] - 1_848_193) < 1024
    assert result["parity_event_count"] == result["measured_events"] + 1

    legacy = result["legacy_full_checkpoint_under_lock"]
    owned = result["owned_wal_with_coalesced_checkpoint_after_lock"]
    assert owned["median_event_lock_ns"] < legacy["median_event_lock_ns"]
    assert owned["median_published_bytes"] < legacy["median_published_bytes"]
    assert owned["median_checkpoints"] < legacy["median_checkpoints"]
    assert owned["median_checkpoints"] == 0
    assert result["lock_time_reduction_percent"] >= 90.0
    assert result["write_byte_reduction_percent"] >= 80.0

    scaling = result["registry_scaling"]
    assert scaling["small_fixture_tasks"] == 1
    assert scaling["large_fixture_tasks"] == result["fixture_tasks"]
    assert scaling["small_owned_median_published_bytes"] > 0
    assert scaling["published_byte_ratio_large_over_small"] <= 1.05
    actual_ratio = (
        scaling["large_owned_median_published_bytes"]
        / scaling["small_owned_median_published_bytes"]
    )
    assert abs(actual_ratio - scaling["published_byte_ratio_large_over_small"]) < 1e-9

    actual_lock_reduction = (
        1.0 - owned["median_event_lock_ns"] / legacy["median_event_lock_ns"]
    ) * 100.0
    actual_byte_reduction = (
        1.0 - owned["median_published_bytes"] / legacy["median_published_bytes"]
    ) * 100.0
    actual_speedup = legacy["median_elapsed_us"] / owned["median_elapsed_us"]
    assert abs(actual_lock_reduction - result["lock_time_reduction_percent"]) < 1e-9
    assert abs(actual_byte_reduction - result["write_byte_reduction_percent"]) < 1e-9
    assert abs(actual_speedup - result["elapsed_speedup"]) < 1e-9

    for relative, expected in result["source"]["files"].items():
        actual = sha256(ROOT / relative)
        assert actual == expected, f"{relative}: expected {expected}, found {actual}"

    print("PER-01 daemon task-persistence result verified")


if __name__ == "__main__":
    main()
