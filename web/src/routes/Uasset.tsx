import { For, Show, createMemo, createSignal } from "solid-js";
import { DropZone } from "../components/DropZone";
import { DonutChart, HorizontalBars } from "../components/Charts";
import { DataAssetView } from "../components/DataAssetView";
import { DataTableView } from "../components/DataTableView";
import { ParseRequestError, inspectUasset } from "../lib/api";
import { isDataAssetKind, isDataTableKind } from "../lib/datatable";
import type { UassetInspect } from "../lib/types";

export default function UassetPage() {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [result, setResult] = createSignal<UassetInspect | null>(null);
  const [fileName, setFileName] = createSignal<string | null>(null);

  const kindCounts = createMemo(() => {
    const assets = result()?.assets ?? [];
    const counts = new Map<string, number>();
    for (const asset of assets) {
      counts.set(asset.kind, (counts.get(asset.kind) ?? 0) + 1);
    }
    return [...counts.entries()]
      .map(([name, value]) => ({ name, value }))
      .sort((a, b) => b.value - a.value);
  });

  const tableCounts = createMemo(() => {
    const pkg = result()?.package;
    if (!pkg) return [];
    return [
      { name: "names", value: pkg.names.count },
      { name: "imports", value: pkg.imports.count },
      { name: "exports", value: pkg.exports.count },
      {
        name: "soft paths",
        value: pkg.soft_object_paths?.parsed_count ?? 0,
      },
    ];
  });

  const dataTables = createMemo(
    () => result()?.assets.filter((asset) => isDataTableKind(asset.kind)) ?? [],
  );

  const dataAssets = createMemo(
    () => result()?.assets.filter((asset) => isDataAssetKind(asset.kind)) ?? [],
  );

  const onFile = async (file: File) => {
    setBusy(true);
    setError(null);
    setFileName(file.name);
    try {
      const inspect = await inspectUasset(file);
      setResult(inspect);
    } catch (err) {
      setResult(null);
      if (err instanceof ParseRequestError) {
        setError(err.body.stderr || err.message);
      } else {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <section class="page">
      <header class="page-head">
        <p class="eyebrow">Route /uasset</p>
        <h1>Package inspect</h1>
        <p class="lede">
          Runs <code>uasset inspect --format json</code> on the dropped file.
          Decoded DataTables and DataAssets open as inspectors below the drop
          zone; package charts follow.
        </p>
      </header>

      <DropZone
        accept=".uasset,.umap"
        label="Drop a .uasset / .umap"
        hint="Uncooked classic packages. Cooked / unversioned property packages are rejected by the parser."
        busy={busy()}
        onFile={onFile}
      />

      <Show when={error()}>
        <div class="banner error" role="alert">
          <strong>Parse failed</strong>
          <pre>{error()}</pre>
        </div>
      </Show>

      <Show when={result()} keyed>
        {(inspect) => (
          <>
            <div class="status-row">
              <span class={`pill status-${inspect.status}`}>{inspect.status}</span>
              <span class="mono">{fileName()}</span>
              <span class="muted">schema {inspect.schema_version}</span>
            </div>

            <Show when={dataTables().length > 0}>
              <For each={dataTables()}>
                {(asset) => <DataTableView asset={asset} />}
              </For>
            </Show>

            <Show when={dataAssets().length > 0}>
              <For each={dataAssets()}>
                {(asset) => <DataAssetView asset={asset} />}
              </For>
            </Show>

            <div class="stat-grid">
              <Stat label="Package" value={inspect.package.name || "—"} />
              <Stat
                label="UE5 / UE4"
                value={`${inspect.package.version.ue5} / ${inspect.package.version.ue4}`}
              />
              <Stat label="Decoded assets" value={String(inspect.assets.length)} />
              <Stat
                label="Tables / Assets"
                value={`${dataTables().length} / ${dataAssets().length}`}
              />
            </div>

            <div class="chart-grid">
              <DonutChart
                title="Decoded export kinds"
                subtitle="Count of successfully decoded assets by adapter kind"
                data={kindCounts()}
              />
              <HorizontalBars
                title="Package tables"
                subtitle="Entry counts from the package summary"
                data={tableCounts()}
                valueLabel="entries"
              />
            </div>

            <Show when={(inspect.decode_errors?.length ?? 0) > 0}>
              <section class="panel">
                <h2>Decode errors</h2>
                <ul class="error-list">
                  <For each={inspect.decode_errors}>
                    {(item) => (
                      <li>
                        <code>{item.object_path}</code>
                        <span class="muted">{item.kind}</span>
                        <p>{item.message}</p>
                      </li>
                    )}
                  </For>
                </ul>
              </section>
            </Show>

            <section class="panel">
              <h2>Assets</h2>
              <div class="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>Kind</th>
                      <th>Object path</th>
                      <th>Class</th>
                      <th>Rows</th>
                      <th>Props</th>
                      <th>Tail</th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={inspect.assets}>
                      {(asset) => (
                        <tr>
                          <td>
                            <code>{asset.kind}</code>
                          </td>
                          <td class="mono">{asset.object_path}</td>
                          <td class="mono muted">{asset.class_path ?? "—"}</td>
                          <td>{asset.row_count}</td>
                          <td>{asset.properties?.length ?? 0}</td>
                          <td>{asset.tail_bytes ?? 0}</td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </section>
          </>
        )}
      </Show>
    </section>
  );
}

function Stat(props: { label: string; value: string }) {
  return (
    <div class="stat">
      <span class="stat-label">{props.label}</span>
      <span class="stat-value">{props.value}</span>
    </div>
  );
}
