import { useEffect, useState } from "react";
import type { ReactNode } from "react";

import {
  checkCrashRecovery,
  getCurrentUser,
  getLegalConsentAccepted,
  importFromSmartResume,
  setSessionState,
  type CompanyIntelDto,
  type RecoveryOffer,
} from "./commands";
import { onSmartResumeImportToken } from "./events";
import { parseFlintImportToken } from "./lib/smartResumeImport";
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

function buildContextText(jdText: string, companyIntel?: CompanyIntelDto): string {
  const parts = [jdText.trim()];
  if (companyIntel) {
    const block: string[] = ["--- COMPANY CONTEXT (from Smart Resume) ---"];
    if (companyIntel.mission) block.push(`Company Mission: ${companyIntel.mission}`);
    if (companyIntel.values.length > 0) block.push(`Core Values: ${companyIntel.values.join(", ")}`);
    if (companyIntel.cultureNotes) block.push(`Culture: ${companyIntel.cultureNotes}`);
    block.push("---");
    parts.push(block.join("\n"));
  }
  return parts.filter(Boolean).join("\n\n");
}

function App() {
  const [screen, setScreen] = useState<AppScreen>("loading");
  const [onboardingStep, setOnboardingStep] = useState<"legal" | "auth">("legal");
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [recoveryOffer, setRecoveryOffer] = useState<RecoveryOffer | null>(null);
  const [sessionPreFill, setSessionPreFill] = useState<SessionPreFill | null>(null);
  const [pendingImportToken, setPendingImportToken] = useState<string | null>(null);
  const [importError, setImportError] = useState<string | null>(null);
  const [importLoading, setImportLoading] = useState(false);

  const queueImportToken = (token: string | null) => {
    if (token) setPendingImportToken(token);
  };

  const processSmartResumeImport = async (token: string) => {
    setImportLoading(true);
    setImportError(null);
    try {
      const result = await importFromSmartResume(token);
      setSessionPreFill({
        name: result.sessionName,
        sessionType: result.sessionType,
        domain: result.domain,
        contextText: buildContextText(result.jdText, result.companyIntel),
        profileText: result.resumeSummary,
        smartResumeSessionId: result.smartResumeSessionId,
      });
      setScreen("session-design");
    } catch (err) {
      setImportError(String(err));
    } finally {
      setImportLoading(false);
      setPendingImportToken(null);
    }
  };

  useEffect(() => {
    let active = true;
    const unlistenPromise = onSmartResumeImportToken((token) => {
      if (active) queueImportToken(token);
    });

    void (async () => {
      try {
        const { getCurrent, onOpenUrl } = await import("@tauri-apps/plugin-deep-link");
        const startUrls = await getCurrent();
        if (active && startUrls) {
          for (const url of startUrls) {
            const token = parseFlintImportToken(url);
            if (token) queueImportToken(token);
          }
        }
        await onOpenUrl((urls) => {
          if (!active) return;
          for (const url of urls) {
            const token = parseFlintImportToken(url);
            if (token) queueImportToken(token);
          }
        });
      } catch {
        // Deep-link plugin unavailable outside Tauri shell (e.g. vitest).
      }
    })();

    return () => {
      active = false;
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    if (!pendingImportToken || importLoading) return;
    const readyScreens: AppScreen[] = ["health", "session-design", "session-list"];
    if (!readyScreens.includes(screen)) return;
    void processSmartResumeImport(pendingImportToken);
  }, [pendingImportToken, screen, importLoading]);

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
        {importLoading && (
          <div className="sd-import-banner" role="status" aria-live="polite">
            Importing from Smart Resume…
          </div>
        )}
        {importError && (
          <div className="sd-import-error" role="alert">
            {importError}
          </div>
        )}
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
