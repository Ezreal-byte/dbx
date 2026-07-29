import { describe, expect, it } from "vitest";
import { sampleTransferSpeed } from "@/lib/ssh/transferSpeed";

describe("sampleTransferSpeed", () => {
  it("ignores bursty chunk intervals until a stable sample window is available", () => {
    let sample = sampleTransferSpeed(undefined, 0, 0);
    sample = sampleTransferSpeed(sample, 64 * 1024, 5);
    sample = sampleTransferSpeed(sample, 512 * 1024, 250);
    expect(sample.speed).toBe(0);

    sample = sampleTransferSpeed(sample, 2 * 1024 * 1024, 1000);
    expect(sample.speed).toBe(2 * 1024 * 1024);
  });

  it("resets when a task restarts from a smaller transferred value", () => {
    const previous = {
      transferred: 1024,
      windowStartedAt: 100,
      windowStartedTransferred: 0,
      speed: 2048,
    };
    expect(sampleTransferSpeed(previous, 0, 200)).toEqual({
      transferred: 0,
      windowStartedAt: 200,
      windowStartedTransferred: 0,
      speed: 0,
    });
  });
});
