#!/usr/bin/env python3
"""Extend electroswag-v13.json mirror with a simple CurveTable fixture."""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V13 = ROOT / "tests" / "fixtures" / "electroswag-v13.json"
V14 = ROOT / "tests" / "fixtures" / "electroswag-v14.json"

CT_SIMPLE_PACKAGE = {
    "file": "Content/E2EFixture/Data/CT_Simple.uasset",
    "package_name": "/Game/E2EFixture/Data/CT_Simple",
}

CT_SIMPLE = {
    "file": "Content/E2EFixture/Data/CT_Simple.uasset",
    "object_path": "/Game/E2EFixture/Data/CT_Simple.CT_Simple",
    "rows": [
        {
            "name": "Linear_A",
            "keys": [
                {"time": 0.0, "value": 0.0},
                {"time": 1.0, "value": 10.0},
                {"time": 2.0, "value": 20.0},
            ],
        },
        {
            "name": "Linear_B",
            "keys": [
                {"time": 0.0, "value": 5.0},
                {"time": 1.0, "value": 15.0},
                {"time": 2.0, "value": 30.0},
            ],
        },
    ],
}


def append_unique(entries: list[dict], entry: dict, key: str = "file") -> None:
    if not any(existing[key] == entry[key] for existing in entries):
        entries.append(entry)


def main() -> None:
    contract = json.loads(V13.read_text(encoding="utf-8"))
    contract["contract_version"] = "electroswag-14"

    append_unique(contract["assets"], CT_SIMPLE_PACKAGE)
    append_unique(contract.setdefault("curve_tables", []), CT_SIMPLE)

    V14.write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {V14}")


if __name__ == "__main__":
    main()
