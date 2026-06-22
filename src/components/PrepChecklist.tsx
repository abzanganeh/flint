import { useState } from "react";

import { updateSessionVocabulary, type SessionContextFields } from "../commands";

interface FieldSpec {
  key: keyof SessionContextFields;
  label: string;
  searchGuide: string;
  required: boolean;
  /** Editable inline from Rehearsal without returning to Session Design. */
  inlineEditable?: boolean;
}

const FIELD_SPECS: FieldSpec[] = [
  {
    key: "jobDescription",
    label: "Job Description",
    searchGuide: 'Search "[company] [role] job description site:linkedin.com OR greenhouse.io"',
    required: true,
  },
  {
    key: "profile",
    label: "Your Profile",
    searchGuide: "Paste your resume or LinkedIn summary",
    required: true,
  },
  {
    key: "companyOverview",
    label: "Company Overview",
    searchGuide: 'Search "[company] about mission values culture 2025"',
    required: false,
  },
  {
    key: "leadershipPrinciples",
    label: "Leadership Principles",
    searchGuide: 'Search "[company] leadership principles values"',
    required: false,
  },
  {
    key: "roleExpectations",
    label: "Role Expectations",
    searchGuide: 'Search "[company] [role] expectations responsibilities"',
    required: false,
  },
  {
    key: "technicalPrep",
    label: "Technical Prep",
    searchGuide: 'Search "[role] technical interview topics [company]"',
    required: false,
  },
  {
    key: "strategyNotes",
    label: "Strategy Notes",
    searchGuide: "Add your personal talking points, stories, or preparation notes",
    required: false,
  },
  {
    key: "speakingStyle",
    label: "Speaking Style",
    searchGuide: "Choose natural vs polished voice for mock coach feedback",
    required: false,
  },
  {
    key: "sessionVocabulary",
    label: "Session Vocabulary",
    searchGuide: "Add domain acronyms (RBAC, OIDC) to improve speech recognition",
    required: false,
    inlineEditable: true,
  },
];

interface PrepChecklistProps {
  fields: SessionContextFields;
  sessionId: string;
  onFieldsUpdated?: () => void;
  onOpenSessionDesign?: () => void;
}

export default function PrepChecklist({
  fields,
  sessionId,
  onFieldsUpdated,
  onOpenSessionDesign,
}: PrepChecklistProps) {
  const [editingVocabulary, setEditingVocabulary] = useState(false);
  const [vocabularyDraft, setVocabularyDraft] = useState("");
  const [savingVocabulary, setSavingVocabulary] = useState(false);
  const [vocabularyError, setVocabularyError] = useState<string | null>(null);

  const filled = FIELD_SPECS.filter((f) => fields[f.key]?.trim().length > 0);
  const total = FIELD_SPECS.length;

  const openVocabularyEditor = () => {
    setVocabularyDraft(fields.sessionVocabulary ?? "");
    setVocabularyError(null);
    setEditingVocabulary(true);
  };

  const cancelVocabularyEdit = () => {
    setEditingVocabulary(false);
    setVocabularyError(null);
  };

  const saveVocabulary = async () => {
    setSavingVocabulary(true);
    setVocabularyError(null);
    try {
      await updateSessionVocabulary(sessionId, vocabularyDraft);
      setEditingVocabulary(false);
      onFieldsUpdated?.();
    } catch (err) {
      setVocabularyError(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingVocabulary(false);
    }
  };

  return (
    <div className="prep-checklist">
      <div className="prep-checklist__header">
        <span className="prep-checklist__title">Prep Checklist</span>
        <span className="prep-checklist__score">
          {filled.length}/{total}
        </span>
      </div>

      <ul className="prep-checklist__list">
        {FIELD_SPECS.map((spec) => {
          const isFilled = fields[spec.key]?.trim().length > 0;
          const isVocabulary = spec.key === "sessionVocabulary";
          const showInlineEditor = isVocabulary && editingVocabulary;

          return (
            <li
              key={spec.key}
              className={`prep-checklist__item ${isFilled ? "prep-checklist__item--filled" : "prep-checklist__item--empty"}${!isFilled && spec.inlineEditable ? " prep-checklist__item--actionable" : ""}`}
            >
              <span className="prep-checklist__status" aria-hidden>
                {isFilled ? "●" : "○"}
              </span>
              <div className="prep-checklist__field">
                <div className="prep-checklist__field-row">
                  <span className="prep-checklist__field-label">
                    {spec.label}
                    {spec.required && <span className="prep-checklist__required"> *</span>}
                  </span>
                  {isVocabulary && !showInlineEditor && (
                    <button
                      type="button"
                      className="prep-checklist__edit-btn"
                      onClick={openVocabularyEditor}
                    >
                      {isFilled ? "Edit" : "Add"}
                    </button>
                  )}
                  {!isFilled && !spec.inlineEditable && onOpenSessionDesign && (
                    <button
                      type="button"
                      className="prep-checklist__edit-btn"
                      onClick={onOpenSessionDesign}
                    >
                      Edit in setup
                    </button>
                  )}
                </div>

                {!isFilled && !showInlineEditor && (
                  <p className="prep-checklist__guide">{spec.searchGuide}</p>
                )}

                {isFilled && isVocabulary && !showInlineEditor && (
                  <p className="prep-checklist__preview">{fields.sessionVocabulary}</p>
                )}

                {showInlineEditor && (
                  <div className="prep-checklist__inline-editor">
                    <textarea
                      className="prep-checklist__textarea"
                      rows={3}
                      value={vocabularyDraft}
                      onChange={(e) => setVocabularyDraft(e.target.value)}
                      placeholder="RBAC, ABAC, OIDC, SCIM, entitlement review, Fisher"
                      disabled={savingVocabulary}
                      autoFocus
                    />
                    {vocabularyError && (
                      <p className="prep-checklist__error">{vocabularyError}</p>
                    )}
                    <div className="prep-checklist__inline-actions">
                      <button
                        type="button"
                        className="prep-checklist__save-btn"
                        onClick={() => void saveVocabulary()}
                        disabled={savingVocabulary}
                      >
                        {savingVocabulary ? "Saving…" : "Save"}
                      </button>
                      <button
                        type="button"
                        className="prep-checklist__cancel-btn"
                        onClick={cancelVocabularyEdit}
                        disabled={savingVocabulary}
                      >
                        Cancel
                      </button>
                    </div>
                  </div>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
