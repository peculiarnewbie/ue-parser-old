export type PublicationScheduler = {
  isDocumentHidden: () => boolean;
  requestAnimationFrame: (callback: () => void) => number;
  cancelAnimationFrame: (handle: number) => void;
  setTimeout: (callback: () => void, delayMs: number) => number;
  clearTimeout: (handle: number) => void;
};

export type LatestValuePublisher<T> = {
  publishLatest: (value: T) => void;
  flush: () => void;
  cancel: () => void;
};

type ScheduledPublication =
  | { kind: "animation-frame"; handle: number }
  | { kind: "timeout"; handle: number };

const BACKGROUND_PUBLICATION_DELAY_MS = 100;

/** Coalesces a burst of values so only the latest one reaches the UI per paint. */
export function createLatestValuePublisher<T>({
  publish,
  scheduler = browserPublicationScheduler,
}: {
  publish: (value: T) => void;
  scheduler?: PublicationScheduler;
}): LatestValuePublisher<T> {
  let hasPendingValue = false;
  let pendingValue: T;
  let scheduled: ScheduledPublication | undefined;

  const clearScheduled = () => {
    if (!scheduled) return;
    if (scheduled.kind === "animation-frame") {
      scheduler.cancelAnimationFrame(scheduled.handle);
    } else {
      scheduler.clearTimeout(scheduled.handle);
    }
    scheduled = undefined;
  };

  const publishPending = () => {
    if (!hasPendingValue) return;
    hasPendingValue = false;
    publish(pendingValue);
  };

  const schedulePublication = () => {
    if (scheduled) return;
    const callback = () => {
      scheduled = undefined;
      publishPending();
    };
    scheduled = scheduler.isDocumentHidden()
      ? {
          kind: "timeout",
          handle: scheduler.setTimeout(callback, BACKGROUND_PUBLICATION_DELAY_MS),
        }
      : {
          kind: "animation-frame",
          handle: scheduler.requestAnimationFrame(callback),
        };
  };

  return {
    publishLatest(value) {
      pendingValue = value;
      hasPendingValue = true;
      schedulePublication();
    },
    flush() {
      clearScheduled();
      publishPending();
    },
    cancel() {
      clearScheduled();
      hasPendingValue = false;
    },
  };
}

const browserPublicationScheduler: PublicationScheduler = {
  isDocumentHidden: () => document.visibilityState === "hidden",
  requestAnimationFrame: (callback) => window.requestAnimationFrame(callback),
  cancelAnimationFrame: (handle) => window.cancelAnimationFrame(handle),
  setTimeout: (callback, delayMs) => window.setTimeout(callback, delayMs),
  clearTimeout: (handle) => window.clearTimeout(handle),
};
