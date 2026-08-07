import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(scriptDirectory, "..");
const repositoryRoot = resolve(packageRoot, "..", "..");
const packageManifest = JSON.parse(await readFile(resolve(packageRoot, "package.json"), "utf8"));
const temporaryRoot = await mkdtemp(resolve(tmpdir(), "utrace-parser-wasm-packed-"));

try {
  const pack = runNpm([
    "pack",
    "--json",
    "--ignore-scripts",
    "--pack-destination",
    temporaryRoot,
  ], packageRoot);
  const [packResult] = JSON.parse(pack.stdout);
  assert.ok(packResult, "npm pack did not describe the tarball");
  assertPackageFiles(packResult.files.map(({ path }) => path));

  const tarball = resolve(temporaryRoot, packResult.filename);
  const consumerRoot = resolve(temporaryRoot, "consumer");
  await writeFile(
    resolve(temporaryRoot, "package.json"),
    `${JSON.stringify({ private: true, type: "module" })}\n`,
  );
  await mkdirFor(consumerRoot);
  runNpm(["install", "--ignore-scripts", "--no-package-lock", tarball], temporaryRoot);

  const fixtureHex = await readFile(
    resolve(repositoryRoot, "tests", "fixtures", "tiny", "minimal-prologue.utrace.hex"),
    "utf8",
  );
  const fixture = decodeFixture(fixtureHex);
  const fixturePath = resolve(temporaryRoot, "fixture with spaces.utrace");
  await writeFile(fixturePath, fixture);

  const consumerSource = `
    import assert from "node:assert/strict";
    import { readFile } from "node:fs/promises";
    import * as browser from "@ue-shed/utrace-parser-wasm";
    import * as node from "@ue-shed/utrace-parser-wasm/node";
    import { parserManifest } from "@ue-shed/utrace-parser-wasm/manifest";

    globalThis.fetch = () => { throw new Error("Node entry attempted fetch"); };
    await node.init();
    const bytes = await readFile(${JSON.stringify(fixturePath)});
    const result = await node.dashboard({ bytes, filename: "fixture.utrace" });
    assert.equal(result.schema_version, 2);
    assert.equal(result.status, "ok");
    assert.equal(typeof result.dashboard, "object");
    assert.equal(parserManifest.packageVersion, ${JSON.stringify(packageManifest.version)});
    assert.equal(parserManifest.utraceSchemaVersion, 2);
    for (const name of ["init", "inspect", "inventory", "dashboard", "dashboardBundle", "createProgressiveDashboard", "createUtraceParserWorker"]) {
      assert.equal(typeof browser[name], "function", \`missing browser export \${name}\`);
      assert.equal(typeof node[name], "function", \`missing Node export \${name}\`);
    }
  `;
  run(process.execPath, ["--input-type=module", "--eval", consumerSource], temporaryRoot);

  const cli = resolve(
    temporaryRoot,
    "node_modules",
    "@ue-shed",
    "utrace-parser-wasm",
    "dist",
    "cli.js",
  );
  const outputPath = resolve(temporaryRoot, "nested output", "dashboard.json");
  const cliSuccess = run(process.execPath, [
    cli,
    "dashboard",
    "--input",
    fixturePath,
    "--output",
    outputPath,
    "--max-frames",
    "120",
  ], temporaryRoot);
  const successStatus = JSON.parse(cliSuccess.stderr.trim());
  assert.equal(successStatus.type, "success");
  assert.equal(successStatus.schema_version, 2);
  const output = JSON.parse(await readFile(outputPath, "utf8"));
  assert.equal(output.schema_version, 2);

  const originalOutput = await readFile(outputPath, "utf8");
  const overwrite = spawn(process.execPath, [
    cli,
    "dashboard",
    "--input",
    fixturePath,
    "--output",
    outputPath,
  ], temporaryRoot);
  assert.equal(overwrite.status, 5, overwrite.stderr);
  assert.equal(await readFile(outputPath, "utf8"), originalOutput);

  const invalidPath = resolve(temporaryRoot, "invalid.utrace");
  const failedOutputPath = resolve(temporaryRoot, "failed.json");
  await writeFile(invalidPath, new Uint8Array([0, 1, 2, 3]));
  const invalid = spawn(process.execPath, [
    cli,
    "dashboard",
    "--input",
    invalidPath,
    "--output",
    failedOutputPath,
  ], temporaryRoot);
  assert.equal(invalid.status, 4, invalid.stderr);
  assert.equal((await readdir(temporaryRoot)).some((name) => name.includes("failed.json")), false);

  process.stdout.write("Packed-package Node consumer and CLI tests passed.\n");
} finally {
  await rm(temporaryRoot, { recursive: true, force: true });
}

function assertPackageFiles(files) {
  const expected = new Set([
    "LICENSE",
    "README.md",
    "dist/cli.js",
    "dist/index.d.ts",
    "dist/index.js",
    "dist/manifest.d.ts",
    "dist/manifest.js",
    "dist/node.d.ts",
    "dist/node.js",
    "dist/parser-manifest.json",
    "dist/wasm/utrace_parser_wasm_bg.wasm",
    "dist/wasm/utrace_parser_wasm_bg.wasm.d.ts",
    "dist/wasm/utrace_parser_wasm.d.ts",
    "dist/wasm/utrace_parser_wasm.js",
    "dist/worker-client.js",
    "dist/worker-operations.js",
    "dist/worker.d.ts",
    "dist/worker.js",
    "package.json",
  ]);
  assert.deepEqual(new Set(files), expected, "packed package contains unexpected or missing files");
}

function decodeFixture(source) {
  const hexadecimal = source
    .split("\n")
    .map((line) => line.split("#", 1)[0])
    .join("")
    .replaceAll(/\s/g, "");
  return Uint8Array.from(Buffer.from(hexadecimal, "hex"));
}

async function mkdirFor(path) {
  const { mkdir } = await import("node:fs/promises");
  await mkdir(path, { recursive: true });
}

function runNpm(arguments_, cwd) {
  const npmCli = process.env.npm_execpath;
  if (!npmCli) throw new Error("npm_execpath is unavailable; run this test through npm");
  return run(process.execPath, [npmCli, ...arguments_], cwd);
}

function run(command, arguments_, cwd) {
  const result = spawn(command, arguments_, cwd);
  if (result.status !== 0) {
    throw new Error(`${command} ${arguments_.join(" ")} failed (${result.status}):\n${result.stderr}`);
  }
  return result;
}

function spawn(command, arguments_, cwd) {
  const result = spawnSync(command, arguments_, { cwd, encoding: "utf8" });
  if (result.error) throw result.error;
  return result;
}
