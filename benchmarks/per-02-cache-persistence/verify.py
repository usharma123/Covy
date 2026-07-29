#!/usr/bin/env python3
"""Verify the checked-in PER-02 result and its measured source identity."""

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
    assert result["schema_version"] == 1
    assert result["parity_entries"] == (
        result["fixture_entries"] + result["measured_writes"]
    )

    legacy = result["legacy_full_checkpoint_under_lock"]
    owned = result["owned_delta_wal_after_lock"]
    assert owned["median_write_lock_ns"] < legacy["median_write_lock_ns"]
    assert owned["median_published_bytes"] < legacy["median_published_bytes"]
    assert result["lock_time_reduction_percent"] >= 90.0
    assert result["write_byte_reduction_percent"] >= 90.0

    for relative, expected in result["source"]["files"].items():
        actual = sha256(ROOT / relative)
        assert actual == expected, f"{relative}: expected {expected}, found {actual}"

    print("PER-02 cache-persistence result verified")


if __name__ == "__main__":
    main()
