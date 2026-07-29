import { describe, expect, it } from "vitest";
import { getSshWorkbenchSplitLayout } from "@/lib/ssh/workbenchLayout";

describe("getSshWorkbenchSplitLayout", () => {
  it("keeps the terminal on the left by default", () => {
    expect(getSshWorkbenchSplitLayout("terminal-left")).toEqual({
      rtl: false,
      flexDirection: "row",
    });
  });

  it("reverses both the visual order and Splitpanes drag coordinates", () => {
    expect(getSshWorkbenchSplitLayout("sftp-left")).toEqual({
      rtl: true,
      flexDirection: "row-reverse",
    });
  });
});
