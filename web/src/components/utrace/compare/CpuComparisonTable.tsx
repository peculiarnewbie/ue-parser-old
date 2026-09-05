import type { ColumnDef } from "@tanstack/solid-table";
import { createMemo, createSignal } from "solid-js";
import { formatCompact, formatNumber } from "../../../lib/format";
import type { TimerComparisonRow, TimerDisposition } from "../../../lib/utrace-comparison/model";
import { SortableTable } from "../SortableTable";

const FILTERS = ["regressions", "improvements", "new_removed", "all"] as const;
type TimerFilter = (typeof FILTERS)[number];

function deltaLabel(value: number | null): string {
  if (value == null) return "—";
  return `${value > 0 ? "+" : ""}${formatNumber(value, 3)}`;
}

function relativeLabel(value: number | null): string {
  if (value == null) return "—";
  return `${value > 0 ? "+" : ""}${formatNumber(value * 100, 1)}%`;
}

function filterDisposition(filter: TimerFilter, disposition: TimerDisposition): boolean {
  switch (filter) {
    case "regressions": return disposition === "slower" || disposition === "candidate_only";
    case "improvements": return disposition === "faster";
    case "new_removed": return disposition === "candidate_only" || disposition === "baseline_only";
    case "all": return true;
  }
}

export function CpuComparisonTable(props: {
  rows: TimerComparisonRow[];
  onSelect?: (row: TimerComparisonRow) => void;
}) {
  const [filter, setFilter] = createSignal<TimerFilter>("regressions");
  const filtered = createMemo(() => props.rows.filter((row) => filterDisposition(filter(), row.disposition)));
  const normalization = createMemo(() => props.rows[0]?.normalization ?? "game_frame");
  const impactUnit = () => normalization() === "game_frame" ? "ms/frame" : "ms/selected s";
  const rateUnit = () => normalization() === "game_frame" ? "calls/frame" : "calls/selected s";
  const columns: ColumnDef<TimerComparisonRow, unknown>[] = [
    {
      accessorKey: "name",
      header: "Timer",
      cell: (info) => {
        const row = info.row.original;
        return <span class="compare-timer-name"><strong>{row.name}</strong><small>{row.source ?? row.matchConfidence.replace("_", " ")}</small></span>;
      },
    },
    {
      accessorKey: "disposition",
      header: "State",
      cell: (info) => <span class={`compare-disposition compare-disposition-${info.getValue<string>()}`}>{info.getValue<string>().replace("_", " ")}</span>,
    },
    {
      accessorKey: "baselineNormalizedMs",
      header: () => `A ${impactUnit()}`,
      cell: (info) => formatNumber(info.getValue<number | null>() ?? Number.NaN, 3),
      sortingFn: "basic",
    },
    {
      accessorKey: "candidateNormalizedMs",
      header: () => `B ${impactUnit()}`,
      cell: (info) => formatNumber(info.getValue<number | null>() ?? Number.NaN, 3),
      sortingFn: "basic",
    },
    {
      accessorKey: "deltaNormalizedMs",
      header: () => `Δ ${impactUnit()}`,
      cell: (info) => <span class={Number(info.getValue()) > 0 ? "compare-worse" : Number(info.getValue()) < 0 ? "compare-better" : ""}>{deltaLabel(info.getValue<number | null>())}</span>,
      sortingFn: "basic",
    },
    {
      accessorKey: "relativeDelta",
      header: "Δ %",
      cell: (info) => relativeLabel(info.getValue<number | null>()),
      sortingFn: "basic",
    },
    {
      accessorKey: "candidateCountPerUnit",
      header: () => `B ${rateUnit()}`,
      cell: (info) => formatNumber(info.getValue<number | null>() ?? Number.NaN, 2),
      sortingFn: "basic",
    },
    {
      accessorKey: "candidateMeanUs",
      header: "B mean µs",
      cell: (info) => formatCompact(info.getValue<number | null>() ?? Number.NaN),
      sortingFn: "basic",
    },
  ];

  return (
    <SortableTable
      title="CPU attribution"
      eyebrow={normalization() === "game_frame" ? "Whole captures · inclusive" : "Selected ranges · inclusive"}
      subtitle={`${filtered().length.toLocaleString()} of ${props.rows.length.toLocaleString()} matched timer identities · inclusive time normalized by ${normalization() === "game_frame" ? "completed Game frame" : "each independently selected second"}`}
      data={filtered()}
      columns={columns}
      initialSort={[]}
      filterPlaceholder="timer or source…"
      filterFn={(row, query) => row.name.toLocaleLowerCase().includes(query) || row.source?.includes(query) === true}
      onRowClick={props.onSelect}
      rowClass={(row) => ({
        "compare-row-regression": row.disposition === "slower" || row.disposition === "candidate_only",
        "compare-row-improvement": row.disposition === "faster",
      })}
      actions={
        <div class="toggle-group compare-table-filters" role="group" aria-label="Timer disposition">
          {FILTERS.map((value) => (
            <button
              type="button"
              class="toggle-btn"
              classList={{ active: filter() === value }}
              onClick={() => setFilter(value)}
            >
              {value.replace("_", " ")}
            </button>
          ))}
        </div>
      }
    />
  );
}
