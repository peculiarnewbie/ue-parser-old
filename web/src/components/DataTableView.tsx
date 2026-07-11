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
  collectColumnNames,
  flattenDataTableRows,
  type DataTableRow,
} from "../lib/datatable";
import type { AssetSummary } from "../lib/types";

type DataTableViewProps = {
  asset: AssetSummary;
};

export function DataTableView(props: DataTableViewProps) {
  const [sorting, setSorting] = createSignal<SortingState>([]);
  const [globalFilter, setGlobalFilter] = createSignal("");

  const columnNames = createMemo(() => collectColumnNames(props.asset.rows ?? []));
  const data = createMemo(() =>
    flattenDataTableRows(props.asset.rows ?? [], columnNames()),
  );

  const columns = createMemo<ColumnDef<DataTableRow>[]>(() => [
    {
      id: "__rowName",
      accessorKey: "__rowName",
      header: "Row",
      cell: (info) => info.getValue<string>(),
    },
    ...columnNames().map(
      (name): ColumnDef<DataTableRow> => ({
        id: name,
        accessorKey: name,
        header: name,
        cell: (info) => info.getValue<string>() || "—",
      }),
    ),
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
    <section class="panel datatable-panel">
      <header class="datatable-head">
        <div>
          <p class="eyebrow">{props.asset.kind}</p>
          <h2 class="mono">{props.asset.object_path}</h2>
          <p class="muted datatable-meta">
            <Show when={props.asset.row_struct} fallback="no row struct">
              row struct <code>{props.asset.row_struct}</code>
            </Show>
            {" · "}
            {props.asset.row_count} rows
            <Show when={(props.asset.parent_tables?.length ?? 0) > 0}>
              {" · "}
              parents {props.asset.parent_tables!.join(", ")}
            </Show>
          </p>
        </div>
        <label class="datatable-filter">
          <span class="muted">Filter</span>
          <input
            type="search"
            value={globalFilter()}
            placeholder="Search rows…"
            onInput={(event) => setGlobalFilter(event.currentTarget.value)}
          />
        </label>
      </header>

      <Show
        when={data().length > 0}
        fallback={<p class="chart-empty">DataTable decoded with zero rows.</p>}
      >
        <div class="table-wrap datatable-wrap">
          <table class="datatable">
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
                {(row) => (
                  <tr>
                    <For each={row.getVisibleCells()}>
                      {(cell) => (
                        <td
                          classList={{
                            "row-name": cell.column.id === "__rowName",
                            mono: true,
                          }}
                        >
                          {flexRender(cell.column.columnDef.cell, cell.getContext())}
                        </td>
                      )}
                    </For>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
    </section>
  );
}
