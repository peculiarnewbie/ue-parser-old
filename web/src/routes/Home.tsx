import { A } from "@solidjs/router";

export default function Home() {
  return (
    <section class="hero-page">
      <p class="eyebrow">Local Unreal file inspector</p>
      <h1 class="hero-title">
        Drop a package.
        <span>Read the wire.</span>
      </h1>
      <p class="hero-lede">
        Browser UI over the Rust <code>uasset</code> CLI — inspect classic{" "}
        <code>.uasset</code> packages and summarize <code>.utrace</code> captures
        without leaving the machine.
      </p>
      <div class="hero-actions">
        <A class="btn primary" href="/uasset">
          Open .uasset
        </A>
        <A class="btn ghost" href="/utrace">
          Open .utrace
        </A>
      </div>
      <p class="hero-note">
        Dev server posts uploads to a Vite middleware that shells out to{" "}
        <code>target/*/uasset</code> (or <code>cargo run --features utrace</code>).
      </p>
    </section>
  );
}
