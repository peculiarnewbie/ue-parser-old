#!/usr/bin/env python3
"""Extend electroswag-v12.json mirror with weak/lazy object refs fixture."""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V12 = ROOT / "tests" / "fixtures" / "electroswag-v12.json"
V13 = ROOT / "tests" / "fixtures" / "electroswag-v13.json"

DA_WEAK_LAZY_PACKAGE = {
    "file": "Content/E2EFixture/Data/DA_WeakLazyRefs.uasset",
    "package_name": "/Game/E2EFixture/Data/DA_WeakLazyRefs",
}

DA_WEAK_LAZY = {
    "file": "Content/E2EFixture/Data/DA_WeakLazyRefs.uasset",
    "object_path": "/Game/E2EFixture/Data/DA_WeakLazyRefs.DA_WeakLazyRefs",
    "class_path": "E2EFixtureWeakLazyRefsDataAsset",
    "columns": ["WeakObject", "LazyObject"],
    "cells": [
        {
            "column": "WeakObject",
            "value": {
                "object_path": "/Game/E2EFixture/Data/DA_Scalars.DA_Scalars",
            },
        },
        {
            "column": "LazyObject",
            "value": {
                "object_path": "/Game/E2EFixture/Data/DA_Scalars.DA_Scalars",
            },
        },
    ],
}


def append_unique(entries: list[dict], entry: dict, key: str = "file") -> None:
    if not any(existing[key] == entry[key] for existing in entries):
        entries.append(entry)


def main() -> None:
    contract = json.loads(V12.read_text(encoding="utf-8"))
    contract["contract_version"] = "electroswag-13"

    append_unique(contract["assets"], DA_WEAK_LAZY_PACKAGE)
    append_unique(contract.setdefault("data_assets", []), DA_WEAK_LAZY)

    V13.write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {V13}")


if __name__ == "__main__":
    main()
