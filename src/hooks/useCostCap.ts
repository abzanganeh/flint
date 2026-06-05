import { useEffect } from "react";

import { onCostCapStatus, onInferenceSuspended } from "../events";
import { useUIStore } from "../store/ui";

/**
 * Subscribe to cost-cap lifecycle events emitted by the Rust orchestrator.
 *
 * The Rust side is authoritative — this hook only mirrors transitions into
 * the Zustand store so that the overlay can render warnings, badges, and
 * the suspension banner. Users decide how to respond via the dedicated
 * `setCostCap` / `liftCostSuspension` Tauri commands.
 */
export function useCostCap(): void {
  const setCostCap = useUIStore((s) => s.setCostCap);
  const pushNotification = useUIStore((s) => s.pushNotification);

  useEffect(() => {
    let cancelled = false;
    let unsubStatus: (() => void) | null = null;
    let unsubSuspended: (() => void) | null = null;

    const setup = async () => {
      const statusFn = await onCostCapStatus((payload) => {
        setCostCap({
          status: payload.status,
          suspended: payload.suspended,
          fractionUsed: payload.fraction_used,
          maxTotalTokens: payload.max_total_tokens,
          maxCostEstimateUsd: payload.max_cost_estimate_usd,
        });
      });
      const suspendedFn = await onInferenceSuspended((payload) => {
        pushNotification({
          id: `inference-suspended-${Date.now()}`,
          level: "error",
          message: `Inference suspended — cost cap reached (${payload.total_tokens.toLocaleString()} tokens, $${payload.cost_estimate_usd.toFixed(4)}).`,
        });
      });
      if (cancelled) {
        statusFn();
        suspendedFn();
      } else {
        unsubStatus = statusFn;
        unsubSuspended = suspendedFn;
      }
    };

    void setup();

    return () => {
      cancelled = true;
      unsubStatus?.();
      unsubSuspended?.();
    };
  }, [setCostCap, pushNotification]);
}
