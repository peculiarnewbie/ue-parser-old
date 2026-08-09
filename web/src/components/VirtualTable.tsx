import { createVirtualizer, type VirtualItem } from "@tanstack/solid-virtual";
import { For, Show, createEffect, type JSX } from "solid-js";

const ROW_HEIGHT_PX = 36;
const DEFAULT_OVERSCAN = 8;

type VirtualTableProps<T> = {
  rows: readonly T[];
  columnCount: number;
  containerClass: string;
  tableClass: string;
  header: JSX.Element;
  overscan?: number;
  resetKey?: unknown;
  getRowKey?: (row: T, index: number) => VirtualItem["key"];
  containerProps?: JSX.HTMLAttributes<HTMLDivElement> &
    Record<`data-${string}`, string | number | boolean | undefined>;
  rowProps?: (
    row: T,
    index: number,
  ) => JSX.HTMLAttributes<HTMLTableRowElement>;
  children: (row: T, index: number) => JSX.Element;
};

export function VirtualTable<T>(props: VirtualTableProps<T>) {
  let scrollElement: HTMLDivElement | undefined;
  const virtualizer = createVirtualizer<HTMLDivElement, HTMLTableRowElement>({
    get count() {
      return props.rows.length;
    },
    getScrollElement: () => scrollElement ?? null,
    estimateSize: () => ROW_HEIGHT_PX,
    overscan: props.overscan ?? DEFAULT_OVERSCAN,
    getItemKey: (index) =>
      props.getRowKey?.(props.rows[index]!, index) ?? index,
  });

  const virtualRows = () => virtualizer.getVirtualItems();
  const paddingTop = () => virtualRows()[0]?.start ?? 0;
  const paddingBottom = () => {
    const last = virtualRows().at(-1);
    return last == null ? 0 : virtualizer.getTotalSize() - last.end;
  };

  createEffect(() => {
    props.resetKey;
    scrollElement?.scrollTo({ top: 0 });
  });

  return (
    <div
      {...props.containerProps}
      class={props.containerClass}
      ref={(element) => {
        scrollElement = element;
      }}
    >
      <table class={props.tableClass} aria-rowcount={props.rows.length + 1}>
        {props.header}
        <tbody data-virtualized-total-rows={props.rows.length}>
          <Show when={paddingTop() > 0}>
            <tr class="table-virtual-spacer" aria-hidden="true">
              <td
                colSpan={props.columnCount}
                style={{ height: `${paddingTop()}px` }}
              />
            </tr>
          </Show>
          <For each={virtualRows()}>
            {(virtualRow) => {
              const row = () => props.rows[virtualRow.index]!;
              return (
                <tr
                  {...props.rowProps?.(row(), virtualRow.index)}
                  data-index={virtualRow.index}
                  data-virtualized-row
                  data-virtualized-index={virtualRow.index}
                  aria-rowindex={virtualRow.index + 2}
                >
                  {props.children(row(), virtualRow.index)}
                </tr>
              );
            }}
          </For>
          <Show when={paddingBottom() > 0}>
            <tr class="table-virtual-spacer" aria-hidden="true">
              <td
                colSpan={props.columnCount}
                style={{ height: `${paddingBottom()}px` }}
              />
            </tr>
          </Show>
        </tbody>
      </table>
    </div>
  );
}
