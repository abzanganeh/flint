import { describe, expect, it } from "vitest";

import type { DigestDto } from "../../commands";
import { isPlaceholderDigest } from "../../lib/digestPlaceholder";

const stubDigest: DigestDto = {
  role: "Unknown",
  company: "Unknown",
  domain: "software engineering",
  keySkills: [],
  seniority: "unknown",
  likelyQuestions: ["Tell me about yourself"],
  topicsToAvoid: [],
};

describe("isPlaceholderDigest", () => {
  it("detects stub LLM placeholder fields", () => {
    expect(isPlaceholderDigest(stubDigest)).toBe(true);
  });

  it("returns false for a real extracted digest", () => {
    expect(
      isPlaceholderDigest({
        ...stubDigest,
        role: "Senior Engineer",
        company: "Acme",
        seniority: "senior",
      }),
    ).toBe(false);
  });
});
