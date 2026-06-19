#!/usr/bin/env python3
"""Seed Supabase global_question_bank from Flint evals question packs."""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

CANONICAL_BY_CATEGORY: dict[str, str] = {
    "introduction": "Open with current role, 2–3 relevant outcomes, and why this role fits your trajectory.",
    "strengths": "One strength plus a concrete STAR example with measurable impact tied to the JD.",
    "weaknesses": "A real growth area, improvement actions taken, and how you manage it under pressure.",
    "star_story": "STAR format: stakes, your actions, trade-offs, measurable outcome, one lesson.",
    "behavioural": "Situation, approach, collaboration choices, and a clear professional outcome.",
    "technical": "Define terms, explain trade-offs, cite experience, note when alternatives apply.",
    "system_design": "Requirements, components, data flow, bottlenecks, failure modes, trade-offs.",
    "general": "Prepare a personal answer grounded in your resume and this role's context.",
}


def canonical_for(category: str) -> str:
    return CANONICAL_BY_CATEGORY.get(category.lower(), CANONICAL_BY_CATEGORY["general"])


def load_rows(evals_dir: Path) -> list[dict]:
    rows: list[dict] = []
    for path in sorted(evals_dir.glob("*.json")):
        payload = json.loads(path.read_text(encoding="utf-8"))
        domain = str(payload.get("domain") or path.stem)
        for item in payload.get("questions", []):
            text = str(item.get("text") or "").strip()
            if not text:
                continue
            category = str(item.get("category") or "general")
            rows.append(
                {
                    "question_text": text,
                    "domain": domain,
                    "subdomain": category,
                    "difficulty": "mid",
                    "canonical_answer": item.get("canonical_answer") or canonical_for(category),
                    "source": "flint_curated",
                    "quality_score": 8.5,
                    "review_status": "auto_approved",
                }
            )
    return rows


def post_rows(base_url: str, anon_key: str, rows: list[dict]) -> None:
    url = f"{base_url.rstrip('/')}/rest/v1/global_question_bank"
    body = json.dumps(rows).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={
            "apikey": anon_key,
            "Authorization": f"Bearer {anon_key}",
            "Content-Type": "application/json",
            "Prefer": "resolution=ignore-duplicates",
        },
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        print(f"insert status: {resp.status}")


def main() -> int:
    base = os.environ.get("FLINT_SUPABASE_URL", "http://127.0.0.1:54321")
    key = os.environ.get("FLINT_SUPABASE_ANON_KEY")
    if not key:
        print("Set FLINT_SUPABASE_ANON_KEY", file=sys.stderr)
        return 1

    evals_dir = Path(__file__).resolve().parents[1] / "evals" / "questions"
    rows = load_rows(evals_dir)
    print(f"seeding {len(rows)} rows")
    try:
        post_rows(base, key, rows)
    except urllib.error.HTTPError as exc:
        print(f"seed failed: {exc.read().decode()}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
