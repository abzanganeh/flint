/** Parse `flint://import?token=<uuid>` deep links from Smart Resume. */
export function parseFlintImportToken(url: string): string | null {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "flint:") return null;
    if (parsed.hostname !== "import") return null;
    const token = parsed.searchParams.get("token")?.trim();
    if (!token || token.length > 64) return null;
    return token;
  } catch {
    return null;
  }
}

export const SMART_RESUME_SESSION_ID_KEY = "flint.smartResumeSessionId";
