import { describe, expect, it } from "vitest";

import { parseFlintImportToken, resolveSmartResumeImportInput, buildContextText } from "./smartResumeImport";

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

describe("resolveSmartResumeImportInput", () => {
  const token = "550e8400-e29b-41d4-a716-446655440000";

  it("accepts a full flint import URL", () => {
    expect(resolveSmartResumeImportInput(`flint://import?token=${token}`)).toBe(token);
  });

  it("accepts a raw handoff token", () => {
    expect(resolveSmartResumeImportInput(token)).toBe(token);
  });

  it("rejects invalid input", () => {
    expect(resolveSmartResumeImportInput("not-a-token")).toBeNull();
  });
});

describe("buildContextText", () => {
  it("appends company culture block when intel is present", () => {
    const result = buildContextText("Senior Engineer role", {
      mission: "Better the investment universe",
      values: ["Collaboration", "Learning"],
      cultureNotes: "Inclusive, in-office culture",
    });
    expect(result).toContain("--- COMPANY CONTEXT (from Smart Resume) ---");
    expect(result).toContain("Company Mission: Better the investment universe");
    expect(result).toContain("Core Values: Collaboration, Learning");
    expect(result).toContain("Culture: Inclusive, in-office culture");
  });

  it("returns JD only when company intel is empty", () => {
    expect(buildContextText("Senior Engineer role")).toBe("Senior Engineer role");
  });
});
