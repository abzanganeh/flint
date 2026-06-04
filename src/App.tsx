import { useEffect, useState } from "react";

import {
  checkCrashRecovery,
  getCurrentUser,
  getLegalConsentAccepted,
  setSessionState,
  type RecoveryOffer,
} from "./commands";
import { SessionState } from "./types";
import "./App.css";
import DigestReview from "./screens/DigestReview";
import HealthCheck from "./screens/HealthCheck";
import LiveOverlay from "./screens/LiveOverlay";
import Onboarding from "./screens/Onboarding";
import { Recovery } from "./screens/Recovery";
import Rehearsal from "./screens/Rehearsal";
import SessionDesign from "./screens/SessionDesign";
import { SessionList } from "./screens/SessionList";

type AppScreen =
  | "loading"
  | "onboarding"
  | "health"
  | "recovery"
  | "session-design"
  | "session-list"
  | "digest-review"
  | "rehearsal"
  | "live";

function App() {
  const [screen, setScreen] = useState<AppScreen>("loading");
  const [onboardingStep, setOnboardingStep] = useState<"legal" | "auth">("legal");
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [recoveryOffer, setRecoveryOffer] = useState<RecoveryOffer | null>(null);

  useEffect(() => {
    let cancelled = false;

    const bootstrap = async () => {
      try {
        const consentAccepted = await getLegalConsentAccepted();
        if (cancelled) return;

        if (!consentAccepted) {
          setOnboardingStep("legal");
          setScreen("onboarding");
          return;
        }

        const user = await getCurrentUser().catch(() => null);
        if (cancelled) return;

        if (user) {
          // Check for a crashed session before showing the normal flow.
          const offer = await checkCrashRecovery().catch(() => null);
          if (cancelled) return;
          if (offer) {
            setRecoveryOffer(offer);
            setScreen("recovery");
            return;
          }

          await setSessionState(SessionState.IDLE);
          setScreen("health");
          return;
        }

        setOnboardingStep("auth");
        setScreen("onboarding");
      } catch {
        if (!cancelled) {
          setOnboardingStep("legal");
          setScreen("onboarding");
        }
      }
    };

    void bootstrap();
    return () => {
      cancelled = true;
    };
  }, []);

  if (screen === "loading") {
    return (
      <main className="app-loading" data-testid="app-loading">
        <p>Loading Flint…</p>
      </main>
    );
  }

  if (screen === "onboarding") {
    return (
      <Onboarding
        initialStep={onboardingStep}
        onComplete={() => setScreen("health")}
      />
    );
  }

  if (screen === "recovery" && recoveryOffer) {
    return (
      <Recovery
        offer={recoveryOffer}
        onResume={() => {
          setRecoveryOffer(null);
          setScreen("live");
        }}
        onDiscard={() => {
          setRecoveryOffer(null);
          setScreen("health");
        }}
      />
    );
  }

  if (screen === "health") {
    return <HealthCheck onComplete={() => setScreen("session-design")} />;
  }

  if (screen === "session-list") {
    return <SessionList onBack={() => setScreen("session-design")} />;
  }

  if (screen === "session-design") {
    return (
      <SessionDesign
        onComplete={(sid) => {
          setSessionId(sid);
          setScreen("digest-review");
        }}
        onViewSessions={() => setScreen("session-list")}
      />
    );
  }

  if (screen === "digest-review" && sessionId) {
    return (
      <DigestReview
        sessionId={sessionId}
        onComplete={() => setScreen("rehearsal")}
        onStartOver={() => {
          setSessionId(null);
          setScreen("session-design");
        }}
      />
    );
  }

  if (screen === "rehearsal" && sessionId) {
    return (
      <Rehearsal
        sessionId={sessionId}
        onComplete={() => setScreen("live")}
      />
    );
  }

  if (screen === "live" && sessionId) {
    return (
      <LiveOverlay
        sessionId={sessionId}
        onEnded={() => {
          setSessionId(null);
          setScreen("session-design");
        }}
      />
    );
  }

  return (
    <main className="container" data-testid="app-shell">
      <p>Flint</p>
    </main>
  );
}

export default App;
