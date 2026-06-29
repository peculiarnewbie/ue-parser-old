#!/usr/bin/env python3
"""Extend electroswag-v11.json mirror with contract v12 plain UObject fixture."""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V11 = ROOT / "tests" / "fixtures" / "electroswag-v11.json"
V12 = ROOT / "tests" / "fixtures" / "electroswag-v12.json"

UO_PLAIN_PACKAGE = {
    "file": "Content/E2EFixture/Data/UO_Plain.uasset",
    "package_name": "/Game/E2EFixture/Data/UO_Plain",
}

UO_PLAIN = {
    "file": "Content/E2EFixture/Data/UO_Plain.uasset",
    "object_path": "/Game/E2EFixture/Data/UO_Plain.Default__UO_Plain_C",
    "class_path": "UO_Plain_C",
    "columns": ["IntValue", "Label", "NestedStruct"],
    "cells": [
        {
            "column": "IntValue",
            "value": {"int": 9001},
        },
        {
            "column": "Label",
            "value": {"string": "UO_Plain"},
        },
        {
            "column": "NestedStruct",
            "value": {
                "struct_fields": {
                    "NestedInt": {"int": 7},
                    "NestedString": {"string": "nested"},
                    "NestedVector": {"vector": [1.0, 2.0, 3.0]},
                }
            },
        },
    ],
}


def append_unique(entries: list[dict], entry: dict, key: str = "file") -> None:
    if not any(existing[key] == entry[key] for existing in entries):
        entries.append(entry)


def main() -> None:
    contract = json.loads(V11.read_text(encoding="utf-8"))
    contract["contract_version"] = "electroswag-12"

    append_unique(contract["assets"], UO_PLAIN_PACKAGE)
    append_unique(contract.setdefault("uobjects", []), UO_PLAIN)

    V12.write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {V12}")


if __name__ == "__main__":
    main()
