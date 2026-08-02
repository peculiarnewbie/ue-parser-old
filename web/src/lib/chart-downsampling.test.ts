import { describe, expect, it } from "vitest";
import { downsampleMinMax } from "./chart-downsampling";

describe("downsampleMinMax", () => {
  it("keeps the min and max in every bucket, including a spike", () => {
    const values = [
      { frame: 0, cpu: 3 },
      { frame: 1, cpu: 9 },
      { frame: 2, cpu: 1 },
      { frame: 3, cpu: 5 },
      { frame: 4, cpu: 4 },
      { frame: 5, cpu: -2 },
      { frame: 6, cpu: 80 },
      { frame: 7, cpu: 3 },
    ];

    const sampled = downsampleMinMax({
      values,
      maxPoints: 6,
      metrics: (point) => [point.cpu],
    });

    expect(sampled.map((point) => point.frame)).toEqual([0, 1, 2, 5, 6, 7]);
  });

  it("preserves extrema for every rendered metric", () => {
    const values = [
      { frame: 0, cpu: 1, gpu: 8 },
      { frame: 1, cpu: 9, gpu: 2 },
      { frame: 2, cpu: 2, gpu: 10 },
      { frame: 3, cpu: 8, gpu: 1 },
      { frame: 4, cpu: 3, gpu: 7 },
      { frame: 5, cpu: 7, gpu: 3 },
    ];

    const sampled = downsampleMinMax({
      values,
      maxPoints: 6,
      metrics: (point) => [point.cpu, point.gpu],
    });

    expect(sampled.map((point) => point.frame)).toEqual([0, 1, 2, 3, 4, 5]);
  });
});
