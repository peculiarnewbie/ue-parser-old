#!/usr/bin/env python3
"""Extend electroswag-v10.json mirror with contract v11 assets."""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V10 = ROOT / "tests" / "fixtures" / "electroswag-v10.json"
V11 = ROOT / "tests" / "fixtures" / "electroswag-v11.json"

V11_PACKAGES = [
    {
        "file": "Content/E2EFixture/Data/DT_MapSet.uasset",
        "package_name": "/Game/E2EFixture/Data/DT_MapSet",
    },
    {
        "file": "Content/E2EFixture/Data/DA_Collections.uasset",
        "package_name": "/Game/E2EFixture/Data/DA_Collections",
    },
    {
        "file": "Content/E2EFixture/Data/DA_Localized.uasset",
        "package_name": "/Game/E2EFixture/Data/DA_Localized",
    },
]

DT_MAPSET = {
    "file": "Content/E2EFixture/Data/DT_MapSet.uasset",
    "object_path": "/Game/E2EFixture/Data/DT_MapSet.DT_MapSet",
    "row_struct": "E2EFixtureMapSetRow",
    "rows": ["MapSet_One", "MapSet_Two"],
    "columns": ["IntToStringMap", "NameSet", "Label"],
    "cells": [
        {
            "row": "MapSet_One",
            "column": "IntToStringMap",
            "value": {
                "map_entries": [
                    {"key": {"int": 1}, "value": {"string": "one"}},
                    {"key": {"int": 2}, "value": {"string": "two"}},
                ]
            },
        },
        {
            "row": "MapSet_One",
            "column": "NameSet",
            "value": {
                "set_values": [
                    {"name": "Alpha"},
                    {"name": "Beta"},
                ]
            },
        },
        {
            "row": "MapSet_One",
            "column": "Label",
            "value": {"string": "first row"},
        },
        {
            "row": "MapSet_Two",
            "column": "IntToStringMap",
            "value": {
                "map_entries": [
                    {"key": {"int": 3}, "value": {"string": "three"}},
                ]
            },
        },
        {
            "row": "MapSet_Two",
            "column": "NameSet",
            "value": {
                "set_values": [
                    {"name": "Gamma"},
                ]
            },
        },
        {
            "row": "MapSet_Two",
            "column": "Label",
            "value": {"string": "second row"},
        },
    ],
}

DA_COLLECTIONS = {
    "file": "Content/E2EFixture/Data/DA_Collections.uasset",
    "object_path": "/Game/E2EFixture/Data/DA_Collections.DA_Collections",
    "class_path": "E2EFixtureCollectionsDataAsset",
    "columns": ["IntList", "NameList"],
    "cells": [
        {
            "column": "IntList",
            "value": {"ints": [10, 20, 30]},
        },
        {
            "column": "NameList",
            "value": {
                "names": ["TagA", "TagB"],
            },
        },
    ],
}

DA_LOCALIZED = {
    "file": "Content/E2EFixture/Data/DA_Localized.uasset",
    "object_path": "/Game/E2EFixture/Data/DA_Localized.DA_Localized",
    "class_path": "E2EFixtureLocalizedDataAsset",
    "columns": ["LocalizedText"],
    "cells": [
        {
            "column": "LocalizedText",
            "value": {"text": "E2E DA localized source"},
        }
    ],
}


def append_unique(entries: list[dict], entry: dict, key: str = "file") -> None:
    if not any(existing[key] == entry[key] for existing in entries):
        entries.append(entry)


def main() -> None:
    contract = json.loads(V10.read_text(encoding="utf-8"))
    contract["contract_version"] = "electroswag-11"

    for package in V11_PACKAGES:
        append_unique(contract["assets"], package)

    append_unique(contract["datatables"], DT_MAPSET)
    append_unique(contract["data_assets"], DA_COLLECTIONS)
    append_unique(contract["data_assets"], DA_LOCALIZED)

    V11.write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {V11}")


if __name__ == "__main__":
    main()
