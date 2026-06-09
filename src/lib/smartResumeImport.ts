import type { CompanyIntelDto } from "../commands";

const IMPORT_TOKEN_RE = /^[0-9a-f-]{1,64}$/i;

function isImportToken(value: string): boolean {
  const trimmed = value.trim();
  return trimmed.length > 0 && trimmed.length <= 64 && IMPORT_TOKEN_RE.test(trimmed);
}

/** Parse `flint://import?token=<uuid>` deep links from Smart Resume. */
export function parseFlintImportToken(url: string): string | null {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "flint:") return null;
    if (parsed.hostname !== "import") return null;
    const token = parsed.searchParams.get("token")?.trim();
    if (!token || !isImportToken(token)) return null;
    return token;
  } catch {
    return null;
  }
}

/** Accept a full `flint://` link or a raw handoff token pasted from Smart Resume. */
export function resolveSmartResumeImportInput(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;

  const fromUrl = parseFlintImportToken(trimmed);
  if (fromUrl) return fromUrl;

  return isImportToken(trimmed) ? trimmed : null;
}

export const SMART_RESUME_SESSION_ID_KEY = "flint.smartResumeSessionId";
export const PENDING_COMPANY_INTEL_KEY = "flint.pendingCompanyIntel";

export function hasCompanyIntel(intel?: CompanyIntelDto): boolean {
  return Boolean(
    intel &&
      (intel.mission || intel.values.length > 0 || intel.cultureNotes),
  );
}

export function persistCompanyIntel(intel?: CompanyIntelDto): void {
  if (hasCompanyIntel(intel)) {
    localStorage.setItem(PENDING_COMPANY_INTEL_KEY, JSON.stringify(intel));
  } else {
    localStorage.removeItem(PENDING_COMPANY_INTEL_KEY);
  }
}

export function loadPendingCompanyIntel(): CompanyIntelDto | undefined {
  try {
    const raw = localStorage.getItem(PENDING_COMPANY_INTEL_KEY);
    if (!raw) return undefined;
    const parsed = JSON.parse(raw) as CompanyIntelDto;
    return hasCompanyIntel(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

/** Append Smart Resume company culture block after the job description. */
export function buildContextText(jdText: string, companyIntel?: CompanyIntelDto): string {
  const parts = [jdText.trim()];
  if (companyIntel && hasCompanyIntel(companyIntel)) {
    const block: string[] = ["--- COMPANY CONTEXT (from Smart Resume) ---"];
    if (companyIntel.mission) block.push(`Company Mission: ${companyIntel.mission}`);
    if (companyIntel.values.length > 0) block.push(`Core Values: ${companyIntel.values.join(", ")}`);
    if (companyIntel.cultureNotes) block.push(`Culture: ${companyIntel.cultureNotes}`);
    block.push("---");
    parts.push(block.join("\n"));
  }
  return parts.filter(Boolean).join("\n\n");
}
