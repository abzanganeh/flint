import { useUIStore } from "../store/ui";

const CATEGORY_LABELS: Record<string, string> = {
  digest: "Digest",
  pre_warm: "Pre-warm",
  rehearsal_turn: "Rehearsal",
  live_turn: "Live",
  research_chat: "Research",
};

const COST_PER_TOKEN = 0.0000002;

export default function UsageWidget() {
  const tokenUsage = useUIStore((s) => s.tokenUsage);
  const { total, costEstimate, breakdown } = tokenUsage;

  const categories = Object.entries(breakdown).filter(([, v]) => v > 0);
  // Prefer the LLM-reported cost; only fall back to the per-token heuristic
  // when the backend hasn't sent a cost yet but tokens have moved.
  const displayCost = costEstimate > 0 ? costEstimate : total * COST_PER_TOKEN;

  if (total === 0) {
    return (
      <div className="usage-widget" title="Token usage this session">
        <div className="usage-widget__total">
          <span className="usage-widget__tokens">0 tokens</span>
        </div>
      </div>
    );
  }

  return (
    <div className="usage-widget" title="Token usage this session">
      <div className="usage-widget__total">
        <span className="usage-widget__tokens">{total.toLocaleString()} tokens</span>
        <span className="usage-widget__cost">${displayCost.toFixed(4)}</span>
      </div>

      {categories.length > 0 && (
        <ul className="usage-widget__breakdown">
          {categories.map(([cat, tokens]) => (
            <li key={cat} className="usage-widget__breakdown-item">
              <span className="usage-widget__cat-label">
                {CATEGORY_LABELS[cat] ?? cat}
              </span>
              <span className="usage-widget__cat-tokens">{tokens.toLocaleString()}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
