import type {
  ParseErrorBody,
  UassetInspect,
  UtraceDashboard,
  UtraceInventory,
} from "./types";

export class ParseRequestError extends Error {
  readonly status: number;
  readonly body: ParseErrorBody;

  constructor(status: number, body: ParseErrorBody) {
    super(body.stderr || body.error || `parse failed (${status})`);
    this.name = "ParseRequestError";
    this.status = status;
    this.body = body;
  }
}

async function postFile<T>(
  url: string,
  file: File,
  query?: Record<string, string | number | undefined>,
): Promise<T> {
  const params = new URLSearchParams();
  if (query) {
    for (const [key, value] of Object.entries(query)) {
      if (value != null && value !== "") {
        params.set(key, String(value));
      }
    }
  }
  const suffix = params.size > 0 ? `?${params}` : "";
  const response = await fetch(`${url}${suffix}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/octet-stream",
      "X-Filename": file.name,
    },
    body: file,
  });

  const text = await response.text();
  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch {
    throw new ParseRequestError(response.status, {
      error: "non-json response from parse API",
      stdout: text.slice(0, 2000),
    });
  }

  if (!response.ok) {
    throw new ParseRequestError(response.status, json as ParseErrorBody);
  }

  return json as T;
}

export function inspectUasset(file: File): Promise<UassetInspect> {
  return postFile("/api/uasset/inspect", file);
}

export type UtraceDashboardQuery = {
  max_frames?: number;
  frame?: number;
  timeline_limit?: number;
  gpu_frame?: number;
  gpu_timeline_limit?: number;
};

export function utraceDashboard(
  file: File,
  query?: UtraceDashboardQuery,
): Promise<UtraceDashboard> {
  return postFile("/api/utrace/dashboard", file, query);
}

export function utraceInventory(file: File): Promise<UtraceInventory> {
  return postFile("/api/utrace/inventory", file);
}
