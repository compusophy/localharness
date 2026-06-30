# CREDENTIALS.template.md — what the human owner must provide

> The exact credential checklist for the perpetual marketing agent. **This file is a
> TEMPLATE — it contains placeholders only, never real secrets.** It is safe to commit.
> Real values go in a gitignored secrets file (see §Delivery), which is NEVER committed.
>
> Principle: hand the agent **scoped API tokens**, not account passwords, wherever the
> platform allows it. Passwords appear below only where a platform's API genuinely has no
> token-only path (e.g. Reddit "script" apps), and even there a refresh token is preferred.

---

## 0. The human-only one-time setup (the agent cannot do these)

For every platform: **a human creates the account** (phone/SMS + CAPTCHA verified),
**reserves the handle `@localharness`** (keep it identical everywhere), and **creates the
developer app / requests API access**. Signup automation is impossible *and* against ToS —
see GROWTH.md §0. Budget for the slow approvals up front:

- Instagram `instagram_business_content_publish` app review: **2–4 weeks**.
- TikTok Content Posting audit (to leave SELF_ONLY/private): **days–weeks**.
- LinkedIn Community Management API access form: **manual review**.
- X: add a **payment method** (pay-per-use billing is now default for new dev accounts).

---

## 1. Shared / cross-platform

| Item | Provide as | What it unlocks |
|---|---|---|
| **One dedicated marketing email** | `growth@localharness.xyz` (or a Gmail) | Single inbox to register every account + dev app + receive verification. Human holds the password for recovery; the agent does **not** need it. |
| **Reserved handle** | `@localharness` everywhere | Consistent brand + AI-discoverability. Reserve even on channels you won't use yet. |
| **Web analytics read token** *(optional)* | `ANALYTICS_API_KEY` | Pull referral + conversion KPIs (AI-referrers, UTM attribution). Skip if you read server logs directly. |
| **LLM keys for citation monitoring** *(optional but recommended)* | `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `PERPLEXITY_API_KEY` | Run the weekly GEO citation panel (Experiment 1). Can reuse the platform's existing model access. |

---

## 2. Per-platform credential checklist

### GitHub — Tier 1
| Credential | What the agent DOES with it |
|---|---|
| `GITHUB_TOKEN` (fine-scoped **PAT** or **GitHub App** installation token; scopes: `repo` metadata + `contents` for release notes, read for traffic/insights) | Maintain repo topics/description/social-preview, draft+cut release notes, pull stars/clones/traffic for KPIs. **Least privilege — do NOT hand it `admin` or org-wide write.** |
| *(no auth)* crates.io public stats | Read download counts via `https://crates.io/api/v1/crates/localharness` — no token needed. |

### X / Twitter — Tier 2
| Credential | What the agent DOES with it |
|---|---|
| `X_API_KEY`, `X_API_SECRET` | App (consumer) identity. |
| `X_ACCESS_TOKEN`, `X_ACCESS_TOKEN_SECRET` (OAuth 1.0a **user-context**, scoped to `@localharness` with write) | Post own tweets/threads to the own account; nothing else. |
| `X_BEARER_TOKEN` (app-only) | Read own-post public metrics for KPIs. |
| *(human action)* payment method on the dev account | Required — posting is billed per call (~$0.015/post; $0.20 if it contains a link). |

> The agent uses these ONLY to post own content + read own analytics. It is configured to
> never call follow/like/DM endpoints (prohibited).

### dev.to (Forem) — Tier 2
| Credential | What the agent DOES with it |
|---|---|
| `DEVTO_API_KEY` (Settings → Extensions → API Key) | `POST /api/articles` to publish own technical posts (with `canonical_url` back to the site); pull views/reactions for KPIs. |

### Reddit — Tier 2
| Credential | What the agent DOES with it |
|---|---|
| `REDDIT_CLIENT_ID`, `REDDIT_CLIENT_SECRET` ("script"-type OAuth app) | App identity. |
| `REDDIT_REFRESH_TOKEN` *(preferred)* **or** `REDDIT_USERNAME` + `REDDIT_PASSWORD` | Authenticate to read own-post metrics and (human-approved only) submit. Prefer the refresh token so the raw password never enters the env. |
| `REDDIT_USER_AGENT` (e.g. `localharness-growth/0.1 by u/localharness`) | Required by Reddit API ToS — descriptive UA string. |

> Submissions are **human-approved** (9:1 rule, karma/age gates). The agent drafts; a human
> posts from the aged account.

### LinkedIn — Tier 3
| Credential | What the agent DOES with it |
|---|---|
| `LINKEDIN_CLIENT_ID`, `LINKEDIN_CLIENT_SECRET` | App identity. |
| `LINKEDIN_ACCESS_TOKEN` (scope `w_member_social`) and/or `LINKEDIN_ORG_ID` + `w_organization_social` token | Post own milestones (member or Page); pull post analytics. **~60-day expiry → needs refresh.** |
| `LINKEDIN_REFRESH_TOKEN` | Refresh the access token before expiry. |

### Instagram — Tier 3
| Credential | What the agent DOES with it |
|---|---|
| `META_APP_ID`, `META_APP_SECRET` | Meta app identity. |
| `IG_USER_ID` (the IG Professional account id) | Target for publishing. |
| `IG_LONG_LIVED_ACCESS_TOKEN` (60-day, refreshable; needs `instagram_business_content_publish` approved) | Create media container → publish own demo reels/images; pull insights. |

### TikTok — Tier 3
| Credential | What the agent DOES with it |
|---|---|
| `TIKTOK_CLIENT_KEY`, `TIKTOK_CLIENT_SECRET` | App identity. |
| `TIKTOK_ACCESS_TOKEN`, `TIKTOK_REFRESH_TOKEN`, `TIKTOK_OPEN_ID` | Direct-post own demo videos via Content Posting API (**SELF_ONLY/private until the client is audited**); pull video metrics. |

### AI-discoverability (llms.txt / GEO) — Tier 1
No new platform credential. Uses `GITHUB_TOKEN` (repo content) + the optional LLM keys in
§1 (citation panel). The agent edits `web/llms.txt` / `web/skill.md` / site content via the
repo and respects the GEN-block SOP (`src/docs_manifest.rs` is the source of truth).

---

## 3. Secure delivery — how the owner hands these over

**Never commit secrets.** Two acceptable homes, pick one:

- **Preferred — outside the repo entirely:** `~/.lh_marketing_secrets`
  (cannot be committed because it isn't in the tree). The agent loads it explicitly.
- **In-repo alternative:** `.env.marketing` at the repo root. This is **already covered by
  the existing `.gitignore` rule `.env.*` (line 14)** — git will not track it. For
  belt-and-suspenders clarity you may add an explicit line:

```gitignore
# Marketing agent secrets — NEVER commit
.env.marketing
.lh_marketing_secrets
```

> Note: the repo's `.gitignore` un-ignores **only** `.env.example` (`!.env.example`). A
> hypothetical `.env.marketing.example` would still be ignored by `.env.*` — so keep the
> committed template as THIS markdown file in `design/`, not as a dotfile.

### `.env.marketing` template (placeholders ONLY — copy, fill privately, never commit)

```bash
# ---- shared ----
MARKETING_EMAIL=growth@localharness.xyz
ANALYTICS_API_KEY=__optional__
ANTHROPIC_API_KEY=__optional_for_citation_panel__
OPENAI_API_KEY=__optional_for_citation_panel__
PERPLEXITY_API_KEY=__optional_for_citation_panel__

# ---- github ----
GITHUB_TOKEN=__fine_scoped_pat_or_app_token__

# ---- x / twitter ----
X_API_KEY=__app_consumer_key__
X_API_SECRET=__app_consumer_secret__
X_ACCESS_TOKEN=__user_context_token__
X_ACCESS_TOKEN_SECRET=__user_context_secret__
X_BEARER_TOKEN=__app_only_bearer__

# ---- dev.to ----
DEVTO_API_KEY=__forem_api_key__

# ---- reddit ----
REDDIT_CLIENT_ID=__script_app_id__
REDDIT_CLIENT_SECRET=__script_app_secret__
REDDIT_REFRESH_TOKEN=__preferred__
REDDIT_USERNAME=__only_if_no_refresh_token__
REDDIT_PASSWORD=__only_if_no_refresh_token__
REDDIT_USER_AGENT=localharness-growth/0.1 by u/localharness

# ---- linkedin (tier 3) ----
LINKEDIN_CLIENT_ID=__app_id__
LINKEDIN_CLIENT_SECRET=__app_secret__
LINKEDIN_ACCESS_TOKEN=__w_member_social_token__
LINKEDIN_REFRESH_TOKEN=__refresh__
LINKEDIN_ORG_ID=__optional_page_id__

# ---- instagram (tier 3) ----
META_APP_ID=__app_id__
META_APP_SECRET=__app_secret__
IG_USER_ID=__ig_professional_account_id__
IG_LONG_LIVED_ACCESS_TOKEN=__60_day_token__

# ---- tiktok (tier 3) ----
TIKTOK_CLIENT_KEY=__client_key__
TIKTOK_CLIENT_SECRET=__client_secret__
TIKTOK_ACCESS_TOKEN=__access_token__
TIKTOK_REFRESH_TOKEN=__refresh_token__
TIKTOK_OPEN_ID=__open_id__
```

### Handling rules for the agent
- Load secrets from the gitignored file at startup; **never echo, log, or paste a secret
  value** into any post, commit, issue, or transcript.
- Treat every token as **least-privilege + revocable**: request the narrowest scope that
  works; the owner can revoke any token without touching the others.
- **Rotate** on a schedule and immediately on any suspected leak. Refresh 60-day tokens
  (LinkedIn, Instagram) before expiry.
- Tier 3 lines may stay as placeholders until those channels are activated — the agent
  simply treats a missing/placeholder credential as "channel disabled".
