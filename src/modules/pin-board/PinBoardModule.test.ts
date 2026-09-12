import { describe, expect, it } from "vitest";

describe("pin-board module entry", () => {
  it("loads through its public React entry without starting native work", async () => {
    const module = await import("./PinBoardModule");
    expect(module.PinBoardModule).toBeTypeOf("function");
  });
});
