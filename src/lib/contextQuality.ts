import type { ConfidenceLevel, RagChunk } from "../types";

/** RAG score floor used by research chat — chunks below this are weak. */
export const RAG_SUFFICIENCY_THRESHOLD = 0.45;

/** Minimum directional length before treating an answer as substantive. */
const MIN_GROUNDED_ANSWER_CHARS = 80;

/** Significant word length for overlap matching. */
const MIN_WORD_LEN = 4;

/** Overlap count with a RAG chunk that indicates the answer used that context. */
const MIN_CHUNK_WORD_OVERLAP = 3;

const UNCERTAIN_CONFIDENCE: ConfidenceLevel[] = [
  "amber",
  "amber_low",
  "blue",
  "red",
];

function significantWords(text: string): Set<string> {
  return new Set(
    text
      .toLowerCase()
      .split(/\W+/)
      .filter((w) => w.length >= MIN_WORD_LEN),
  );
}

function chunkWordOverlap(response: string, chunkText: string): number {
  const responseWords = significantWords(response);
  const chunkWords = chunkText
    .toLowerCase()
    .split(/\W+/)
    .filter((w) => w.length >= MIN_WORD_LEN);
  return chunkWords.filter((w) => responseWords.has(w)).length;
}

/**
 * True when the directional answer likely drew on retrieved session context
 * (story, technical prep, profile) — not just JD boilerplate at a high score.
 */
export function isAnswerGroundedInContext(
  directionalText: string,
  ragChunks: RagChunk[],
): boolean {
  const answer = directionalText.trim();
  if (answer.length < MIN_GROUNDED_ANSWER_CHARS || ragChunks.length === 0) {
    return false;
  }

  const maxScore = Math.max(...ragChunks.map((c) => c.score));
  if (maxScore < RAG_SUFFICIENCY_THRESHOLD) {
    return false;
  }

  const topChunks = [...ragChunks]
    .sort((a, b) => b.score - a.score)
    .slice(0, 3);

  for (const chunk of topChunks) {
    if (chunkWordOverlap(answer, chunk.text) >= MIN_CHUNK_WORD_OVERLAP) {
      return true;
    }
  }

  // Strong retrieval + detailed answer — likely grounded even without lexical hit
  // on short chunk previews (e.g. truncated context panel text).
  return maxScore >= 0.55 && answer.length >= 120;
}

/**
 * True when Flint likely answered without a user-specific story in context.
 * Uses orchestrator confidence plus whether the directional answer overlaps RAG.
 * Clarifying questions and grey confidence alone do not imply missing stories.
 */
export function needsUserContext(
  confidence: ConfidenceLevel | null,
  ragChunks: RagChunk[],
  directionalText = "",
): boolean {

  if (confidence === "green") {
    return false;
  }

  if (isAnswerGroundedInContext(directionalText, ragChunks)) {
    return false;
  }

  if (ragChunks.length === 0) {
    return true;
  }

  const maxScore = Math.max(...ragChunks.map((c) => c.score));
  if (maxScore < RAG_SUFFICIENCY_THRESHOLD) {
    return true;
  }

  if (confidence != null && UNCERTAIN_CONFIDENCE.includes(confidence)) {
    return true;
  }

  // Grey = clarifying question emitted; only prompt for more context when the
  // directional answer is empty or too thin to be story-backed.
  if (confidence === "grey") {
    return directionalText.trim().length < MIN_GROUNDED_ANSWER_CHARS;
  }

  return false;
}
