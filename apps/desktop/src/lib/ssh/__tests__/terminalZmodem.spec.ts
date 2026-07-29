import { describe, expect, it, vi } from "vitest";
import type { Session, Transfer } from "zmodem.js";
import { sendZmodemFiles } from "../terminalZmodem";

function fakeFile(name: string, size: number): File {
  const bytes = new Uint8Array(size);
  return {
    name,
    size,
    lastModified: 1_700_000_000_000,
    slice(start = 0, end = size) {
      return new Blob([bytes.slice(start, end)]);
    },
  } as File;
}

describe("ZMODEM upload streaming", () => {
  it("streams files in bounded chunks and closes the session", async () => {
    const sends: number[] = [];
    const ends: number[] = [];
    const transfer: Transfer = {
      get_offset: () => 0,
      send: vi.fn((data: Uint8Array) => {
        sends.push(data.byteLength);
      }),
      end: vi.fn(async (data: Uint8Array) => {
        ends.push(data.byteLength);
      }),
    };
    const session = {
      send_offer: vi.fn(async () => transfer),
      aborted: () => false,
      close: vi.fn(async () => undefined),
    } as unknown as Session;
    const progress = vi.fn();

    await sendZmodemFiles(session, [fakeFile("large.bin", 150_000)], progress);

    expect(sends).toEqual([65_536, 65_536]);
    expect(ends).toEqual([18_928]);
    expect(progress).toHaveBeenLastCalledWith(
      expect.objectContaining({
        fileTransferred: 150_000,
        totalTransferred: 150_000,
        totalSize: 150_000,
      }),
    );
    expect(session.close).toHaveBeenCalledOnce();
  });

  it("offers multiple files and handles empty files", async () => {
    const transfer: Transfer = {
      get_offset: () => 0,
      send: vi.fn(),
      end: vi.fn(async () => undefined),
    };
    const session = {
      send_offer: vi.fn(async () => transfer),
      aborted: () => false,
      close: vi.fn(async () => undefined),
    } as unknown as Session;

    await sendZmodemFiles(session, [fakeFile("empty.txt", 0), fakeFile("data.bin", 3)]);

    expect(session.send_offer).toHaveBeenCalledTimes(2);
    expect(transfer.end).toHaveBeenCalledTimes(2);
    expect(session.close).toHaveBeenCalledOnce();
  });
});
