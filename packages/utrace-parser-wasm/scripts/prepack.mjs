import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(scriptDirectory, "..");
const repositoryRoot = resolve(packageRoot, "..", "..");

for (const [command, arguments_, cwd] of [
  ["cargo", ["fmt", "--manifest-path", resolve(repositoryRoot, "Cargo.toml"), "--all", "--", "--check"], repositoryRoot],
  ["cargo", ["clippy", "--manifest-path", resolve(repositoryRoot, "Cargo.toml"), "--all-targets", "--all-features", "--", "-D", "warnings"], repositoryRoot],
  ["cargo", ["test", "--manifest-path", resolve(repositoryRoot, "Cargo.toml"), "--all-targets", "--all-features"], repositoryRoot],
  [process.execPath, [resolve(scriptDirectory, "build.mjs")], packageRoot],
  [process.execPath, [resolve(scriptDirectory, "check-package.mjs")], packageRoot],
]) {
  const result = spawnSync(command, arguments_, { cwd, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
