import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, resolve, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import {
  createParserManifest,
  readParserBuildConfig,
  writeParserManifest,
} from "./manifest.mjs";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(scriptDirectory, "..");
const repositoryRoot = resolve(packageRoot, "..", "..");
const sourceDirectory = resolve(packageRoot, "src");
const distDirectory = resolve(packageRoot, "dist");
const wasmDirectory = resolve(distDirectory, "wasm");
const parserConfig = await readParserBuildConfig({ packageRoot, repositoryRoot });

assertChildPath(packageRoot, distDirectory);
rmSync(distDirectory, { recursive: true, force: true });
mkdirSync(wasmDirectory, { recursive: true });

const build = spawnSync(
  "wasm-pack",
  [
    "build",
    repositoryRoot,
    "--release",
    "--target",
    "web",
    "--out-dir",
    wasmDirectory,
    "--out-name",
    "utrace_parser_wasm",
    "--",
    "--no-default-features",
    "--features",
    "utrace-wasm",
  ],
  { cwd: repositoryRoot, stdio: "inherit" },
);

if (build.error) throw build.error;
if (build.status !== 0) process.exit(build.status ?? 1);

for (const file of ["index.js", "index.d.ts", "worker.js", "worker.d.ts", "worker-client.js", "worker-operations.js"]) {
  cpSync(resolve(sourceDirectory, file), resolve(distDirectory, file));
}

for (const generatedMetadata of ["package.json", "README.md", "LICENSE", ".gitignore"]) {
  rmSync(resolve(wasmDirectory, generatedMetadata), { force: true });
}

writeParserManifest({
  packageRoot,
  manifest: createParserManifest({
    packageRoot,
    repositoryRoot,
    config: parserConfig,
  }),
});

function assertChildPath(parent, child) {
  const relation = relative(parent, child);
  if (relation === "" || relation === ".." || relation.startsWith(`..${sep}`)) {
    throw new Error(`Refusing to clear a path outside the package directory: ${child}`);
  }
}
