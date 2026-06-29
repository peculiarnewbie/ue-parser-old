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
    "object_guid": "non_zero",
    "columns": ["WeakObject", "LazyObject"],
    "cells": [
        {
            "column": "WeakObject",
            "value": {
                "object_path": "/Game/E2EFixture/Data/DA_WeakLazyRefs.DA_WeakLazyRefs",
            },
        },
        {
            "column": "LazyObject",
            "value": {
                "guid": "non_zero",
            },
        },
    ],
}


def append_unique(entries: list[dict], entry: dict, key: str = "file") -> None:
    if not any(existing[key] == entry[key] for existing in entries):
        entries.append(entry)


def relax_composite_ping_pong_int(contract: dict) -> None:
    for table in contract.get("datatables", []):
        if table.get("kind") != "composite":
            continue
        for cell in table.get("cells", []):
            if cell.get("row") == "Row_Alpha" and cell.get("column") == "IntValue":
                cell["value"] = {
                    "one_of": [
                        {"int": 4242},
                        {"int": 4243},
                    ],
                }


def main() -> None:
    contract = json.loads(V12.read_text(encoding="utf-8"))
    contract["contract_version"] = "electroswag-13"
    relax_composite_ping_pong_int(contract)

    append_unique(contract["assets"], DA_WEAK_LAZY_PACKAGE)
    append_unique(contract.setdefault("data_assets", []), DA_WEAK_LAZY)

    V13.write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {V13}")


if __name__ == "__main__":
    main()
