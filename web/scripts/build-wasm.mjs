import { spawnSync } from "node:child_process";

const toolchain = "nightly-2025-11-15";
const rustFlags = [
  "-C target-feature=+atomics,+bulk-memory",
  "-C link-arg=--shared-memory",
  "-C link-arg=--max-memory=4294967296",
  "-C link-arg=--import-memory",
  "-C link-arg=--export=__wasm_init_tls",
  "-C link-arg=--export=__tls_size",
  "-C link-arg=--export=__tls_align",
  "-C link-arg=--export=__tls_base",
].join(" ");

const build = spawnSync(
  "rustup",
  [
    "run",
    toolchain,
    "wasm-pack",
    "build",
    "..",
    "--release",
    "--target",
    "web",
    "--out-dir",
    "web/src/generated/wasm",
    "--out-name",
    "uasset_parser_wasm",
    "--no-typescript",
    "--",
    "--no-default-features",
    "--features",
    "utrace-wasm-threads,wasm-uasset",
    "-Z",
    "build-std=panic_abort,std",
  ],
  {
    cwd: new URL("..", import.meta.url),
    env: { ...process.env, RUSTFLAGS: rustFlags },
    stdio: "inherit",
  },
);

if (build.error) throw build.error;
if (build.status !== 0) {
  console.error(
    `Threaded WASM requires ${toolchain} with rust-src and the wasm32-unknown-unknown target.`,
  );
  process.exit(build.status ?? 1);
}
