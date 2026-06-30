#!/usr/bin/env python3
"""Extend electroswag-v14.json mirror with an empty StringTable fixture."""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V14 = ROOT / "tests" / "fixtures" / "electroswag-v14.json"
V15 = ROOT / "tests" / "fixtures" / "electroswag-v15.json"

ST_SIMPLE_PACKAGE = {
    "file": "Content/E2EFixture/Data/ST_Simple.uasset",
    "package_name": "/Game/E2EFixture/Data/ST_Simple",
}

ST_SIMPLE = {
    "file": "Content/E2EFixture/Data/ST_Simple.uasset",
    "object_path": "/Game/E2EFixture/Data/ST_Simple.ST_Simple",
    "namespace": "ST_Simple",
    "entries": [],
}


def append_unique(entries: list[dict], entry: dict, key: str = "file") -> None:
    if not any(existing[key] == entry[key] for existing in entries):
        entries.append(entry)


def main() -> None:
    contract = json.loads(V14.read_text(encoding="utf-8"))
    contract["contract_version"] = "electroswag-15"

    append_unique(contract["assets"], ST_SIMPLE_PACKAGE)
    append_unique(contract.setdefault("string_tables", []), ST_SIMPLE)

    V15.write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {V15}")


if __name__ == "__main__":
    main()
