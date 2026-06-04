import { useEffect } from "react";

import { onTokenUsageUpdate } from "../events";
import { useUIStore } from "../store/ui";

// Cap beyond which the amber warning fires — matches the cost cap default in
// the Rust orchestrator (configurable by the user in Settings, not wired here).
const WARN_TOTAL_TOKENS = 50_000;

// ── Component ────────────────────────────────────────────────────────────────

const TokenBudgetIndicator = () => {
  const { tokenUsage, setTokenUsage } = useUIStore();

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    const setup = async () => {
      const fn = await onTokenUsageUpdate(
        ({ input, output, total, cost_estimate }) => {
          setTokenUsage({
            input,
            output,
            total,
            costEstimate: cost_estimate,
          });
        },
      );
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    };

    void setup();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const { total, costEstimate } = tokenUsage;
  const isWarning = total >= WARN_TOTAL_TOKENS;

  if (total === 0) return null;

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "3px 10px",
        backgroundColor: "#0f1117",
        borderTop: "1px solid #1e2028",
        fontSize: "11px",
        color: isWarning ? "#f59e0b" : "#6b7280",
        flexShrink: 0,
      }}
    >
      <span>
        {total.toLocaleString()} tokens
      </span>
      <span style={{ color: "#374151" }}>·</span>
      <span>${costEstimate.toFixed(4)}</span>
      {isWarning && (
        <span style={{ color: "#f59e0b", fontWeight: 600 }}>
          Approaching free tier limit
        </span>
      )}
    </div>
  );
};

export default TokenBudgetIndicator;
