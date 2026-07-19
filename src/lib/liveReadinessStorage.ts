import type { LiveReadinessConfigFingerprintDto } from "../commands";

const TESTED_KEY_PREFIX = "flint.liveReadinessTested.v1";

export interface LiveReadinessTestRecord {
  fingerprint: LiveReadinessConfigFingerprintDto;
  testedAt: number;
}

function storageKey(sessionId: string): string {
  return `${TESTED_KEY_PREFIX}.${sessionId}`;
}

export function fingerprintMatches(
  a: LiveReadinessConfigFingerprintDto,
  b: LiveReadinessConfigFingerprintDto,
): boolean {
  return (
    a.phoneCallMode === b.phoneCallMode &&
    a.headphoneOverride === b.headphoneOverride &&
    a.micCalibrationPassed === b.micCalibrationPassed &&
    a.deviceFingerprint === b.deviceFingerprint &&
    a.pulseSystemSource === b.pulseSystemSource &&
    a.pulseMicSource === b.pulseMicSource
  );
}

export function loadLiveReadinessTestRecord(
  sessionId: string,
): LiveReadinessTestRecord | null {
  try {
    const raw = window.localStorage.getItem(storageKey(sessionId));
    if (!raw) return null;
    return JSON.parse(raw) as LiveReadinessTestRecord;
  } catch {
    return null;
  }
}

export function saveLiveReadinessTestRecord(
  sessionId: string,
  fingerprint: LiveReadinessConfigFingerprintDto,
): void {
  try {
    const record: LiveReadinessTestRecord = {
      fingerprint,
      testedAt: Date.now(),
    };
    window.localStorage.setItem(storageKey(sessionId), JSON.stringify(record));
  } catch {
    // Non-fatal.
  }
}

export function isLiveReadinessTestCurrent(
  sessionId: string,
  fingerprint: LiveReadinessConfigFingerprintDto,
): boolean {
  const record = loadLiveReadinessTestRecord(sessionId);
  if (!record) return false;
  return fingerprintMatches(record.fingerprint, fingerprint);
}
