import { parserManifest } from "./manifest.js";

export class UtraceParserWorkerError extends Error {
  constructor(message, options = {}) {
    super(message, options);
    this.name = "UtraceParserWorkerError";
  }
}

/**
 * Creates a module Web Worker that owns one UTrace WASM instance.
 *
 * The default transport sends a private copy of the input bytes as a
 * transferable buffer, so calling a parser method does not detach the
 * caller's Uint8Array.
 */
export function createUtraceParserWorker(options = {}) {
  const worker = options.worker ?? createWorker(options.workerOptions);
  const pending = new Map();
  let nextRequestId = 0;
  let terminated = false;

  worker.onmessage = (event) => {
    const response = event.data;
    if (!isObject(response) || !Number.isSafeInteger(response.id)) return;
    const entry = pending.get(response.id);
    if (!entry) return;
    pending.delete(response.id);

    if (response.type === "result") {
      entry.resolve(response.result);
    } else if (response.type === "error") {
      entry.reject(deserializeError(response.error));
    } else {
      entry.reject(new UtraceParserWorkerError("The UTrace parser worker returned an invalid response"));
    }
  };

  worker.onerror = (event) => {
    fail(new UtraceParserWorkerError(event.message || "The UTrace parser worker failed"));
  };
  worker.onmessageerror = () => {
    fail(new UtraceParserWorkerError("The UTrace parser worker returned an unserializable response"));
  };

  function request(operation, input) {
    if (terminated) {
      return Promise.reject(new UtraceParserWorkerError("The UTrace parser worker is terminated"));
    }

    let prepared;
    try {
      prepared = prepareInput(input);
    } catch (cause) {
      return Promise.reject(cause);
    }

    const id = nextRequestId;
    nextRequestId += 1;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      try {
        worker.postMessage(
          { id, operation, input: prepared.input },
          prepared.transfer,
        );
      } catch (cause) {
        pending.delete(id);
        reject(new UtraceParserWorkerError("Failed to send a request to the UTrace parser worker", { cause }));
      }
    });
  }

  function fail(error) {
    if (terminated) return;
    terminated = true;
    try {
      worker.terminate();
    } catch {
      // The worker has already failed; pending callers still need to settle.
    }
    rejectPending(error);
  }

  function terminate() {
    if (terminated) return;
    terminated = true;
    const error = new UtraceParserWorkerError("The UTrace parser worker was terminated");
    try {
      worker.terminate();
    } finally {
      rejectPending(error);
    }
  }

  function rejectPending(error) {
    for (const entry of pending.values()) entry.reject(error);
    pending.clear();
  }

  return Object.freeze({
    inspect(input) {
      return request("inspect", input);
    },
    inventory(input) {
      return request("inventory", input);
    },
    dashboard(input) {
      return request("dashboard", input);
    },
    dashboardBundle(input) {
      return request("dashboardBundle", input);
    },
    terminate,
  });
}

function createWorker(workerOptions = {}) {
  if (typeof globalThis.Worker !== "function") {
    throw new UtraceParserWorkerError(
      "The UTrace parser Worker API requires a browser Worker implementation",
    );
  }
  try {
    return new globalThis.Worker(new URL("./worker.js", import.meta.url), {
      ...workerOptions,
      type: "module",
    });
  } catch (cause) {
    throw new UtraceParserWorkerError("Failed to create the UTrace parser worker", { cause });
  }
}

function prepareInput(input) {
  if (!isObject(input)) {
    throw new TypeError("UTrace parser input must be an object");
  }
  if (!(input.bytes instanceof Uint8Array)) {
    throw new TypeError("UTrace parser input bytes must be a Uint8Array");
  }
  if (input.bytes.byteLength > parserManifest.maxInputBytes) {
    throw new RangeError(
      `UTrace input is ${input.bytes.byteLength} bytes; maximum is ${parserManifest.maxInputBytes} bytes`,
    );
  }
  const bytes = input.bytes.slice();
  return {
    input: { ...input, bytes },
    transfer: bytes.buffer instanceof ArrayBuffer ? [bytes.buffer] : [],
  };
}

function deserializeError(value) {
  const error = new UtraceParserWorkerError(
    isObject(value) && typeof value.message === "string"
      ? value.message
      : "The UTrace parser worker failed",
  );
  if (isObject(value) && typeof value.name === "string") error.name = value.name;
  if (isObject(value) && typeof value.stack === "string") error.stack = value.stack;
  return error;
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
