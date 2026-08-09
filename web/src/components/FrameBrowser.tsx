import {
  createSolidTable,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  type ColumnDef,
  type SortingState,
} from "@tanstack/solid-table";
import { createVirtualizer } from "@tanstack/solid-virtual";
import { For, Show, createMemo, createSignal } from "solid-js";
import type { CorrelatedFrameSummary } from "../lib/types";

const FRAME_ROW_HEIGHT_PX = 36;
const FRAME_ROW_OVERSCAN = 8;

export type FrameRow = {
  frame_number: number;
  frame_label: string;
  elapsed_seconds?: number;
  elapsed_label: string;
  cpu_s: number;
  gpu_work: number;
  gpu_breadcrumbs: number;
  top_scope: string;
  spike: boolean;
};

export type FramePresentation = {
  label: string;
  elapsedSeconds?: number;
  elapsedLabel: string;
};

type FrameBrowserProps = {
  frames: CorrelatedFrameSummary[];
  selectedFrame: number | null;
  onSelect: (frameNumber: number) => void;
  presentFrame?: (frame: CorrelatedFrameSummary) => FramePresentation;
};

function toRows(input: {
  frames: CorrelatedFrameSummary[];
  presentFrame?: (frame: CorrelatedFrameSummary) => FramePresentation;
}): FrameRow[] {
  const costs = input.frames
    .map((frame) => frame.cpu_metadata_seconds ?? 0)
    .filter((value) => value > 0)
    .sort((a, b) => a - b);
  const p90 =
    costs.length === 0 ? Number.POSITIVE_INFINITY : costs[Math.floor(costs.length * 0.9)]!;

  return input.frames.map((frame) => {
    const cpu_s = frame.cpu_metadata_seconds ?? 0;
    const presentation = input.presentFrame?.(frame) ?? {
      label: String(frame.frame_number),
      elapsedLabel: "—",
    };
    return {
      frame_number: frame.frame_number,
      frame_label: presentation.label,
      elapsed_seconds: presentation.elapsedSeconds,
      elapsed_label: presentation.elapsedLabel,
      cpu_s,
      gpu_work: frame.gpu_work_cycles,
      gpu_breadcrumbs: frame.gpu_breadcrumb_cycles,
      top_scope: frame.top_cpu_scopes?.[0]?.name ?? "—",
      spike: cpu_s >= p90 && cpu_s > 0,
    };
  });
}

export function FrameBrowser(props: FrameBrowserProps) {
  const [sorting, setSorting] = createSignal<SortingState>([
    { id: "cpu_s", desc: true },
  ]);
  let tableContainer: HTMLDivElement | undefined;

  const data = createMemo(() =>
    toRows({ frames: props.frames, presentFrame: props.presentFrame }),
  );

  const columns = createMemo<ColumnDef<FrameRow>[]>(() => [
    {
      accessorKey: "frame_number",
      header: "Frame",
      cell: (info) => info.row.original.frame_label,
    },
    {
      accessorKey: "elapsed_seconds",
      header: "Capture time",
      cell: (info) => info.row.original.elapsed_label,
    },
    {
      accessorKey: "cpu_s",
      header: "CPU s",
      cell: (info) => info.getValue<number>().toFixed(4),
    },
    {
      accessorKey: "gpu_work",
      header: "GPU work",
      cell: (info) => formatCompact(info.getValue<number>()),
    },
    {
      accessorKey: "gpu_breadcrumbs",
      header: "GPU crumbs",
      cell: (info) => formatCompact(info.getValue<number>()),
    },
    {
      accessorKey: "top_scope",
      header: "Top scope",
      cell: (info) => info.getValue<string>(),
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
    },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });

  const rows = () => table.getRowModel().rows;
  const rowVirtualizer = createVirtualizer<HTMLDivElement, HTMLTableRowElement>({
    get count() {
      return rows().length;
    },
    getScrollElement: () => tableContainer ?? null,
    estimateSize: () => FRAME_ROW_HEIGHT_PX,
    overscan: FRAME_ROW_OVERSCAN,
  });
  const virtualRows = () => rowVirtualizer.getVirtualItems();
  const paddingTop = () => virtualRows()[0]?.start ?? 0;
  const paddingBottom = () => {
    const last = virtualRows().at(-1);
    return last == null ? 0 : rowVirtualizer.getTotalSize() - last.end;
  };

  return (
    <section class="panel">
      <header class="datatable-head">
        <div>
          <p class="eyebrow">Correlated CPU frames</p>
          <h2>Pick a CPU frame to inspect</h2>
          <p class="muted datatable-meta">
            Sorted by CPU cost by default. Spikes (≥ p90) are marked. Click a row
            to query that frame&apos;s CPU cycle window from the browser index.
          </p>
        </div>
      </header>
      <div
        class="table-wrap datatable-wrap frame-browser-wrap"
        data-utrace-frame-count={rows().length}
        ref={(element) => {
          tableContainer = element;
        }}
      >
        <table class="datatable">
          <thead>
            <For each={table.getHeaderGroups()}>
              {(headerGroup) => (
                <tr>
                  <For each={headerGroup.headers}>
                    {(header) => (
                      <th
                        classList={{ sortable: header.column.getCanSort() }}
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
            <Show when={paddingTop() > 0}>
              <tr class="frame-virtual-spacer" aria-hidden="true">
                <td colSpan={columns().length} style={{ height: `${paddingTop()}px` }} />
              </tr>
            </Show>
            <For each={virtualRows()}>
              {(virtualRow) => {
                const row = () => rows()[virtualRow.index]!;
                return (
                  <tr
                    data-utrace-frame-row
                    data-utrace-frame-virtual-index={virtualRow.index}
                    aria-rowindex={virtualRow.index + 2}
                    classList={{
                      clickable: true,
                      selected: props.selectedFrame === row().original.frame_number,
                      spike: row().original.spike,
                    }}
                    onClick={() => props.onSelect(row().original.frame_number)}
                  >
                    <For each={row().getVisibleCells()}>
                      {(cell) => (
                        <td class="mono">
                          {flexRender(cell.column.columnDef.cell, cell.getContext())}
                        </td>
                      )}
                    </For>
                  </tr>
                );
              }}
            </For>
            <Show when={paddingBottom() > 0}>
              <tr class="frame-virtual-spacer" aria-hidden="true">
                <td colSpan={columns().length} style={{ height: `${paddingBottom()}px` }} />
              </tr>
            </Show>
          </tbody>
        </table>
      </div>
      <footer class="datatable-virtual-status">
        <p class="muted mono">
          {data().length.toLocaleString()} frames · scroll to browse the full capture
        </p>
        <span class="muted">Visible rows render on demand</span>
      </footer>
    </section>
  );
}

function formatCompact(value: number): string {
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}
