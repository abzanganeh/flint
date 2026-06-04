import { useUIStore } from "../store/ui";

const WARN_TOTAL_TOKENS = 50_000;

const TokenBudgetIndicator = () => {
  const tokenUsage = useUIStore((s) => s.tokenUsage);
  const { total, costEstimate } = tokenUsage;
  const isWarning = total >= WARN_TOTAL_TOKENS;

  if (total === 0) return null;

  return (
    <div
      data-testid="token-budget-indicator"
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
      <span>{total.toLocaleString()} tokens</span>
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
