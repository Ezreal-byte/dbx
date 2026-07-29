import { describe, expect, it } from "vitest";
import { Osc7DirectoryParser, parseOsc7Path } from "../terminalDirectoryTracking";

describe("OSC 7 directory tracking", () => {
  it("parses URL encoded paths", () => {
    expect(parseOsc7Path("file://host/home/test%20user")).toBe("/home/test user");
  });

  it("handles BEL terminated sequences split across frames", () => {
    const parser = new Osc7DirectoryParser();
    expect(parser.push("\u001b]7;file://host/home/te")).toEqual([]);
    expect(parser.push("st\u0007prompt")).toEqual(["/home/test"]);
  });

  it("handles ST terminated sequences", () => {
    const parser = new Osc7DirectoryParser();
    expect(parser.push("\u001b]7;file://host/tmp\u001b\\")).toEqual(["/tmp"]);
  });
});
