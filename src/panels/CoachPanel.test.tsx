import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import CoachPanel from "./CoachPanel";
import type { CoachFeedback } from "../commands";

const sampleFeedback: CoachFeedback = {
  grammar_issues: [],
  tone: { assessment: "confident", suggestion: "Strong opening." },
  context_gaps: ["Add a metric for impact"],
  corrected_answer: "I led the IAM migration.",
  score: 78,
  axes: {
    content: 82,
    specificity: 70,
    company_alignment: 75,
    delivery: 80,
  },
};

describe("CoachPanel", () => {
  it("renders rubric axes when present", () => {
    render(<CoachPanel feedback={sampleFeedback} isLoading={false} score={78} />);
    expect(screen.getByText("RUBRIC")).toBeTruthy();
    expect(screen.getByText("Content")).toBeTruthy();
    expect(screen.getByText("Company fit")).toBeTruthy();
    expect(screen.getByText("78")).toBeTruthy();
  });

  it("renders loading state", () => {
    render(<CoachPanel feedback={null} isLoading score={0} />);
    expect(screen.getByText("Analyzing your answer…")).toBeTruthy();
  });

  it("hides rubric when axes are all zero", () => {
    const legacy: CoachFeedback = {
      ...sampleFeedback,
      axes: { content: 0, specificity: 0, company_alignment: 0, delivery: 0 },
    };
    render(<CoachPanel feedback={legacy} isLoading={false} score={0} />);
    expect(screen.queryByText("RUBRIC")).toBeNull();
  });
});
