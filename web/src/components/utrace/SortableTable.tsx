import {
  createSolidTable,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  type ColumnDef,
  type SortingState,
} from "@tanstack/solid-table";
import { For, Show, createMemo, createSignal, type JSX } from "solid-js";

type SortableTableProps<T extends object> = {
  title: string;
  eyebrow?: string;
  subtitle?: string;
  data: T[];
  columns: ColumnDef<T, unknown>[];
  initialSort?: SortingState;
  maxHeightClass?: string;
  empty?: string;
  filterPlaceholder?: string;
  filterFn?: (row: T, query: string) => boolean;
  onRowClick?: (row: T) => void;
  rowClass?: (row: T) => Record<string, boolean> | undefined;
  actions?: JSX.Element;
};

export function SortableTable<T extends object>(props: SortableTableProps<T>) {
  const [sorting, setSorting] = createSignal<SortingState>(props.initialSort ?? []);
  const [filter, setFilter] = createSignal("");

  const filtered = createMemo(() => {
    const query = filter().trim().toLowerCase();
    if (!query || !props.filterFn) return props.data;
    return props.data.filter((row) => props.filterFn!(row, query));
  });

  const table = createSolidTable({
    get data() {
      return filtered();
    },
    get columns() {
      return props.columns;
    },
    state: {
      get sorting() {
        return sorting();
      },
    },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });

  return (
    <section class="panel">
      <header class="datatable-head">
        <div>
          <Show when={props.eyebrow}>
            <p class="eyebrow">{props.eyebrow}</p>
          </Show>
          <h2>{props.title}</h2>
          <Show when={props.subtitle}>
            <p class="muted datatable-meta">{props.subtitle}</p>
          </Show>
        </div>
        <div class="panel-head-actions">
          {props.actions}
          <Show when={props.filterFn}>
            <label class="datatable-filter">
              Filter
              <input
                value={filter()}
                placeholder={props.filterPlaceholder ?? "filter…"}
                onInput={(event) => setFilter(event.currentTarget.value)}
              />
            </label>
          </Show>
        </div>
      </header>
      <Show
        when={filtered().length > 0}
        fallback={<p class="chart-empty">{props.empty ?? "No rows."}</p>}
      >
        <div class={`table-wrap datatable-wrap ${props.maxHeightClass ?? ""}`}>
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
                            sorted: !!header.column.getIsSorted(),
                          }}
                          onClick={header.column.getToggleSortingHandler()}
                        >
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
                  <tr
                    classList={{
                      clickable: !!props.onRowClick,
                      ...props.rowClass?.(row.original),
                    }}
                    onClick={() => props.onRowClick?.(row.original)}
                  >
                    <For each={row.getVisibleCells()}>
                      {(cell) => (
                        <td class="mono">
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

export function StatCard(props: { label: string; value: string; hint?: string }) {
  return (
    <div class="stat">
      <span class="stat-label">{props.label}</span>
      <span class="stat-value">{props.value}</span>
      <Show when={props.hint}>
        <span class="stat-hint">{props.hint}</span>
      </Show>
    </div>
  );
}
