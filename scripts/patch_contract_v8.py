#!/usr/bin/env python3
"""Patch electroswag contract.ts for fixture contract v8."""

from pathlib import Path

CONTRACT = Path(r"C:\Users\Ryzen\git\swag\electroswag\e2e\fixtures\unreal\contract.ts")

NEW_INTERFACES = '''
export interface WideScalarsTable extends TableRef {
	readonly columns: {
		readonly byte: string;
		readonly int8: string;
		readonly int16: string;
		readonly int64: string;
		readonly uint32: string;
		readonly double: string;
		readonly name: string;
	};
	readonly sampleRow: {
		readonly rowName: string;
		readonly byteValue: number;
		readonly int8Value: number;
		readonly int16Value: number;
		readonly int64Value: number;
		readonly uint32Value: number;
		readonly doubleValue: number;
		readonly nameValue: string;
	};
}

export interface CompositeFixtureRef {
	readonly objectPath: string;
	readonly name: string;
	readonly parentTablePaths: readonly string[];
	readonly composedRowNames: readonly string[];
}

export interface AssetRefTable extends TableRef {
	readonly textureColumn: string;
	/** Committed soft-object path stored in the fixture asset (not just the M8 round-trip target). */
	readonly committedTexturePath: string;
}

'''


def main() -> None:
    text = CONTRACT.read_text(encoding="utf-8")
    text = text.replace('export const FIXTURE_CONTRACT_VERSION = "7";', 'export const FIXTURE_CONTRACT_VERSION = "8";')

    if "WideScalarsTable" not in text:
        text = text.replace(
            "export interface LocalizedTable extends TableRef {",
            NEW_INTERFACES + "export interface LocalizedTable extends TableRef {",
        )

    text = text.replace(
        "\treadonly columns: {\n\t\treadonly localizedText: string;\n\t};",
        "\treadonly columns: {\n\t\treadonly localizedText: string;\n\t};\n"
        "\t/** Non-empty FText source string committed on Loc_Row1. */\n"
        "\treadonly committedLocalizedSource: string;",
    )

    if "readonly emptyTable:" not in text:
        text = text.replace(
            "\treadonly assetRefTable: TableRef;",
            "\treadonly assetRefTable: AssetRefTable;\n"
            "\treadonly emptyTable: TableRef;\n"
            "\treadonly wideScalarsTable: WideScalarsTable;",
        )

    if "readonly compositeEmptyTable:" not in text:
        text = text.replace(
            "\treadonly compositeTable: {",
            "\treadonly compositeEmptyTable: CompositeFixtureRef;\n"
            "\treadonly compositeNestedTable: CompositeFixtureRef;\n"
            "\treadonly compositeTable: {",
        )

    default_asset_ref = """\tassetRefTable: {
\t\tname: "DT_AssetRefs",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/DT_AssetRefs.DT_AssetRefs`,
\t\trowNames: ["AssetRef_One"]
\t},"""

    new_asset_ref = """\tassetRefTable: {
\t\tname: "DT_AssetRefs",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/DT_AssetRefs.DT_AssetRefs`,
\t\trowNames: ["AssetRef_One"],
\t\ttextureColumn: "Texture",
\t\tcommittedTexturePath: "/Engine/EngineResources/DefaultTexture.DefaultTexture"
\t},"""

    text = text.replace(default_asset_ref, new_asset_ref)

    default_localized = """\tlocalizedTable: {
\t\tcolumns: {
\t\t\tlocalizedText: "LocalizedText"
\t\t},
\t\tname: "DT_Localized",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/DT_Localized.DT_Localized`,
\t\trowNames: ["Loc_Row1", "Loc_Row2"]
\t},"""

    new_localized = """\tlocalizedTable: {
\t\tcolumns: {
\t\t\tlocalizedText: "LocalizedText"
\t\t},
\t\tcommittedLocalizedSource: "E2E localized source",
\t\tname: "DT_Localized",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/DT_Localized.DT_Localized`,
\t\trowNames: ["Loc_Row1", "Loc_Row2"]
\t},"""

    text = text.replace(default_localized, new_localized)

    composite_block = """\tcompositeTable: {
\t\tcomposedRowNames: ["Row_Alpha", "Row_Beta", "Row_Gamma"],
\t\tname: "CDT_E2EFixture",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/CDT_E2EFixture.CDT_E2EFixture`,"""

    new_composite_block = """\tcompositeEmptyTable: {
\t\tcomposedRowNames: [],
\t\tname: "CDT_Empty",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/CDT_Empty.CDT_Empty`,
\t\tparentTablePaths: []
\t},
\tcompositeNestedTable: {
\t\tcomposedRowNames: ["Row_Alpha", "Row_Beta", "Row_Gamma"],
\t\tname: "CDT_Nested",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/CDT_Nested.CDT_Nested`,
\t\tparentTablePaths: [`${FIXTURE_ROOT}/Data/CDT_E2EFixture.CDT_E2EFixture`]
\t},
\tcompositeTable: {
\t\tcomposedRowNames: ["Row_Alpha", "Row_Beta", "Row_Gamma"],
\t\tname: "CDT_E2EFixture",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/CDT_E2EFixture.CDT_E2EFixture`,"""

    text = text.replace(composite_block, new_composite_block)

    enum_block = """\tenumTable: {
\t\tname: "DT_Enums",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/DT_Enums.DT_Enums`,
\t\trowNames: ["Enum_RowA", "Enum_RowB"]
\t},"""

    new_tables = """\temptyTable: {
\t\tname: "DT_Empty",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/DT_Empty.DT_Empty`,
\t\trowNames: []
\t},
\tenumTable: {
\t\tname: "DT_Enums",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/DT_Enums.DT_Enums`,
\t\trowNames: ["Enum_RowA", "Enum_RowB"]
\t},"""

    text = text.replace(enum_block, new_tables)

    wide_block = """\tlaunchMapPath: `${FIXTURE_ROOT}/Maps/E2EFixtureMap`,"""

    new_wide = """\twideScalarsTable: {
\t\tcolumns: {
\t\t\tbyte: "ByteValue",
\t\t\tdouble: "DoubleValue",
\t\t\tint8: "Int8Value",
\t\t\tint16: "Int16Value",
\t\t\tint64: "Int64Value",
\t\t\tname: "NameValue",
\t\t\tuint32: "UInt32Value"
\t\t},
\t\tname: "DT_WideScalars",
\t\tobjectPath: `${FIXTURE_ROOT}/Data/DT_WideScalars.DT_WideScalars`,
\t\trowNames: ["Wide_Row1"],
\t\tsampleRow: {
\t\t\tbyteValue: 255,
\t\t\tdoubleValue: 2.718281828,
\t\t\tint8Value: -42,
\t\t\tint16Value: -1000,
\t\t\tint64Value: 9_999_999_999,
\t\t\tnameValue: "FixtureName",
\t\t\trowName: "Wide_Row1",
\t\t\tuint32Value: 4_000_000_000
\t\t}
\t},
\tlaunchMapPath: `${FIXTURE_ROOT}/Maps/E2EFixtureMap`,"""

    text = text.replace(wide_block, new_wide)

    resolve_asset_ref = "\t\tassetRefTable: resolveTableRef(\"ASSET_REF\", DEFAULT_CONTRACT.assetRefTable),"
    new_resolve_asset_ref = """\t\tassetRefTable: {
\t\t\t...resolveTableRef("ASSET_REF", DEFAULT_CONTRACT.assetRefTable),
\t\t\tcommittedTexturePath: readString(
\t\t\t\t"UNREAL_E2E_ASSET_REF_COMMITTED_TEXTURE_PATH",
\t\t\t\tDEFAULT_CONTRACT.assetRefTable.committedTexturePath
\t\t\t),
\t\t\ttextureColumn: readString(
\t\t\t\t"UNREAL_E2E_ASSET_REF_TEXTURE_COLUMN",
\t\t\t\tDEFAULT_CONTRACT.assetRefTable.textureColumn
\t\t\t)
\t\t},"""

    text = text.replace(resolve_asset_ref, new_resolve_asset_ref)

    resolve_localized = """\t\tlocalizedTable: {
\t\t\t...resolveTableRef("LOCALIZED", DEFAULT_CONTRACT.localizedTable),
\t\t\tcolumns: {
\t\t\t\tlocalizedText: readString(
\t\t\t\t\t"UNREAL_E2E_LOCALIZED_COLUMN_LOCALIZED_TEXT",
\t\t\t\t\tDEFAULT_CONTRACT.localizedTable.columns.localizedText
\t\t\t\t)
\t\t\t}
\t\t},"""

    new_resolve_localized = """\t\tlocalizedTable: {
\t\t\t...resolveTableRef("LOCALIZED", DEFAULT_CONTRACT.localizedTable),
\t\t\tcolumns: {
\t\t\t\tlocalizedText: readString(
\t\t\t\t\t"UNREAL_E2E_LOCALIZED_COLUMN_LOCALIZED_TEXT",
\t\t\t\t\tDEFAULT_CONTRACT.localizedTable.columns.localizedText
\t\t\t\t)
\t\t\t},
\t\t\tcommittedLocalizedSource: readString(
\t\t\t\t"UNREAL_E2E_LOCALIZED_COMMITTED_SOURCE",
\t\t\t\tDEFAULT_CONTRACT.localizedTable.committedLocalizedSource
\t\t\t)
\t\t},"""

    text = text.replace(resolve_localized, new_resolve_localized)

    if "emptyTable: resolveTableRef" not in text:
        text = text.replace(
            "\t\tenumTable: resolveTableRef(\"ENUM\", DEFAULT_CONTRACT.enumTable),",
            "\t\temptyTable: resolveTableRef(\"EMPTY\", DEFAULT_CONTRACT.emptyTable),\n"
            "\t\tenumTable: resolveTableRef(\"ENUM\", DEFAULT_CONTRACT.enumTable),",
        )

    if "compositeEmptyTable:" not in text.split("resolveFixtureContract")[1]:
        text = text.replace(
            "\t\tcompositeTable: {",
            "\t\tcompositeEmptyTable: DEFAULT_CONTRACT.compositeEmptyTable,\n"
            "\t\tcompositeNestedTable: DEFAULT_CONTRACT.compositeNestedTable,\n"
            "\t\tcompositeTable: {",
            1,
        )

    if "wideScalarsTable:" not in text.split("resolveFixtureContract")[1]:
        text = text.replace(
            "\t\tstructTable: {",
            "\t\twideScalarsTable: DEFAULT_CONTRACT.wideScalarsTable,\n"
            "\t\tstructTable: {",
            1,
        )

    CONTRACT.write_text(text, encoding="utf-8", newline="\n")
    print(f"patched {CONTRACT}")


if __name__ == "__main__":
    main()
