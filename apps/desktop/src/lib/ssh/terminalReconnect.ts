export const TERMINAL_RECONNECT_DELAYS = [500, 1000, 2000, 5000] as const;

export function terminalReconnectDelay(attempt: number): number {
  const normalizedAttempt = Number.isFinite(attempt) ? Math.max(0, Math.floor(attempt)) : 0;
  return TERMINAL_RECONNECT_DELAYS[Math.min(normalizedAttempt, TERMINAL_RECONNECT_DELAYS.length - 1)];
}

export function shouldReattachTerminal(options: { disposed: boolean; state: "connecting" | "connected" | "disconnected" | "error"; expectedSessionId: string; currentSessionId?: string }): boolean {
  return !options.disposed && options.expectedSessionId === options.currentSessionId && (options.state === "connecting" || options.state === "connected");
}
