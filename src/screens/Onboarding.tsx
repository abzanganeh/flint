import { FormEvent, useCallback, useEffect, useState } from "react";

import {
  cancelGoogleOAuth,
  login,
  setLegalConsentAccepted,
  setSessionState,
  signup,
  startGoogleOAuth,
} from "../commands";
import { onAuthOAuthComplete, onAuthOAuthError } from "../events";
import { SessionState } from "../types";
import "./Onboarding.css";

const LEGAL_DISCLAIMER = `Flint is an AI-powered advisory tool designed to help you prepare and perform better in conversations. By using Flint, you acknowledge and agree to the following:

All suggestions, responses, and guidance provided by Flint are AI-generated and carry a probability of error. Flint does not guarantee the accuracy, appropriateness, or completeness of any response.

You are solely responsible for the answers you give in any conversation, interview, examination, or meeting. You are solely responsible for the consequences of choosing to use any response Flint provides.

Flint is a supplementary preparation and support tool. It is not a replacement for your own knowledge, preparation, and judgment. You must be genuinely prepared for your meeting independently of Flint.

By proceeding, you accept full responsibility for your use of this tool.`;

export interface OnboardingProps {
  onComplete: () => void;
  initialStep: "legal" | "auth";
}

type OnboardingStep = "legal" | "auth";
type AuthMode = "signup" | "login";

/** Auto-cancel if the browser tab is closed and no deep link arrives. */
const OAUTH_WAIT_TIMEOUT_MS = 90 * 1000;

const Onboarding = ({ onComplete, initialStep }: OnboardingProps) => {
  const [step, setStep] = useState<OnboardingStep>(initialStep);
  const [authMode, setAuthMode] = useState<AuthMode>("signup");
  const [consentChecked, setConsentChecked] = useState(false);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [oauthPending, setOauthPending] = useState(false);
  const [oauthStarting, setOauthStarting] = useState(false);

  const clearOAuthWait = useCallback((message?: string) => {
    setOauthPending(false);
    setOauthStarting(false);
    if (message) {
      setError(message);
    }
  }, []);

  const finishAuth = useCallback(async () => {
    await setSessionState(SessionState.IDLE);
    onComplete();
  }, [onComplete]);

  useEffect(() => {
    let active = true;
    const unsubs: Array<Promise<() => void>> = [
      onAuthOAuthComplete(() => {
        if (!active) return;
        setOauthPending(false);
        setSubmitting(false);
        void finishAuth();
      }),
      onAuthOAuthError(({ message }) => {
        if (!active) return;
        setOauthPending(false);
        setSubmitting(false);
        setError(message);
      }),
    ];

    return () => {
      active = false;
      void Promise.all(unsubs).then((fns) => fns.forEach((fn) => fn()));
    };
  }, [finishAuth]);

  useEffect(() => {
    if (!oauthPending) return;
    const timeoutId = window.setTimeout(() => {
      void cancelGoogleOAuth().catch(() => {
        clearOAuthWait("Google sign-in timed out. Try again or use email below.");
      });
    }, OAUTH_WAIT_TIMEOUT_MS);
    return () => window.clearTimeout(timeoutId);
  }, [oauthPending, clearOAuthWait]);

  const handleAcceptConsent = async () => {
    setError(null);
    setSubmitting(true);
    try {
      await setLegalConsentAccepted();
      setStep("auth");
    } catch {
      setError("Could not save your acceptance. Please try again.");
    } finally {
      setSubmitting(false);
    }
  };

  const handleAuthSubmit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);

    if (authMode === "signup" && password !== confirmPassword) {
      setError("Passwords do not match.");
      return;
    }

    setSubmitting(true);
    try {
      if (authMode === "signup") {
        await signup(email.trim(), password);
        await login(email.trim(), password);
      } else {
        await login(email.trim(), password);
      }
      await finishAuth();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Authentication failed. Please try again.");
    } finally {
      setSubmitting(false);
    }
  };

  const handleGoogleSignIn = async () => {
    setError(null);
    setOauthStarting(true);
    try {
      await startGoogleOAuth();
      setOauthPending(true);
    } catch (err) {
      clearOAuthWait(
        err instanceof Error ? err.message : "Could not start Google sign-in. Please try again.",
      );
    } finally {
      setOauthStarting(false);
    }
  };

  const handleCancelGoogleOAuth = () => {
    void cancelGoogleOAuth().catch((err: unknown) => {
      clearOAuthWait(
        err instanceof Error ? err.message : "Could not cancel Google sign-in.",
      );
    });
  };

  const handleRetryGoogleOAuth = () => {
    void cancelGoogleOAuth()
      .catch(() => undefined)
      .finally(() => {
        clearOAuthWait();
        void handleGoogleSignIn();
      });
  };

  const resetAuthFormState = () => {
    if (oauthPending) {
      void cancelGoogleOAuth()
        .catch(() => undefined)
        .finally(() => clearOAuthWait());
      return;
    }
    setError(null);
    clearOAuthWait();
  };

  if (step === "legal") {
    return (
      <div className="onboarding" data-testid="onboarding-panel">
        <div className="onboarding-card" role="dialog" aria-modal="true" aria-labelledby="legal-title">
          <h1 id="legal-title" className="onboarding-title">
            Legal notice
          </h1>
          <p className="onboarding-subtitle">You must read and accept this before using Flint.</p>
          <div className="legal-text">
            {LEGAL_DISCLAIMER.split("\n\n").map((paragraph) => (
              <p key={paragraph.slice(0, 24)}>{paragraph}</p>
            ))}
          </div>
          <label className="consent-label">
            <input
              type="checkbox"
              checked={consentChecked}
              onChange={(e) => setConsentChecked(e.target.checked)}
              data-testid="legal-consent-checkbox"
            />
            <span>I understand and accept full responsibility</span>
          </label>
          {error ? (
            <p className="onboarding-error" role="alert">
              {error}
            </p>
          ) : null}
          <div className="onboarding-actions">
            <button
              type="button"
              className="btn-primary"
              disabled={!consentChecked || submitting}
              onClick={() => void handleAcceptConsent()}
              data-testid="legal-consent-continue"
            >
              Continue
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="onboarding" data-testid="onboarding-panel">
      <div className="onboarding-card">
        <h1 className="onboarding-title">Welcome to Flint</h1>
        <p className="onboarding-subtitle">Create an account or sign in to continue.</p>

        <button
          type="button"
          className="btn-google"
          disabled={oauthStarting || oauthPending}
          onClick={() => void handleGoogleSignIn()}
          data-testid="google-sign-in-button"
        >
          {oauthPending ? "Waiting for Google…" : "Continue with Google"}
        </button>
        {oauthPending ? (
          <>
            <p className="oauth-wait-hint" data-testid="google-oauth-hint">
              After you pick an account, choose <strong>Open Flint</strong> in the system dialog.
              If you cancel that dialog, click <strong>Cancel sign-in</strong> below, close the
              browser tab, then try again.
            </p>
            <div className="oauth-wait-actions">
              <button
                type="button"
                className="btn-oauth-cancel"
                onClick={handleCancelGoogleOAuth}
                data-testid="google-sign-in-cancel"
              >
                Cancel sign-in
              </button>
              <button
                type="button"
                className="btn-oauth-retry"
                onClick={() => void handleRetryGoogleOAuth()}
                data-testid="google-sign-in-retry"
              >
                Try a different account
              </button>
            </div>
          </>
        ) : null}

        <div className="auth-divider" aria-hidden="true">
          <span>or</span>
        </div>

        <div className="auth-tabs" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={authMode === "signup"}
            className={`auth-tab ${authMode === "signup" ? "active" : ""}`}
            onClick={() => {
              setAuthMode("signup");
              resetAuthFormState();
            }}
          >
            Sign up
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={authMode === "login"}
            className={`auth-tab ${authMode === "login" ? "active" : ""}`}
            onClick={() => {
              setAuthMode("login");
              resetAuthFormState();
            }}
          >
            Log in
          </button>
        </div>

        <form className="auth-form" onSubmit={(e) => void handleAuthSubmit(e)} noValidate>
          <div className="field">
            <label htmlFor="onboarding-email">Email</label>
            <input
              id="onboarding-email"
              type="email"
              autoComplete="email"
              required
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              disabled={submitting || oauthStarting}
            />
          </div>
          <div className="field">
            <label htmlFor="onboarding-password">Password</label>
            <input
              id="onboarding-password"
              type="password"
              autoComplete={authMode === "signup" ? "new-password" : "current-password"}
              required
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              disabled={submitting || oauthStarting}
            />
          </div>
          {authMode === "signup" ? (
            <div className="field">
              <label htmlFor="onboarding-confirm-password">Confirm password</label>
              <input
                id="onboarding-confirm-password"
                type="password"
                autoComplete="new-password"
                required
                value={confirmPassword}
                onChange={(e) => setConfirmPassword(e.target.value)}
                disabled={submitting || oauthStarting}
              />
            </div>
          ) : null}
          {error ? (
            <p className="onboarding-error" role="alert">
              {error}
            </p>
          ) : null}
          <button type="submit" className="btn-primary" disabled={submitting || oauthStarting}>
            {submitting && !oauthPending
              ? "Please wait…"
              : authMode === "signup"
                ? "Create account"
                : "Log in"}
          </button>
        </form>
      </div>
    </div>
  );
};

export default Onboarding;
