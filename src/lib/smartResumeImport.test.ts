import { describe, expect, it } from "vitest";

import { parseFlintImportToken } from "./smartResumeImport";

describe("parseFlintImportToken", () => {
  it("extracts token from flint import URL", () => {
    expect(
      parseFlintImportToken(
        "flint://import?token=550e8400-e29b-41d4-a716-446655440000",
      ),
    ).toBe("550e8400-e29b-41d4-a716-446655440000");
  });

  it("returns null for wrong scheme", () => {
    expect(parseFlintImportToken("https://example.com/import?token=abc")).toBeNull();
  });

  it("returns null for missing token", () => {
    expect(parseFlintImportToken("flint://import")).toBeNull();
  });
});
