import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { IncomingMessage, ServerResponse } from "node:http";
import type { Plugin } from "vite";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const repoRoot = path.resolve(__dirname, "..");

type ParseKind = "uasset-inspect" | "utrace-dashboard" | "utrace-inventory";

const ROUTES: Record<string, ParseKind> = {
  "/api/uasset/inspect": "uasset-inspect",
  "/api/utrace/dashboard": "utrace-dashboard",
  "/api/utrace/inventory": "utrace-inventory",
};

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

  type Candidate = { path: string; mtimeMs: number };
  const candidates: Candidate[] = [];
  for (const profile of ["debug", "release"] as const) {
    for (const name of names) {
      const candidate = path.join(repoRoot, "target", profile, name);
      try {
        const stat = await fs.stat(candidate);
        candidates.push({ path: candidate, mtimeMs: stat.mtimeMs });
      } catch {
        // keep looking
      }
    }
  }

  candidates.sort((a, b) => b.mtimeMs - a.mtimeMs);
  if (candidates[0]) {
    return { command: candidates[0].path, prefixArgs: [] };
  }

  return {
    command: "cargo",
    prefixArgs: ["run", "--quiet", "--features", "utrace", "--"],
  };
}

type DashboardOptions = {
  maxFrames?: number;
  frame?: number;
  timelineLimit?: number;
  gpuFrame?: number;
  gpuTimelineLimit?: number;
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
        String(options.maxFrames ?? 500),
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
  }
}

function runCli(
  command: string,
  args: string[],
): Promise<{ code: number; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: repoRoot,
      windowsHide: true,
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
      resolve({ code: code ?? 1, stdout, stderr });
    });
  });
}

async function handleParse(
  kind: ParseKind,
  req: IncomingMessage,
  res: ServerResponse,
  search: string,
): Promise<void> {
  const filenameHeader = req.headers["x-filename"];
  const filename =
    typeof filenameHeader === "string" && filenameHeader.length > 0
      ? path.basename(filenameHeader)
      : "upload.bin";

  const body = await readBody(req);
  if (body.length === 0) {
    res.statusCode = 400;
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify({ error: "empty upload" }));
    return;
  }

  const dashboardOptions =
    kind === "utrace-dashboard" ? parseDashboardOptions(search) : {};

  const tempDir = await fs.mkdtemp(path.join(os.tmpdir(), "ue-parser-web-"));
  const inputPath = path.join(tempDir, filename);
  try {
    await fs.writeFile(inputPath, body);
    const binary = await resolveUassetBinary();
    const args = [
      ...binary.prefixArgs,
      ...cliArgsFor(kind, inputPath, dashboardOptions),
    ];
    const result = await runCli(binary.command, args);

    // CLI exit 6 = partial success with JSON still on stdout.
    if (result.code !== 0 && result.code !== 6) {
      res.statusCode = 422;
      res.setHeader("Content-Type", "application/json");
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
    res.end(result.stdout);
  } finally {
    await fs.rm(tempDir, { recursive: true, force: true });
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
