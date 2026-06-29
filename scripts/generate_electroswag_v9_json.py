#!/usr/bin/env python3
"""Extend electroswag-v8.json mirror with data_assets for contract v9."""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V8 = ROOT / "tests" / "fixtures" / "electroswag-v8.json"
V9 = ROOT / "tests" / "fixtures" / "electroswag-v9.json"

DATA_ASSET_PACKAGES = [
    {
        "file": "Content/E2EFixture/Data/DA_Empty.uasset",
        "package_name": "/Game/E2EFixture/Data/DA_Empty",
    },
    {
        "file": "Content/E2EFixture/Data/DA_Scalars.uasset",
        "package_name": "/Game/E2EFixture/Data/DA_Scalars",
    },
    {
        "file": "Content/E2EFixture/Data/DA_Structs.uasset",
        "package_name": "/Game/E2EFixture/Data/DA_Structs",
    },
    {
        "file": "Content/E2EFixture/Data/DA_Enums.uasset",
        "package_name": "/Game/E2EFixture/Data/DA_Enums",
    },
    {
        "file": "Content/E2EFixture/Data/DA_WideScalars.uasset",
        "package_name": "/Game/E2EFixture/Data/DA_WideScalars",
    },
]

DATA_ASSETS = [
    {
        "file": "Content/E2EFixture/Data/DA_Empty.uasset",
        "object_path": "/Game/E2EFixture/Data/DA_Empty.DA_Empty",
        "class_path": "E2EFixtureEmptyDataAsset",
        "columns": [],
        "cells": [],
    },
    {
        "file": "Content/E2EFixture/Data/DA_Scalars.uasset",
        "object_path": "/Game/E2EFixture/Data/DA_Scalars.DA_Scalars",
        "class_path": "E2EFixtureScalarsDataAsset",
        "columns": ["IntValue", "FloatValue", "BoolValue", "StringValue"],
        "cells": [
            {"column": "IntValue", "value": {"int": 4243}},
            {"column": "FloatValue", "value": {"float": 1.5}},
            {"column": "BoolValue", "value": {"bool": True}},
            {"column": "StringValue", "value": {"string": "DA_Scalars"}},
        ],
    },
    {
        "file": "Content/E2EFixture/Data/DA_Structs.uasset",
        "object_path": "/Game/E2EFixture/Data/DA_Structs.DA_Structs",
        "class_path": "E2EFixtureStructsDataAsset",
        "columns": ["Nested", "Label"],
        "cells": [
            {
                "column": "Nested",
                "value": {
                    "struct_fields": {
                        "NestedInt": {"int": 7},
                        "NestedString": {"string": "nested"},
                        "NestedVector": {"vector": [1.0, 2.0, 3.0]},
                    }
                },
            },
            {"column": "Label", "value": {"string": "DA_Structs"}},
        ],
    },
    {
        "file": "Content/E2EFixture/Data/DA_Enums.uasset",
        "object_path": "/Game/E2EFixture/Data/DA_Enums.DA_Enums",
        "class_path": "E2EFixtureEnumsDataAsset",
        "columns": ["EnumValue"],
        "cells": [
            {"column": "EnumValue", "value": {"enum": "EE2EFixtureEnum::Beta"}},
        ],
    },
    {
        "file": "Content/E2EFixture/Data/DA_WideScalars.uasset",
        "object_path": "/Game/E2EFixture/Data/DA_WideScalars.DA_WideScalars",
        "class_path": "E2EFixtureWideScalarsDataAsset",
        "columns": [
            "ByteValue",
            "Int8Value",
            "Int16Value",
            "Int64Value",
            "UInt32Value",
            "DoubleValue",
            "NameValue",
        ],
        "cells": [
            {"column": "ByteValue", "value": {"uint": 255}},
            {"column": "Int8Value", "value": {"int": -42}},
            {"column": "Int16Value", "value": {"int": -1000}},
            {"column": "Int64Value", "value": {"int": 9_999_999_999}},
            {"column": "UInt32Value", "value": {"uint": 4_000_000_000}},
            {"column": "DoubleValue", "value": {"double": 2.718281828}},
            {"column": "NameValue", "value": {"name": "FixtureName"}},
        ],
    },
]


def main() -> None:
    contract = json.loads(V8.read_text(encoding="utf-8"))
    contract["contract_version"] = "electroswag-9"
    existing = {entry["file"] for entry in contract["assets"]}
    for entry in DATA_ASSET_PACKAGES:
        if entry["file"] not in existing:
            contract["assets"].append(entry)
    contract["data_assets"] = DATA_ASSETS
    V9.write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {V9}")


if __name__ == "__main__":
    main()
