#!/usr/bin/env python3
"""Patches Arif_Fixtures + electroswag contract for fixture v8. Run from ue-parser root."""

from __future__ import annotations

import json
import sys
from pathlib import Path

FIXTURE_ROOT = Path(r"D:\Perforce\Arif_Fixtures")
ELECTROSWAG_ROOT = Path(r"C:\Users\Ryzen\git\swag\electroswag")
PARSER_ROOT = Path(__file__).resolve().parents[1]

WIDE_SCALARS_STRUCT = '''
/**
 * DT_WideScalars row struct. Exercises additional primitive widths the parser
 * must decode (byte/uint, narrow ints, int64, double, name).
 *
 * Column names are load-bearing — they match `contract.wideScalarsTable.columns`.
 */
USTRUCT(BlueprintType)
struct E2EFIXTURES_API FE2EFixtureWideScalarsRow : public FTableRowBase
{
	GENERATED_BODY()

	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "E2E")
	uint8 ByteValue = 0;

	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "E2E")
	int8 Int8Value = 0;

	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "E2E")
	int16 Int16Value = 0;

	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "E2E")
	int64 Int64Value = 0;

	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "E2E")
	uint32 UInt32Value = 0;

	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "E2E")
	double DoubleValue = 0.0;

	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "E2E")
	FName NameValue;
};

'''


def patch_types_header() -> None:
    path = FIXTURE_ROOT / "Source/E2EFixtures/E2EFixtureTypes.h"
    text = path.read_text(encoding="utf-8")
    marker = "/** Non-row nested struct embedded inside DT_Structs rows and the fixture actor. */"
    if "FE2EFixtureWideScalarsRow" in text:
        print(f"skip {path} (already patched)")
        return
    path.write_text(text.replace(marker, WIDE_SCALARS_STRUCT + marker), encoding="utf-8", newline="\n")
    print(f"patched {path}")


BOOTSTRAP_SCRIPT = r'''# Contract v8 fixture bootstrap — run inside Unreal Editor Python.
import unreal

DATA_PATH = "/Game/E2EFixture/Data"
SCALARS_STRUCT = "/Script/E2EFixtures.E2EFixtureScalarsRow"
WIDE_STRUCT = "/Script/E2EFixtures.E2EFixtureWideScalarsRow"
ASSET_REF_TEXTURE = "/Engine/EngineResources/DefaultTexture.DefaultTexture"
LOCALIZED_SOURCE = "E2E localized source"


def log(msg: str) -> None:
    unreal.log(f"[fixture-v8] {msg}")


def load_struct(path: str):
    struct = unreal.load_object(None, path)
    if struct is None:
        raise RuntimeError(f"missing struct {path} — compile E2EFixtures first")
    return struct


def save_asset(asset) -> None:
    unreal.EditorAssetLibrary.save_asset(asset.get_path_name(), only_if_is_dirty=False)


def remove_if_exists(asset_path: str) -> None:
    if unreal.EditorAssetLibrary.does_asset_exist(asset_path):
        unreal.EditorAssetLibrary.delete_asset(asset_path)
        log(f"deleted existing {asset_path}")


def create_datatable(asset_name: str, struct_path: str) -> unreal.DataTable:
    remove_if_exists(f"{DATA_PATH}/{asset_name}.{asset_name}")
    factory = unreal.DataTableFactory()
    factory.struct = load_struct(struct_path)
    asset = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        asset_name, DATA_PATH, unreal.DataTable, factory
    )
    if asset is None:
        raise RuntimeError(f"failed to create DataTable {asset_name}")
    save_asset(asset)
    log(f"created DataTable {asset.get_path_name()}")
    return asset


def create_composite(asset_name: str, struct_path: str, parent_paths: list[str]) -> unreal.CompositeDataTable:
    remove_if_exists(f"{DATA_PATH}/{asset_name}.{asset_name}")
    factory = unreal.CompositeDataTableFactory()
    factory.struct = load_struct(struct_path)
    asset = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        asset_name, DATA_PATH, unreal.CompositeDataTable, factory
    )
    if asset is None:
        raise RuntimeError(f"failed to create CompositeDataTable {asset_name}")
    parents = []
    for parent_path in parent_paths:
        parent = unreal.EditorAssetLibrary.load_asset(parent_path)
        if parent is None:
            raise RuntimeError(f"missing parent table {parent_path}")
        parents.append(parent)
    asset.set_editor_property("parent_tables", parents)
    save_asset(asset)
    log(f"created CompositeDataTable {asset.get_path_name()} parents={parent_paths}")
    return asset


def set_row(datatable: unreal.DataTable, row_name: str, row_struct) -> None:
  row_name = unreal.Name(row_name)
  if unreal.DataTableFunctionLibrary.does_data_table_row_exist(datatable, row_name):
    unreal.DataTableFunctionLibrary.remove_data_table_row(datatable, row_name)
  unreal.DataTableFunctionLibrary.add_data_table_row(datatable, row_name, row_struct)


def bootstrap_wide_scalars() -> None:
    table = create_datatable("DT_WideScalars", WIDE_STRUCT)
    row = unreal.E2EFixtureWideScalarsRow()
    row.byte_value = 255
    row.int8_value = -42
    row.int16_value = -1000
    row.int64_value = 9_999_999_999
    row.uint32_value = 4_000_000_000
    row.double_value = 2.718281828
    row.name_value = unreal.Name("FixtureName")
    set_row(table, "Wide_Row1", row)
    save_asset(table)


def bootstrap_empty_table() -> None:
    create_datatable("DT_Empty", SCALARS_STRUCT)


def bootstrap_composite_empty() -> None:
    create_composite("CDT_Empty", SCALARS_STRUCT, [])


def bootstrap_composite_nested() -> None:
    create_composite(
        "CDT_Nested",
        SCALARS_STRUCT,
        [f"{DATA_PATH}/CDT_E2EFixture.CDT_E2EFixture"],
    )


def bootstrap_asset_refs() -> None:
    path = f"{DATA_PATH}/DT_AssetRefs.DT_AssetRefs"
    table = unreal.EditorAssetLibrary.load_asset(path)
    if table is None:
        raise RuntimeError(f"missing {path}")
    row = unreal.E2EFixtureAssetRefRow()
    row.texture = unreal.SoftObjectPath(ASSET_REF_TEXTURE)
    set_row(table, "AssetRef_One", row)
    save_asset(table)
    log(f"populated {path} Texture={ASSET_REF_TEXTURE}")


def bootstrap_localized() -> None:
    path = f"{DATA_PATH}/DT_Localized.DT_Localized"
    table = unreal.EditorAssetLibrary.load_asset(path)
    if table is None:
        raise RuntimeError(f"missing {path}")
    row_one = unreal.E2EFixtureLocalizedRow()
    row_one.localized_text = unreal.Text.from_string(LOCALIZED_SOURCE)
    set_row(table, "Loc_Row1", row_one)
    row_two = unreal.E2EFixtureLocalizedRow()
    row_two.localized_text = unreal.Text.from_string("")
    set_row(table, "Loc_Row2", row_two)
    save_asset(table)
    log(f"populated {path} Loc_Row1={LOCALIZED_SOURCE!r}")


def main() -> None:
    bootstrap_empty_table()
    bootstrap_wide_scalars()
    bootstrap_composite_empty()
    bootstrap_composite_nested()
    bootstrap_asset_refs()
    bootstrap_localized()
    unreal.EditorAssetLibrary.save_directory(DATA_PATH, only_if_is_dirty=False, recursive=True)
    log("fixture v8 bootstrap complete")


main()
'''


def write_bootstrap_script() -> Path:
    path = FIXTURE_ROOT / "Scripts/bootstrap_fixture_v8.py"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(BOOTSTRAP_SCRIPT, encoding="utf-8", newline="\n")
    print(f"wrote {path}")
    return path


def main() -> int:
    if not FIXTURE_ROOT.is_dir():
        print(f"missing fixture root {FIXTURE_ROOT}", file=sys.stderr)
        return 1
    patch_types_header()
    write_bootstrap_script()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
