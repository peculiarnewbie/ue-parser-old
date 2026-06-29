#!/usr/bin/env python3
"""Extend electroswag-v9.json mirror with DA_MapSet for contract v10."""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V9 = ROOT / "tests" / "fixtures" / "electroswag-v9.json"
V10 = ROOT / "tests" / "fixtures" / "electroswag-v10.json"

DA_MAPSET_PACKAGE = {
    "file": "Content/E2EFixture/Data/DA_MapSet.uasset",
    "package_name": "/Game/E2EFixture/Data/DA_MapSet",
}

DA_MAPSET = {
    "file": "Content/E2EFixture/Data/DA_MapSet.uasset",
    "object_path": "/Game/E2EFixture/Data/DA_MapSet.DA_MapSet",
    "class_path": "E2EFixtureMapSetDataAsset",
    "columns": ["IntToStringMap", "NameSet"],
    "cells": [
        {
            "column": "IntToStringMap",
            "value": {
                "map_entries": [
                    {"key": {"int": 1}, "value": {"string": "one"}},
                    {"key": {"int": 2}, "value": {"string": "two"}},
                ]
            },
        },
        {
            "column": "NameSet",
            "value": {
                "set_values": [
                    {"name": "Alpha"},
                    {"name": "Beta"},
                ]
            },
        },
    ],
}


def main() -> None:
    contract = json.loads(V9.read_text(encoding="utf-8"))
    contract["contract_version"] = "electroswag-10"
    existing = {entry["file"] for entry in contract["assets"]}
    if DA_MAPSET_PACKAGE["file"] not in existing:
        contract["assets"].append(DA_MAPSET_PACKAGE)
    data_assets = contract.setdefault("data_assets", [])
    if not any(entry["file"] == DA_MAPSET["file"] for entry in data_assets):
        data_assets.append(DA_MAPSET)
    V10.write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {V10}")


if __name__ == "__main__":
    main()
