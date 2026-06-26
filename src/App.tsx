import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

import {
  abandonSessionDraft,
  checkCrashRecovery,
  getCurrentUser,
  getLegalConsentAccepted,
  getPendingImportToken,
  getSessionSnapshot,
  getSessionFocus,
  importFromSmartResume,
  reopenSession,
  restoreDraftSession,
  returnToSessionDesign,
  stopSession,
  type RecoveryOffer,
  type SessionSnapshotDto,
} from "./commands";
import { onSmartResumeImportToken } from "./events";
import {
  buildCompanyOverviewText,
  parseFlintImportToken,
  persistCompanyIntel,
} from "./lib/smartResumeImport";
import { useFeatureFlag } from "./hooks/useFeatureFlag";
import { SessionState } from "./types";
import "./App.css";
import "./components/rehearsal-enrichment.css";
import DigestReview from "./screens/DigestReview";
import HealthCheck from "./screens/HealthCheck";
import LiveOverlay from "./screens/LiveOverlay";
import Onboarding from "./screens/Onboarding";
import { Recovery } from "./screens/Recovery";
import Rehearsal from "./screens/Rehearsal";
import MockInterview from "./screens/MockInterview";
import MockSummary from "./screens/MockSummary";
import SessionDesign, { type SessionPreFill } from "./screens/SessionDesign";
import SessionFocusGate from "./screens/SessionFocusGate";
import MicCalibration from "./screens/MicCalibration";
import TitleBar, { type NavItem } from "./components/TitleBar";
import { SessionList } from "./screens/SessionList";
import { SessionReview } from "./screens/SessionReview";
import { SessionSummary } from "./screens/SessionSummary";
import Settings from "./screens/Settings";

type AppScreen =
  | "loading"
  | "onboarding"
  | "health"
  | "recovery"
  | "session-design"
  | "session-list"
  | "settings"
  | "digest-review"
  | "session-focus"
  | "mic-calibration"
  | "rehearsal"
  | "mock-interview"
  | "mock-summary"
  | "live"
  | "session-summary"
  | "session-review";

// Screens that render inside the standard shell (title bar + top padding).
// "live" has its own frameless overlay layout.
const SHELL_SCREENS: AppScreen[] = [
  "onboarding",
  "health",
  "recovery",
  "session-design",
  "session-list",
  "settings",
  "digest-review",
  "session-focus",
  "mic-calibration",
  "rehearsal",
  "mock-interview",
  "mock-summary",
  "session-summary",
  "session-review",
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

function preFillFromSnapshot(snapshot: SessionSnapshotDto): SessionPreFill | null {
  if (!snapshot.sessionId) return null;
  return {
    name: snapshot.name ?? "",
    sessionType: snapshot.sessionType ?? "interview",
    domain: snapshot.domain ?? "software engineering",
    // Carry both for backward compat — SessionDesign prefers contextFields.
    contextText: snapshot.contextText,
    contextFields: snapshot.contextFields,
  };
}

function screenForDraftState(state: string): AppScreen {
  switch (state) {
    case SessionState.CONFIGURING:
    case SessionState.INGESTING:
      return "session-design";
    case SessionState.DIGEST_REVIEW:
    case SessionState.PRE_WARMING:
      return "digest-review";
    case SessionState.REHEARSING:
    case SessionState.MOCK_INTERVIEW:
    case SessionState.READY:
      return "rehearsal";
    default:
      return "session-design";
  }
}

function applyDraftSnapshot(
  snapshot: SessionSnapshotDto,
  setSessionId: (id: string | null) => void,
  setSessionPreFill: (preFill: SessionPreFill | null) => void,
  setScreen: (screen: AppScreen) => void,
) {
  if (!snapshot.sessionId || snapshot.state === SessionState.IDLE) {
    setSessionId(null);
    setSessionPreFill(null);
    setScreen("session-design");
    return;
  }

  setSessionId(snapshot.sessionId);
  setSessionPreFill(preFillFromSnapshot(snapshot));
  setScreen(screenForDraftState(snapshot.state));
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
  const [settingsReturnScreen, setSettingsReturnScreen] =
    useState<AppScreen>("session-design");
  const [settingsInitialTab, setSettingsInitialTab] = useState<
    "api-keys" | "usage-cap" | "account" | "privacy" | "session-focus"
  >("account");
  /** When true, Rehearsal clears stale panel state (after re-ingest). */
  const [resetRehearsalPanels, setResetRehearsalPanels] = useState(false);
  const [forceMicCalibrationRetest, setForceMicCalibrationRetest] = useState(false);
  const [sessionPhoneCallMode, setSessionPhoneCallMode] = useState(false);
  const [reviewSessionId, setReviewSessionId] = useState<string | null>(null);
  const importInFlightRef = useRef<string | null>(null);
  const queuedTokenRef = useRef<string | null>(null);
  const postSessionSummaryEnabled = useFeatureFlag("post_session_summary", true);

  const openSettings = (
    returnTo: AppScreen = screen,
    initialTab: "api-keys" | "usage-cap" | "account" | "privacy" | "session-focus" = "account",
  ) => {
    if (screen !== "settings") {
      const safeReturn = returnTo === "settings" ? screen : returnTo;
      setSettingsReturnScreen(safeReturn);
    }
    setSettingsInitialTab(initialTab);
    setScreen("settings");
  };

  const handleSettingsBack = useCallback(() => {
    let target = settingsReturnScreen;
    if (target === "settings") {
      void getSessionSnapshot()
        .then((snapshot) => {
          if (snapshot.sessionId) {
            setSessionId(snapshot.sessionId);
            setScreen(screenForDraftState(snapshot.state));
          } else {
            setScreen("session-design");
          }
        })
        .catch(() => setScreen("session-design"));
      return;
    }
    if (target === "rehearsal" && !sessionId) {
      void getSessionSnapshot()
        .then((snapshot) => {
          if (snapshot.sessionId) {
            setSessionId(snapshot.sessionId);
            setScreen("rehearsal");
          } else {
            setScreen("session-design");
          }
        })
        .catch(() => setScreen("session-design"));
      return;
    }
    setScreen(target);
  }, [settingsReturnScreen, sessionId]);

  const routeAfterDigestOrLive = useCallback(async (sid: string) => {
    try {
      const [focus, snapshot] = await Promise.all([
        getSessionFocus(sid),
        getSessionSnapshot(),
      ]);
      setSessionPhoneCallMode(snapshot.phoneCallMode ?? false);
      if (!focus.focusConfirmedAt || focus.needsFocusRefresh) {
        setScreen("session-focus");
      } else {
        setScreen("mic-calibration");
      }
    } catch {
      setScreen("session-focus");
    }
  }, []);

  const routeAfterSessionFocus = useCallback(() => {
    setForceMicCalibrationRetest(false);
    setScreen("mic-calibration");
  }, []);

  const openMicCalibrationRetest = useCallback(() => {
    setForceMicCalibrationRetest(true);
    setScreen("mic-calibration");
  }, []);

  const queueImportToken = (token: string | null) => {
    if (!token) return;
    if (queuedTokenRef.current === token) return;
    queuedTokenRef.current = token;
    setPendingImportToken(token);
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        await win.unminimize();
        await win.show();
        await win.setFocus();
      } catch {
        // Not running inside the Tauri shell (e.g. vitest).
      }
    })();
  };

  const processSmartResumeImport = async (token: string) => {
    if (importInFlightRef.current === token) return;
    importInFlightRef.current = token;
    setImportLoading(true);
    setImportError(null);
    try {
      await abandonSessionDraft().catch(() => undefined);
      const result = await importFromSmartResume(token);
      persistCompanyIntel(result.companyIntel);
      setSessionId(null);
      setSessionPreFill({
        name: result.sessionName,
        sessionType: result.sessionType,
        domain: result.domain,
        smartResumeSessionId: result.smartResumeSessionId,
        // Map Smart Resume fields directly to structured context fields.
        contextFields: {
          jobDescription: result.jdText,
          profile: result.resumeSummary ?? "",
          companyOverview: buildCompanyOverviewText(result.companyIntel),
          leadershipPrinciples: "",
          roleExpectations: "",
          technicalPrep: "",
          strategyNotes: "",
        },
      });
      setScreen("session-design");
    } catch (err) {
      setImportError(String(err));
      queuedTokenRef.current = null;
    } finally {
      setImportLoading(false);
      setPendingImportToken(null);
      if (importInFlightRef.current === token) {
        importInFlightRef.current = null;
      }
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

        let pendingImport = queuedTokenRef.current;
        if (!pendingImport) {
          // Poll the token Rust stored before the WebView mounted (cold start).
          // getCurrent() from the deep-link plugin is unreliable on Linux with
          // a custom handler script, so we use the stored AppState value.
          const stored = await getPendingImportToken().catch(() => null);
          if (stored) {
            pendingImport = stored;
            queueImportToken(stored);
          }
        }

        if (pendingImport) {
          await abandonSessionDraft().catch(() => undefined);
        } else {
          await restoreDraftSession().catch(() => false);
        }

        const user = await getCurrentUser().catch(() => null);
        if (cancelled) return;

        if (user) {
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

  // If Rust still has an in-progress draft but the UI landed on session-design
  // (e.g. Settings → Back used the wrong screen), route to the matching screen.
  useEffect(() => {
    if (screen !== "session-design") return;
    let cancelled = false;
    void getSessionSnapshot()
      .then((snapshot) => {
        if (cancelled || !snapshot.sessionId) return;
        const draftScreen = screenForDraftState(snapshot.state);
        if (draftScreen !== "session-design") {
          setSessionId(snapshot.sessionId);
          setScreen(draftScreen);
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [screen]);

  const handleEditContextFromDigest = async () => {
    if (!sessionId) return;
    try {
      const snapshot = await returnToSessionDesign(sessionId);
      setSessionId(snapshot.sessionId);
      setSessionPreFill(preFillFromSnapshot(snapshot));
      setScreen("session-design");
    } catch (err) {
      setImportError(String(err));
    }
  };

  const handleReturnToSessionDesign = async () => {
    if (!sessionId) {
      setScreen("session-design");
      return;
    }
    try {
      const snapshot = await getSessionSnapshot().catch(() => null);
      if (snapshot?.state === SessionState.LIVE) {
        await stopSession().catch(() => undefined);
      }
      const updated = await returnToSessionDesign(sessionId);
      setSessionId(updated.sessionId);
      setSessionPreFill(preFillFromSnapshot(updated));
      setScreen("session-design");
    } catch (err) {
      const fallback = await getSessionSnapshot().catch(() => null);
      if (fallback?.sessionId) {
        setSessionId(fallback.sessionId);
        setSessionPreFill(preFillFromSnapshot(fallback));
      }
      setImportError(String(err));
      setScreen("session-design");
    }
  };

  // Nav items shown on setup/design screens.
  const designNav: NavItem[] = [
    {
      label: "New Session",
      onClick: () => {
        void (async () => {
          const snapshot = await getSessionSnapshot().catch(() => null);
          if (snapshot && snapshot.state !== SessionState.IDLE) {
            await abandonSessionDraft().catch(() => undefined);
          }
          setSessionId(null);
          setSessionPreFill(null);
          setScreen("session-design");
        })();
      },
      active: screen === "session-design",
    },
    {
      label: "Past Sessions",
      onClick: () => setScreen("session-list"),
      active: screen === "session-list",
    },
    {
      label: "Settings",
      onClick: () => {
        if (screen !== "settings") {
          openSettings(screen);
        }
      },
      active: screen === "settings",
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
          if (postSessionSummaryEnabled) {
            setScreen("session-summary");
          } else {
            setSessionId(null);
            setScreen("session-list");
          }
        }}
        onReturnToSetup={() => void handleReturnToSessionDesign()}
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
            void (async () => {
              const snapshot = await getSessionSnapshot();
              applyDraftSnapshot(snapshot, setSessionId, setSessionPreFill, setScreen);
              setRecoveryOffer(null);
            })();
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
        <HealthCheck
          onComplete={() => {
            void (async () => {
              const snapshot = await getSessionSnapshot();
              applyDraftSnapshot(snapshot, setSessionId, setSessionPreFill, setScreen);
            })();
          }}
        />
      </Shell>
    );
  }

  if (screen === "session-list") {
    return (
      <Shell nav={nav}>
        <SessionList
          onBack={() => setScreen("session-design")}
          activeSessionId={sessionId ?? undefined}
          onResumeSession={(resumeId, resumeState) => {
            if (resumeId === sessionId) {
              setScreen(screenForDraftState(resumeState));
            }
          }}
          onReopenSession={async (id) => {
            try {
              const snapshot = await reopenSession(id);
              applyDraftSnapshot(snapshot, setSessionId, setSessionPreFill, setScreen);
            } catch (err: unknown) {
              setImportError(String(err));
            }
          }}
          onStartSimilar={(preFill) => {
            void (async () => {
              const snapshot = await getSessionSnapshot().catch(() => null);
              if (snapshot && snapshot.state !== SessionState.IDLE) {
                await abandonSessionDraft().catch(() => undefined);
              }
              setSessionId(null);
              setSessionPreFill(preFill);
              setScreen("session-design");
            })();
          }}
          onReviewSession={(id) => {
            setReviewSessionId(id);
            setScreen("session-review");
          }}
        />
      </Shell>
    );
  }

  if (screen === "session-review" && reviewSessionId) {
    return (
      <Shell nav={nav}>
        <SessionReview
          sessionId={reviewSessionId}
          onBack={() => setScreen("session-list")}
        />
      </Shell>
    );
  }

  if (screen === "settings") {
    return (
      <Shell nav={nav}>
        <Settings
          sessionId={sessionId}
          initialTab={settingsInitialTab}
          onBack={handleSettingsBack}
          onRetestMic={openMicCalibrationRetest}
          onLoggedOut={() => {
            setOnboardingStep("auth");
            setScreen("onboarding");
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
          importLoading={importLoading}
          onImportFromSmartResume={(token) => void processSmartResumeImport(token)}
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
          onComplete={() => {
            setResetRehearsalPanels(true);
            void routeAfterDigestOrLive(sessionId);
          }}
          onOpenSettings={() => openSettings("digest-review", "api-keys")}
          onEditContext={() => void handleEditContextFromDigest()}
          onStartOver={() => {
            void abandonSessionDraft()
              .catch(() => undefined)
              .finally(() => {
                setSessionId(null);
                setScreen("session-design");
              });
          }}
        />
      </Shell>
    );
  }

  if (screen === "session-focus" && sessionId) {
    return (
      <Shell nav={nav}>
        <SessionFocusGate
          sessionId={sessionId}
          onComplete={routeAfterSessionFocus}
        />
      </Shell>
    );
  }

  if (screen === "mic-calibration") {
    return (
      <Shell nav={nav}>
        <MicCalibration
          forceRetest={forceMicCalibrationRetest}
          phoneCallMode={sessionPhoneCallMode}
          onComplete={() => {
            setForceMicCalibrationRetest(false);
            setScreen("rehearsal");
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
          resetPanelsOnEntry={resetRehearsalPanels}
          onResetPanelsHandled={() => setResetRehearsalPanels(false)}
          onComplete={() => setScreen("live")}
          onReturnToSetup={() => void handleReturnToSessionDesign()}
          onOpenSettings={() => openSettings("rehearsal", "session-focus")}
          onStartMock={() => setScreen("mock-interview")}
        />
      </Shell>
    );
  }

  if (screen === "mock-interview") {
    return (
      <Shell nav={nav}>
        <MockInterview
          sessionId={sessionId ?? ""}
          onComplete={() => setScreen("mock-summary")}
          onAbort={() => setScreen("rehearsal")}
        />
      </Shell>
    );
  }

  if (screen === "mock-summary") {
    return (
      <Shell nav={nav}>
        <MockSummary
          onContinue={() => {
            if (sessionId) {
              void routeAfterDigestOrLive(sessionId);
            } else {
              setScreen("rehearsal");
            }
          }}
        />
      </Shell>
    );
  }

  if (screen === "session-summary") {
    return (
      <Shell nav={nav}>
        <SessionSummary
          onDone={() => {
            setSessionId(null);
            setScreen("session-list");
          }}
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
