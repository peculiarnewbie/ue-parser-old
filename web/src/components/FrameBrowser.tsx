import {
  createSolidTable,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  type ColumnDef,
  type SortingState,
} from "@tanstack/solid-table";
import { For, Show, createMemo, createSignal } from "solid-js";
import type { CorrelatedFrameSummary } from "../lib/types";

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
      <div class="table-wrap datatable-wrap frame-browser-wrap">
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
            <For each={table.getRowModel().rows}>
              {(row) => (
                <tr
                  classList={{
                    clickable: true,
                    selected: props.selectedFrame === row.original.frame_number,
                    spike: row.original.spike,
                  }}
                  onClick={() => props.onSelect(row.original.frame_number)}
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
    </section>
  );
}

function formatCompact(value: number): string {
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}
