---
name: ue-source-reference
description: Local Unreal Engine source tree for verifying serialization contracts
metadata:
  type: reference
---

The parser targets UE 5.7.2 uncooked editor packages. When you need to verify
wire-format contracts (package summary layout, version gates, property tags,
primitive serialization), check the local engine source at:

```text
C:\Users\Ryzen\Perforce\Arif_UE-ManaBreak
```

Key files mapped to this repo:

| Parser area | UE source |
|-------------|-----------|
| `src/package.rs` — `FPackageFileSummary`, import/export maps | `Engine/Source/Runtime/CoreUObject/Public/UObject/PackageFileSummary.h` |
| `src/package.rs`, `src/version.rs` — `UE4_*` / `UE5_*` gates | `Engine/Source/Runtime/Core/Public/UObject/ObjectVersion.h` |
| `src/property.rs` — tagged-property envelope | `Engine/Source/Runtime/CoreUObject/Private/UObject/PropertyTag.cpp` |
| `src/archive.rs` — `FName`, `FString`, `TArray` | `Engine/Source/Runtime/Core/Public/UObject/NameTypes.h`, `Containers/UnrealString.h` |
| `src/codec.rs` — soft object paths (inline + index) | `Engine/Source/Runtime/CoreUObject/Public/UObject/SoftObjectPath.h` |
| `src/package.rs` — `Summary.SoftObjectPaths` | `Engine/Source/Runtime/CoreUObject/Private/UObject/LinkerLoad.cpp` (`SerializeSoftObjectPathList`), `SavePackage2.cpp` |
| `src/asset.rs` — DataTable row payload | `Engine/Source/Runtime/Engine/Private/DataTable.cpp` |
| `src/asset.rs` — DataAsset UObject properties | `Engine/Source/Runtime/Engine/Private/DataAsset.cpp` |

Fixture expected values are separate — see [[fixture-ground-truth-contract]].
