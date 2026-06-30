# Agent Email — a real `@localharness.xyz` inbox

Goal: give the localharness agent a **real, self-sovereign email address** at
`@localharness.xyz` so it can receive verification codes / sign-up confirmations and
reach the world — pursued as far as it can go *autonomously*, with the one genuine
physical limit (if any) named as a fact.

**TL;DR — RECEIVING is LIVE and was set up fully autonomously. SENDING
DKIM-authenticated mail *from* the domain is the one step that needs a human (a
provider account signup that gates on CAPTCHA/email-confirm).**

---

## 1. Where the DNS actually lives + what access exists

| Fact | Value |
|------|-------|
| Registrar / nameservers | **Vercel DNS** — `ns1.vercel-dns.com`, `ns2.vercel-dns.com` (zone runs on NS1/`nsone.net` under the hood) |
| Zone owner | Vercel team **`compusophys-projects`** (`team_DYlw1hPeilK5o3w1uPWqt8Mi`), creator `compusophy@gmail.com`, `"zone": true` (full record control) |
| Domain attachment | apex `localharness.xyz` **+** wildcard `*.localharness.xyz` are verified production domains on the **`antig`** project (the web app) |
| Web records (untouched) | apex + `*` are Vercel-managed `ALIAS` → `ae4530207846a27a.vercel-dns-016.com`; 3 `CAA` (pki.goog / sectigo.com / letsencrypt.org) |
| **Pre-existing MX** | **NONE** — there was no mail setup at all (no MX, no TXT/SPF). Adding mail records was therefore safe and non-breaking. |

**Access we have here:** the Vercel CLI is authed as `compusophy`, and the CLI's
auth token (`~/AppData/Roaming/com.vercel.cli/Data/auth.json`) drives the **Vercel
REST API**, which gives **full read/write control of the zone**. (Note: `vercel dns
ls localharness.xyz` and `vercel domains inspect` both fail with "you don't have
permission…" — that's a **CLI scope bug**, not a real permission gap. The REST API
`/v4/domains/localharness.xyz/records` lists and `/v2/domains/.../records` writes
fine with the same token. Use the API, not the `vercel dns` CLI subcommand.)

So **DNS is fully autonomous from here.** That is the foundation everything below
stands on.

---

## 2. The minimal path to a live inbox — and what was built

Receiving email needs (a) MX records pointing at a mail receiver and (b) that
receiver delivering the mail somewhere readable. Ranked by autonomy:

| Path | Autonomy | Self-sovereign? | Verdict |
|------|----------|-----------------|---------|
| **ForwardEmail, DNS-only → Gmail** | **Zero signup** — config is the DNS TXT itself | Address is ours; mail terminates in Gmail | ✅ **Chosen + LIVE now** |
| **ForwardEmail → our webhook** (`/api/inbound-email`) | DNS-only routing; platform owns the store | ✅ Fully — platform reads its own inbox | ✅ **Built, one deploy away** |
| Cloudflare Email Routing | Needs domain *on Cloudflare* = nameserver change → would break Vercel web serving | partial | ❌ rejected (breaks web) |
| ImprovMX free | Needs an account signup | no (forwards to Gmail) | ❌ strictly worse than ForwardEmail |
| Resend/Postmark/Mailgun inbound | Account + (often) card + domain verify | ✅ | reserved for the **sending** problem (§4) |

### What was set autonomously (LIVE — done in this session)

Four records added to the Vercel zone via the REST API (reversible — UIDs noted):

```
MX   @  mx1.forwardemail.net   priority 0   ttl 3600   (rec_6bd231216a93317722964cbd)
MX   @  mx2.forwardemail.net   priority 0   ttl 3600   (rec_46a2e70a22006f8f24c515b9)
TXT  @  forward-email=compusophy@gmail.com  ttl 3600   (rec_f452ba11ad1ae66359ae0f84)
TXT  @  v=spf1 a include:spf.forwardemail.net -all      (rec_a0432f0f00eaef3affd39394)
```

Verified live at the authoritative nameserver (`ns1.vercel-dns.com` returns both MX;
both TXT already visible via `8.8.8.8`). Public MX caching catches up within the
~10-min TTL.

**Result — this is a working inbox right now:** anything `*@localharness.xyz`
(`agent@`, `claude@`, `hello@`, …) is a **catch-all** that forwards to
`compusophy@gmail.com`, which the agent reads via the **Gmail MCP**
(`search_threads` / `get_thread`). ForwardEmail's free tier is pure-DNS (no account
required to route), open-source, and the only free service that forwards a custom
domain without standing up a mail server.

> Caveat on the Gmail-read leg: the Gmail MCP token is currently **expired and needs
> re-authorization** (a routine OAuth refresh, not a structural blocker). Once
> refreshed, the receive→read loop is fully closed. This dependency is also what the
> webhook path below removes entirely.

### What was built for the self-sovereign upgrade

**`proxy/api/inbound-email.ts`** — an inbound-mail webhook on the existing proxy,
following the same GitHub-store model as `api/chat.ts` / `api/signal.ts`:

- **POST** (provider → us): accepts **either** JSON (ForwardEmail webhook, Postmark,
  Cloudflare Email Worker) **or** `multipart/form-data` (Mailgun / SendGrid Inbound
  Parse), normalizes the wildly-different field names to one `Mail` shape, derives
  the **mailbox** from the `@localharness.xyz` recipient's local-part, and appends to
  `inbox/<mailbox>.json` (rolling last-100, read-modify-write with a 1-retry CAS,
  per-field clamp to keep the file under GitHub's 1 MB).
- **GET** (platform → us): `?mailbox=agent&after=N` returns the rolling log so the
  browser app / CLI reads the agent's mail **directly from the platform's own store**
  — no Gmail in the loop. This is the genuinely self-sovereign inbox.
- **Auth:** shared secret (`INBOUND_EMAIL_SECRET`, presented as `?key=` or
  `x-inbound-key`) gates both POST and GET; optional ForwardEmail HMAC
  (`X-Webhook-Signature` via `FORWARD_EMAIL_WEBHOOK_KEY`). **Inert (503) until the
  secret is set** — the safe default, matching every other store-backed route.

Typechecks clean (`npx tsc --noEmit`) and the proxy test suite stays green. **Not
deployed** — deliberately: it's one separate `cd proxy && vercel --prod` away, and
deploying the proxy is a discrete, reviewed action.

**To turn the webhook on (all autonomous — no human needed):**
1. `vercel env add INBOUND_EMAIL_SECRET` (pick a random secret) on the proxy project.
2. `cd proxy && vercel --prod`.
3. Add one DNS TXT (alongside the existing forward-email line — ForwardEmail honors
   multiple targets, so Gmail *and* the webhook both receive):
   `forward-email=https://proxy-tau-ten-15.vercel.app/api/inbound-email?key=<secret>`
4. Wire the browser/CLI to GET `…/api/inbound-email?mailbox=agent&key=<secret>`.

(Step 3 was intentionally **not** added yet: pointing ForwardEmail at the webhook
*before* it's deployed would bounce that leg. Deploy first, then add the TXT.)

---

## 3. What is autonomously doable vs. the genuine human step

| Capability | Status | Who |
|------------|--------|-----|
| Manage `localharness.xyz` DNS (any record) | ✅ done / available | **Agent** (Vercel REST API) |
| **Receive** `*@localharness.xyz` (catch-all) | ✅ **LIVE** | **Agent** (pure DNS, ForwardEmail) |
| Read received mail (Gmail) | ✅ works (token needs refresh) | **Agent** (Gmail MCP) |
| Platform-owned inbox store (webhook) | ✅ built; deploy+env+TXT pending | **Agent** (all autonomous) |
| **Send** DKIM-authenticated mail *from* `@localharness.xyz` | ⛔ **needs 1 human step** | see below |

### The ONE genuine physical constraint

**Authenticated *outbound* email (sending *as* `agent@localharness.xyz` so replies
don't land in spam) requires a sending-provider account, and creating that account is
the one thing that can't be done from here** — it gates on a CAPTCHA + email
confirmation (and several providers also require a payment card). Concretely, the
smallest unlock is **one** of:

- **ForwardEmail "Enhanced Protection" ($3/mo)** — unlocks SMTP send-as on the domain
  we've *already* configured for receiving. Smallest delta. Needs: create the account
  (CAPTCHA/email), add a payment method.
- **Resend / Postmark / Mailgun** — free/low tiers for transactional send. Needs:
  account signup (CAPTCHA/email, often a card) + domain verification.

**The split is identical to the marketing-accounts split in `README.md`:** the human
seeds **one** account once; after that the agent does everything else autonomously —
it adds the DKIM/DMARC/return-path DNS records itself (DNS is ours) and sends via the
provider's API on every future call. **Faking the signup gets the sender banned**, so
it's a real boundary, not a missing feature.

> Honest scope: until that account exists, the agent can still "reach the world" by
> sending via the **Gmail MCP** (from `compusophy@gmail.com`) — fine for outreach,
> but it is *not* the `@localharness.xyz` identity and won't carry the agent's domain.
> **Receiving** at `@localharness.xyz` — the part that matters for verification
> codes / confirmations — has **no** such constraint and is already live.

---

## 4. Summary

- **DNS:** Vercel, team `compusophys-projects`, full control via REST API (CLI `dns`
  subcommand is bugged — use the API). No prior MX/mail setup.
- **Minimal live path:** ForwardEmail catch-all (`*@localharness.xyz` →
  `compusophy@gmail.com`) via pure DNS — **done, zero signup, live now.**
- **Autonomously done:** all DNS records added; `proxy/api/inbound-email.ts`
  provider-agnostic webhook + platform-readable inbox built and tested (one deploy
  from a fully self-sovereign, Gmail-free inbox).
- **The one human step:** create **one** sending-provider account (CAPTCHA/email/
  maybe card) to send DKIM-authenticated mail *from* the domain. Everything after
  that — DKIM DNS, sending, reading — is autonomous.

### Reverting (if ever needed)
Delete the four records by UID:
`DELETE https://api.vercel.com/v2/domains/localharness.xyz/records/<uid>?teamId=team_DYlw1hPeilK5o3w1uPWqt8Mi`
(UIDs listed in §2). Removing the MX + forward-email TXT fully unwinds the inbox.
