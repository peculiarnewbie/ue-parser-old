import { A } from "@solidjs/router";

export default function Home() {
  return (
    <section class="hero-page">
      <p class="eyebrow">Local file inspector</p>
      <h1 class="hero-title">
        Unreal package and trace inspector.
      </h1>
      <p class="hero-lede">
        Inspect classic <code>.uasset</code> packages or summarize <code>.utrace</code>{" "}
        captures using the local Rust parser.
      </p>
      <div class="hero-actions">
        <A class="btn primary" href="/uasset">
          Inspect .uasset
        </A>
        <A class="btn ghost" href="/utrace">
          Inspect .utrace
        </A>
      </div>
      <p class="hero-note">
        Files stay in this browser. Inspection runs in a dedicated WASM worker.
      </p>
    </section>
  );
}
