export interface LiveHelpDrawerProps {
  open: boolean;
  onClose: () => void;
}

const SECTIONS = [
  {
    title: "Ask now (Ctrl+Q)",
    body: "Marks the end of the interviewer's question. Flint sends the accumulated interviewer span to the Answer thread. Use it in phone mode when auto-detection is unreliable.",
  },
  {
    title: "Answer panel",
    body: "Streams a conclusion-first draft with brief reasoning and a follow-up line you can say aloud.",
  },
  {
    title: "Visual panel",
    body: "Appears for system-design or whiteboard questions. Shows Mermaid diagrams or highlighted code — use manual visual trigger if only the Answer panel fired.",
  },
  {
    title: "Context panel",
    body: "Shows RAG chunks from your prep and turn history from earlier questions in this session.",
  },
  {
    title: "Speaker swap",
    body: "Click Swap on any transcript line to relabel Interviewer vs You. User relabels always win over the classifier.",
  },
  {
    title: "Private mode hotkeys",
    body: "Ctrl+Alt+Space — answer now. Hold 2s — longer answer. Double-tap — cancel generation. Ctrl+Alt+Shift+Space — panic hide overlay.",
  },
] as const;

export default function LiveHelpDrawer({ open, onClose }: LiveHelpDrawerProps) {
  if (!open) return null;

  return (
    <div
      className="live-help-drawer__backdrop"
      data-testid="live-help-drawer-backdrop"
      onClick={onClose}
      role="presentation"
    >
      <aside
        className="live-help-drawer"
        data-testid="live-help-drawer"
        role="dialog"
        aria-modal="true"
        aria-label="Live session help"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="live-help-drawer__header">
          <h2>Live session guide</h2>
          <button
            type="button"
            data-testid="live-help-drawer-close"
            onClick={onClose}
            aria-label="Close help"
          >
            Close
          </button>
        </div>
        <div className="live-help-drawer__body">
          {SECTIONS.map((section) => (
            <section key={section.title} className="live-help-drawer__section">
              <h3>{section.title}</h3>
              <p>{section.body}</p>
            </section>
          ))}
        </div>
      </aside>
    </div>
  );
}
