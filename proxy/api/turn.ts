// /api/turn — WebRTC ICE servers (STUN always; TURN when provisioned).
//
// STUN gives reflexive candidates (works for cone NATs); TURN RELAYS the media
// for symmetric NATs (~20-30%) that STUN can't punch — needed for "two people on
// different networks." STUN is free + always on. TURN needs a PROVIDER: set
// TURN_URLS (comma-separated, e.g. "turn:host:3478,turns:host:5349"),
// TURN_USERNAME, TURN_CREDENTIAL (from metered.ca's free tier, Cloudflare, Twilio,
// or self-hosted coturn). Until then this returns STUN-only — exactly today's
// behavior, no regression. webrtc.rs fetches this once at mount and caches it.
//
// NOTE: with STATIC env creds this GET is open (the creds grant relay bandwidth,
// bounded by the provider quota). For production use TIME-LIMITED creds (the TURN
// REST API / provider token endpoint) so an exposed value expires — a follow-up.

import { isAllowedOrigin } from './_auth';

export const config = { runtime: 'edge' };

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  const headers: Record<string, string> = {
    'content-type': 'application/json',
    'cache-control': 'public, max-age=300',
    Vary: 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) headers['Access-Control-Allow-Origin'] = origin;

  const iceServers: Array<{ urls: string[]; username?: string; credential?: string }> = [
    { urls: (process.env.STUN_URLS ?? 'stun:stun.l.google.com:19302').split(',').map((s) => s.trim()).filter(Boolean) },
  ];
  const turnUrls = process.env.TURN_URLS;
  if (turnUrls && process.env.TURN_USERNAME && process.env.TURN_CREDENTIAL) {
    iceServers.push({
      urls: turnUrls.split(',').map((s) => s.trim()).filter(Boolean),
      username: process.env.TURN_USERNAME,
      credential: process.env.TURN_CREDENTIAL,
    });
  }
  return new Response(JSON.stringify({ iceServers }), { status: 200, headers });
}
