import { describe, expect, it } from "vitest";

import {
  isAnswerGroundedInContext,
  needsUserContext,
  RAG_SUFFICIENCY_THRESHOLD,
} from "../../lib/contextQuality";
import type { RagChunk } from "../../types";

const fraudChunk: RagChunk = {
  text: "Fraud Shield AI project: silent data leakage from card-level velocity features before train/val split. Rebuilt pipeline with time-aware splits and train-only scaling.",
  score: 0.72,
};

const jdChunk: RagChunk = {
  text: "Senior AI Developer at Fisher Investments. Hybrid RAG combining BM25 and vector search.",
  score: 0.66,
};

const fraudAnswer =
  "On the Fraud Shield AI project, I encountered significant technical debt due to silent data leakage, " +
  "where early checkpoints computed card-level velocity features before the train/val split.";

describe("isAnswerGroundedInContext", () => {
  it("detects overlap between directional answer and story chunk", () => {
    expect(isAnswerGroundedInContext(fraudAnswer, [fraudChunk])).toBe(true);
  });

  it("returns false for generic answer with only JD chunks", () => {
    const generic =
      "Technical debt is common in microservices. Teams should refactor incrementally and add tests.";
    expect(isAnswerGroundedInContext(generic, [jdChunk])).toBe(false);
  });
});

describe("needsUserContext", () => {
  it("does not warn when grey confidence but answer uses saved story", () => {
    expect(
      needsUserContext("grey", [fraudChunk], fraudAnswer),
    ).toBe(false);
  });

  it("warns when grey confidence and directional is empty", () => {
    expect(needsUserContext("grey", [jdChunk], "")).toBe(true);
  });

  it("warns when RAG scores are below threshold", () => {
    const weak: RagChunk = { text: "some context", score: 0.2 };
    expect(
      needsUserContext("amber", [weak], "Short."),
    ).toBe(true);
    expect(weak.score).toBeLessThan(RAG_SUFFICIENCY_THRESHOLD);
  });

  it("does not warn on green confidence", () => {
    expect(needsUserContext("green", [jdChunk], "")).toBe(false);
  });
});
