export interface MockSaveForLiveModalProps {
  open: boolean;
  previewText: string;
  saving: boolean;
  saved: boolean;
  error: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}

const MockSaveForLiveModal = ({
  open,
  previewText,
  saving,
  saved,
  error,
  onCancel,
  onConfirm,
}: MockSaveForLiveModalProps) => {
  if (!open) return null;

  return (
    <div
      data-testid="mock-save-for-live-modal"
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.65)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1000,
        padding: 16,
      }}
    >
      <div
        style={{
          background: "#0f1117",
          border: "1px solid #374151",
          borderRadius: 10,
          maxWidth: 520,
          width: "100%",
          padding: "18px 20px",
          display: "flex",
          flexDirection: "column",
          gap: 14,
        }}
      >
        <h3 style={{ margin: 0, fontSize: "15px", color: "#e2e8f0" }}>
          Save for Live
        </h3>
        <p style={{ margin: 0, fontSize: "12px", color: "#64748b", lineHeight: 1.5 }}>
          This answer will be served as your Live script for this question.
        </p>
        <div
          data-testid="mock-save-for-live-preview"
          style={{
            background: "#080a0f",
            border: "1px solid #1e2028",
            borderRadius: 8,
            padding: "10px 12px",
            fontSize: "13px",
            lineHeight: 1.6,
            color: "#cbd5e1",
            maxHeight: 180,
            overflowY: "auto",
            whiteSpace: "pre-wrap",
          }}
        >
          {previewText}
        </div>
        {error && (
          <p style={{ margin: 0, fontSize: "12px", color: "#fca5a5" }}>{error}</p>
        )}
        <div style={{ display: "flex", justifyContent: "flex-end", gap: 10 }}>
          <button
            type="button"
            data-testid="mock-save-for-live-cancel"
            onClick={onCancel}
            disabled={saving}
            style={{
              padding: "8px 14px",
              background: "none",
              border: "1px solid #374151",
              color: "#94a3b8",
              borderRadius: 6,
              fontSize: "13px",
              cursor: "pointer",
            }}
          >
            Cancel
          </button>
          <button
            type="button"
            data-testid="mock-save-for-live-confirm"
            onClick={onConfirm}
            disabled={saving || saved || !previewText.trim()}
            style={{
              padding: "8px 16px",
              background: saved ? "#16a34a" : "#7c3aed",
              color: "#fff",
              border: "none",
              borderRadius: 6,
              fontSize: "13px",
              fontWeight: 600,
              cursor: saving || saved ? "default" : "pointer",
              opacity: saving ? 0.6 : 1,
            }}
          >
            {saving ? "Saving…" : saved ? "Saved for Live" : "Save for Live"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default MockSaveForLiveModal;
