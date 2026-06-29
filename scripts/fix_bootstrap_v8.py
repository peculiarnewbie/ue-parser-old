from pathlib import Path

p = Path(r"D:\Perforce\Arif_Fixtures\Scripts\bootstrap_fixture_v8.py")
text = p.read_text(encoding="utf-8")
replacements = [
    (
        "    row.byte_value = 255\n    row.int8_value = -42\n    row.int16_value = -1000\n    row.int64_value = 9_999_999_999\n    row.uint32_value = 4_000_000_000\n    row.double_value = 2.718281828\n    row.name_value = unreal.Name(\"FixtureName\")",
        "    row.set_editor_property(\"ByteValue\", 255)\n    row.set_editor_property(\"Int8Value\", -42)\n    row.set_editor_property(\"Int16Value\", -1000)\n    row.set_editor_property(\"Int64Value\", 9_999_999_999)\n    row.set_editor_property(\"UInt32Value\", 4_000_000_000)\n    row.set_editor_property(\"DoubleValue\", 2.718281828)\n    row.set_editor_property(\"NameValue\", unreal.Name(\"FixtureName\"))",
    ),
    (
        "    row.texture = unreal.SoftObjectPath(ASSET_REF_TEXTURE)",
        "    row.set_editor_property(\"Texture\", unreal.SoftObjectPath(ASSET_REF_TEXTURE))",
    ),
    (
        "    row_one.localized_text = unreal.Text.from_string(LOCALIZED_SOURCE)",
        "    row_one.set_editor_property(\"LocalizedText\", unreal.Text.from_string(LOCALIZED_SOURCE))",
    ),
    (
        "    row_two.localized_text = unreal.Text.from_string(\"\")",
        "    row_two.set_editor_property(\"LocalizedText\", unreal.Text.from_string(\"\"))",
    ),
]
for old, new in replacements:
    text = text.replace(old, new)
p.write_text(text, encoding="utf-8", newline="\n")
print("fixed", p)
