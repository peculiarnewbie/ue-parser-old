import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { IncomingMessage, ServerResponse } from "node:http";
import type { Plugin } from "vite";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const repoRoot = path.resolve(__dirname, "..");

type ParseKind =
  | "uasset-inspect"
  | "utrace-dashboard"
  | "utrace-inventory"
  | "utrace-timeline";

const ROUTES: Record<string, ParseKind> = {
  "/api/uasset/inspect": "uasset-inspect",
  "/api/utrace/dashboard": "utrace-dashboard",
  "/api/utrace/inventory": "utrace-inventory",
  "/api/utrace/timeline": "utrace-timeline",
};

const maxProgressUploadBytes = 8 * 1024 * 1024 * 1024;

const timelineCacheDir = path.join(os.tmpdir(), "ue-parser-web-timeline");
const maxTimelineCacheBytes = 512 * 1024 * 1024;
// The Vite workbench is an analysis tool: complete captures are the default.
// Parser transport limits still reject malformed or impractically large input.
const allTimelineIntervals = "18446744073709551615";
const sourceHashPattern = /^[a-f0-9]{64}$/;

async function readBody(req: IncomingMessage): Promise<Buffer> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks);
}

async function resolveUassetBinary(): Promise<{ command: string; prefixArgs: string[] }> {
  const names =
    process.platform === "win32"
      ? ["uasset.exe", "uasset"]
      : ["uasset"];

  // Dashboard decoding is several times slower without Rust optimizations. Prefer
  // release deterministically instead of allowing a newer debug build to win.
  for (const profile of ["release", "debug"] as const) {
    for (const name of names) {
      const candidate = path.join(repoRoot, "target", profile, name);
      try {
        await fs.stat(candidate);
        return { command: candidate, prefixArgs: [] };
      } catch {
        // keep looking
      }
    }
  }

  return {
    command: "cargo",
    prefixArgs: ["run", "--release", "--quiet", "--features", "utrace", "--"],
  };
}

type DashboardOptions = {
  maxFrames?: number;
  frame?: number;
  timelineLimit?: number;
  gpuFrame?: number;
  gpuTimelineLimit?: number;
  timelineIndex?: boolean;
};

type TimelineOptions = {
  startCycle?: number;
  endCycle?: number;
  threadId?: number;
  search?: string;
  limit?: number;
};

function parseDashboardOptions(search: string): DashboardOptions {
  const params = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  const readInt = (key: string) => {
    const raw = params.get(key);
    if (raw == null || raw === "") return undefined;
    const value = Number(raw);
    return Number.isFinite(value) ? Math.trunc(value) : undefined;
  };
  return {
    maxFrames: readInt("max_frames"),
    frame: readInt("frame"),
    timelineLimit: readInt("timeline_limit"),
    gpuFrame: readInt("gpu_frame"),
    gpuTimelineLimit: readInt("gpu_timeline_limit"),
    timelineIndex: params.get("timeline_index") === "1",
  };
}

function parseTimelineOptions(search: string): TimelineOptions {
  const params = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  const readInt = (key: string) => {
    const raw = params.get(key);
    if (raw == null || raw === "") return undefined;
    const value = Number(raw);
    return Number.isSafeInteger(value) && value >= 0 ? value : undefined;
  };
  const searchText = params.get("search")?.trim();
  return {
    startCycle: readInt("start_cycle"),
    endCycle: readInt("end_cycle"),
    threadId: readInt("thread"),
    search: searchText || undefined,
    limit: readInt("limit"),
  };
}

function cliArgsFor(
  kind: ParseKind,
  inputPath: string,
  options: DashboardOptions = {},
): string[] {
  switch (kind) {
    case "uasset-inspect":
      return ["inspect", inputPath, "--format", "json"];
    case "utrace-dashboard": {
      const args = [
        "utrace",
        "dashboard",
        inputPath,
        "--format",
        "json",
        "--max-frames",
        String(options.maxFrames ?? "18446744073709551615"),
      ];
      if (options.frame != null) {
        args.push("--frame", String(options.frame));
        args.push("--timeline-limit", String(options.timelineLimit ?? 2500));
      }
      if (options.gpuFrame != null) {
        args.push("--gpu-frame", String(options.gpuFrame));
        args.push(
          "--gpu-timeline-limit",
          String(options.gpuTimelineLimit ?? 2500),
        );
      }
      return args;
    }
    case "utrace-inventory":
      return ["utrace", "inventory", inputPath, "--format", "json"];
    case "utrace-timeline":
      throw new Error("timeline requests are handled through the cached index path");
  }
}

function runCli(
  command: string,
  args: string[],
  signal?: AbortSignal,
): Promise<{ code: number; stdout: string; stderr: string; cli_ms: number }> {
  const started = performance.now();
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: repoRoot,
      windowsHide: true,
      signal,
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => {
      resolve({
        code: code ?? 1,
        stdout,
        stderr,
        cli_ms: Math.round(performance.now() - started),
      });
    });
  });
}

function progressLine(value: unknown): string {
  const line = `${JSON.stringify(value)}\n`;
  if (Buffer.byteLength(line) > 64 * 1024 * 1024) {
    throw new Error("progressive output line exceeds 64 MiB limit");
  }
  return line;
}

async function forwardProgressCli(
  command: string,
  args: string[],
  signal: AbortSignal,
  req: IncomingMessage,
  res: ServerResponse,
  declaredLength: number,
  filename: string,
  nextSequence: () => number,
  hashUpload: boolean,
  finalizeTimelineIndex?: (sourceHash: string) => Promise<string | undefined>,
): Promise<{ code: number; stderr: string; uploadBytes: number; sourceHash?: string }> {
  const child = spawn(command, args, { cwd: repoRoot, windowsHide: true, signal });
  const closed = new Promise<number>((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code) => resolve(code ?? 1));
  });
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  let stderr = "";
  child.stderr.on("data", (chunk: string) => {
    if (stderr.length < 1024 * 1024) stderr += chunk;
  });
  const sourceHash = hashUpload ? createHash("sha256") : undefined;
  const upload = (async () => {
    let consumed = 0;
    for await (const chunk of req) {
      const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
      consumed += bytes.length;
      if (consumed > maxProgressUploadBytes || consumed > declaredLength) {
        throw new Error("upload exceeds declared or configured size");
      }
      sourceHash?.update(bytes);
      if (!child.stdin.write(bytes)) await once(child.stdin, "drain");
    }
    if (consumed !== declaredLength) throw new Error(`upload ended at ${consumed} of ${declaredLength} bytes`);
    child.stdin.end();
    return { bytes: consumed, sourceHash: sourceHash?.digest("hex") };
  })();
  const download = (async () => {
    let buffered = "";
    for await (const chunk of child.stdout) {
      buffered += chunk as string;
      if (Buffer.byteLength(buffered) > 64 * 1024 * 1024) {
        child.kill();
        throw new Error("progressive CLI line exceeds 64 MiB limit");
      }
      for (;;) {
        const newline = buffered.indexOf("\n");
        if (newline < 0) break;
        const line = buffered.slice(0, newline);
        buffered = buffered.slice(newline + 1);
        if (!line) continue;
        const event = JSON.parse(line) as Record<string, unknown> & { progress?: Record<string, unknown> };
        event.sequence = nextSequence();
        if (event.progress) event.progress.total_bytes = declaredLength;
        if (event.type === "complete") {
          const complete = event as Record<string, unknown> & {
            dashboard?: { path?: string };
            inventory?: { path?: string };
            timeline_index?: Record<string, unknown>;
            timeline_index_warning?: string;
          };
          if (complete.dashboard) complete.dashboard.path = filename;
          if (complete.inventory) complete.inventory.path = filename;
          if (complete.timeline_index) {
            const uploaded = await upload;
            const exitCode = await closed;
            const warning = !isSuccessful({ code: exitCode })
              ? "timeline index process did not complete successfully"
              : uploaded.sourceHash && finalizeTimelineIndex
                ? await finalizeTimelineIndex(uploaded.sourceHash)
                : "timeline index source hash was unavailable";
            if (warning) {
              delete complete.timeline_index;
              complete.timeline_index_warning = warning;
            } else if (uploaded.sourceHash) {
              complete.timeline_index.source_hash = uploaded.sourceHash;
            }
          }
        }
        if (!res.write(progressLine(event))) await once(res, "drain");
      }
    }
    if (buffered.trim()) throw new Error("progressive CLI ended with an unterminated JSON line");
  })();
  const [uploaded, code] = await Promise.all([upload, closed, download])
    .then(([result, exitCode]) => [result, exitCode] as const)
    .catch((error: unknown) => {
      child.kill();
      throw error;
    });
  return { code, stderr, uploadBytes: uploaded.bytes, sourceHash: uploaded.sourceHash };
}

async function handleProgress(
  req: IncomingMessage,
  res: ServerResponse,
  search: string,
): Promise<void> {
  const filenameHeader = req.headers["x-filename"];
  const filename = typeof filenameHeader === "string" && filenameHeader.length > 0
    ? path.basename(filenameHeader)
    : "upload.utrace";
  const declaredLength = Number(req.headers["content-length"] ?? 0);
  if (!Number.isSafeInteger(declaredLength) || declaredLength <= 0 || declaredLength > maxProgressUploadBytes) {
    res.statusCode = 413;
    res.end("invalid or oversized upload");
    return;
  }

  const abort = new AbortController();
  let sequence = 0;
  req.once("aborted", () => abort.abort());
  res.once("close", () => {
    if (!res.writableEnded) abort.abort();
  });
  res.statusCode = 200;
  res.setHeader("Content-Type", "application/x-ndjson");
  res.setHeader("Cache-Control", "no-store");
  const emit = (event: unknown) => res.write(progressLine(event));
  emit({
    protocol_version: 1,
    type: "bootstrap",
    sequence: sequence++,
    progress: { phase: "reading", bytes_consumed: 0, total_bytes: declaredLength },
  });
  let pendingIndexPath: string | undefined;
  try {
    const binary = await resolveUassetBinary();
    const options = parseDashboardOptions(search);
    const dashboardArgs = cliArgsFor("utrace-dashboard", "-", options);
    dashboardArgs[1] = "dashboard-progress";
    if (options.timelineIndex) {
      await fs.mkdir(timelineCacheDir, { recursive: true });
      pendingIndexPath = path.join(
        timelineCacheDir,
        `.pending-${process.pid}-${Date.now()}-${Math.random().toString(36).slice(2)}.utix`,
      );
      dashboardArgs.push(
        "--timeline-index-output",
        pendingIndexPath,
        "--timeline-index-max-intervals",
        allTimelineIntervals,
      );
    }
    const result = await forwardProgressCli(binary.command, [
      ...binary.prefixArgs,
      ...dashboardArgs,
    ], abort.signal, req, res, declaredLength, filename, () => sequence++, options.timelineIndex === true, async (sourceHash) => {
      if (!pendingIndexPath) return "timeline index output was not configured";
      const indexPath = path.join(timelineCacheDir, `${sourceHash}.utix`);
      try {
        try {
          await fs.stat(indexPath);
          await fs.rm(pendingIndexPath, { force: true });
        } catch {
          await fs.rename(pendingIndexPath, indexPath);
        }
        await pruneTimelineCache(indexPath);
        return undefined;
      } catch (error) {
        await fs.rm(pendingIndexPath, { force: true }).catch(() => undefined);
        return `timeline index cache finalization failed: ${error instanceof Error ? error.message : String(error)}`;
      }
    });
    if (!isSuccessful(result)) throw new Error(result.stderr.trim() || "dashboard decode failed");
    res.end();
  } catch (error) {
    if (!abort.signal.aborted && !res.writableEnded) {
      emit({
        protocol_version: 1,
        type: "failed",
        sequence: sequence++,
        error: error instanceof Error ? error.message : String(error),
      });
      res.end();
    }
  } finally {
    if (pendingIndexPath) await fs.rm(pendingIndexPath, { force: true }).catch(() => undefined);
    // Request/response close handlers own cancellation of the streaming child.
  }
}

type ParsePhaseTiming = {
  kind: ParseKind;
  total_ms: number;
  read_body_ms: number;
  write_temp_ms: number;
  resolve_binary_ms: number;
  cli_ms: number;
  index_ms?: number;
  index_cache_hit?: boolean;
  query_ms?: number;
  response_bytes: number;
  upload_bytes: number;
  binary: string;
};

function binaryLabel(binary: { command: string; prefixArgs: string[] }): string {
  if (binary.prefixArgs.length > 0) return "cargo run";
  return path.basename(binary.command);
}

function isSuccessful(result: { code: number }): boolean {
  return result.code === 0 || result.code === 6;
}

async function pruneTimelineCache(keep: string): Promise<void> {
  try {
    const entries = await Promise.all(
      (await fs.readdir(timelineCacheDir)).map(async (name) => {
        const filePath = path.join(timelineCacheDir, name);
        const stat = await fs.stat(filePath);
        return { filePath, bytes: stat.size, mtimeMs: stat.mtimeMs };
      }),
    );
    let total = entries.reduce((sum, entry) => sum + entry.bytes, 0);
    for (const entry of entries.sort((left, right) => left.mtimeMs - right.mtimeMs)) {
      if (total <= maxTimelineCacheBytes || entry.filePath === keep) break;
      await fs.rm(entry.filePath, { force: true });
      total -= entry.bytes;
    }
  } catch {
    // The cache is an optional speed-up; parsing must not fail because pruning did.
  }
}

async function cachedTimelineIndex(
  binary: { command: string; prefixArgs: string[] },
  inputPath: string,
  sourceHash: string,
): Promise<{
  indexPath?: string;
  failure?: { code: number; stdout: string; stderr: string; cli_ms: number };
  index_ms: number;
  index_cache_hit: boolean;
}> {
  await fs.mkdir(timelineCacheDir, { recursive: true });
  const indexPath = path.join(timelineCacheDir, `${sourceHash}.utix`);
  try {
    const stat = await fs.stat(indexPath);
    if (stat.size > 0) {
      await fs.utimes(indexPath, new Date(), new Date());
      return { indexPath, index_ms: 0, index_cache_hit: true };
    }
  } catch {
    // Build the missing sidecar below.
  }

  const build = await runCli(binary.command, [
    ...binary.prefixArgs,
    "utrace",
    "timeline",
    "index",
    inputPath,
    "--output",
    indexPath,
    "--max-intervals",
    allTimelineIntervals,
    "--format",
    "json",
  ]);
  if (!isSuccessful(build)) {
    return {
      failure: build,
      index_ms: build.cli_ms,
      index_cache_hit: false,
    };
  }
  await pruneTimelineCache(indexPath);
  return {
    indexPath,
    index_ms: build.cli_ms,
    index_cache_hit: false,
  };
}

async function cachedTimelineIndexPath(sourceHash: string): Promise<string | undefined> {
  const indexPath = path.join(timelineCacheDir, `${sourceHash}.utix`);
  try {
    const stat = await fs.stat(indexPath);
    if (stat.size <= 0) return undefined;
    await fs.utimes(indexPath, new Date(), new Date());
    return indexPath;
  } catch {
    return undefined;
  }
}

function timelineSourceToken(search: string): string | undefined {
  const params = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  const sourceHash = params.get("src");
  return sourceHash && sourceHashPattern.test(sourceHash) ? sourceHash : undefined;
}

function requestHasNoBody(req: IncomingMessage): boolean {
  const contentLength = req.headers["content-length"];
  return (
    (contentLength == null || contentLength === "0") &&
    req.headers["transfer-encoding"] == null
  );
}

function timelineQueryArgs(indexPath: string, options: TimelineOptions): string[] {
  const args = ["utrace", "timeline", "query", indexPath, "--format", "json"];
  if (options.startCycle != null) args.push("--start-cycle", String(options.startCycle));
  if (options.endCycle != null) args.push("--end-cycle", String(options.endCycle));
  if (options.threadId != null) args.push("--thread", String(options.threadId));
  if (options.search) args.push("--search", options.search);
  args.push("--limit", String(options.limit ?? 2500));
  return args;
}

async function handleTimelineTokenQuery(
  res: ServerResponse,
  sourceHash: string,
  options: TimelineOptions,
  totalStarted: number,
): Promise<void> {
  const indexPath = await cachedTimelineIndexPath(sourceHash);
  if (!indexPath) {
    res.statusCode = 409;
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify({ error: "index_missing" }));
    return;
  }

  const resolveStarted = performance.now();
  const binary = await resolveUassetBinary();
  const resolveBinaryMs = Math.round(performance.now() - resolveStarted);
  const query = await runCli(binary.command, [
    ...binary.prefixArgs,
    ...timelineQueryArgs(indexPath, options),
  ]);
  const timing: ParsePhaseTiming = {
    kind: "utrace-timeline",
    total_ms: Math.round(performance.now() - totalStarted),
    read_body_ms: 0,
    write_temp_ms: 0,
    resolve_binary_ms: resolveBinaryMs,
    cli_ms: query.cli_ms,
    index_ms: 0,
    index_cache_hit: true,
    query_ms: query.cli_ms,
    response_bytes: Buffer.byteLength(query.stdout, "utf8"),
    upload_bytes: 0,
    binary: binaryLabel(binary),
  };
  res.statusCode = isSuccessful(query) ? 200 : 422;
  res.setHeader("Content-Type", "application/json");
  res.setHeader("X-Ue-Parse-Timing", JSON.stringify(timing));
  res.setHeader("X-Ue-Source-Hash", sourceHash);
  res.setHeader(
    "Server-Timing",
    [
      `total;dur=${timing.total_ms}`,
      `cli;dur=${timing.cli_ms}`,
      "upload;dur=0",
      "write;dur=0",
    ].join(", "),
  );
  if (isSuccessful(query)) {
    res.end(query.stdout);
  } else {
    res.end(
      JSON.stringify({
        error: "parse failed",
        exit_code: query.code,
        stderr: query.stderr.trim(),
        stdout: query.stdout.trim(),
      }),
    );
  }
}

async function handleParse(
  kind: ParseKind,
  req: IncomingMessage,
  res: ServerResponse,
  search: string,
): Promise<void> {
  const totalStarted = performance.now();
  const filenameHeader = req.headers["x-filename"];
  const filename =
    typeof filenameHeader === "string" && filenameHeader.length > 0
      ? path.basename(filenameHeader)
      : "upload.bin";

  const timelineOptions =
    kind === "utrace-timeline" ? parseTimelineOptions(search) : {};
  const sourceToken = kind === "utrace-timeline" ? timelineSourceToken(search) : undefined;
  if (sourceToken && requestHasNoBody(req)) {
    await handleTimelineTokenQuery(res, sourceToken, timelineOptions, totalStarted);
    return;
  }

  const readStarted = performance.now();
  const body = await readBody(req);
  const readBodyMs = Math.round(performance.now() - readStarted);
  if (body.length === 0) {
    res.statusCode = 400;
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify({ error: "empty upload" }));
    return;
  }

  const dashboardOptions =
    kind === "utrace-dashboard" ? parseDashboardOptions(search) : {};
  const sourceHash =
    kind === "utrace-timeline"
      ? createHash("sha256").update(body).digest("hex")
      : undefined;
  let tempDir: string | undefined;
  try {
    const resolveStarted = performance.now();
    const binary = await resolveUassetBinary();
    const resolveBinaryMs = Math.round(performance.now() - resolveStarted);

    let cliMs = 0;
    let indexMs: number | undefined;
    let indexCacheHit: boolean | undefined;
    let queryMs: number | undefined;
    let writeTempMs = 0;

    const result =
      kind === "utrace-timeline"
        ? await (async () => {
            let indexPath = await cachedTimelineIndexPath(sourceHash!);
            if (indexPath) {
              indexMs = 0;
              indexCacheHit = true;
              const query = await runCli(binary.command, [
                ...binary.prefixArgs,
                ...timelineQueryArgs(indexPath, timelineOptions),
              ]);
              queryMs = query.cli_ms;
              cliMs = query.cli_ms;
              return query;
            }

            tempDir = await fs.mkdtemp(path.join(os.tmpdir(), "ue-parser-web-"));
            const inputPath = path.join(tempDir, filename);
            const writeStarted = performance.now();
            await fs.writeFile(inputPath, body);
            writeTempMs = Math.round(performance.now() - writeStarted);
            const cached = await cachedTimelineIndex(binary, inputPath, sourceHash!);
            indexMs = cached.index_ms;
            indexCacheHit = cached.index_cache_hit;
            if (cached.failure) {
              cliMs = cached.failure.cli_ms;
              return cached.failure;
            }
            const query = await runCli(binary.command, [
              ...binary.prefixArgs,
              ...timelineQueryArgs(cached.indexPath!, timelineOptions),
            ]);
            queryMs = query.cli_ms;
            cliMs = cached.index_ms + query.cli_ms;
            return query;
          })()
        : await (async () => {
            tempDir = await fs.mkdtemp(path.join(os.tmpdir(), "ue-parser-web-"));
            const inputPath = path.join(tempDir, filename);
            const writeStarted = performance.now();
            await fs.writeFile(inputPath, body);
            writeTempMs = Math.round(performance.now() - writeStarted);
            return runCli(binary.command, [
              ...binary.prefixArgs,
              ...cliArgsFor(kind, inputPath, dashboardOptions),
            ]);
          })();

    if (kind !== "utrace-timeline") {
      cliMs = result.cli_ms;
    }

    const timing: ParsePhaseTiming = {
      kind,
      total_ms: Math.round(performance.now() - totalStarted),
      read_body_ms: readBodyMs,
      write_temp_ms: writeTempMs,
      resolve_binary_ms: resolveBinaryMs,
      cli_ms: cliMs,
      index_ms: indexMs,
      index_cache_hit: indexCacheHit,
      query_ms: queryMs,
      response_bytes: Buffer.byteLength(result.stdout, "utf8"),
      upload_bytes: body.length,
      binary: binaryLabel(binary),
    };

    // CLI exit 6 = partial success with JSON still on stdout.
    if (!isSuccessful(result)) {
      res.statusCode = 422;
      res.setHeader("Content-Type", "application/json");
      res.setHeader("X-Ue-Parse-Timing", JSON.stringify(timing));
      if (sourceHash) res.setHeader("X-Ue-Source-Hash", sourceHash);
      res.end(
        JSON.stringify({
          error: "parse failed",
          exit_code: result.code,
          stderr: result.stderr.trim(),
          stdout: result.stdout.trim(),
        }),
      );
      return;
    }

    res.statusCode = 200;
    res.setHeader("Content-Type", "application/json");
    res.setHeader("X-Ue-Parse-Timing", JSON.stringify(timing));
    if (sourceHash) res.setHeader("X-Ue-Source-Hash", sourceHash);
    res.setHeader(
      "Server-Timing",
      [
        `total;dur=${timing.total_ms}`,
        `cli;dur=${timing.cli_ms}`,
        `upload;dur=${timing.read_body_ms}`,
        `write;dur=${timing.write_temp_ms}`,
      ].join(", "),
    );
    res.end(result.stdout);
  } finally {
    if (tempDir) await fs.rm(tempDir, { recursive: true, force: true });
  }
}

export function ueParserApiPlugin(): Plugin {
  return {
    name: "ue-parser-api",
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        const rawUrl = req.url ?? "";
        const q = rawUrl.indexOf("?");
        const url = q >= 0 ? rawUrl.slice(0, q) : rawUrl;
        const search = q >= 0 ? rawUrl.slice(q) : "";
        if (url === "/api/utrace/progress" && req.method === "POST") {
          try {
            await handleProgress(req, res, search);
          } catch (error) {
            if (!res.headersSent) res.statusCode = 500;
            if (!res.writableEnded) res.end(error instanceof Error ? error.message : String(error));
          }
          return;
        }
        const kind = ROUTES[url];
        if (!kind || req.method !== "POST") {
          next();
          return;
        }
        try {
          await handleParse(kind, req, res, search);
        } catch (error) {
          res.statusCode = 500;
          res.setHeader("Content-Type", "application/json");
          res.end(
            JSON.stringify({
              error: error instanceof Error ? error.message : String(error),
            }),
          );
        }
      });
    },
  };
}
