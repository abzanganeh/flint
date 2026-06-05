import { useEffect, useState } from "react";
import type { ReactNode } from "react";

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
import SessionDesign, { type SessionPreFill } from "./screens/SessionDesign";
import { SessionList } from "./screens/SessionList";
import TitleBar, { type NavItem } from "./components/TitleBar";

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

// Screens that render inside the standard shell (title bar + top padding).
// "live" has its own frameless overlay layout.
const SHELL_SCREENS: AppScreen[] = [
  "onboarding",
  "health",
  "recovery",
  "session-design",
  "session-list",
  "digest-review",
  "rehearsal",
];

interface ShellProps {
  children: ReactNode;
  nav?: NavItem[];
}

function Shell({ children, nav }: ShellProps) {
  return (
    <>
      <TitleBar nav={nav} />
      <div style={{ paddingTop: 36 }}>{children}</div>
    </>
  );
}

function App() {
  const [screen, setScreen] = useState<AppScreen>("loading");
  const [onboardingStep, setOnboardingStep] = useState<"legal" | "auth">("legal");
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [recoveryOffer, setRecoveryOffer] = useState<RecoveryOffer | null>(null);
  const [sessionPreFill, setSessionPreFill] = useState<SessionPreFill | null>(null);

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

        // Crash recovery check runs before auth — local SQLite data is
        // independent of Supabase login state.
        const offer = await checkCrashRecovery().catch(() => null);
        if (cancelled) return;
        if (offer) {
          setRecoveryOffer(offer);
          setScreen("recovery");
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
    return () => {
      cancelled = true;
    };
  }, []);

  // Nav items shown on setup/design screens.
  const designNav: NavItem[] = [
    {
      label: "New Session",
      onClick: () => setScreen("session-design"),
      active: screen === "session-design",
    },
    {
      label: "Past Sessions",
      onClick: () => setScreen("session-list"),
      active: screen === "session-list",
    },
  ];

  const isShellScreen = SHELL_SCREENS.includes(screen);

  if (screen === "loading") {
    return (
      <main className="app-loading" data-testid="app-loading">
        <p>Loading Flint…</p>
      </main>
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

  const nav = isShellScreen ? designNav : undefined;

  if (screen === "onboarding") {
    return (
      <Shell nav={nav}>
        <Onboarding
          initialStep={onboardingStep}
          onComplete={() => setScreen("health")}
        />
      </Shell>
    );
  }

  if (screen === "recovery" && recoveryOffer) {
    return (
      <Shell>
        <Recovery
          offer={recoveryOffer}
          onResume={() => {
            setSessionId(recoveryOffer.sessionId);
            setRecoveryOffer(null);
            setScreen("live");
          }}
          onDiscard={() => {
            setRecoveryOffer(null);
            setScreen("health");
          }}
        />
      </Shell>
    );
  }

  if (screen === "health") {
    return (
      <Shell>
        <HealthCheck onComplete={() => setScreen("session-design")} />
      </Shell>
    );
  }

  if (screen === "session-list") {
    return (
      <Shell nav={nav}>
        <SessionList
          onBack={() => setScreen("session-design")}
          onStartSimilar={(preFill) => {
            setSessionPreFill(preFill);
            setScreen("session-design");
          }}
        />
      </Shell>
    );
  }

  if (screen === "session-design") {
    return (
      <Shell nav={nav}>
        <SessionDesign
          // key forces a fresh mount when preFill changes so useState
          // initial values pick up the new data.
          key={sessionPreFill ? `${sessionPreFill.name}|${sessionPreFill.sessionType}` : "new"}
          preFill={sessionPreFill ?? undefined}
          onComplete={(sid) => {
            setSessionPreFill(null);
            setSessionId(sid);
            setScreen("digest-review");
          }}
          onViewSessions={() => setScreen("session-list")}
        />
      </Shell>
    );
  }

  if (screen === "digest-review" && sessionId) {
    return (
      <Shell nav={nav}>
        <DigestReview
          sessionId={sessionId}
          onComplete={() => setScreen("rehearsal")}
          onStartOver={() => {
            setSessionId(null);
            setScreen("session-design");
          }}
        />
      </Shell>
    );
  }

  if (screen === "rehearsal" && sessionId) {
    return (
      <Shell nav={nav}>
        <Rehearsal
          sessionId={sessionId}
          onComplete={() => setScreen("live")}
        />
      </Shell>
    );
  }

  return (
    <main className="container" data-testid="app-shell">
      <p>Flint</p>
    </main>
  );
}

export default App;
