-- Phase 10: global interview question bank (read-only seed, pgvector).
-- Design: docs/QA_RETRIEVAL_AND_QUESTION_BANK.md §6.2

create extension if not exists vector;

create table if not exists public.global_question_bank (
    id uuid primary key default gen_random_uuid(),
    question_text text not null,
    question_embedding vector(384),
    domain text not null default 'universal',
    subdomain text,
    difficulty text,
    canonical_answer text,
    answer_variants jsonb not null default '[]'::jsonb,
    usage_count integer not null default 0,
    upvote_count integer not null default 0,
    last_enriched_at timestamptz,
    source text not null default 'flint_curated',
    quality_score double precision,
    review_status text not null default 'auto_approved',
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    constraint global_question_bank_question_text_key unique (question_text)
);

create index if not exists global_question_bank_domain_idx
    on public.global_question_bank (domain);

create index if not exists global_question_bank_review_status_idx
    on public.global_question_bank (review_status);

alter table public.global_question_bank enable row level security;

create policy "global_question_bank_select_authenticated"
    on public.global_question_bank
    for select
    to authenticated
    using (review_status in ('auto_approved', 'pending_review'));

create policy "global_question_bank_select_anon_read_seed"
    on public.global_question_bank
    for select
    to anon
    using (review_status = 'auto_approved');

insert into public.global_question_bank (
    question_text,
    domain,
    subdomain,
    difficulty,
    canonical_answer,
    source,
    quality_score,
    review_status
) values
(
    'Tell me about yourself.',
    'universal',
    'introduction',
    'mid',
    'Open with your current role and scope, highlight 2–3 relevant accomplishments tied to this job, then connect your trajectory to why this team and role are the right next step.',
    'flint_curated',
    9.0,
    'auto_approved'
),
(
    'What are your greatest strengths?',
    'universal',
    'strengths',
    'mid',
    'Pick one strength with a concrete example: context, action, measurable outcome, and why it matters for the role you are interviewing for.',
    'flint_curated',
    8.8,
    'auto_approved'
),
(
    'Why are you interested in this role?',
    'universal',
    'introduction',
    'mid',
    'Reference specific responsibilities from the JD, tie them to evidence from your background, and explain what you want to learn or deliver in the first 6–12 months.',
    'flint_curated',
    8.9,
    'auto_approved'
),
(
    'Tell me about a significant challenge you faced.',
    'universal',
    'star_story',
    'mid',
    'Use STAR: situation with stakes, your specific actions, trade-offs considered, and a quantified or qualitative result plus what you would do differently.',
    'flint_curated',
    9.1,
    'auto_approved'
),
(
    'Why should we hire you?',
    'universal',
    'strengths',
    'mid',
    'Summarize the intersection of role requirements and your proven outcomes, name one risk you reduce for the team, and close with enthusiasm grounded in company context.',
    'flint_curated',
    8.7,
    'auto_approved'
)
on conflict (question_text) do nothing;
