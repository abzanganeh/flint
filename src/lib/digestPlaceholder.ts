import type { DigestDto } from "../commands";

/** True when digest was produced by the stub LLM (no Groq key at extract time). */
export function isPlaceholderDigest(digest: DigestDto): boolean {
  const role = digest.role.trim().toLowerCase();
  const company = digest.company.trim().toLowerCase();
  const seniority = digest.seniority.trim().toLowerCase();
  return (
    role === "unknown" ||
    company === "unknown" ||
    seniority === "unknown"
  );
}
