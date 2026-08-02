import { describe, expect, it } from "vitest";
import {
  createLatestValuePublisher,
  type PublicationScheduler,
} from "./latest-value-publisher";

function createScheduler(hidden = false): {
  scheduler: PublicationScheduler;
  animationFrames: Map<number, () => void>;
  timeouts: Map<number, () => void>;
  timeoutDelays: number[];
} {
  let handle = 0;
  const animationFrames = new Map<number, () => void>();
  const timeouts = new Map<number, () => void>();
  const timeoutDelays: number[] = [];
  return {
    scheduler: {
      isDocumentHidden: () => hidden,
      requestAnimationFrame: (callback) => {
        handle += 1;
        animationFrames.set(handle, callback);
        return handle;
      },
      cancelAnimationFrame: (id) => animationFrames.delete(id),
      setTimeout: (callback, delayMs) => {
        handle += 1;
        timeoutDelays.push(delayMs);
        timeouts.set(handle, callback);
        return handle;
      },
      clearTimeout: (id) => timeouts.delete(id),
    },
    animationFrames,
    timeouts,
    timeoutDelays,
  };
}

describe("createLatestValuePublisher", () => {
  it("publishes only the latest value from a burst at the next paint", () => {
    const published: number[] = [];
    const testScheduler = createScheduler();
    const publisher = createLatestValuePublisher<number>({
      publish: (value) => published.push(value),
      scheduler: testScheduler.scheduler,
    });

    publisher.publishLatest(1);
    publisher.publishLatest(2);
    publisher.publishLatest(3);

    expect(testScheduler.animationFrames.size).toBe(1);
    expect(published).toEqual([]);
    [...testScheduler.animationFrames.values()][0]();
    expect(published).toEqual([3]);
  });

  it("flushes the latest value immediately and cancels the queued paint", () => {
    const published: number[] = [];
    const testScheduler = createScheduler();
    const publisher = createLatestValuePublisher<number>({
      publish: (value) => published.push(value),
      scheduler: testScheduler.scheduler,
    });

    publisher.publishLatest(4);
    publisher.flush();

    expect(published).toEqual([4]);
    expect(testScheduler.animationFrames.size).toBe(0);
  });

  it("cancels a pending value on abort", () => {
    const published: number[] = [];
    const testScheduler = createScheduler();
    const publisher = createLatestValuePublisher<number>({
      publish: (value) => published.push(value),
      scheduler: testScheduler.scheduler,
    });

    publisher.publishLatest(5);
    publisher.cancel();

    expect(testScheduler.animationFrames.size).toBe(0);
    expect(published).toEqual([]);
  });

  it("uses a 100 ms timer while the document is hidden", () => {
    const published: number[] = [];
    const testScheduler = createScheduler(true);
    const publisher = createLatestValuePublisher<number>({
      publish: (value) => published.push(value),
      scheduler: testScheduler.scheduler,
    });

    publisher.publishLatest(6);

    expect(testScheduler.animationFrames.size).toBe(0);
    expect(testScheduler.timeoutDelays).toEqual([100]);
    [...testScheduler.timeouts.values()][0]();
    expect(published).toEqual([6]);
  });
});
