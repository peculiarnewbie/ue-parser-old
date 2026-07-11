import type { PropertyValue, UassetProperty } from "./types";

/** Flattened row for TanStack — row name plus one string cell per column. */
export type DataTableRow = {
  __rowName: string;
  [column: string]: string;
};

export function isDataTableKind(kind: string): boolean {
  return kind === "DataTable" || kind === "CompositeDataTable";
}

export function isDataAssetKind(kind: string): boolean {
  return kind === "DataAsset" || kind === "PrimaryDataAsset";
}

/** Flattened property row for the DataAsset inspector. */
export type PropertyRow = {
  path: string;
  name: string;
  type: string;
  value: string;
  depth: number;
  isStruct: boolean;
};

/** Expand top-level and nested struct properties into inspector rows. */
export function flattenProperties(
  properties: UassetProperty[],
  parentPath = "",
  depth = 0,
): PropertyRow[] {
  const rows: PropertyRow[] = [];
  for (const prop of properties) {
    const path = parentPath ? `${parentPath}.${prop.name}` : prop.name;
    if (prop.value_kind === "struct") {
      const fieldCount = prop.properties.length;
      rows.push({
        path,
        name: path,
        type: prop.type,
        value: fieldCount === 0 ? "{}" : `{${fieldCount} fields}`,
        depth,
        isStruct: true,
      });
      rows.push(...flattenProperties(prop.properties, path, depth + 1));
    } else {
      rows.push({
        path,
        name: depth > 0 ? path : prop.name,
        type: prop.type,
        value: formatPropertyValue(prop),
        depth,
        isStruct: false,
      });
    }
  }
  return rows;
}

export function formatPropertyValue(value: PropertyValue): string {
  switch (value.value_kind) {
    case "bool":
    case "int":
    case "uint":
    case "float":
    case "double":
    case "name":
    case "enum":
    case "string":
    case "text":
    case "guid":
    case "soft_object_path":
      return String(value.value);
    case "object_ref":
      return value.value ?? "None";
    case "vector":
      return `(${value.x}, ${value.y}, ${value.z})`;
    case "array":
      return `[${value.values.map(formatPropertyValue).join(", ")}]`;
    case "set":
      return `{${value.values.map(formatPropertyValue).join(", ")}}`;
    case "map":
      return `{${value.entries
        .map(
          (entry) =>
            `${formatPropertyValue(entry.key)}: ${formatPropertyValue(entry.value)}`,
        )
        .join(", ")}}`;
    case "struct":
      return `{${value.properties
        .map((prop) => `${prop.name}: ${formatPropertyValue(prop as PropertyValue)}`)
        .join(", ")}}`;
    case "raw":
      return `raw(${value.reason}, ${value.size}b)`;
  }
}

export function collectColumnNames(rows: { properties: UassetProperty[] }[]): string[] {
  const seen = new Set<string>();
  const columns: string[] = [];
  for (const row of rows) {
    for (const prop of row.properties) {
      if (!seen.has(prop.name)) {
        seen.add(prop.name);
        columns.push(prop.name);
      }
    }
  }
  return columns;
}

export function flattenDataTableRows(
  rows: { name: string; properties: UassetProperty[] }[],
  columns: string[],
): DataTableRow[] {
  return rows.map((row) => {
    const cells: DataTableRow = { __rowName: row.name };
    const byName = new Map(row.properties.map((prop) => [prop.name, prop]));
    for (const column of columns) {
      const prop = byName.get(column);
      cells[column] = prop ? formatPropertyValue(prop) : "";
    }
    return cells;
  });
}
