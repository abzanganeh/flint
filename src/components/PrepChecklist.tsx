import type { SessionContextFields } from "../commands";

interface FieldSpec {
  key: keyof SessionContextFields;
  label: string;
  searchGuide: string;
  required: boolean;
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
];

interface PrepChecklistProps {
  fields: SessionContextFields;
  onFieldClick?: (fieldKey: keyof SessionContextFields) => void;
}

export default function PrepChecklist({ fields, onFieldClick }: PrepChecklistProps) {
  const filled = FIELD_SPECS.filter((f) => fields[f.key]?.trim().length > 0);
  const total = FIELD_SPECS.length;

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
          return (
            <li
              key={spec.key}
              className={`prep-checklist__item ${isFilled ? "prep-checklist__item--filled" : "prep-checklist__item--empty"}`}
            >
              <span className="prep-checklist__status" aria-hidden>
                {isFilled ? "●" : "○"}
              </span>
              <div className="prep-checklist__field">
                <button
                  className="prep-checklist__field-label"
                  onClick={() => onFieldClick?.(spec.key)}
                  disabled={!onFieldClick}
                >
                  {spec.label}
                  {spec.required && <span className="prep-checklist__required"> *</span>}
                </button>
                {!isFilled && (
                  <p className="prep-checklist__guide">{spec.searchGuide}</p>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
