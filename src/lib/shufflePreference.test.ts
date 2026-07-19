import { beforeEach, describe, expect, it } from "vitest";

import { readShuffleQuestionsPreference, writeShuffleQuestionsPreference } from "./shufflePreference";

describe("shufflePreference", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("defaults to disabled (insertion order) when unset", () => {
    expect(readShuffleQuestionsPreference()).toBe(false);
  });

  it("persists an explicit preference", () => {
    writeShuffleQuestionsPreference(true);
    expect(readShuffleQuestionsPreference()).toBe(true);

    writeShuffleQuestionsPreference(false);
    expect(readShuffleQuestionsPreference()).toBe(false);
  });
});
