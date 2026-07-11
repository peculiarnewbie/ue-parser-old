import {
  createSolidTable,
  flexRender,
  getCoreRowModel,
  getFilteredRowModel,
  getSortedRowModel,
  type ColumnDef,
  type SortingState,
} from "@tanstack/solid-table";
import { For, Show, createMemo, createSignal } from "solid-js";
import {
  flattenProperties,
  type PropertyRow,
} from "../lib/datatable";
import type { AssetSummary } from "../lib/types";

type DataAssetViewProps = {
  asset: AssetSummary;
};

export function DataAssetView(props: DataAssetViewProps) {
  const [sorting, setSorting] = createSignal<SortingState>([]);
  const [globalFilter, setGlobalFilter] = createSignal("");

  const data = createMemo(() => flattenProperties(props.asset.properties ?? []));

  const columns = createMemo<ColumnDef<PropertyRow>[]>(() => [
    {
      id: "name",
      accessorKey: "name",
      header: "Property",
      cell: (info) => info.getValue<string>(),
    },
    {
      id: "type",
      accessorKey: "type",
      header: "Type",
      cell: (info) => info.getValue<string>(),
    },
    {
      id: "value",
      accessorKey: "value",
      header: "Value",
      cell: (info) => info.getValue<string>() || "—",
    },
  ]);

  const table = createSolidTable({
    get data() {
      return data();
    },
    get columns() {
      return columns();
    },
    state: {
      get sorting() {
        return sorting();
      },
      get globalFilter() {
        return globalFilter();
      },
    },
    onSortingChange: setSorting,
    onGlobalFilterChange: setGlobalFilter,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
  });

  return (
    <section class="panel datatable-panel dataasset-panel">
      <header class="datatable-head">
        <div>
          <p class="eyebrow">{props.asset.kind}</p>
          <h2 class="mono">{props.asset.object_path}</h2>
          <p class="muted datatable-meta">
            <Show when={props.asset.class_path} fallback="no class path">
              class <code>{props.asset.class_path}</code>
            </Show>
            {" · "}
            {props.asset.properties?.length ?? 0} properties
            <Show when={props.asset.object_guid}>
              {" · "}
              guid <code>{props.asset.object_guid}</code>
            </Show>
          </p>
        </div>
        <label class="datatable-filter">
          <span class="muted">Filter</span>
          <input
            type="search"
            value={globalFilter()}
            placeholder="Search properties…"
            onInput={(event) => setGlobalFilter(event.currentTarget.value)}
          />
        </label>
      </header>

      <Show
        when={data().length > 0}
        fallback={<p class="chart-empty">DataAsset decoded with zero properties.</p>}
      >
        <div class="table-wrap datatable-wrap">
          <table class="datatable dataasset-table">
            <thead>
              <For each={table.getHeaderGroups()}>
                {(headerGroup) => (
                  <tr>
                    <For each={headerGroup.headers}>
                      {(header) => (
                        <th
                          classList={{
                            sortable: header.column.getCanSort(),
                            sorted: header.column.getIsSorted() !== false,
                          }}
                          onClick={header.column.getToggleSortingHandler()}
                        >
                          <Show when={!header.isPlaceholder}>
                            <span class="th-label">
                              {flexRender(
                                header.column.columnDef.header,
                                header.getContext(),
                              )}
                              <Show when={header.column.getIsSorted() === "asc"}>
                                <span class="sort-mark">↑</span>
                              </Show>
                              <Show when={header.column.getIsSorted() === "desc"}>
                                <span class="sort-mark">↓</span>
                              </Show>
                            </span>
                          </Show>
                        </th>
                      )}
                    </For>
                  </tr>
                )}
              </For>
            </thead>
            <tbody>
              <For each={table.getRowModel().rows}>
                {(row) => {
                  const depth = row.original.depth;
                  return (
                    <tr
                      classList={{
                        "prop-nested": depth > 0,
                        "prop-struct": row.original.isStruct,
                      }}
                    >
                      <For each={row.getVisibleCells()}>
                        {(cell) => (
                          <td
                            classList={{
                              "prop-name": cell.column.id === "name",
                              "prop-type": cell.column.id === "type",
                              "prop-value": cell.column.id === "value",
                              mono: true,
                            }}
                            data-depth={cell.column.id === "name" ? depth : undefined}
                            title={
                              cell.column.id === "name"
                                ? row.original.path
                                : cell.getValue<string>() || undefined
                            }
                          >
                            {flexRender(cell.column.columnDef.cell, cell.getContext())}
                          </td>
                        )}
                      </For>
                    </tr>
                  );
                }}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
    </section>
  );
}
