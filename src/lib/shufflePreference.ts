const STORAGE_KEY = "flint-shuffle-questions";

export function readShuffleQuestionsPreference(): boolean {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) return true;
    return raw === "true";
  } catch {
    return true;
  }
}

export function writeShuffleQuestionsPreference(enabled: boolean): void {
  try {
    localStorage.setItem(STORAGE_KEY, String(enabled));
  } catch {
    // Private browsing or storage blocked — ignore.
  }
}
