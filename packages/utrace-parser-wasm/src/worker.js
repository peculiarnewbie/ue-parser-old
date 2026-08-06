import {
  dashboard,
  dashboardBundle,
  inspect,
  inventory,
} from "./index.js";
import { UTRACE_WORKER_OPERATIONS } from "./worker-operations.js";

const handlers = Object.freeze({
  inspect,
  inventory,
  dashboard,
  dashboardBundle,
});

globalThis.onmessage = (event) => {
  void handleRequest(event.data);
};

async function handleRequest(request) {
  if (!isObject(request) || !Number.isSafeInteger(request.id) || request.id < 0) {
    return;
  }

  try {
    if (!UTRACE_WORKER_OPERATIONS.includes(request.operation)) {
      throw new Error(`Unsupported UTrace parser worker operation: ${String(request.operation)}`);
    }
    const handler = handlers[request.operation];
    const result = await handler(request.input);
    globalThis.postMessage({ id: request.id, type: "result", result });
  } catch (cause) {
    globalThis.postMessage({
      id: request.id,
      type: "error",
      error: serializeError(cause),
    });
  }
}

function serializeError(cause) {
  if (cause instanceof Error) {
    return {
      name: cause.name,
      message: cause.message,
      ...(cause.stack ? { stack: cause.stack } : {}),
    };
  }
  return { name: "Error", message: String(cause) };
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
