import { describe, expect, it } from "vitest";
import { shouldReattachTerminal, terminalReconnectDelay } from "../terminalReconnect";

describe("terminal reconnect policy", () => {
  it("backs off and caps retries at five seconds", () => {
    expect([0, 1, 2, 3, 4, 20].map(terminalReconnectDelay)).toEqual([500, 1000, 2000, 5000, 5000, 5000]);
  });

  it("only reattaches the same active session", () => {
    const base = {
      disposed: false,
      expectedSessionId: "session-a",
      currentSessionId: "session-a",
    };

    expect(shouldReattachTerminal({ ...base, state: "connected" })).toBe(true);
    expect(shouldReattachTerminal({ ...base, state: "connecting" })).toBe(true);
    expect(shouldReattachTerminal({ ...base, state: "disconnected" })).toBe(false);
    expect(shouldReattachTerminal({ ...base, state: "error" })).toBe(false);
    expect(shouldReattachTerminal({ ...base, disposed: true, state: "connected" })).toBe(false);
    expect(
      shouldReattachTerminal({
        ...base,
        currentSessionId: "session-b",
        state: "connected",
      }),
    ).toBe(false);
  });
});
