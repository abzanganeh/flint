import { useCallback, useEffect, useState } from "react";

import {
  getSessionFocus,
  getSessionSnapshot,
  type SessionFocusDto,
} from "../commands";

interface Props {
  sessionId: string;
  /** Opens Settings on the Session Focus tab when a badge is clicked. */
  onOpenSettings?: () => void;
}

const SESSION_TYPE_LABELS: Record<string, string> = {
  interview: "Interview",
  meeting: "Meeting",
  presentation: "Presentation",
  negotiation: "Negotiation",
  other: "Other",
};

const MAX_FOCUS_TAGS = 3;

export default function SessionContextBadges({ sessionId, onOpenSettings }: Props) {
  const [phoneCallMode, setPhoneCallMode] = useState(false);
  const [sessionType, setSessionType] = useState<string | null>(null);
  const [domain, setDomain] = useState<string | null>(null);
  const [focus, setFocus] = useState<SessionFocusDto | null>(null);

  const load = useCallback(async () => {
    try {
      const [snapshot, focusData] = await Promise.all([
        getSessionSnapshot(),
        getSessionFocus(sessionId),
      ]);
      setPhoneCallMode(snapshot.phoneCallMode ?? false);
      setSessionType(snapshot.sessionType ?? null);
      setDomain(snapshot.domain ?? null);
      setFocus(focusData);
    } catch {
      // Non-fatal — header badges are informational only.
    }
  }, [sessionId]);

  useEffect(() => {
    void load();
  }, [load]);

  const focusTags = focus?.focusTags ?? [];
  const visibleTags = focusTags.slice(0, MAX_FOCUS_TAGS);
  const hiddenTagCount = Math.max(0, focusTags.length - MAX_FOCUS_TAGS);
  const sessionTypeLabel = sessionType
    ? SESSION_TYPE_LABELS[sessionType] ?? sessionType
    : null;
  const domainLabel =
    domain && domain.trim() && domain.trim().toLowerCase() !== "general"
      ? domain.trim()
      : null;

  const hasBadges =
    phoneCallMode ||
    visibleTags.length > 0 ||
    hiddenTagCount > 0 ||
    sessionTypeLabel ||
    domainLabel;

  if (!hasBadges) return null;

  const handleClick = onOpenSettings
    ? () => onOpenSettings()
    : undefined;

  return (
    <div
      className="session-context-badges"
      data-testid="session-context-badges"
      role={handleClick ? "button" : undefined}
      tabIndex={handleClick ? 0 : undefined}
      onClick={handleClick}
      onKeyDown={
        handleClick
          ? (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onOpenSettings?.();
              }
            }
          : undefined
      }
      title={handleClick ? "Open session settings" : undefined}
    >
      {phoneCallMode && (
        <span className="session-context-badge session-context-badge--phone">
          Phone interview
        </span>
      )}
      {sessionTypeLabel && (
        <span className="session-context-badge session-context-badge--meta">
          {sessionTypeLabel}
        </span>
      )}
      {domainLabel && (
        <span
          className="session-context-badge session-context-badge--meta"
          title={domainLabel}
        >
          {domainLabel.length > 24 ? `${domainLabel.slice(0, 22)}…` : domainLabel}
        </span>
      )}
      {visibleTags.map((tag) => (
        <span key={tag} className="session-context-badge session-context-badge--tag">
          {tag}
        </span>
      ))}
      {hiddenTagCount > 0 && (
        <span
          className="session-context-badge session-context-badge--tag"
          title={focusTags.slice(MAX_FOCUS_TAGS).join(", ")}
        >
          +{hiddenTagCount}
        </span>
      )}
    </div>
  );
}
