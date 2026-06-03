create extension if not exists pgcrypto;

create table if not exists public.users (
    id uuid primary key default gen_random_uuid(),
    email text not null unique,
    plan text not null default 'basic',
    created_at timestamptz not null default now()
);

create table if not exists public.profiles (
    id uuid primary key default gen_random_uuid(),
    user_id uuid not null unique references public.users(id) on delete cascade,
    name text,
    role text,
    industry text,
    domain text,
    style_prefs jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now()
);

create table if not exists public.sessions (
    id uuid primary key default gen_random_uuid(),
    user_id uuid not null references public.users(id) on delete cascade,
    name text not null,
    type text not null,
    domain text not null,
    status text,
    expires_at timestamptz not null default now() + interval '30 days',
    promoted boolean not null default false,
    created_at timestamptz not null default now()
);

create table if not exists public.transcripts (
    id uuid primary key default gen_random_uuid(),
    session_id uuid not null references public.sessions(id) on delete cascade,
    speaker text not null,
    content text not null,
    "timestamp" timestamptz not null,
    created_at timestamptz not null default now()
);

create table if not exists public.responses (
    id uuid primary key default gen_random_uuid(),
    session_id uuid not null references public.sessions(id) on delete cascade,
    type text not null,
    content text not null,
    confidence text,
    created_at timestamptz not null default now()
);

create table if not exists public.session_insights (
    id uuid primary key default gen_random_uuid(),
    session_id uuid not null references public.sessions(id) on delete cascade,
    usage_breakdown jsonb not null default '{}'::jsonb,
    low_confidence_topics jsonb not null default '[]'::jsonb,
    gaps jsonb not null default '[]'::jsonb,
    created_at timestamptz not null default now()
);

create table if not exists public.credentials (
    id uuid primary key default gen_random_uuid(),
    user_id uuid not null references public.users(id) on delete cascade,
    provider text not null,
    encrypted_key text not null,
    iv text not null,
    created_at timestamptz not null default now()
);

create table if not exists public.templates (
    id uuid primary key default gen_random_uuid(),
    user_id uuid not null references public.users(id) on delete cascade,
    name text not null,
    type text not null,
    domain text not null,
    config jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now()
);

alter table public.users enable row level security;
alter table public.profiles enable row level security;
alter table public.sessions enable row level security;
alter table public.transcripts enable row level security;
alter table public.responses enable row level security;
alter table public.session_insights enable row level security;
alter table public.credentials enable row level security;
alter table public.templates enable row level security;

create policy "users_select_own"
    on public.users
    for select
    using (auth.uid() = id);

create policy "profiles_select_own"
    on public.profiles
    for select
    using (auth.uid() = user_id);

create policy "sessions_select_own"
    on public.sessions
    for select
    using (auth.uid() = user_id);

create policy "transcripts_select_for_own_sessions"
    on public.transcripts
    for select
    using (
        auth.uid() = (
            select s.user_id
            from public.sessions s
            where s.id = transcripts.session_id
        )
    );

create policy "responses_select_for_own_sessions"
    on public.responses
    for select
    using (
        auth.uid() = (
            select s.user_id
            from public.sessions s
            where s.id = responses.session_id
        )
    );

create policy "session_insights_select_for_own_sessions"
    on public.session_insights
    for select
    using (
        auth.uid() = (
            select s.user_id
            from public.sessions s
            where s.id = session_insights.session_id
        )
    );

create policy "credentials_select_own"
    on public.credentials
    for select
    using (auth.uid() = user_id);

create policy "templates_select_own"
    on public.templates
    for select
    using (auth.uid() = user_id);
