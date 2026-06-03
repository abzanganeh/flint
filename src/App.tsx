import { useEffect, useState } from "react";

import {
  getCurrentUser,
  getLegalConsentAccepted,
  setSessionState,
} from "./commands";
import { SessionState } from "./types";
import "./App.css";
import DigestReview from "./screens/DigestReview";
import HealthCheck from "./screens/HealthCheck";
import Onboarding from "./screens/Onboarding";
import SessionDesign from "./screens/SessionDesign";

type AppScreen =
  | "loading"
  | "onboarding"
  | "health"
  | "session-design"
  | "digest-review"
  | "shell";

function App() {
  const [screen, setScreen] = useState<AppScreen>("loading");
  const [onboardingStep, setOnboardingStep] = useState<"legal" | "auth">("legal");
  const [sessionId, setSessionId] = useState<string | null>(null);

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
    return () => { cancelled = true; };
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

  if (screen === "health") {
    return <HealthCheck onComplete={() => setScreen("session-design")} />;
  }

  if (screen === "session-design") {
    return (
      <SessionDesign
        onComplete={(sid) => {
          setSessionId(sid);
          setScreen("digest-review");
        }}
      />
    );
  }

  if (screen === "digest-review" && sessionId) {
    return (
      <DigestReview
        sessionId={sessionId}
        onComplete={() => setScreen("shell")}
        onStartOver={() => {
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
