import { A } from "@solidjs/router";
import type { ParentProps } from "solid-js";

export function Shell(props: ParentProps) {
  return (
    <div class="shell">
      <header class="topbar">
        <A href="/" class="brand">
          <span class="brand-mark">UE</span>
          <span class="brand-name">parser</span>
        </A>
        <nav class="nav">
          <A href="/uasset" activeClass="active" end={false}>
            .uasset
          </A>
          <A href="/utrace" activeClass="active" end={false}>
            .utrace
          </A>
        </nav>
      </header>
      <main class="main">{props.children}</main>
    </div>
  );
}
